---
name: perf-U1-logger-handle-C1-current-thread-U3-reeval
overview: 实施 U1（daemon.log 持久化句柄 + 记账为主低频校验的自愈写入）、C1（两个 tokio runtime 改 current_thread），并对 U3 的 cpu_monitor 侧给出「不做」的量化结论；yumi 兼容不再作为约束。
todos:
  - id: impl-u1
    content: 用 [skill:m02-resource] 实施 U1：SelfHealingAppender 改常驻句柄 + 8KB 低频巡检，保持 note_write 锁外与字节口径
    status: pending
  - id: impl-c1
    content: 用 [skill:coding-guidelines] 实施 C1：monitor/mod.rs 两处 runtime 改 current_thread + enable_time，Cargo.toml tokio features 收窄到 rt
    status: pending
  - id: verify-build
    content: 用 [skill:m10-performance] 跑 cargo check（android 目标）确认无 error/warning，并核对 git 改动足迹
    status: pending
    dependencies:
      - impl-u1
      - impl-c1
  - id: sync-docs
    content: 用 [skill:token-efficient-coding] 同步报告总表、[equiv]、[impl] 三处的 U1/C1 结论与 U3 量化结论，并追加当日 memory
    status: pending
    dependencies:
      - verify-build
---

## 用户需求

在已完成的 10 项性能优化基础上继续推进三件事：

1. **完成 U1**：日志落盘路径去掉「每条日志一次 mkdirat + 一次 stat + 一次 open/close」，改为常驻写句柄与低频校验。
2. **重新评估 U3**：上一轮只做了 CLG 侧（`Arc` 双缓冲），cpu_monitor 侧因 `core_utils` 被 move 进事件而未做，需要重新判断是否值得做。
3. **重新评估 C1**：两个 tokio runtime 是否可以从多线程改为 current_thread。
4. **约束变化**：`src/scheduler/`（Yumi 调度）与 Yumi 设备兼容性进入零权重状态，改动即使波及也不必回避。

## 交付内容

- U1 落地：`daemon.log` 写路径从「每条约 5 次 syscall」降到稳态只剩一次 write，同时保住「文件被外部删除能自愈」「写路径绝不 panic」「尺寸上限 50MB 轮转」三条既有语义。
- C1 落地：两个 runtime 各从 8 个 worker 线程降到 1 个线程，并同步把 tokio features 收窄。
- U3 结论：给出量化评估，明确 cpu_monitor 侧不做，并把理由写入报告（不再挂待办）。
- 报告与 memory 同步：总表「完成」列、[equiv] 等价性复核、[impl] 实施记录三处补 U1/C1，U3 结论单列。

## 核心要点

- U1 唯一的**有意权衡**：外部删除 `daemon.log` 后的自愈检测窗口从「每条一次 stat」变成「累计写入 8KB 后巡检一次」，最坏丢 8KB 日志；相对 50MB 上限与 16MB 打包门限可忽略，且换来日志路径几乎零 syscall。
- U1 必须保住的三条不变量：`note_write` 的记账字节口径（编码长度，无论写入成功与否）、`note_write` 必须在**锁外**调用（其内部会 `log::info!` 并可能 `exit(0)`，持锁重入会死锁）、轮转前必须先丢弃写句柄（否则继续写已改名/已 unlink 的 inode）。
- C1 的评估结论是不影响采样周期：`interval.tick().await` 的唤醒延迟取决于任务让出时刻，与 runtime 线程数无关；循环体内只有 syscall 级阻塞，不会长时间占住 driver。
- 两项都不改变任何落盘内容与调度判定，唯一格式相关行为不变（日志行格式、轮转命名、status.csv 与 devimp 均不受影响）。

## 技术栈

- Rust edition 2024 / nightly，交叉目标 `aarch64-linux-android`（本机验证命令固定为 `cargo +nightly check -p chiri --target aarch64-linux-android`）。
- U1 改造 `log4rs` 的自定义 `Append` 实现（`SelfHealingAppender`）；C1 改造 `tokio::runtime` 的构造方式并同步 `Cargo.toml` features。
- 不新增任何依赖；不涉及 eBPF、不涉及 WebUI、不涉及 `module/` 模板。

## 实施方案

### U1：常驻句柄 + 低频巡检（复用同文件既有做法）

**先例**：同一文件里 `status.csv` 早已用「常驻句柄 + 每 N 次写入巡检 metadata」解决过同样的问题（`StatusWriter` / `status_open` / `status_check`，logger.rs:378-423）。U1 直接沿用这套形状，不引入新模式。

