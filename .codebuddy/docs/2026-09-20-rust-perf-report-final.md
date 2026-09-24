# ChiRi Rust 性能优化综合报告（审查修订版）

> 2026-09-20 · 本文**取代**前两份过程稿：
> `2026-09-20-rust-perf-optimization-report.md`（架构/构建视角）
> `2026-09-20-rust-zero-impact-perf.md`（Rust 语言特性视角）
> 两份过程稿保留备查，一切以本文为准。本文仍未改动任何源码。
> 约束：**不得影响任何现有功能与业务**。每项都标注了影响判定。
> 修订：已完成两轮源码复核（第一轮见 [review]，第二轮见 [verify]），并按执行频率补了收益量级评估 [scale]。
>
> **权重口径（2026-09-20 用户声明）**：Yumi（`src/scheduler/`）与 Yumi 设备兼容进入**零权重**，性能优化改动即使波及也不再作为否决理由。据此 B3 从「降级」改回「待实施」——它的收益仍然很小（见 [rust] 与 [impl]），但不再因波及 Yumi 而被排除，可在后续批次与 B1 / B2 同批顺手做。

## [review] 对过程稿的审查修订

复核源码后修正 10 处，其中 3 处是结论级修正：

| #   | 原结论                                                 | 修订                                                                                                                                                                                                               |
| --- | ------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| 1   | 日志每条 6 次 syscall                                  | **5 次**：`f.flush()` 对 `File` 不产生 syscall（无用户态缓冲）。`create_dir_all` + `metadata` + `open` + `write` + `close`                                                                                         |
| 2   | cpu_monitor 每 tick 9 次 bpf 读                        | **6 次**：5 个 percpu + 1 次 tgid hash 每 tick；3 个计数 map 由 `last_stats_check` 门控，每 **2s** 一次（cpu_monitor.rs:425-433）                                                                                  |
| 3   | K3「默认只挂 migrate+cpufreq，wakeup 按需」            | **降级为需决策项**：`wakeups`/`migrations` 被 WebUI 消费（`webui/src/data/status-csv.ts:118-119` 解析第 18/19 列），默认关闭会改变界面数据，不属于零影响                                                           |
| 4   | tokio runtime「只跑一个定时器循环」                    | **结论加强**：fps_monitor 的 mio 轮询跑在独立 std 线程（fps_monitor.rs:279），该 runtime 上只剩一个 `watch` 桥接任务（fps_monitor.rs:268），几乎空转                                                               |
| 5   | `DaemonEvent` 「百来字节」                             | 约 **200 字节量级**（`RulesConfig` = 2×bool + String(24) + HashMap(48) + Vec(24) + `FasRulesConfig`，后者含 `Vec<f32>`/`Vec<ClusterProfile>`/多个 f32）。收益标注为「微小」，价值在结构卫生                        |
| 6   | K2「FAS 激活且主路径失败时置位」                       | **改为与 `FAS_FG_UTIL_ENABLED` 同条件置位**（cpu_monitor.rs:81-83）：降级路径本来就只在这条开关打开时才可能被调用（cpu_monitor.rs:326 门控）。若等到「主路径失败才开户」，第一拍读到 0 会多绕一个 tick，属行为差异 |
| 7   | A1「三处每秒一次」                                     | 精确化：1477 与 1516 是同一 if/else 链（1s 巡检块，mod.rs:1469），每秒求值其中之一；1614 在 2s 热保护块。且 clone 在条件求值里，**无论当前是否 fas 模式都执行**                                                    |
| 8   | K1 合并 percpu map                                     | 补实现约束：结构体需 `#[repr(C)]` 且两侧逐字段同布局（第三轮按实装更正：用户态用 aya 自带 `Pod`，无需 bytemuck；见 [equiv]）                                                                                       |
| 9   | hasher 风险写成「影响 `special_tuned_entries` 优先级」 | 更正：`special_tuned_entries()` 返回 `&'static [SpecialTunedEntry]`（切片，与 hasher 无关）。真实风险是 `app_modes` 这类 `HashMap` 的**迭代顺序**影响导出 yaml / 遍历的行序                                        |
| 10  | 两份稿子重复列出 i18n、devimp、`Arc<Vec>`              | 本文去重，统一编号                                                                                                                                                                                                 |

新增风险提示 2 条：**i18n 缓存必须在 `load_language` 切换语言时整体失效重建**，否则热重载切语言后仍返回旧语言；**U2 去掉 sysfs 节点的 chmod 回写会改变节点权限**，属对外可见的行为变化，默认不动。

## [verify] 二轮源码复核（本轮）

逐条回读源码后，又修正 6 处，其中 2 处改变了实施办法：

| #   | 项                         | 复核结果                                                                                                                                                                                                                                                    |
| --- | -------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| V1  | A4 的 `perf_pool.clone()`  | **结论改为「纯多余」**：`set_tid_affinity(tid, cpu_ids: &[usize])`（affinity.rs:412）形参本就是切片，1141 / 1253 的 `perf_pool.clone()` 与 1661 的 `perf_pool.to_vec()` 三处都不必要，直接传 `&perf_pool` 即可。无需动任何签名，是全部清单里最干净的一项    |
| V2  | A3 的 `Cow` 方案           | **降级**：`DevRow::set(idx, v: impl Into<String>)`（logger.rs:879）与多处传**非 `'static` 的 `&str`** 的调用点（如 `devimp_event` 的 `set(D_DECISION, kind)`）使 `[Cow<'static, str>; 40]` 不是局部改动，需同步签名与调用点生命周期。已改为更稳妥的实施办法 |
| V3  | A5 的 `orig_group`         | **收益上调**：真正的分配大头不在 1472，而在 1402-1412 —— `bg: Vec<(i32, String)>` 每 2 轮用 `group.to_string()` 为每个后台 tid 建 String（后台可达数百条），改 `Vec<(i32, &'static str)>` 后两处一起消失                                                    |
| V4  | B1 的 `ConfigReload`       | **结论加强**：两个消费方都忽略 payload —— `scheduler/mod.rs:483-485` 是 `let _ = new_rules;`，`chiri/mod.rs:2465` 是 `_new_rules`。Box 化完全安全，且枚举尺寸将由 `ModeChange` 而非 `RulesConfig` 决定                                                      |
| V5  | U1 用记账替代 `metadata`   | **补风险**：`note_write` 累计的是**本会话写入量**，外部删除/截断 `daemon.log` 后它不会归零，与真实大小会逐渐漂移。正确做法是「以记账为主 + 低频（如每 N 次或每 N KB）校验一次真实大小」，而不是完全删掉 stat                                                |
| V6  | A1 改 `.as_str()` 的锁语义 | 已验证无风险：临时 `MutexGuard` 在 `if` 条件求值结束时 drop，改后锁的持有范围从「clone 完成」延到「比较完成」，两者都在同一个条件表达式内；1477 / 1516 / 1614 三个条件里的其它调用（`fas_enabled()`、`fas_mgr.is_active()`）都不再取这把锁，无重入可能      |

另补两条诚实结论见 [scale]：A1 / A2 这类「每秒一次小分配」的真实收益是亚微秒级（daemon CPU 的 0.01% 以下），价值在代码卫生而非性能；真正的大头只可能是 [kernel] 与 U1。

## [baseline] 第一件事仍然是补基线

