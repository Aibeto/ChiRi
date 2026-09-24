# ChiRi Rust 性能优化评估报告

> 2026-09-20 · AI 产出文档（`.codebuddy/docs/`）· 只评估不动源码
> 范围：`src/`（守护进程）、`yumi-ebpf/`（内核探针）、`Cargo.toml`（构建剖面）
> 所有收益均为**量级估算，必须实测**；未实测前不改默认行为。

## [summary] 结论摘要

代码里已经做过一轮热路径优化（`FastWriter` 持 fd + 栈缓冲、`log_enabled!` 门控 `format!`、`Arc` 共享替代逐簇 `to_vec`、事件驱动 deadline 替代 100ms 空转）。剩余空间分三档：

| 级别 | 项 | 现状 | 收益（估算） |
|---|---|---|---|
| P0 | K1 合并 eBPF percpu map | 每次 sched_switch 5 次 map 查找 | 探针耗时降 30%~50% |
| P0 | K2 THREAD_RUN_TIME 记账开关 | 32768 项 hash 每次开关无条件累加，仅降级路径消费 | 每次开关省 1 次 hash 查找/插入 |
| P0 | K3 遥测探针按需挂载 | sched_wakeup 等 3 个 tracepoint 无条件挂 | 高频唤醒场景省 1%~3% CPU |
| P1 | U1 日志落盘 syscall | 每条日志 6 次 syscall | 高日志量下 syscall 降 5/6 |
| P1 | U3 采样期 map 读取 | 每 tick 9 次 bpf 读 + 1 次 Vec 分配 | 与 K1 联动降到 5 次 |
| P2 | A1 i18n `t()` | 每条日志 RwLock + Fluent 解析 + String 分配 | 日志路径分配趋零 |
| P2 | A2 devimp 行构造 | 每行 40 列 String + join | dev 路径分配降一个量级 |
| P3 | B1/B2 tokio 运行时 | 2 个多线程 runtime，8 核约 16 工作线程 | 线程与内存显著下降 |
| P3 | B4 构建剖面 | `opt-level="z"` + fat LTO | 需 A/B 实测后定 |
| P3 | B3/B5/B6 依赖裁剪 | 4 个未使用依赖、log4rs 只用一个编码器 | 体积与 CI 时间 |

**首要动作建议**：先补自测量（[measure]），再动 K1/K2/U1。理由：没有基线，P0 的收益无法证明，而 P0 改动面在内核探针，回归风险最高。

## [measure] 先量化：当前缺一条可复现的基线

仓库里没有 `benches/`、没有 criterion，真机也没有 daemon 自身 CPU 的自测量。建议三层：

1. **纯函数**：`cargo bench` 覆盖 `ClusterState::find_nearest_freq`（cpu_load_governor.rs:61）、`TempFilter`、`Config::load` 解析嵌入 yaml、`is_special_mode_allowed` 的 `re:` 匹配。这些是唯一能在 PC 上跑的部分。
2. **整机**：`simpleperf` 对 chiri 进程采样，看用户态热点；eBPF 侧用内核 `bpftool prog show` 的 run_time 看探针自身开销。
3. **自测量（最值得加）**：1s 采样一次 `/proc/self/stat` 的 utime+stime，作为**新增列追加到 status.csv 末尾**（符合「新增列一律追加末尾」的契约）。这样每次优化都能用同一份 CSV 对比「daemon 自身 CPU」与「整机功耗」，不用靠感觉。

验收口径：同一机型同一场景（建议 8550 + 30 分钟 bili + 30 分钟游戏），A/B 各跑一轮，看 daemon CPU 占比与亮屏平均功耗。

## [kernel] P0：eBPF 探针（整机层面的最大杠杆）

探针跑在每次上下文切换上，是唯一「频率极高」的代码。8 核设备繁忙时 1 万~5 万次/秒，任何单次查找的节省都会被放大。

### K1 合并 percpu map

现状（`yumi-ebpf/src/main.rs:79-95`）：`CORE_LAST_TIME`、`CORE_IDLE_TIME`、`CORE_BUSY_TIME`、`CORE_CURRENT_TID`、`CORE_CURRENT_TGID` 五个独立 `PerCpuArray`。`try_handle_sched_switch`（main.rs:166-203）每次切换做 5 次 `get_ptr_mut`；用户态每采样做 5 次 `map.get`（cpu_monitor.rs:250-254），每次一个 `bpf` syscall。

建议：合成一个 `#[repr(C)] struct CoreState { last_time, idle, busy, cur_tid, cur_tgid }`，单个 `PerCpuArray<CoreState>`。内核侧 5 次查找降到 1 次，用户态 5 次 syscall 降到 1 次、5 次 `PerCpuValues` 分配降到 1 次。

风险：eBPF 与 daemon 版本偏差时 map 缺失已有容错（cpu_monitor.rs:166-183 的 `fetch_counter_map` 语义），沿用同一套「缺失即 0」即可。

### K2 THREAD_RUN_TIME 只在需要时记账

