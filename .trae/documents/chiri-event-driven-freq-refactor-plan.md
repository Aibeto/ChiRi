# Chiri 事件驱动升降频重构计划

## Summary

把 Chiri（8550）CLG 的升降频从"决策+写频同循环耦合"重构为**事件驱动**：
- CLG（160ms 采样决策）与 **touch**（真实事件）作为决策源，只更新共享目标态（cluster 的 `current_perf`/目标频率）；
- 新增统一写频层 **backset**（`CpuLoadGovernor::flush()`）负责把目标态去重写回 sysfs；
- touch 从"共享时间戳 + 每 tick 轮询"改为**独立事件通道即时通知**，升频延迟从 ≤160ms 降到 ≤100ms；
- CLG 采样从 120ms 减慢到 160ms（省 25% eBPF 读取/事件开销）；
- 顺带修复数据获取浪费：`foreground_max_util` 每 tick 计算但两套调度均不消费（FAS 禁用），改为跳过；
- akmode（明日方舟特调）**不纳入**事件驱动，保持独立 40ms 直写；
- yaml 仅做轻微重调（用户反馈存在性能过剩，但不要改保守以免卡顿）。

主要目标：审查并理顺升降频代码逻辑、降低数据获取与共享开销、提升触摸响应。

---

## Current State Analysis（基于已探索代码）