两稿都强调过，这里只留执行要点：仓库无 benches、无真机自测量。建议把 `/proc/self/stat` 的 utime+stime 按 1s 采样，作为**新列追加到 status.csv 末尾**（合「新增列一律追加末尾」契约）。此后每次优化都能在同一份 CSV 上对比 daemon 自身 CPU 与整机功耗，而不是靠体感。

## [table] 优化项总表

| 编号 | 项                                                | 层     | 影响判定                                           | 优先级            | 完成                                                             |
| ---- | ------------------------------------------------- | ------ | -------------------------------------------------- | ----------------- | ---------------------------------------------------------------- |
| K1   | eBPF 五个 percpu map 合并为一个 `CoreState`       | 内核   | 零影响（eBPF 产物编译期嵌入 daemon，两侧天然同批） | P0                | 已完成 · 等价                                                    |
| K2   | `THREAD_RUN_TIME` 记账加开关                      | 内核   | 零影响                                             | P0                | 已完成 · 等价                                                    |
| K3   | 遥测探针按需 attach/detach                        | 内核   | **需决策**（改 status.csv + devimp snap 的取值）   | 待定              | 未做                                                             |
| U1   | 日志句柄持久化，去掉每条的 mkdir/stat             | 用户态 | 零影响（自愈语义必须保留；代价是 8KB 巡检窗口）    | P1                | 已完成 · 等价                                                    |
| U2   | `write_to_file` 的 chmod 三连                     | 用户态 | **行为变化**，默认不动                             | 搁置              | 未做                                                             |
| U3   | 采样期 map 读取与 Vec 复用                        | 用户态 | 零影响                                             | P1（随 K1）       | 已完成 · 等价（仅 CLG 侧）；cpu_monitor 侧评估后不做（见 [evt]） |
| A1   | 为比较而 `clone()` / `to_string()`                | Rust   | 零影响（已验证无锁范围变化）                       | P2（收益 <0.01%） | 已完成 · 等价                                                    |
| A2   | 访问器返回克隆（`get_current_package` 等）        | Rust   | 零影响（加新 API，旧函数不动）                     | P3（收益 <0.01%） | 已完成 · 等价（改用 Arc&lt;str&gt; 快照）                        |
| A3   | devimp 行：先复用 join 缓冲，Cow 方案待议         | Rust   | 零影响（仅 dev 路径）                              | P2                | 未做                                                             |
| A4   | 删除 affinity 三处多余 `perf_pool.clone()`        | Rust   | 零影响（形参本就是切片，无需改签名）               | P1                | 已完成 · 等价                                                    |
| A5   | 后台候选表 group / `orig_group` 改 `&'static str` | Rust   | 零影响                                             | P2                | 已完成 · 等价                                                    |
| A5b  | affinity 核心池每轮重建 → 缓存一次                | Rust   | 零影响                                             | P3                | 跳过 · 收益可忽略                                                |
| A6   | `#[inline]` 提示（`opt-level="z"` 下）            | Rust   | 零影响                                             | P2                | 已完成 · 无语义                                                  |
| A7   | `i18n::t()` 预格式化缓存                          | Rust   | 零影响（**切语言必须重建缓存**）                   | P2                | 未做                                                             |
| B1   | `DaemonEvent::ConfigReload(Box<RulesConfig>)`     | Rust   | 零影响，收益微小                                   | P3                | 已完成 · 等价                                                    |
| B2   | `sample_one_tid` 解析去 collect                   | Rust   | 零影响（字段口径已逐项对齐）                       | P3                | 已完成 · 等价                                                    |
| B3   | `write_nodes` 等改 `&[(&str, &str)]`              | Rust   | 零影响，收益小                                     | P3                | 已完成 · 等价                                                    |
| C1   | 两个 tokio runtime → current_thread               | 运行时 | 零影响（需真机验证周期不漂）                       | P2                | 未做 · 方案已定稿（见 [pending]）                                |
| C2   | tokio features 裁剪 + 删除未使用依赖              | 构建   | 零影响                                             | P2                | 已完成 · 等价                                                    |
| C3   | `opt-level` / LTO 的 A/B                          | 构建   | 零影响（需实测取舍）                               | P3                | 未做                                                             |
| C4   | log4rs 替换为自写 logger                          | 构建   | 零影响（行格式与动态 level 已逐项保住）            | P3                | 已完成 · 等价（log4rs 依赖已移除）                               |
| C5   | regex 编译结果跨热重载缓存                        | 构建   | 零影响                                             | P3                | 未做                                                             |
| E1   | app_detect 检测：去 collect + 路径缓冲复用        | 监听   | 零影响；内容比对短路方案经复核撤销（见 [evt]）     | P1                | 已完成（缩范围）· 等价                                           |
| E2   | uevent 监听 `cpu` hotplug（power_supply 不做）    | 监听   | 零影响（收益为响应性：在线核刷新 ≤8s → ≤2s）       | P2                | 已完成（缩范围）· 等价                                           |
| E3   | logd events buffer 订阅前台切换                   | 监听   | **需真机验证** socket 可达性                       | P3（研究项）      | 未做                                                             |
| E4   | `pid_watcher` 并入 app_detect                     | 监听   | **需用户确认** 时延取舍（500ms → 1.5s）            | P3                | 未做                                                             |

## [kernel] 内核探针（唯一的高频代码）

探针跑在每次上下文切换上，8 核繁忙时 1 万~5 万次/秒，是唯一能把单次开销放大的地方。

**K1 合并 percpu map**。`yumi-ebpf/src/main.rs:79-95` 的五个 `PerCpuArray`（LAST_TIME / IDLE / BUSY / CUR_TID / CUR_TGID）在 `try_handle_sched_switch`（main.rs:166-203）里产生 5 次 map 查找，用户态每采样 5 次 `get`（cpu_monitor.rs:250-254）。合成一个 `#[repr(C)] struct CoreState` 后：内核 5→1 次查找，用户态 5→1 次 syscall 与 1→1 次 `PerCpuValues` 分配。

> 实现约束（第三轮按实装更正）：结构体 `#[repr(C)]` 且两侧逐字段同布局；用户态需 `unsafe impl aya::Pod`（aya 0.14 自带 `unsafe trait Pod: Copy + 'static`，**不是** bytemuck），内核侧 aya-ebpf 0.2.1 的 `PerCpuArray<T>` 无 trait 约束 → 实装未新增任何依赖。eBPF 产物由 `build.rs` 编译后经 `include_bytes!` 嵌入 daemon，两侧天然同批发布，不存在版本错配；map 缺失的容错沿用 `fetch_counter_map` 的「缺失即 0」语义（cpu_monitor.rs:166-190）。

**K2 线程级记账开关**。`THREAD_RUN_TIME`（32768 项，main.rs:99）每次切换无条件 `add_to_hash`（main.rs:187,211），但只有 TGID 主路径失败时的降级路径消费它（cpu_monitor.rs:374 → `compute_thread_level_util`）。加一个 `Array<u32>` 开关，探针开头读一次即决定是否记账；**开关与用户态的 `FAS_FG_UTIL_ENABLED`（cpu_monitor.rs:81-83）同条件置位**——降级路径只在这条开关打开时才可能被调用（cpu_monitor.rs:326 门控），因此语义完全等价。