结构（logger.rs:74-111）：

- `SelfHealingAppender.lock: Mutex&lt;()&gt;` 换成 `state: Mutex&lt;AppendState&gt;`，`AppendState { file: Option&lt;fs::File&gt;, size: u64, since_verify: u64 }`（`AppendState` 加 `#[derive(Debug)]`，`fs::File` 本身实现 Debug）。
- 新增常量 `LOG_VERIFY_BYTES: u64 = 8 * 1024`（与 status.csv 的「16 行 ≈ 4KB」同量级）。
- 新增 `fn append_verify(&self, st: &mut AppendState)`，形状对齐 `status_check`：`fs::metadata(&self.path)` 成功且未超 `max_bytes` → 只校正 `st.size`；成功但超限 → 先 `st.file = None` 再 `self.rotate()` 并置 `size = 0`；`metadata` 失败（文件被删）→ `st.file = None`、`size = 0`。
- 新增 `fn append_open(&self, st: &mut AppendState)`：`create_dir_all(parent)`（**只在这里做**）+ `OpenOptions::new().create(true).append(true).open(&path)`，成功后用 `f.metadata()` 校正 `st.size`（覆盖 `update_level` 重建 appender、文件已有 40MB 的场景）。

`append` 新流程（保持「锁内 IO、锁外 note_write」）：

1. 取 `state.lock()`（毒化用 `into_inner` 剥除，与现状一致）。
2. 编码进内存缓冲；编码失败直接 `return Ok(())`（与现状一致）。
3. 若 `st.since_verify &gt;= LOG_VERIFY_BYTES` → `append_verify` + `since_verify = 0`。
4. 若 `st.file.is_none()` → `append_open`（含尺寸校正）。
5. `write_all + flush`：成功则 `size += bytes; since_verify += bytes`；失败则 `st.file = None`（下次重建，本条丢弃）。
6. 释放锁后 `note_write(&amp;LOGS_BYTES_WRITTEN, "logs/", written)`，`written` 仍为编码后的字节长度（口径不变）。

删除 `current_size()`（logger.rs:96-98）——其唯一调用点在 append 内，改由 `append_verify`/`append_open` 内部按需 stat。`rotate()`（100-110）与页头注释（61-73）同步更新为「常驻句柄 + 低频巡检自愈」的描述。

### C1：current_thread runtime

- `src/monitor/mod.rs:168`（fps）与 `:193`（cpu）两处 `tokio::runtime::Runtime::new()` 换成 `tokio::runtime::Builder::new_current_thread().enable_time().build()`；失败分支的 `error!` 提示保持不变。
- 必须 `enable_time()`：cpu_monitor 用 `tokio::time::interval/interval_at`，缺 time driver 会 panic；两处 fps 侧只用 `watch::changed().await`，多开一个 time driver 无副作用。io driver 不需要（fps 的 RingBuf 轮询走 mio 自有 `Poll`，在独立 std 线程里）。
- `Cargo.toml` 的 tokio features 从 `["rt-multi-thread", "time", "sync"]` 收窄到 `["rt", "time", "sync"]`（`new_current_thread`/`block_on`/`spawn` 只需 `rt`）。
- 周期精度不变的技术依据：`interval.tick().await` 的唤醒延迟取决于任务让出时刻，与 worker 线程数无关；循环体阻塞段只有 `/proc` 读取与 bpf map 读取（syscall 级）。
- 收益如实表述：2×(8→1) 个 worker 线程、约省 14 个空闲线程的栈与调度条目；CPU 收益接近 0。

### U3 重新评估结论：cpu_monitor 侧不做

CLG 侧已兑现（每 tick 2 次分配 → 0）。cpu_monitor 侧的 `vec![0.0_f32; max_cpu_id + 1]`（cpu_monitor.rs:279）无法原地复用，因为该 Vec 被 move 进 `DaemonEvent::SystemLoadUpdate`。三条替代路径（事件改 `Arc&lt;Vec&lt;f32&gt;&gt;` 池化 / chiri 侧每 tick 拷贝 / 事件改固定数组）分别为：chiri 的 `last_core_utils` 跨 tick 保留导致引用计数长期大于 1，池化收益归零；拷贝方案等于「省一次 36B 分配换一次 36B 拷贝」，约 40ns/tick（6~25 次/s）；固定数组方案收益同量级却要改两个消费方的事件契约。结论：收益低于 0.0001% CPU，不值得改，仅在报告里记录该结论。