1. **采样**：[cpu_monitor.rs](file:///e:/code/ChiRi/src/monitor/cpu_monitor.rs) `SAMPLE_MS_NORMAL=120` / `SAMPLE_MS_TUNED=40`（L32-35）；每 tick 新建 `core_utils` Vec（L172）并发送 `DaemonEvent::SystemLoadUpdate`；`foreground_max_util` 每 tick 经 TGID/线程级计算（L239-299），但 chiri/mod.rs L540 与 scheduler/mod.rs L399 均以 `foreground_max_util: _` 忽略（仅 FAS 消费，FAS 已禁用）。
2. **scheduler_ipc 循环**：[chiri/mod.rs](file:///e:/code/ChiRi/src/chiri/mod.rs) L331 `rx.recv_timeout(CLG_STALE_POLL=1s)`；SystemLoadUpdate 分支（L540-560）直接调 `ak_governor.on_load_update` 或 `cpu_governor.on_load_update`。
3. **CLG 决策+写频耦合**：[cpu_load_governor.rs](file:///e:/code/ChiRi/src/chiri/cpu_load_governor.rs) `on_load_update`（L500-645）内完成：尖峰抑制 → headroom → 目标 perf → 升/降频分支（含 schedutil 余量跳过、直接降频）→ 底部触摸地板钳位 → 写频（`write_freq` 走 FastWriter 去重）。**决策与写频在同一个 `&mut self` 循环内完成**。
4. **touch 现状**：[touch_detect.rs](file:///e:/code/ChiRi/src/chiri/touch_detect.rs) 读 `/dev/input/event*`，把最近触摸时间写入 `Arc<Mutex<Option<Instant>>>`；CLG 在 `on_load_update` 每 tick 轮询该时间戳判断窗口——**轮询式、非事件驱动**，升频延迟 ≤ 下一个采样 tick（160ms 后）。
5. **已知逻辑缺陷（上一轮审查确认）**：
   - 触摸地板钳位未校验 `touch_boost_enabled`，且 `release()`/`reload_config` 不清空 `touch_boost_until/floor`，配置切换后残留升频 ≤1s。
   - `on_load_update` 职责混杂（触摸窗口管理 + 逐 cluster 决策 + 写频），超 100 行。
6. **akmode**：[akmode.rs](file:///e:/code/ChiRi/src/chiri/akmode.rs) `on_load_update` 自带升降 max 直写（40ms tick），与 CLG 互斥接管，不共享写频路径。

---

## Proposed Changes

### 1. 新增统一写频层 backset —— [src/chiri/cpu_load_governor.rs](file:///e:/code/ChiRi/src/chiri/cpu_load_governor.rs)

**目标**：决策与写频解耦，touch 与 CLG 都通过同一 `flush()` 写频（去重由 `FastWriter`/`write_freq` 承担）。

**改动**：
- 移除结构体字段 `last_touch: Arc<Mutex<Option<Instant>>>`；`CpuLoadGovernor::new()` 恢复无参签名（mod.rs 同步）。
- `on_load_update(&core_utils)` 收敛为**纯决策**：
  - 删除顶部触摸窗口管理块（30 行）与底部写频块；
  - 保留尖峰抑制 / headroom / 升频分支（schedutil 余量跳过、直接降频），只更新 `cluster.current_perf`；
  - 不再调用 `write_freq`。
- 新增 `pub fn flush(&mut self)`（即 backset 写频入口）：
  - 窗口过期清理：`if let Some(until) = touch_boost_until { if now >= until { until=None; floor=0.0 } }`；
  - 逐 cluster：触摸地板钳位（**补上 `self.cfg.touch_boost_enabled &&` 前置判断**，修复审查缺陷）→ `clamp(perf_floor, perf_ceil)` → `find_nearest_freq` → `write_freq`（FastWriter 自带值去重）。
- 新增 `pub fn on_touch(&mut self)`：`if !active || !cfg.touch_boost_enabled { return; }`；`touch_boost_floor = compute_touch_boost_floor()`；`touch_boost_until = now + touch_boost_ms`；`debug!(clg-touch-boost)`。
- `release()` 追加清空 `touch_boost_until=None; touch_boost_floor=0.0`（修复残留缺陷）。
- 保留 `read_cur_freq`（升频跳过/直接降频仍需要）、`is_big_cluster`、`compute_touch_boost_floor`。

**为什么**：写频收敛到单一入口，CLG 决策与 touch 都能触发；`flush` 去重避免冗余 sysfs 写；顺带消除上轮审查的两处缺陷。

### 2. touch 改事件驱动 —— [src/chiri/touch_detect.rs](file:///e:/code/ChiRi/src/chiri/touch_detect.rs)

**改动**：
- 签名改为 `pub fn monitor_touch(tx: std::sync::mpsc::SyncSender<()>)`，删除 `last_touch: Arc<Mutex<Option<Instant>>>` 参数与写入逻辑；
- 触摸按下时 `let _ = tx.try_send(());`（保留 `touch-detect-down` debug 日志）；
- 其余 evdev 解析逻辑不动。

**为什么**：touch 成为真正的独立事件源，不再共享时间戳、不再被 CLG 轮询。

### 3. scheduler_ipc 接入事件 —— [src/chiri/mod.rs](file:///e:/code/ChiRi/src/chiri/mod.rs)

**改动**：
- 删除 `touch_state: Arc<Mutex<Option<Instant>>>`，改为创建 `let (touch_tx, touch_rx) = mpsc::sync_channel::<()>(8);`；
- touch 线程 spawn 改为 `touch_detect::monitor_touch(touch_tx)`；`CpuLoadGovernor::new()` 无参调用；
- 新增常量 `EVENT_POLL_MS: Duration = Duration::from_millis(100)`，`rx.recv_timeout(EVENT_POLL_MS)`（原 1s），保证 touch 事件 ≤100ms 被处理，同时看门狗检查仍按 `CLG_STALE_MAX=5s` 阈值工作（频率提升到 10 次/s，开销可忽略）；
- 循环体每次醒来（无论是否超时）先 `while touch_rx.try_recv().is_ok() { if cpu_governor.is_active() { cpu_governor.on_touch(); cpu_governor.flush(); } }`；
- SystemLoadUpdate 分支 CLG 路径：`cpu_governor.on_load_update(&core_utils); cpu_governor.flush();`（akmode 路径不变，akmode 仍自行直写）；
- 看门狗超时分支保持（仅检查阈值），`continue` 前照常。

**为什么**：touch 即时通知 → 立即 `on_touch()+flush()` 写频，摆脱 160ms tick 等待；CLG 决策后统一走 flush。

### 4. CLG 采样 120ms → 160ms —— [src/monitor/cpu_monitor.rs](file:///e:/code/ChiRi/src/monitor/cpu_monitor.rs)

**改动**：
- `SAMPLE_MS_NORMAL: u64 = 120` → `160`（`SAMPLE_MS_TUNED=40` 不变，akmode 不受影响）。
- **跳过无消费的 foreground 计算**：引入 `const FAS_FG_UTIL_ENABLED: bool = false;`（"恢复 FAS 时启用"注释），`foreground_max_util` 计算块改为 `if FAS_FG_UTIL_ENABLED { …现有计算… } else { 0.0 }`；`compute_tgid_util` / `compute_thread_level_util` 加 `#[allow(dead_code)]` 并注明恢复 FAS 时启用（与 AGENTS 约定一致，不删除）。`DaemonEvent` 结构体字段保留（两套调度均 `_` 忽略，发送 0.0 无副作用）。

**为什么**：160ms 直接削减 25% eBPF map 读取与事件发送；foreground 计算（TGID 读 + 逐核 pending 扫描）每 tick 白算，跳过是立竿见影的数据获取优化。`core_utils` 保持 `Vec` 每次新建（约 150B/160ms，分配开销可忽略，不改通道负载类型以免波及 Yumi/akmode）。

### 5. yaml 轻微重调 —— [module/config/8550/config.yaml](file:///e:/code/ChiRi/module/config/8550/config.yaml)

按用户意见"稍微调一点点、削减性能过剩但不保守"，配合 160ms tick 轻微收敛：

| 模式 | 参数 | 现值 → 建议值 |
|------|------|--------------|
| balance | up_threshold | 0.75 → 0.78 |
| balance | headroom_factor | 1.30 → 1.20 |
| balance | smoothing_up | 0.80 → 0.75 |
| balance | down_rate_limit_ticks / up_rate_limit_ticks | 2 / 1（保持，160ms 下响应仍快） |
| powersave | up_threshold | 0.88 → 0.90 |
| powersave | headroom_factor | 1.10 → 1.05 |
| performance | headroom_factor | 1.50 → 1.40 |
| performance | smoothing_up | 1.0 → 0.95 |
| fast / scenemode | — | 不动 |

说明：以上为"轻微"建议值，真机实测后可按相同方向微调；`rate_limit_ticks` 保持整数 1-2 档、不强求按 0.75 比例换算（160ms 下 1-2 tick 的防抖时长仍合理，且不过度削弱敢降频）。

### 6. i18n 双语 key —— [module/config/i18n/zh.ftl](file:///e:/code/ChiRi/module/config/i18n/zh.ftl) / [en.ftl](file:///e:/code/ChiRi/module/config/i18n/en.ftl)

- 新增 `touch-event-received = [Touch] 收到触摸事件，触发大核升频` / 英文对应（debug，scheduler_ipc 处理触摸事件时打点）。
- 现有 `clg-touch-boost`（on_touch 内沿用）、`touch-detect-*`、`clg-up-skipped` 等 key 不变。

### 7. 文档 —— [AGENTS.md](file:///e:/code/ChiRi/AGENTS.md)

更新约定 10（CLG 调频语义）：补充"事件驱动升降频"——CLG 决策只更新共享目标态、`flush()` 统一写频去重、touch 经独立事件通道即时通知（≤100ms）、采样 160ms、`foreground_max_util` FAS 恢复前跳过计算。更新时间戳。

---

## Assumptions & Decisions

1. **backset = 共享目标态 + 统一写频（单线程，无新线程）**：决策者（CLG/touch）更新共享目标，`flush()` 统一写频；不引入独立写频线程/通道（用户已选此项，风险最低）。
2. **akmode 不纳入**：保持独立 40ms 直写，与 CLG 互斥接管；touch 事件在 akmode 接管时被 `cpu_governor.is_active()` 挡掉。
3. **touch 延迟 ≤100ms**：通过 `EVENT_POLL_MS=100` 的 recv_timeout 实现"事件驱动 + 及时处理"，代价是空闲唤醒 10 次/s（可忽略）；不采用 touch 线程直接写 sysfs（避免双写竞争）。
4. **core_utils 保持 `Vec`**：不改通道负载类型（避免波及 Yumi/akmode/DaemonEvent），分配开销可忽略。
5. **yaml 轻微重调**：数值为建议值，真机实测后微调；不按严格 0.75 比例换算 tick 参数。
6. **Yumi 调度器（src/scheduler/）与默认 config.yaml 不动**：本改动仅限 chiri + 共享的 cpu_monitor（payload 不变）。

## Verification

1. `cargo check -p yumi --target aarch64-linux-android`（Windows 本机 eBPF 走占位 stub，验证编译通过；最终产物由云端 CI 构建）。
2. 逻辑自检：确认 `flush()` 只在 CLG 激活时被调用（touch/load 两条路径）、akmode 路径不经过 flush、`release()` 清空触摸状态。
3. 人工核对 i18n 新增 key 双语齐全、无孤儿 key。
4. 真机（8550）验证点：触摸响应（B站/滚动）延迟是否改善、160ms 下是否仍不卡顿、`clg-up-skipped`/`clg-touch-boost` debug 日志是否符合预期、息屏 scenemode 切换正常。

## Out of Scope

- GPU 相关（用户明确不动）。
- akmode 事件化、Yumi 调度器、默认 config.yaml、WebUI、CI 配置。
- 新线程/通道的完全异步化（tokio select 重写）——当前单线程方案足够达成目标。