**K3（待定，且是唯一会同时改动 devimp 与日志数据面的项）**。`sched_wakeup` / `sched_migrate_task` / `cpufreq_transition` 三个 tracepoint 无条件挂载（cpu_monitor.rs:88-118），其中 `sched_wakeup` 是内核最热的 tracepoint 之一。但摘除探针会改变**三处**已落盘的数据：

| 落点                                              | 位置                                   | 变化                         |
| ------------------------------------------------- | -------------------------------------- | ---------------------------- |
| status.csv 第 18/19 列                            | logger.rs:1282-1284                    | wakeups / migrations 写 `-`  |
| devimp snap 行的 wakeups/migrations/freq_trans 列 | logger.rs:1379-1381                    | 同上                         |
| WebUI 状态快照                                    | `webui/src/data/status-csv.ts:118-119` | 数值变空（`number \| null`） |

即 **devimp CSV 与 status.csv 的 schema 不变、行数不变，只是这三列取值变成 `-`**；日志摘要 `telemetry-summary`（mod.rs:1394-1396）也会变成 `0`。

正因为它是数据面变更，K3 不作零影响项推进。若要动，需要：加显式开关（如 meta 项），关闭时的取值口径写清楚，WebUI 对 `null` 的展示同步确认。

## [syscall] 用户态 syscall

**U1 日志（本档性价比最高）**。`SelfHealingAppender::append`（logger.rs:113-156）每条日志 5 次 syscall：`fs::create_dir_all`（132-135）、`current_size()` 的 `metadata`（96-98）、`open`、`write`、`close`。

改法：dir 用 `OnceLock` 建一次；持持久化 `File` 句柄（O_APPEND），与 `StatusWriter`（385）/ `DevimpWriter`（920）/ `FastWriter`（utils.rs:424）同一套路。

尺寸判定（按 V5 修正）：不要直接删掉 `metadata`。`note_write`（logger.rs:193）累计的是**本会话写入量**，外部删除或截断 `daemon.log` 后它不会归零，与真实大小会逐渐漂移，导致轮转时机算错。正确做法是「**记账为主 + 低频校验**」：平时用累计值判断，每写入 N 次或每达到某个字节步长再 `metadata` 核对一次并校正基准。

**自愈语义不能动**：logger.rs:63-66 写得很清楚，当初放弃 `RollingFileAppender` 就是因为持久化句柄在文件被外部删除后写进已 unlink 的 inode。新句柄必须在写入失败时 reopen，或按 `fstat` 的 `st_nlink == 0` / inode 变化检测重建。

**U2（搁置）**。`write_to_file`（utils.rs:21-34）每次做 `exists()` + chmod 664 + write + chmod 444。热路径已被 `FastWriter` 规避，剩下的都是冷路径；而去掉 chmod 回写会改变 sysfs 节点的最终权限，是对外可见的行为变化，默认不动。

**U3（随 K1）**。每 tick 新建 `vec![0.0_f32; max_cpu_id + 1]`（cpu_monitor.rs:256）、每 tick `Arc::new(core_utils.to_vec())`（cpu_load_governor.rs:950，Worker 不跨 tick 持有，可用双缓冲 + `Arc::get_mut` 复用）。另可把逐核循环里对 `last_idle_times[idx]` 等的重复下标换成一次性取 `&mut`，越界仍然 panic，语义不变。

## [rust] Rust 语言级（可证等价）

- **A1 为比较而分配**。
  - `chiri/mod.rs:1477 / 1516`：`mode_clone.lock().unwrap().clone() == "fas"`（1s 巡检块，mod.rs:1469 注释「1s 巡检一次」）；`mod.rs:1614` 同款在 2s 热保护块。clone 发生在条件求值里，**不论当前是否 fas 模式都会执行**。改成 `.as_str() == "fas"`，比较结果逐字节相同。
  - `chiri/mod.rs:1520`：`fas_mgr.active_pkg().map(str::to_string)` 只为和 `cur_pkg` 比较，直接比 `&str`。
- **A2 访问器返回克隆**。`app_detect.rs:75 get_current_package()` 与 `:361 last_determined_mode()` 每次 `lock().unwrap().clone()`。保留原函数不动（冷路径继续用），新增一个 `with_current_package(|s| ..)` 之类的无分配入口供 1s 周期块使用（mod.rs:1517 等）。零调用点风险。
- **A3 devimp 行（收益集中在 dev 路径，实施办法已按 V2 修正）**。`logger.rs:858` `struct DevRow([String; 40])`，`new()` 里 `std::array::from_fn(|_| NA.to_string())`（865）每个 "-" 都是独立堆分配，加上 `join(",")`（1154）。
  稳妥实施按优先级二选一：
  1. **先做这个**：`join` 的结果改为写入一个复用的 `String` 缓冲（清空后逐列 `push_str`），省掉每次 `join` 的一次分配，不动 `DevRow` 任何类型，零调用点影响；
  2. 若要连 40 次初始化分配一起省，再改 `[Cow<'static, str>; 40]`，同时必须处理：`set()` 签名改 `impl Into<Cow<'static, str>>`（logger.rs:879），以及所有传**非 `'static` 生命周期 `&str`** 的调用点（如 `devimp_event` 的 `set(D_DECISION, kind)`、`set(D_REASON, reason)`）需要 `.to_string()` 或改用 `'static` 字面量。
     输出字节必须逐字符一致，改完用同一份 devimp 样本做 diff。
- **A4 affinity 的多余 clone（按 V1 修正，当前零风险首选）**。`set_tid_affinity(tid, cpu_ids: &[usize])`（affinity.rs:412）形参已是切片，因此 1141、1253 的 `let perf = perf_pool.clone();` 与 1661 的 `perf_pool.to_vec()` 三处全是多余的 Vec 复制，删除后直接传 `&perf_pool`（`Vec<usize>` 自动 deref），语义与结果完全一致。
  另：`affinity.rs:994-1002` 每轮多次 `ranges.*.clone().collect()` 重建核心池，可在 `AffinityManager` 里缓存一次（`chiri_core_ranges()` 进程内恒定）。选核逻辑与结果不变。
- **A5 后台候选表的 group String（按 V3 上调收益）**。`affinity.rs:1402` `bg: Vec<(i32, String)>`，1412 对每个后台 tid 做一次 `group.to_string()`（后台常达数百条，每 2 轮全量重建一次）；1472 建档时再 `group.clone()` 一次。常量数组 `BACKGROUND_GROUPS` 的元素是 `&'static str`，把 `bg` 改成 `Vec<(i32, &'static str)>`、1412 直接存 `group`、同时把 `ThreadState.orig_group`（550）改成 `&'static str`，这两处分配一起消失。`cleanup_thread` / `demote` 用它写回 cpuset 路径，取值集合不变。
  （同一函数里 `unpin_core` 的 914 `let all: Vec<usize> = (0..max).collect()` 也是每次分配，可一并归入本项。）