## 实施要点（防回归）

- **锁外记账**：`note_write` 必须在 `state` 锁释放之后调用（logger.rs:115-118 的死锁说明），改造后这一约定不能破。
- **无新增日志**：append 内不新增任何 `log::*`（会重入 append 并可能死锁）；自愈重建保持与原实现一样静默。
- **轮转顺序**：先 `st.file = None` 再 `rotate()`，否则新写入落到已改名的 inode。
- **记账口径**：`written` 恒为编码长度，与写成功与否无关（决定「日志目录预算 / 16MB 打包门限」的触发点）。
- **有意权衡需写进注释**：8KB 自愈窗口、超限最多多写 8KB+一行（原实现为每行 stat、超限最多一行）。
- 提交前 `git status` 核对足迹；两项改动都落在 ChiRi 与 Yumi 共用的 monitor/logger 层，无需分支处理。

## 架构与文件

改动为局部实现替换，不新增模块、不改变分层：

```text
src/
  logger.rs          [MODIFY] U1：AppendState（state 锁 + 常驻 file + size/since_verify）、
                              append_verify / append_open、删除 current_size()、更新页头说明；
                              append 改「低频巡检 + 常驻句柄写」，note_write 仍在锁外
  monitor/mod.rs     [MODIFY] C1：两处 runtime 改 Builder::new_current_thread().enable_time().build()
Cargo.toml           [MODIFY] C1：tokio features rt-multi-thread → rt
.codebuddy/
  docs/2026-09-20-rust-perf-report-final.md  [MODIFY] 总表 U1/C1 行填「已完成 · 等价（8KB 校验窗口 /
                                             周期精度不变）」、U3 行补备注；[equiv] 追加 U1/C1 结论与
                                             「U3 cpu_monitor 侧为何不做」；[impl] 表追加 U1/C1 两行
  memory/2026-09-20.md                       [APPEND] 本轮实施与评估结论
```

## 关键代码结构

```rust
/// 追加状态：常驻写句柄 + 记账尺寸（该 Mutex 同时充当原先的串行化锁）
#[derive(Debug)]
struct AppendState {
    /// 常驻 append 句柄；轮转或被外部删除后置 None，下次写入重建
    file: Option<fs::File>,
    /// 当前文件尺寸：以本进程记账为主，每 LOG_VERIFY_BYTES 用 metadata 校正一次
    size: u64,
    /// 距上次真实尺寸校验已写入的字节数
    since_verify: u64,
}

/// 低频巡检：校正尺寸 / 超限轮转 / 被删丢弃孤儿句柄（形状对齐 status_check）
fn append_verify(&self, st: &mut AppendState);
/// 重建句柄：create_dir_all + open(create+append) + 用 f.metadata() 校正 size
fn append_open(&self, st: &mut AppendState);
```

## 验证

- `cargo +nightly check -p chiri --target aarch64-linux-android`：无 error、无 warning（含 features 收窄后的编译验证）。
- 本机无法执行、需在报告中标注为待真机验证的两项：① 手动 `rm daemon.log` / `rm -rf logs/` 后日志能在 ≤8KB 写入窗口内重建；② 写满 50MB 触发轮转后新文件继续增长、旧备份链完整。

## Agent Extensions

### Skill

- **token-efficient-coding**
- Purpose: 改造 logger.rs 与 monitor/mod.rs 时按区块注释定位做局部 Edit，避免整文件重写
- Expected outcome: 两处改动只产生目标段落的最小 diff
- **m02-resource**
- Purpose: 复核 U1 的 `Option<fs::File>` 常驻句柄生命周期（重建/丢弃时机、Drop 语义、孤儿 inode 场景）
- Expected outcome: 确认轮转与外部删除两条路径都不会继续写已 unlink/已改名的 inode
- **coding-guidelines**
- Purpose: 保证新增的 `AppendState`、`append_verify`、`append_open` 与既有代码风格一致（命名、注释、错误吞掉口径）
- Expected outcome: 改动与同文件 `StatusWriter`/`status_check` 风格统一
- **m10-performance**
- Purpose: 验证 U1 的 syscall 数量下降幅度与 C1 的线程数收益表述是否成立、是否引入新热点
- Expected outcome: 报告中给出可核对的前后 syscall/线程数对比，避免夸大收益