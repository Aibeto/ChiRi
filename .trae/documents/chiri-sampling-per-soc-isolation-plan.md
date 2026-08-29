# ChiRi 采样按 SoC 参数化（Yumi 隔离）修正计划

## Summary

把上一轮事件驱动重构中改到**共享层** [cpu_monitor.rs](file:///e:/code/ChiRi/src/monitor/cpu_monitor.rs) 的常规采样间隔改为**按 SoC 参数化**，满足「只动 ChiRi、不动 Yumi」约束：

- **ChiRi = 160ms**（新优化，上一轮目标）；
- **Yumi = 200ms**（用户确认的"Yumi 原有值"；git 历史佐证，见 Current State Analysis）。

其余上一轮改动（`src/chiri/*`、`module/config/8550/config.yaml`、i18n 新增 key）本就只影响 ChiRi，不涉及共享行为。`foreground_max_util` 跳过对 Yumi 行为中性（两套调度均忽略该字段），保留。

---

## Current State Analysis

1. **共享采样常量**：[cpu_monitor.rs](file:///e:/code/ChiRi/src/monitor/cpu_monitor.rs) `SAMPLE_MS_NORMAL=160`（上一轮从 120 改来）是共享常量，两套调度（Yumi 与 Chiri）的 `SystemLoadUpdate` 都由它决定频率。
2. **Yumi 原有值 = 200ms**：`git log -S SAMPLE_MS_NORMAL`（commit `6af4a4b`）显示，akmode 动态采样引入前，cpu_monitor 用硬编码 `Duration::from_millis(200)`；引入 `SAMPLE_MS_NORMAL=120` 时把 Yumi 采样一并加速到 120ms（副作用）。
3. **调用链**：`main.rs` `monitor::start_monitor(tx, ak_active)` → [monitor/mod.rs](file:///e:/code/ChiRi/src/monitor/mod.rs) L37 → spawn 线程内 `cpu_monitor::start_cpu_loop(tx_cpu, rx_pid_cpu, ak_active_cpu).await`（内部用 `SAMPLE_MS_NORMAL`）。
4. **Yumi 消费方式**：[scheduler/mod.rs](file:///e:/code/ChiRi/src/scheduler/mod.rs) L399-415 每个 `SystemLoadUpdate` 都调其 CLG `on_load_update`，采样间隔直接决定 Yumi CLG 的 tick 频率。
5. **foreground 中性**：`foreground_max_util` 仅 FAS 消费（FAS 禁用），chiri/mod.rs 与 scheduler/mod.rs 均 `_` 忽略；跳过计算对 Yumi 无行为影响。

---

## Proposed Changes

### 1. [src/main.rs](file:///e:/code/ChiRi/src/main.rs) — 按 SoC 决定常规采样间隔
- 已有 `chiri_active = common::is_chiri_soc()`（L49）。
- 新增：`let sample_ms_normal: u64 = if chiri_active { 160 } else { 200 };`
- 调用改为：`monitor::start_monitor(tx, ak_active, sample_ms_normal)`。

### 2. [src/monitor/mod.rs](file:///e:/code/ChiRi/src/monitor/mod.rs) — 透传参数
- `start_monitor` 签名追加参数：`sample_ms_normal: u64`。
- cpu 监控线程 spawn（L179）改为：`cpu_monitor::start_cpu_loop(tx_cpu, rx_pid_cpu, ak_active_cpu, sample_ms_normal).await`。

### 3. [src/monitor/cpu_monitor.rs](file:///e:/code/ChiRi/src/monitor/cpu_monitor.rs) — 参数化常规采样
- 删除 `SAMPLE_MS_NORMAL` 常量（160）；`start_cpu_loop` 签名追加 `sample_ms_normal: u64`。
- 初始 `interval`（L158）与动态切换的"非常规"分支（L341-344，`ak_active=false` 侧）改用传入参数；`SAMPLE_MS_TUNED=40` 常量保留（akmode 仅 ChiRi 激活，Yumi 永不触发 40ms）。
- 更新相关注释（L32-35、L162"160ms/特调40ms"）为"ChiRi 160ms / Yumi 200ms；akmode 40ms"。
- `FAS_FG_UTIL_ENABLED=false` 与三个 `#[allow(dead_code)]`（`get_thread_tids`/`compute_tgid_util`/`compute_thread_level_util`）保留不变（行为中性、FAS 恢复预留）。

### 4. [AGENTS.md](file:///e:/code/ChiRi/AGENTS.md) — 同步描述
- 约定 1「负载采样间隔动态切换」：改为「常规采样按 SoC 参数化——Chiri 160ms、非 Chiri（Yumi）200ms（Yumi 原有值，勿再改动）；akmode 激活时 40ms」。
- 约定 10「数据获取优化」：改为「常规采样按 SoC 参数化（ChiRi 160ms / Yumi 200ms），由 main.rs 按 `is_chiri_soc()` 传入 cpu_monitor」。
- 更新时间戳。

---

## Assumptions & Decisions

1. **Yumi=200ms（用户已确认）**：恢复到 akmode 改造前原值（git 历史佐证：akmode 动态采样引入前硬编码 200ms）；**ChiRi=160ms** 为上一轮优化目标。
2. **允许参数化共享层（用户已确认）**：可修改 `main.rs` / `monitor/mod.rs` / `cpu_monitor.rs` 签名透传采样间隔；共享文件有改动，但 Yumi 行为按上条保持 200ms（恢复原值）。
3. **foreground 跳过保留**：对 Yumi 行为中性（两套调度均忽略该字段），仅削减无用计算；不做额外 gating。
4. **Yumi 的 config.yaml / `src/scheduler/*` / i18n 现有行为均不改**；本次仅把共享采样参数化并恢复 Yumi 原值，属"修复共享层对 Yumi 的意外影响"。

## Verification

1. `cargo check -p yumi --target aarch64-linux-android`（Windows 本机 eBPF 占位 stub，验证编译通过）。
2. 逻辑核对：`SAMPLE_MS_NORMAL` 常量已删除；`start_cpu_loop` / `start_monitor` 签名与全部调用点（main.rs、monitor/mod.rs）一致；Yumi 路径传 200、ChiRi 传 160。
3. `git diff --stat` 复核：`src/scheduler/` 与默认 `module/config/config.yaml` 无任何改动。