- **A6 `#[inline]`**。`opt-level="z"` 下 LLVM 内联阈值保守，给 `percpu_total`（cpu_monitor.rs:30）、`max_util` 与 `find_nearest_freq`（cpu_load_governor.rs:121/61）加 `#[inline]`。纯优化器提示，零语义。
- **A7 i18n**。`i18n.rs:78 t()` 每条日志一次 RwLock 读 + Fluent 解析 + String 分配。无参消息可在 `load_bundle` 时预格式化进 `OnceLock<HashMap<&'static str, String>>`。**风险：语言切换（`load_language`）必须整体重建这份缓存**，否则热重载切语言后仍返回旧语言。
- **B1 `DaemonEvent` 装箱（按 V4 修正）**。`common.rs:41 ConfigReload(RulesConfig)` 把枚举撑到最大变体尺寸，而最高频的 `SystemLoadUpdate`（40ms 一次）每次发送都按整枚举尺寸搬内存。
  已确认**两个消费方都不使用 payload**：`scheduler/mod.rs:483-485` 是 `let _ = new_rules;`（Yumi 侧 FAS 暂禁用），`chiri/mod.rs:2465` 是 `_new_rules`；唯一发送方是 `app_detect.rs:416`。
  改 `ConfigReload(Box<RulesConfig>)`：发送处 `Box::new`，枚举尺寸从约 200B 降到由 `ModeChange` 决定的约 64-72B。收益仍然很小（每次省百来字节的搬运），但因为 payload 无人使用，风险为零。
  （更进一步可以彻底不带 payload，但那是事件契约变更，不在本轮「零影响」范围内。）
- **B2 `sample_one_tid`**。`affinity.rs:517-530` 每次读 `/proc/<tid>/stat` 都要 `format!` 路径 + `read_to_string` + `split_whitespace().collect::<Vec<&str>>()`（约 40 个 `&str`）。后台深扫每轮 64 次。改用复用路径缓冲 + 迭代器定位第 11/12 字段并同时计数，保留 `tokens.len() < 37` 的等价判定。**验证要求：字段索引口径必须逐字对齐（`)` 后切分，[11]/[12] 是 utime/stime）**，用同一份 `/proc` 样本对比输出。
- **B3 签名去 String**。`utils.rs:74 write_nodes(&[(String, String)])`、`affinity.rs:195 set_groups_cpus_tracked` 改成 `&[(&str, &str)]`。写入内容与顺序不变，收益一般（省掉中间层的 String 克隆），主要是消除这类 API 的分配惯性。
  调用点分两类：路径是**运行时拼接**的（`scheduler/scheduler.rs:94`、`scheduler/fas/policy_mgmt.rs:257-259`、`chiri/scheduler.rs:251` 等）改签名后仍需先持有 String，只能省掉中间层的克隆；路径是**字面量**的（`chiri/scheduler.rs:263-273` 的 touch_boost 四项等）可彻底去掉 `to_string()`。
  **Yumi 权重归零后不再缩范围**：`src/scheduler/` 的两处调用点一并做类型适配即可（只改参数类型，不动逻辑），不必再为「不动 Yumi 目录」保留旧签名。

## [build] 运行时与构建

- **C1 tokio runtime**。`monitor/mod.rs:168 / 193` 各建一个默认多线程 runtime（8 核约 16 工作线程）。实际负载：cpu_monitor 只有一个 `interval` 循环加一个 `tokio::spawn` 的 PID 桥接（cpu_monitor.rs:188）；**fps_monitor 的 mio 轮询跑在独立 std 线程（fps_monitor.rs:279），runtime 上只剩一个 `watch` 桥接任务（fps_monitor.rs:268）**。改成 `Builder::new_current_thread()`（甚至进一步去掉 tokio）收益明确。需在真机确认采样周期不漂。
- **C2 依赖**。`tokio` 开了 `features = ["full"]`，实际只用到 rt/time/sync/macros；`itoa` / `bytes` / `futures` / `glob` 在 `src/` 里找不到任何 `use`（Cargo.toml:36-38、50），可直接删；`lazy_static`（app_detect.rs:64）与 `once_cell`（logger.rs:13、i18n.rs:6）并存，统一到 std `LazyLock` / `OnceLock`。
- **C3 构建剖面**。`opt-level="z"` 为体积牺牲内联/展开/向量化，对配置解析、affinity 扫描这类循环是实打实的；建议 `"s"` 与 `3` 各编一版比体积与 8550 实测功耗后再定。`lto = true`（fat）改 `lto = "thin"` 能显著缩短 CI 时间。
- **C4 log4rs**。只用了 `PatternEncoder` + 自定义 `Append`（logger.rs:8-12、113），却带进 chrono / serde / serde_json / parking_lot 一串传递依赖。自写约 50 行格式化（时间用 libc 的 `localtime_r` + `strftime`）+ `log::set_boxed_logger` / `set_max_level` 可替换。保住两件事：行格式 `[时间] [级别] [模块] msg`（本地时间、模块剥 crate 前缀）与运行时动态改 level（现由 `LOG_HANDLE`（logger.rs:40）承担）。
- **C5 regex**。`re:` 条目每次配置热重载都 `Regex::new` 重编译一遍（common.rs:365、583）。加一层进程级 `HashMap<String, Arc<Regex>>` 缓存。

## [scale] 收益量级：别把「代码卫生」当成性能

按执行频率把清单分层，避免误判优先级（daemon 自身 CPU 才是可优化上限；8550 实测结论是「调度层剩余空间是个位数 %」）：

| 层  | 频率                                         | 项          | 量级判断                                                                                                                    |
| --- | -------------------------------------------- | ----------- | --------------------------------------------------------------------------------------------------------------------------- |
| L0  | 每次上下文切换，繁忙时 1 万~5 万次/秒        | K1、K2、K3  | **唯一可能带来可测整机收益的一档**，但每一项仍是「每次省一次 map 查找」级别，需实测确认落在设备 CPU 的具体百分点            |
| L1  | 每日志行（INFO 数十/s，DEBUG/devimp 数百/s） | U1、A7、A3  | 一次 std::fs 写 5 次 syscall → 1 次，在 dev/DEBUG 下差异明显；INFO 稳态下不显著                                             |
| L2  | 每负载 tick（6~25 次/s）                     | U3、A6      | 每次省几笔小分配/拷贝，亚毫秒级                                                                                             |
| L3  | 每 1~2s 一次                                 | A1、A2、A5b | **每秒 1 次堆分配 ≈ 亚微秒/秒，占 daemon CPU 不到 0.01%**。它们的价值是代码卫生（消除「为比较而克隆」这类反模式），不是性能 |
| L4  | 每 2~4s 批量，按线程数放大（可达数百次/轮）  | A4、A5      | 单轮几十到几百次小分配，属于「值得顺手清理」，但不是瓶颈                                                                    |

结论：**只有 L0 与 L1 值得单独安排改动窗口并做 A/B 测量**；L2 建议搭在 L0/L1 的同一批改动里；L3 不要单独立项，顺手做。

## [evt] Android 事件触发器调研（2026-09-20）

目标：在不改变任何行为的前提下进一步降低监听开销。结论先行：**监听层主体已经事件化**，真正还剩的只有一项零依赖改动（E1）；其余要么受采样契约约束，要么收益量级太小，要么需要先做真机验证。

### 现状监听点清单