现状：`THREAD_RUN_TIME` 32768 项（main.rs:99），每次 sched_switch 无条件 `add_to_hash`（main.rs:187,211）。但用户态只有 **TGID 主路径失败时的降级路径**消费它（cpu_monitor.rs:374 调 `compute_thread_level_util`），主路径走 `TGID_RUN_TIME`（1024 项，一次查 1 个 key）。

也就是说绝大多数时间这笔 hash 查找白付，还占着约几百 KB 内核内存，且历史上正是这张表的驱逐问题促成了 TGID 主路径。

建议：加一个 `THREAD_ACCT_ON: Array<u32>`，探针开头读一次（Array 查找比 hash 便宜得多），为 0 就跳过 `add_to_hash`；仅在 FAS 激活且 TGID 主路径连续失败时由用户态置 1。

### K3 遥测探针按需挂载

现状（cpu_monitor.rs:88-118）：`sched_wakeup`、`sched_migrate_task`、`cpufreq_transition` 三个 tracepoint 无条件挂载，纯粹为了 status.csv 的 wakeups/migrations 两列。`sched_wakeup` 是内核最热的 tracepoint 之一，频率常高于 sched_switch。

建议：改成按开关 attach/detach（aya 丢弃 link 即卸载）。默认只保留 `migrate` + `cpufreq`；`wakeup` 仅在 devimp 或遥测开关打开时挂载，否则 CSV 该列写 `-`（与现有缺失占位一致）。

风险：status.csv 列语义变化，WebUI 与契约文档要同步；先确认 wakeups 列的实际消费者。

### K4（研究项，低优先）

长期息屏或 DOWN 停摆期间摘除 sched_switch 探针。收益直观，但会撞上 CLG 看门狗（chiri/mod.rs:14，5s 无负载事件即 release），需要另建低频唤醒源。建议等 P0 三项落地、自测量建立后再评估。

## [userspace-syscall] P1：落盘与采样路径的 syscall

### U1 日志每条 6 次 syscall（本档性价比最高）

`SelfHealingAppender::append`（logger.rs:113-156）每条日志的顺序是：

1. `fs::create_dir_all(parent)`（logger.rs:133-135）→ mkdirat
2. `self.current_size()`（logger.rs:96-98）→ 一次 `metadata` stat
3. `OpenOptions::append(true).open()` → open
4. `write_all` → write
5. `flush` + 关闭 → close

前两步完全可以省：目录建成后记一次标志即可；文件大小已经在 `note_write`（logger.rs:193）按字节记账，不需要再 stat 一遍。

建议：持持久化 `File` 句柄（O_APPEND，与 `StatusWriter`（logger.rs:385）/ `DevimpWriter`（logger.rs:920）/ `FastWriter`（utils.rs:424）同一套路），dir 用 `OnceLock` 建一次，尺寸用已有字节计数；句柄失效的判定保留自愈语义：写入失败即 reopen，或用 `fstat` 的 `st_nlink == 0` 检测外部删除（这正是当初放弃 `RollingFileAppender` 的原因，logger.rs:63-66，不能为了 syscall 把自愈去掉）。

### U2 write_to_file 的 chmod 三连

`write_to_file`（utils.rs:21-34）每次写做 `path.exists()` + chmod 664 + write + chmod 444。热路径已经被 `FastWriter` 规避，剩下的都是冷路径（reload / 一次性系统调整），优先级低。顺带一提：把 sysfs 节点改成 0444 会波及其它写入方，建议只在初始化路径保留，或干脆去掉权限回写。

### U3 采样期的 map 读取与 Vec 分配

每 tick（常规 160ms，特调 40ms）用户态读 5 个 percpu map + tgid hash + 3 个计数 map，约 9 次 bpf syscall（cpu_monitor.rs:250-254、184-186）；`core_utils`（cpu_monitor.rs:256）每 tick 新建 Vec。K1 落地后前 5 次合并为 1 次；Vec 可复用缓冲。这两项单独做的收益很小，建议跟着 K1 一起改。

## [userspace-alloc] P2：分配与锁

- **A1 i18n（logger.rs 每条日志都要过）**：`t()`（i18n.rs:78-102）拿 RwLock 读锁、走 Fluent 解析、返回新 `String`。无参消息可以在 `load_bundle` 时预格式化进 `OnceLock<HashMap<&'static str, String>>`，`t()` 返回 `&'static str`；带参的保留现路径。顺带消掉一个跨线程的全局读锁竞争点。
- **A2 devimp 行**：`devimp_write_line`（logger.rs:1150-1172）对 40 列 `String` 做 `join(",")`，devimp 开启时每行约 40 次分配（tick 行 3 簇 × 6/s，place 行可达百级）。改成复用一条 `String` 缓冲、数值用 `itoa` 直写（`itoa` 已在 Cargo.toml 里但没被用上）。属开发路径，不影响线上。
- **A3 CLG 广播**：`Arc::new(core_utils.to_vec())`（cpu_load_governor.rs:950）每 tick 一次分配。双缓冲 + `Arc::get_mut` 复用即可。收益很小，属于可做可不做的微优化。
- **A4 affinity 放置签名**：`last_place_sig` 用约 180 个线程名拼 String（affinity.rs:665、1583），8s 一次。换成 u64 哈希即可。微。

