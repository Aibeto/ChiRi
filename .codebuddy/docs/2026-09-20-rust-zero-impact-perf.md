# Rust 语言特性视角的零影响优化清单

> 2026-09-20 · 续 `2026-09-20-rust-perf-optimization-report.md`
> 本次约束：**只列对现有功能与业务零影响的项**。判定标准是可证等价：改前后每条路径的输入输出、写入内容、日志文本、panic 行为完全一致。
> 分三档：A 机械等价改写 ／ B 需同步调用点的等价重构 ／ C 有语义风险，不建议。

## [a] A 档：机械等价，可逐个独立提交

### A1 为比较而 clone（每秒都在发生）

`chiri/mod.rs:1477 / 1516 / 1614`：

```rust
mode_clone.lock().unwrap().clone() == "fas"
```

`String == &str` 的比较不需要拥有 String。三处都在 1s 周期块（FAS 兜底与自愈）里，等于每秒一次堆分配（`mod.rs:2291` 的 scenemode 分支在息屏时每 tick 一次，6 次/s）。

改法：`mode_clone.lock().unwrap().as_str() == "fas"`（保持锁的作用域不变）。比较结果逐字节相同。

### A2 为比较而 to_string

`chiri/mod.rs:1520`：

```rust
if let Some(active) = fas_mgr.active_pkg().map(str::to_string) {
    if active == cur_pkg { ... }
```

`active_pkg()` 已经返回 `Option<&str>`，`.map(str::to_string)` 只是为了和 `cur_pkg` 比较。删掉 map 直接比较即可，每秒少一次分配。

### A3 `get_current_package()` 返回克隆

`app_detect.rs:75-77` 每次调用 `CURRENT_PACKAGE.lock().unwrap().clone()`，返回新 `String`。`chiri/mod.rs` 有 4 处调用（1036、1517 等），其中 1517 在 1s 周期块。同理 `last_determined_mode()`（app_detect.rs:361-363）。

零影响改法：保留原签名，另加一个不分配的查询入口，热路径改用它：

```rust
pub fn with_current_package<R>(f: impl FnOnce(&str) -> R) -> R { ... }
```

原 `get_current_package()` 一行不动，调用点零变更风险，旧函数留给冷路径。

### A4 devimp 每行 40 次堆分配

`logger.rs:858` `struct DevRow([String; 40])`，`new()` 里 `std::array::from_fn(|_| NA.to_string())`（logger.rs:865）——每个 "-" 都是一次独立堆分配，加上 `join(",")`（logger.rs:1154）再分配一次。devimp 开启时 tick 行 18 行/s、place 行可达百行级。

改法：字段类型换成 `Cow<'static, str>`，未用列填 `Cow::Borrowed("-")`，有值列填 `Cow::Owned(...)`。`join` 对 `Cow` 依然可用，输出字节完全一致。

### A5 affinity 每轮重建核心池

`affinity.rs:994-1002` 每轮（2s）执行 `ranges.prime.clone().collect()`、`ranges.big.clone().collect()` 等多次 collect；`1141 / 1253` 在**逐线程**分支里 `perf_pool.clone()`，180 个前台线程就是每轮上百次小 Vec 分配。

改法：核心区间来自 `chiri_core_ranges()`（`common.rs:193`），进程内不变，把这几个池在 `AffinityManager` 里建一次缓存；`perf_pool.clone()` 改成传 `&[usize]`（调用方只读）。选核逻辑与结果不变。

### A6 affinity 的 `orig_group` 用常量却存 String

`affinity.rs:550` 字段 `orig_group: String`，赋值点只有两处：`GROUP_FOREGROUND.to_string()`（1080）与 `group.clone()`（1472，`group` 来自 `BACKGROUND_GROUPS: [&str; 3]` 常量）。

改法：字段改 `&'static str`，赋值点去掉 `to_string()`/`clone()`。取值集合完全相同（就是这五个常量），`cleanup_thread` 用它写回 cpuset 路径，行为不变。每建档一个线程省一次分配。

### A7 采样期的一次性 Vec

`cpu_monitor.rs:256` 每 tick `vec![0.0_f32; max_cpu_id + 1]`；`cpu_load_governor.rs:950` 每 tick `Arc::new(core_utils.to_vec())`。

改法：把 `core_utils` 的缓冲提到循环外复用（`clear()` + `resize`），CLG 侧用双缓冲 + `Arc::get_mut` 复用（Worker 不跨 tick 持有，代码注释已确认）。数值语义不变，只是不再每次新建。

### A8 内联提示

`opt-level="z"` 下 LLVM 的内联阈值很保守。给这些确定极小的热函数加 `#[inline]`：
`cpu_monitor.rs:30 percpu_total`、`cpu_load_governor.rs:121 max_util`、`61 find_nearest_freq`、`common.rs:136 hint_matches`（已缓存，非热）。

`#[inline]` 不改变任何语义，纯粹是给优化器的提示。

### A9 冗余边界检查（最小改动版）

`cpu_monitor.rs:263-318` 的逐核循环里对 `last_idle_times[idx]` / `last_busy_times[idx]` / `core_utils[idx]` 反复下标访问，每次带边界检查。改成 `let (idle_slot, busy_slot, util_slot) = (...)` 一次性取 `&mut`，或在 `for` 里用 `iter_mut().zip()`，语义与 panic 行为完全一致（越界依然 panic）。收益很小，可打包进 A7 一起改。

## [b] B 档：等价重构，需要同步调用点

### B1 `Vec<(String, String)>` 换成 `&[(&str, &str)]`