| 监听点                                   | 触发方式                        | 周期                 | 数据源                                 | 事件化          |
| ---------------------------------------- | ------------------------------- | -------------------- | -------------------------------------- | --------------- |
| `screen_watcher`（monitor/mod.rs:90）    | 阻塞读 `NETLINK_KOBJECT_UEVENT` | —                    | backlight / leds / power               | 是              |
| `config_watcher`（:107）                 | inotify 阻塞读                  | —                    | rules.yaml 目录                        | 是              |
| `down_watcher` / rhine watcher           | inotify                         | —                    | down.chr / rhine.chr                   | 是              |
| `fps_monitor_ebpf`（:163）               | mio `Poll` on RingBuf fd        | —                    | eBPF uprobe 帧事件                     | 是              |
| `cpu_monitor_ebpf`（:192）               | tokio `interval`                | 160ms / 40ms（特调） | eBPF map 读取                          | 周期采样        |
| `telemetry_monitor`（:218）              | `thread::sleep`                 | 1s                   | PSI / GPU / 电池                       | 周期采样        |
| `pid_watcher`（:129）                    | `thread::sleep`                 | 500ms                | 进程内原子量                           | **否，纯轮询**  |
| `app_detection_loop`（:224，阻塞主线程） | `thread::sleep`                 | 1.5s 亮 / 1s 息      | cgroup `tasks` + `/proc/<pid>/cmdline` | **否，纯轮询**  |
| 调度循环周期块（chiri/mod.rs）           | mpsc `recv_timeout` + deadline  | 1s / 2s / 5s         | 自身状态                               | 事件驱动 + 兜底 |

两点说明：`cpu_monitor` 与 `telemetry` 不宜事件化，前者是 eBPF 采样的本质，后者要满足 status.csv「每秒一行」的契约。`app_detection_loop` 的单轮开销最大：一次 cgroup `tasks` 读，加通常 1~2 次 `/proc/<pid>/cmdline` 读（倒序扫描命中即止，app_detect.rs:161-167）。

### B1 可低成本扩展（已有基础设施）

- **`cpu` 子系统 uevent（CPU hotplug）**：`/sys/devices/system/cpu/cpuN/online` 变更会广播 uevent。可用于把 affinity 的在线核位图刷新（现每 4 轮约 8s）改为「事件即时刷新 + 低频兜底」。**收益偏响应性**（热插拔感知从 ≤8s 收敛到即时），开销节省很小：每 8s 省一次 `/sys/devices/system/cpu/online` 读取。
- **`power_supply` uevent（充电状态）**：status / capacity 变化时驱动广播 uevent（Android 上常见）。可把 `read_battery_charge_state()` 的每秒固定读改成「缓存 + uevent 失效」。**开销节省在微秒级/秒**，价值主要在充电切换的瞬时感知。
- **`drm` / `graphics` uevent**：可作为屏幕状态的补充源，接入现有多源投票与退役切换（screen_detect.rs 已有 `tally_screen_nodes` 与 `retire_primary_and_switch`）。属稳健性增强，不是开销优化。

落地情况（2026-09-20）：**只做了 `cpu` hotplug 一项**——uevent 线程收到 `cpu` 子系统事件即置 `CPU_HOTPLUG_DIRTY`，affinity 下一轮（≤2s）立即刷新在线核位图；**周期兜底（每 4 轮 / len 不符）完整保留**，所以机型不广播 cpu uevent 时行为与改造前完全一致。`power_supply` 与 `drm/graphics` 仍不排期：前者要配合缓存才有开销收益，会让 status.csv 的 charge 列在 uevent 不广播时滞后，属于数据面变化而收益只有微秒级/秒；后者属稳健性增强而非开销优化。

### B2 零依赖纯优化（推荐先做）

- **E1（实施时缩范围）**：`check_cgroup_path` 去掉 `collect::<Vec<_>>()`，反向直接迭代 `split_whitespace()`（`DoubleEndedIterator`），并复用同一个 `/proc/<pid>/cmdline` 路径缓冲。
  原方案「tasks 内容未变则复用上轮判定」在实施前复核时被否掉，两条原因：① 稳态开销本来就只有 1 次 cgroup 读 + 1 次 cmdline 读（倒序命中即返回），可省的只有那一次；② cgroup procs 内容在应用冷启动期间可能不变（pid 先入组、exec 后 cmdline 才可读），按内容复用会漏检这类前台切换，若再加时间上限则收益归零。

### B3 调研过但不适用（留痕以免重复评估）

- **cgroup v2 `cgroup.events` 的 `poll()`**：内核支持对 `cgroup.events` 做 poll，但唤醒条件是 `populated` 字段 0↔1，只在「组从空变非空 / 非空变空」时触发。前台 A→B 切换是同组内的进程替换，`populated` 不变，**替代不了前台检测**；且目标机型仍走 `/dev/cpuset`、`/dev/cpuctl`（cgroup v1）。最多覆盖「从桌面进入/退出应用」这类跨组场景。
- **`logd` reader socket（`/dev/socket/logdr`）订阅 events buffer**：events buffer 含 `am_focused_activity`（Android 10+）等 tag，是延迟最低（约 10ms）的前台切换源，可与「把 cgroup 轮询周期拉长到 5s」组合，省掉约七成轮询。代价：要实现 liblog reader 协议（`logger_entry` 二进制帧，约 150-250 行），且**必须先真机验证 socket 的 SELinux 可达性**（模块跑在 su 域，通常可行但无保证）与 tag 名的版本差异。列为研究项，验证前不投入。
- **PSI 的 `poll()`（`/proc/pressure/*`）**：内核 5.2+ 支持（需注册 trigger），但本项目的 PSI 是「1s 连续采样写入 status.csv」的连续量，事件驱动替代不了采样。不适用。
- **thermal uevent**：`thermal_zone` 的 uevent 广播依赖驱动实现（不少平台由 vendor userspace 轮询温度），不可靠；温度也没有更轻的事件源。保留 2s 轮询。
- **timerfd + epoll 合并全部周期任务**：技术上可行（timerfd 替代 `thread::sleep`，epoll 多路复用 inotify / netlink / timerfd），但调度循环是 mpsc + deadline 驱动、周期任务分散在多个线程，改造面大，收益主要是线程数与定时精度。列为长期项。

### 待真机验证清单

以下结论尚未在设备上确认，实施前必须先验证：① `cpu` 与 `power_supply` 子系统在本机型是否真的广播 uevent；② `/dev/socket/logdr` 是否可读（SELinux）；③ 各 Android 版本的 events buffer tag 名差异。

## [forbid] 禁区（看着像优化，实际有语义风险）

- **`panic = "abort"` / `"immediate-abort"`**：毁掉 `catch_unwind` 的 panic 自愈（chiri/mod.rs:974、scheduler/mod.rs:269、monitor/mod.rs:34）。
- **换 HashMap hasher**：改变迭代顺序，风险在 `app_modes` 这类 `HashMap` 影响导出 yaml / 遍历行序。（`special_tuned_entries()` 是切片，不受影响。）
- **换 `parking_lot`**：去掉锁毒化语义，而多处依赖 `unwrap_or_else(|p| p.into_inner())` 在 panic 后继续服务（i18n.rs:79、logger.rs:121）。
- **热路径加值缓存**：任何「缓存一次、下次直接用」都会让判定滞后一拍，与 40ms/160ms 实时投喂契约冲突。唯一允许缓存的是 A4（进程内恒定的核心区间）与 U3（缓冲区复用，不是值缓存）。
- **默认摘除 `sched_wakeup` 探针**：WebUI 消费该列，属业务影响（见 K3）。
- **去掉 sysfs 节点的 chmod 回写**：改变节点权限，对外可见（见 U2）。
- **`sort_by` → `sort_unstable_by`**：只放行 `cpu_monitor.rs:473` 的 tgtop top5（纯日志排序）。其余必须先确认是否有并列项依赖稳定顺序。

## [plan] 落地顺序