## [runtime] P3：运行时、依赖与构建

### B1 两个多线程 tokio runtime

monitor/mod.rs:168 与 193 各建一个 `tokio::runtime::Runtime::new()`（默认多线程，worker 数 = CPU 核数）。8 核设备上就是约 16 个工作线程，而每个 runtime 实际只跑一个定时器循环加一个 watch 通道。

建议改成 `Builder::new_current_thread()`：cpu_monitor 用的是 `tokio::time::interval` + `tokio::spawn`（cpu_monitor.rs:188），fps_monitor 直接用 mio 的 `Poll`（fps_monitor.rs:18）轮询 RingBuf fd，两者都不需要多线程调度器。省线程、栈内存与调度竞争。改动小，但要在真机确认采样周期不漂。

### B2/B3 依赖裁剪

- `tokio` 开了 `features = ["full"]`，实际只用到 rt/time/sync/macros。
- `itoa`、`bytes`、`futures`、`glob` 在 `src/` 里找不到任何 `use`（Cargo.toml:36-38、50），是可以直接删的声明。
- `lazy_static`（app_detect.rs:64）与 `once_cell`（logger.rs:13、i18n.rs:6）并存，统一到标准库 `LazyLock`/`OnceLock` 后可以去掉两个依赖。

### B4 构建剖面

```toml
[profile.release]
strip = true
opt-level = "z"     # ← 为体积牺牲了内联/展开/向量化
lto = true          # fat LTO + codegen-units=1 是最慢组合
codegen-units = 1
```

- `opt-level="z"` 对这种「syscall 密集 + 少量计算」的守护进程影响有限，但对配置解析、affinity 扫描、`tgtop` 排序这类循环是实打实的。建议`"s"` 与 `3` 各编一版，比二进制体积与 8550 实测功耗后再定。二进制里已经嵌了 webui dist 和配置 yaml，代码段增量占比应该不高。
- `lto = "thin"` 能把 CI 时间砍下来，性能与 fat LTO 差距通常很小。
- **`panic = "abort"` 不可用**：`catch_unwind` 是 panic 自愈的地基（chiri/mod.rs:974、scheduler/mod.rs:269、monitor/mod.rs:34），abort 之后它直接失效。这一条要在文档里写死，避免以后有人当体积优化捡起来。

### B5 log4rs 只用到一个编码器

logger.rs:8-12 只用了 `PatternEncoder` 加自定义 `Append`，却带进 chrono、serde、serde_json、parking_lot、arc-swap 一串传递依赖。自写约 50 行格式化（时间用 libc 的 `localtime_r` + `strftime`，libc 已是依赖）配合 `log::set_boxed_logger` / `set_max_level` 就能去掉。收益是体积与启动时间；改动面中等，必须保住现有行格式（`[时间] [级别] [模块] msg`，本地时间，模块剥 crate 前缀）与轮转语义。

### B6 regex 每次热重载重编译

`re:` 条目在配置加载时 `Regex::new`（common.rs:365、583），每次热重载全部重编译一遍。加一层进程级 `HashMap<String, Arc<Regex>>` 缓存即可；如果 `re:` 的实际用法只是前缀/通配，也可以自写匹配彻底去掉 regex（体积 1MB 量级 + 编译时间）。

## [anti] 明确不建议做的

- **不要上 `panic = "abort"`**，理由见 B4。
- **不要为了省 syscall 砍掉日志自愈**。logger.rs:63-66 记录得很清楚：外部删掉 `daemon.log` 后持久化句柄会写进已 unlink 的 inode，当初换成按路径 open 就是为了这个，U1 的重开路径必须保留。
- **不要在调度决策里加缓存换延迟**。40ms/160ms 的负载投喂是实时性契约，[evt_load] 里的每次判定都依赖当拍 util。
- **不要顺手改 DOWN 停摆的门控**。停摆期跳过下发的分支是逐条补出来的，性能改动碰这几处要先重读 `docs/agents/03-chiri.md` 的 DOWN 小节。

## [plan] 落地顺序

1. 加自测量（daemon 自身 utime/stime 进 status.csv 新列），跑一轮基线。
2. K1 + U3（探针 map 合并，内核与用户态同改，一次 PR）。
3. U1（日志句柄持久化，保留自愈）。
4. K2（线程记账开关）→ K3（遥测探针按需挂载，需先确认 wakeups 列消费者）。
5. B1/B2/B3（tokio 与依赖，风险低）。
6. A1、A2（分配优化，独立可回滚）。
7. B4/B5/B6（构建与依赖，最后做，需要 A/B 体积与功耗数据支撑）。

每一步都用同一场景 A/B 对比 daemon CPU 占比与亮屏平均功耗，数据不达标就回退。