`utils.rs:74 write_nodes(items: &[(String, String)])` 与 `affinity.rs:195 set_groups_cpus_tracked` 都按拥有 String 的数组传参，调用方（`affinity.rs:79-81`、`742-747`、`278-304`）先 `format!` 出路径再 `to_string()` 出值。

改法：签名改成 `&[(&str, &str)]`，调用方直接传字面量切片。写入内容与顺序不变。冷路径为主，收益一般，主要是消除这类 API 的分配惯性。

### B2 `DaemonEvent` 被最大变体撑大

`common.rs:41` `ConfigReload(RulesConfig)` 内联持有 `RulesConfig`（含 `HashMap` + `Vec<String>` + `String` + `FasRulesConfig`），整个枚举尺寸由它决定，而最高频的 `SystemLoadUpdate`（40ms 一次）每次发送都要按枚举尺寸搬内存。

改法：`ConfigReload(Box<RulesConfig>)`，匹配处加一次解引用。发送/接收语义完全不变。**收益很小**（每次省百来字节的拷贝），价值主要在结构卫生，建议顺手做。

### B3 `sample_one_tid` 的解析分配

`affinity.rs:517-530`：每次读 `/proc/<tid>/stat` 都要 `format!` 出路径（一次 String）、`read_to_string`（一次 String）、`split_whitespace().collect::<Vec<&str>>()`（约 40 个 `&str` 的 Vec）。后台深扫每轮 64 次，即约 32 次/s。

改法：路径用一个复用的 `PathBuf`/`String`（`clear()` + `write!`）；解析改成迭代器定位第 11/12 个字段并同时计数，保留 `tokens.len() < 37` 的等价判定。返回值 `ThreadSample` 不变。

风险点：字段索引口径必须逐字对齐（`tokens` 是 `)` 之后切分，`[11]`/`[12]` 是 utime/stime），改完要用同一份 `/proc` 样本对比输出。

### B4 `lazy_static!` → 标准库 `LazyLock`

`app_detect.rs:64` 用了 `lazy_static`，`logger.rs:13` / `i18n.rs:6` 用了 `once_cell`。两者都可由 `std::sync::LazyLock` / `OnceLock` 替代，访问开销相同（都是一次原子加载 + 初始化分支）。

纯依赖清理，不碰语义。

## [c] C 档：看着像优化，实际有语义风险，不做

- **换 HashMap 的 hasher（如 `ahash`/`fxhash`）**：会改变 `HashMap` 的迭代顺序。`special_tuned_entries` 的顺序决定白名单匹配优先级（`common.rs:401` 按文件顺序 `find`），`app_modes` 的迭代顺序也可能影响导出 yaml 的行序。在能证明某张表的迭代顺序不参与任何决策之前不换。
- **`sort_by` → `sort_unstable_by`**：`cpu_monitor.rs:473` 的 tgtop top5 排序只用于 devimp 日志，稳定性差异无业务含义，**这一处可以改**；但其它地方的 `sort_by` 必须先确认是否有并列项依赖稳定顺序。
- **`panic = "abort"` / `panic = "immediate-abort"`**：破坏 `catch_unwind` 的 panic 自愈（`chiri/mod.rs:974`、`scheduler/mod.rs:269`、`monitor/mod.rs:34`），绝对不做。
- **把 `std::sync::Mutex/RwLock` 换成 `parking_lot`**：会去掉锁毒化语义，而代码多处依赖 `unwrap_or_else(|p| p.into_inner())` 在 panic 后继续服务（如 `i18n.rs:79`、`logger.rs:121`）。不做。
- **给热路径加缓存**：任何「缓存一次、下次直接用」的改动都会让判定滞后一拍，与 40ms/160ms 的实时投喂契约冲突。本清单里唯一允许缓存的是 A5（核心区间，进程内恒定）与 A7（缓冲区复用，不是值缓存）。
- **`#[repr(C)]` / 手动排字段改内存布局**：`ThreadState`（affinity.rs:538）这类结构由编译器自动重排，手动干预收益不确定且易错。不做。

## [tooling] 用工具 mechanically 找剩余项

手工只能覆盖热路径，建议补两道机械扫描（都只报不改）：

1. `cargo clippy -p chiri --target aarch64-linux-android -- -W clippy::perf -W clippy::nursery`，重点看：
   - `redundant_clone`（A1/A2 这类「为比较而克隆」）
   - `needless_collect`（A3/B3 这类 collect 后只用一次迭代）
   - `ptr_arg`（`&String` / `&Vec<_>` 参数，对应 B1）
   - `single_char_pattern`、`or_fun_call`
2. `cargo bench`（新增）：只覆盖纯函数 `find_nearest_freq`、`TempFilter`、`Config::load` 的嵌入 yaml 解析。这几处是唯一能在 PC 上量化、且不受真机噪声影响的。

clippy 需要 NDK target，本机能否跑待确认；跑不了就作为 CI 的一个可选 job。

## [order] 建议顺序

A1 → A2 → A3（三处都是「为比较/传值而分配」，改动各一两行，收益最确定）
→ A4 → A6 → A5（dev 路径与亲和轮，改动局部）
→ A7 → A8 → A9（采样期，与上一份报告的 K1 一起做）
→ B1 → B4 → B2 → B3（签名类，需全量 `cargo check` 验证）

每一步单独提交，`cargo check -p chiri --target aarch64-linux-android` 通过后再进下一个；真机上用上一份报告建议的自测量（daemon 自身 utime/stime 列 + status.csv）对比前后 CPU 占用，确认无回归。