1. 加自测量（utime/stime 进 status.csv 新列），跑基线。之后每一步都按 [scale] 的分层决定要不要单独安排测量窗口。
2. **A4 → A5**：affinity 的三处多余 `perf_pool.clone()` / `to_vec()` 与后台候选表的 group String。纯删除 + 常量化，零签名改动，性价比最高。
3. **U1**：日志句柄持久化 + 尺寸「记账为主、低频校验」；自愈语义逐条对照 logger.rs:63-66 验收（含手动删 `daemon.log` 后能否重建）。
4. **K1 + U3**：探针五个 percpu map 合并 + 采样期缓冲复用（eBPF 产物编译期嵌入 daemon，两侧天然同批，无需额外同步步骤）。
5. **K2**：线程记账开关，与 `FAS_FG_UTIL_ENABLED` 同条件置位。
6. **C1 → C2**：tokio 改 current_thread（真机确认采样周期不漂）+ 依赖裁剪。
7. **A3（先只做 join 缓冲复用）→ A7**（i18n 预格式化缓存，注意 `load_language` 重建）。
8. **顺手项，不单独立项**：A1、A2、A5b、A6 —— 可搭在任意一次 affinity / mod.rs 的改动里，不必单独提交。
9. **B1 → B3 → B2**：签名类与解析改写，B2 需用同一份 `/proc` 样本对比输出。
10. **C3 / C4 / C5**：构建与依赖，等体积与功耗的 A/B 数据支撑。
11. **K3** 待用户决策（WebUI 消费 wakeups 列）后再定。

## [equiv] 等价性复核（第二轮源码核对）

对已落地的 11 项逐条回读改后源码，**全部等价，未发现需要修正的逻辑**。

| 项  | 结论                | 关键证据                                                                                                                                                                                                                                                                                            |
| --- | ------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --- |
| K1  | 等价                | 探针 `state.last_time/idle/busy/cur_tid/cur_tgid` 与原 5 个 map 一一对应、写序一致；用户态取值元组 `(s.idle, s.busy, s.last_time, s.cur_tid)` 与原 raw_idle / raw_busy / last_switch_time / current_tid 逐一对应（cpu_monitor.rs:291-295）                                                          |
| K2  | 等价                | 开关与 `FAS_FG_UTIL_ENABLED` 同源同生命周期（都在 `start_cpu_loop` 启动时定值），「降级路径可能被调用」等价于「记账已开」；旧产物缺 map 时只告警并保持恒记账（等于原行为）。净开销：1 次 Array 查找换掉 1 次 32768 项 hash 查找                                                                     |
| U3  | 等价                | `Arc::get_mut` 成功即无其它持有者，原地 `copy_from_slice` 不可能被观测；长度变化或仍被持有时新建。两条路径下 Worker 收到的内容与长度与原实现一致（cpu_load_governor.rs:952-973）                                                                                                                    |
| A1  | 等价                | `.as_str() == "fas"` 与 clone 后比较同值；临时 `MutexGuard` 仍在 `if` 条件求值结束时 drop，锁范围不变                                                                                                                                                                                               |
| A2  | 等价                | `get_current_package()` 由 `clone()` 改 `to_string()`（同内容 String）；`set_current_package` 由 `to_string()` 改 `Arc::from`；1s 巡检块内 `cur_pkg: &str` 经解引用后比较与传参语义不变；`active_pkg().map(str::to_string)` 保留（后续 `activate(&mut)` 借用所需）；快照生命周期覆盖整个 else-if 块 |
| A4  | 等价                | `set_tid_affinity(tid, cpu_ids: &[usize])` 形参本就是切片，被删的 `clone()` / `to_vec()` 只是复制品                                                                                                                                                                                                 |
| A5  | 等价                | `orig_group: &'static str` 取值仍是那 4 个常量（affinity.rs:552/974/1082/1477/1705）；`bg` 直接借用 `BACKGROUND_GROUPS` 常量字面量；写入 cpuset 的路径字符串内容不变                                                                                                                                |
| A6  | 无语义              | 纯优化器提示                                                                                                                                                                                                                                                                                        |
| B1  | 等价                | 两个消费方本就忽略 payload（`scheduler/mod.rs:483-485` 的 `let _ = new_rules;`、`chiri/mod.rs:2465` 的 `_new_rules`），`Box::new` 只改存放位置，发送的仍是同一份配置                                                                                                                                |
| C2  | 等价                | tokio 仅裁剪未用 feature；被删的 4 个依赖无任何 `use`；`LazyLock` / `OnceLock` 与原 `once_cell` 的 `Lazy` / `OnceCell` 同语义（`set` 返回值与 `Deref` 一致），编译通过且无 unused 警告                                                                                                              |
| B3  | 等价                | `write_nodes` 改 `AsRef<str>` 泛型后，两类调用点传入的是同一份路径/值文本：写入内容与顺序不变，返回的 `written` 仍是 `Vec<String>`，日志口径（debug/warn 里的 `{path}`）不变；两个字面量调用点的 4+2 个路径字符串与改造前逐字节相同                                                                 |     |
| U1  | 等价（含 8KB 窗口） | 稳态每条日志从 5 次 syscall 降到 1 次 write；行内容与 `note_write` 字节口径不变（恒为编码后长度）；尺寸判定改为「记账 + 每 8KB metadata 校正」，轮转阈值仍 50MB；被删/轮转时丢弃句柄后按路径重建——**唯一差异**是外部删除的检测窗口由「每条一次 stat」变为「累计 8KB 一次」（最坏丢 8KB 日志）       |
| B2  | 等价                | 字段口径逐项对齐：`count` 等于原 `tokens.len()`（`< 37` 判定不变），utime/stime 仍取 `)` 之后第 11/12 个字段，parse 失败同样按 0 计                                                                                                                                                                 |
| C4  | 等价                | 行格式逐字段复刻：`[本地时间] [LEVEL] [模块剥 chiri:: 前缀] 消息\n`；`update_level` 改为 `log::set_max_level`（过滤时机与原先 `set_config` 后打点一致）；轮转、自愈、日志量记账仍由同一 appender 承担                                                                                               |
| E1  | 等价                | 反向迭代顺序与原 `collect().iter().rev()` 完全一致，路径字符串内容不变；**明确不做内容缓存**，冷启动漏检风险不存在                                                                                                                                                                                  |
| E2  | 等价                | 只新增一个 uevent 分支（置原子标记）与一个刷新条件；周期兜底未动，机型不广播 uevent 时行为与改造前一致；在线核位图仍取自同一个 `online_bitmap(max_cpu)`                                                                                                                                             |

**复核确认的两条口径更正**（原报告表述有误，见 [kernel] 与 [impl] 两节的同步修订）：

- K1 原写「跨二进制契约需同步」不成立：eBPF 产物由 `build.rs` 编译后经 `include_bytes!` **嵌入 daemon 二进制**，两侧天然同批发布，不存在版本错配。
- 原 [kernel] 节写「需要 `bytemuck::Pod` / `Zeroable`」不准确：用户态用的是 aya 0.14 自带的 `unsafe trait Pod`（`aya-0.14.0/src/bpf.rs:45`），内核侧 aya-ebpf 0.2.1 的 `PerCpuArray<T>` 无 trait 约束，实装未新增任何依赖。

**如实记录的一处边界差异**：`CORE_STATE.get_ptr_mut` 失败时探针提前 `return Ok(0)`（原实现会继续尝试写 3 个 percpu map）。per-cpu array 查找失败在正常内核上不可达，且新结构下那几次写本属同一份数据，无可观测差异。

**未实测**：以上均为源码级核对；真机行为与性能 A/B 仍未做（需 CI 产物 + 8550 实机）。

## [impl] 实施记录（2026-09-20）

本批实施 11 项；B3 在 Yumi 零权重口径下补做完成，A5b 跳过。后续又补做了 **U1 / B2 / C4 / E1 / E2**（见下表末五行）。落地方式与偏差：

| 项  | 落地方式                                                                                                                                                                                                        | 与报告方案的偏差                                                                                                                                                                                                                                                                                                                                                                                                                               |
| --- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| K1  | eBPF 五个 percpu map 合并为 `#[repr(C)] CoreState`（32B：u64×3 + u32×2，无 padding），探针单次 `get_ptr_mut` 写全部字段，用户态单次 `get` 后按字段取值                                                          | **无需新增 bytemuck**：用户态 aya 0.14 自带 `unsafe trait Pod`（`aya-0.14.0/src/bpf.rs:45`），内核侧 aya-ebpf 0.2.1 的 `PerCpuArray<T>` 无 Pod 约束                                                                                                                                                                                                                                                                                            |
| K2  | 新增 `Array<u32> THREAD_ACCT` 开关；探针 busy 分支记账前读一次；用户态在 `FAS_FG_UTIL_ENABLED` 置位处写 1                                                                                                       | 旧产物缺该 map 仅告警，探针保持恒记账（与版本偏差容错语义一致）                                                                                                                                                                                                                                                                                                                                                                                |
| U3  | CLG 加 `load_buf: Option<Arc<Vec<f32>>>` 双缓冲：take → `Arc::get_mut` 成功则 `copy_from_slice` 原地复用，否则新建                                                                                              | cpu_monitor 侧 `core_utils` 因 move 进事件不改（见计划）                                                                                                                                                                                                                                                                                                                                                                                       |
| A1  | `chiri/mod.rs` 三处 `mode_clone...clone() == "fas"` → `.as_str() == "fas"`                                                                                                                                      | 同报告                                                                                                                                                                                                                                                                                                                                                                                                                                         |
| A2  | `CURRENT_PACKAGE` 改存 `Arc<str>`，新增 `current_package_arc()` 零分配快照；1s 巡检块（mod.rs:1517）改用它                                                                                                      | **改用 Arc 快照而非闭包入口**：闭包会把锁持有期拉长到覆盖整段巡检逻辑（内含 IO/日志），Arc 快照零持锁风险。`:1520` 的 `active_pkg().map(str::to_string)` **保留**——去掉会与后续 `fas_mgr.activate(&mut)` 借用冲突（这正是原代码先 to_string 的原因）                                                                                                                                                                                           |
| A4  | 删除三处多余 `perf_pool.clone()`/`to_vec()`，直接传 `&perf_pool` / `perf_pool`（形参本就是切片）                                                                                                                | 同报告（V1）                                                                                                                                                                                                                                                                                                                                                                                                                                   |
| A5  | `bg: Vec<(i32, &'static str)>`、`ThreadState.orig_group: &'static str`，同步 4 处读取/赋值点                                                                                                                    | 同报告（V3）                                                                                                                                                                                                                                                                                                                                                                                                                                   |
| A6  | `percpu_total` / `find_nearest_freq` / `max_util` 加 `#[inline]`                                                                                                                                                | 同报告                                                                                                                                                                                                                                                                                                                                                                                                                                         |
| B1  | `ConfigReload(Box<RulesConfig>)`，发送处 `Box::new`；两个消费方本就忽略 payload，无需改动                                                                                                                       | 同报告（V4）                                                                                                                                                                                                                                                                                                                                                                                                                                   |
| C2  | tokio features 裁为 `["rt-multi-thread","time","sync"]`；删 `itoa`/`bytes`/`futures`/`glob`；`lazy_static` → `std::sync::LazyLock`，`once_cell::{Lazy,OnceCell}` → `std::{LazyLock,OnceLock}`，两个依赖声明删除 | logger 的 OnceCell 用 `std::sync::OnceLock`（std 没有 `sync::OnceCell`）；tokio 必须保留 `rt-multi-thread`（`Runtime::new()` 是多线程调度器）                                                                                                                                                                                                                                                                                                  |
| B3  | `write_nodes` 与 `snapshot_nodes` 泛型化为 `AsRef&lt;str&gt;`；两处字面量调用点（touch_boost 四项、perfmgr 两项）改 `(&str, &str)` 数组                                                                         | **没有把签名改窄成 `&[(&str, &str)]`**：路径是运行时拼接的调用点（`/sys/block` 的 queue 参数、cpuset/cgroup 组、cpu online 等）必须持有 String，强改会让调用方先建一层 `Vec<(&str, &str)>` 中转，反而多一次分配。改用 `AsRef<str>` 泛型后，这两类调用点都能直接传：拼接类保持 `Vec<(String, String)>` 零 diff，字面量类彻底去掉 `to_string()` 与 Vec 分配。按 Yumi 零权重口径一并改了 `scheduler/fas/policy_mgmt.rs`（只做类型适配，不动逻辑） |
| A5b | **跳过**                                                                                                                                                                                                        | 每 2s 重建 3 个小 Vec，收益可忽略，改为结构体字段反而增加改动风险                                                                                                                                                                                                                                                                                                                                                                              |
| U1  | `SelfHealingAppender` 改常驻句柄：`state: Mutex<AppendState> { file, size, since_verify }` + `append_verify` / `append_open`，稳态每条日志只剩一次 write                                                        | 新增 `LOG_VERIFY_BYTES = 8KB` 低频巡检；`note_write` 仍在锁外、字节口径不变；**有意权衡**：外部删除后最坏丢 8KB 日志（已写入代码注释）                                                                                                                                                                                                                                                                                                         |
| B2  | `sample_one_tid` 去掉 `collect::<Vec<&str>>()`，单次遍历同时取 utime / stime 与字段总数（`< 37` 判定保留）                                                                                                      | 路径 `format!` 的缓冲复用未做：需改 3 处调用点签名，收益仅每次一次小分配                                                                                                                                                                                                                                                                                                                                                                       |
| C4  | 移除 log4rs：自写 `log::Log`（`ChiriLogger`）+ `format_line` 纯函数 + `libc::localtime_r` 本地时间；`Cargo.toml` 删 `log4rs`，`log` 开 `std` feature（`set_boxed_logger` 需要）                                 | 行格式逐字节复刻原 pattern；`chiri::` 前缀裁剪保留；`update_level` 改 `log::set_max_level`                                                                                                                                                                                                                                                                                                                                                     |
| E1  | `check_cgroup_path` 反向直接迭代 `split_whitespace()`（`DoubleEndedIterator`）+ 复用 cmdline 路径缓冲                                                                                                           | **原「内容未变则复用上轮判定」被否**：稳态本就只有 1 次 cgroup + 1 次 cmdline 读；冷启动期 pid 先入组、exec 后 cmdline 才可读，按内容复用会漏检（详见 [evt]）                                                                                                                                                                                                                                                                                  |
| E2  | uevent 线程加 `cpu` 子系统分支 → 置 `CPU_HOTPLUG_DIRTY`；affinity 在线核刷新条件加 `dirty.swap(false, ...)`                                                                                                     | 只做 `cpu` hotplug；**周期兜底完整保留**（机型不广播 uevent 时行为与改造前一致）；`power_supply` 不做（会让 status.csv 的 charge 列滞后，属数据面变化而收益仅微秒级）                                                                                                                                                                                                                                                                          |

### 验证

- `cargo +nightly check -p chiri --target aarch64-linux-android`：逐批通过；最终无 error、无 warning（原有的 `unused dependency futures/glob` 警告随 C2 一并消失）。
- `cd yumi-ebpf && cargo +nightly check --target bpfel-unknown-none -Z build-std=core`：K1/K2 内核侧类型检查通过。**本机 build.rs 无 bpf-linker 时走 `write_ebpf_stub` 占位**，eBPF 的真实编译与 verifier 检查仍需 CI。
- LSP diagnostics：改动文件 0 报错。
- 落盘面：本批未触碰 status.csv / devimp.csv 的内容与格式（K3 未做）；WebUI、`module/` 模板、脚本均未改。**daemon.log 的行格式与轮转命名保持不变（C4 逐字段复刻原 pattern），U1 的 8KB 巡检窗口是日志语义上的唯一有意权衡**。
- **未实测**：真机行为与性能 A/B（需 CI 产物 + 8550 实机）；U1 的三条自愈场景（外部删文件 / 删整个目录 / 写满轮转）与 C4 的行格式字节级一致性，也需要真机或 CI 产物后端到端验证。

### 文件清单

`Cargo.toml`、`Cargo.lock`、`src/common.rs`、`src/i18n.rs`、`src/logger.rs`、`src/utils.rs`、`src/monitor/mod.rs`、`src/monitor/app_detect.rs`、`src/monitor/cpu_monitor.rs`、`src/monitor/screen_detect.rs`、`src/chiri/mod.rs`、`src/chiri/affinity.rs`、`src/chiri/cpu_load_governor.rs`、`src/chiri/scheduler.rs`、`src/scheduler/fas/policy_mgmt.rs`、`yumi-ebpf/src/main.rs`。（`src/scheduler/` 下两个文件仅做类型适配与字面量改造，未动 Yumi 逻辑）

每步单独提交，`cargo check -p chiri --target aarch64-linux-android` 通过后用同一场景 A/B 对比 daemon CPU 与亮屏功耗，数据不达标就回退。

## [pending] 已定稿待实施

**U1 已于 2026-09-20 实施完成**（落地方式与偏差见 [impl]，等价性见 [equiv]），下面保留其定稿方案作为实现依据。
**C1 仍待实施**：方案已定稿（含验收口径），代码尚未落地，实施后按 [equiv] 的口径补等价性复核。

### U1 日志落盘：常驻句柄 + 8KB 低频巡检（已实施）

现状：`SelfHealingAppender::append`（logger.rs:113-156）每条日志 5 次 syscall —— `create_dir_all`（每条一次 mkdirat）、`current_size()` 的 `metadata`（每条一次 stat）、`open` / `write` / `close`（`flush` 对 `File` 不产生 syscall）。

方案（形状对齐同文件已有的 `StatusWriter` / `status_check`，logger.rs:378-423）：

- `lock: Mutex<()>` 换成 `state: Mutex<AppendState>`，`AppendState { file: Option<fs::File>, size: u64, since_verify: u64 }`（该 Mutex 同时充当原来的串行化锁）。
- 新增 `LOG_VERIFY_BYTES = 8 * 1024`；新增 `append_verify`（低频巡检：`metadata` 校正尺寸；超限则先丢句柄再 `rotate`；路径消失则丢句柄并置零）与 `append_open`（`create_dir_all` 只在这里做，`create + append` 打开，并用 `metadata` 校正尺寸）。
- `append` 流程：取锁 → 编码（失败仅丢弃本条）→ `since_verify` 超阈值则巡检 → 句柄缺失则重开 → `write_all + flush`（成功累加 `size` / `since_verify`，失败丢句柄、下条重试）→ **锁外** `note_write`。
- 删除 `current_size()`（唯一调用点在 append 内）。

必须保住的三条不变量：`note_write` 的字节口径恒为编码长度（决定「日志目录预算 / 16MB 打包门限」的触发点）；`note_write` 必须在锁外（其内部会 `log::info!` 并可能 `exit(0)`，持锁重入会死锁）；轮转前先丢句柄（否则继续写已改名的 inode）。

四条验收场景：① 外部删 `daemon.log`（保留 logs/）→ ≤8KB 窗口内重建；② 外部删整个 `logs/` → 目录与文件一并重建；③ 写满 `max_bytes` → 先丢句柄再 rename，新文件由后续 open 建立；④ 运行期 `update_level` 重建 appender → 首次 append 用 `metadata` 校正尺寸，不会因记账从 0 开始而把文件写超上限。

有意权衡：外部删除的检测窗口从「每条一次 stat」变为「累计 8KB 一次」，最坏丢 8KB 日志（约 60 行）；相对 50MB 上限与 16MB 打包门限可忽略，换来的是稳态每条日志只剩一次 write。

### C1 两个 tokio runtime 改 current_thread

现状：`monitor/mod.rs:168`（fps）与 `:193`（cpu）各 `tokio::runtime::Runtime::new()`，默认多线程、worker 数等于 CPU 核数；8 核设备即每 runtime 8 个 worker 线程，而实际负载分别是「一个 watch 桥接任务」与「一个 interval 循环 + 一个 watch 桥接任务」。

方案：两处改 `tokio::runtime::Builder::new_current_thread().enable_time().build()`；`Cargo.toml` 的 tokio features 由 `["rt-multi-thread", "time", "sync"]` 收窄为 `["rt", "time", "sync"]`。`enable_time()` 必需（`tokio::time::interval` 缺 time driver 会 panic）；io driver 不需要（fps 的 RingBuf 轮询走 mio 自有 `Poll`，在独立 std 线程里）。

周期精度不变的依据：`interval.tick().await` 的唤醒延迟取决于任务让出时刻，与 worker 线程数无关；循环体阻塞段只有 `/proc` 读取与 bpf map 读取（syscall 级）。收益如实表述为「2×(8→1) 个 worker 线程，约省 14 个空闲线程的栈与调度条目」，CPU 收益接近 0。

### U3 的 cpu_monitor 侧：评估后不做

`core_utils`（cpu_monitor.rs:279）每 tick 分配一次（`vec![0.0_f32; max_cpu_id + 1]`，36 字节量级），因被 move 进 `DaemonEvent::SystemLoadUpdate` 而无法原地复用。三条替代路径的量化结论：

1. 事件改 `Arc<Vec<f32>>` 池化：chiri 侧 `last_core_utils` 需跨 tick 保留（供 2s affinity 选核与 devimp 使用），引用计数长期大于 1，`Arc::get_mut` 永远失败，池化收益归零。
2. chiri 侧每 tick 拷贝到自有缓冲：等于「省一次 36B 分配换一次 36B 拷贝」，约 40ns/tick（6~25 次/s）。
3. 事件改固定数组 `[f32; 16]`：收益同量级，却要把 `SystemLoadUpdate` 从 32B 放大到约 72B 并牵动两个消费方。

结论：收益低于 0.0001% CPU，不做；U3 的价值已在 CLG 侧兑现（每 tick 2 次分配降为 0）。
