---
name: perf-zero-impact-batch-K1-K2-U3-A1-A2-A4-A5-A6-B1-B3-C2
overview: 按已定稿的《Rust 性能优化综合报告》实施 K1、K2、U3、A1、A2、A4、A5、A6、B1、B3、C2 共 11 项零行为变更的优化，逐项独立提交并用 `cargo check`（带 android target）验证。
todos:
  - id: prep-baseline
    content: 读终版报告确认 11 项范围，用 [skill:token-efficient-coding] 建立改动前基线并确认 eBPF stub 现状
    status: completed
  - id: batch-clean
    content: 实施 A4、A5、A5b、A1、A6 的纯删除与常量化改动并通过 cargo check
    status: completed
    dependencies:
      - prep-baseline
  - id: batch-ownership
    content: 实施 A2 的 with_current_package 入口与 B1 的 ConfigReload 装箱，用 [skill:m02-resource] 复核借用
    status: completed
    dependencies:
      - prep-baseline
  - id: batch-sample
    content: 实施 U3 的 CLG Arc 双缓冲复用，cpu_monitor 侧 core_utils 因 move 进事件保持不动
    status: completed
    dependencies:
      - prep-baseline
  - id: batch-ebpf
    content: 实施 K1 的 CoreState 合并与 K2 的记账开关，用 [skill:unsafe-checker] 审查 Pod 实现
    status: completed
    dependencies:
      - prep-baseline
  - id: batch-deps
    content: 用 [skill:m11-ecosystem] 实施 C2 依赖与 features 裁剪、once_cell/lazy_static 迁移，以及缩范围后的 B3
    status: completed
    dependencies:
      - prep-baseline
  - id: verify-closeout
    content: 全量 cargo check 验证、git 足迹核对、同步报告与 memory 口径
    status: completed
    dependencies:
      - batch-clean
      - batch-ownership
      - batch-sample
      - batch-ebpf
      - batch-deps
---

## 需求

用户要求实施《Rust 性能优化综合报告（审查修订版）》（`e:\code\ChiRi\.codebuddy\docs\2026-09-20-rust-perf-report-final.md`）中的 11 项，编号与范围严格以该文档为准：**K1、K2、U3、A1、A2、A4、A5、A6、B1、B3、C2**。

## 硬约束

- **零行为变更**：调度判定逻辑、写入 sysfs 的值与顺序、status.csv / devimp.csv / daemon.log 的输出内容与格式、WebUI 数据面必须逐字节不变。报告结论是这 11 项均不触及落盘数据面（唯一会改数据面的 K3 不在本批），每项实施时都要保留这个自查点。
- 不得改动 `src/scheduler/`（Yumi 调度）的业务逻辑、不得动 `webui/`、`module/` 配置模板、CI 与 xtask 逻辑。C2 只改 `Cargo.toml` 的依赖声明与 features。
- 遵循仓库约定的文件头区块索引 `//! x.rs - 区块索引: [a] [b]`，修改走局部 Edit。

## 三项经源码复核得出的范围偏差（计划已按此收敛）

1. **U3 在 cpu_monitor 侧不可行**：`core_utils`（`cpu_monitor.rs:256`）在 `:414-418` 被 **move** 进 `DaemonEvent::SystemLoadUpdate`，缓冲无法回收复用；要复用必须改事件契约（所有权/共享方式），超出本批零影响范围。故 U3 只保留 CLG 侧 `Arc` 双缓冲那一项。
2. **B3 缩范围**：`write_nodes` 的多数调用点路径是运行时拼接的 String（`queue_path.join(name).to_string_lossy().into_owned()`、`format!("/dev/cpuset/{group}/cpus")`），天然需要拥有 String，改 `&[(&str, &str)]` 会波及至少 8 个文件并触碰约定不可改的 `src/scheduler/`，收益为负。故保持 `write_nodes` 签名与 `Vec<(String, String)>` 不变，只处理路径本来就是字面量的调用点。
3. **K1/K2 本机无法真编译 eBPF**：`yumi-ebpf` 是独立 workspace，Windows 无 bpf-linker 时 `build.rs` 走 `write_ebpf_stub()` 占位，本机 `cargo check` 覆盖不到探针源码，正确性依赖 CI 构建与真机 A/B，计划中需写明兜底做法。

## 核心交付物

11 项按风险由低到高分批落地，每项单独一次 `cargo +nightly check -p chiri --target aarch64-linux-android` 通过后再进下一项；全部完成后报告 file 级改动清单，并按仓库约定同步 `docs/agents/` 与 `.codebuddy/memory/` 的相关口径。

## 技术栈与改动面

纯 Rust 侧改动（edition 2024，nightly，交叉目标 `aarch64-linux-android`），涉及 `src/monitor/`、`src/chiri/`、`src/common.rs`、`src/utils.rs`、`yumi-ebpf/src/main.rs`、`Cargo.toml`。不涉及 WebUI、不涉及module 配置模板、不涉及脚本。

## 实施方案（逐项）

### 低风险批：纯删除 / 常量化 / 提示

- **A4** `src/chiri/affinity.rs:1141`、`:1253`、`:1661` 三处 `perf_pool.clone()` / `to_vec()` 直接删除，`set_tid_affinity(tid, cpu_ids: &[usize])`（`:412`）形参本就是切片，改传 `&perf_pool`；`Vec<usize>` 解引用切片，行为完全一致。
- **A5** `affinity.rs:1402` `bg: Vec<(i32, String)>` → `Vec<(i32, &'static str)>`（`:1412` 的 `group.to_string()` 直接存常量 `group`）；`:550` 字段 `orig_group: String` → `&'static str`，同步 `:1080`、`:1472`、`:972`、`:1700`。`BACKGROUND_GROUPS`（`:68`）是 `'static` 常量数组，取值集合不变。
- **A5b**（可选，同批）`:994-1002` 每轮 `ranges.*.clone().collect()` 重建核心池 → 在 `AffinityManager` 中缓存一次（`chiri_core_ranges()` 进程内恒定）。
- **A1** `src/chiri/mod.rs:1477`、`:1516`、`:1614` 的 `mode_clone.lock().unwrap().clone() == "fas"` → `.as_str() == "fas"`（报告 V6 已验证：临时 `MutexGuard` 在 `if` 条件求值结束时 drop，改后持锁从「clone 完成」延到「比较完成」，同条件内无其它取该锁的调用，无重入）。
- **A6** 对 `cpu_monitor.rs:30 percpu_total`、`cpu_load_governor.rs:61 find_nearest_freq`、`:121 max_util` 加 `#[inline]`（纯优化器提示，零语义；`opt-level="z"` 下 LLVM 内联保守）。

### 中风险批：所有权与事件契约

- **A2** `app_detect.rs:75-77 get_current_package()` 保持签名不动，新增 `pub fn with_current_package<R>(f: impl FnOnce(&str) -> R) -> R`（内部取 `CURRENT_PACKAGE` 的 MutexGuard 并剥毒化，与现有 `unwrap_or_else(|p| p.into_inner())` 口径一致）；调用点只改 1s 周期块内的 `chiri/mod.rs:1517`，把该分支主体包进闭包（闭包内不再调用 `get_current_package`，避免非重入 Mutex 重入）。若重构导致代码块难以收敛，则回退为不改动该项并在报告中标注为 L3 顺手项。
- **B1** `common.rs:41 ConfigReload(RulesConfig)` → `ConfigReload(Box<RulesConfig>)`；发送方 `app_detect.rs:416` 包 `Box::new`；消费方 `chiri/mod.rs:2465`（`_new_rules`）与 `scheduler/mod.rs:483-485`（`let _ = new_rules;`）均为忽略 payload，解构写法按最小改动适配。
- **U3（仅保留一半）** `cpu_load_governor.rs:950` `Arc::new(core_utils.to_vec())` → governor 持有一个 `Arc<Vec<f32>>` 成员，每 tick 先尝试 `Arc::get_mut` 复用（清空后 `extend_from_slice`），不可复用（Worker 仍持有）时才新建；Worker 不跨 tick 持有已由注释确认，语义与数值完全一致。**cpu_monitor 侧 `core_utils` 因被 move 进事件，本批不改**。

### 高风险批：eBPF 探针（本机编译盲区）

- **K1** `yumi-ebpf/src/main.rs:79-95` 五个 `PerCpuArray` 合并为一个 `#[repr(C)] struct CoreState { last_time: u64, idle: u64, busy: u64, cur_tid: u32, cur_tgid: u32 }`（按此顺序排布共 32 字节、对齐 8、**无 padding**，满足 Pod 约束）；`try_handle_sched_switch`（`:166-203`）5 次 `get_ptr_mut` 降为 1 次；用户态 `cpu_monitor.rs:141-151`、`:250-254`、`:263-319` 同步改为单次 `get` + 按字段取值。
- Pod 实现二选一：新增 `bytemuck = { version = "1", features = ["derive"] }` 依赖走 derive，或手写 `unsafe impl bytemuck::Pod` 并加 `// SAFETY` 注释（推荐后者以避免新增依赖，但需过 `unsafe-checker` 审查）。
- eBPF 产物与 daemon 二进制**必须同包替换**，版本偏差容错沿用 `fetch_counter_map` 的「缺失即 0」语义（`cpu_monitor.rs:166-183`）。
- **K2** `main.rs:99 THREAD_RUN_TIME` 记账加开关：新增 `Array<u32>` 开关 map，探针在 `add_to_hash`（`:187`、`:193`、`:211`）前读一次开关；用户态在 `cpu_monitor.rs:81-83` 处置位时同步写该 Array（`FAS_FG_UTIL_ENABLED` 同条件：`is_chiri_soc() && fas_available()`），保证降级路径被调用时记账已开启，无额外延迟。

### 依赖与签名批

- **C2** `Cargo.toml`：删除 `itoa`(39)、`bytes`(37)、`futures`(38)、`glob`(50)（src 内无任何 `use`）；`tokio` features 由 `"full"` 裁剪为 `["rt-multi-thread", "time", "sync"]`（`Runtime::new()` 走多线程调度器故必须保留 `rt-multi-thread`；用到 `tokio::spawn`/`Runtime::block_on`/`time::interval`/`sync::watch`，未用 `#[tokio::main]`，故不需要 `macros`）；`lazy_static` → `LazyLock`（仅 `app_detect.rs:64`），`once_cell` → std（`i18n.rs:6/11` 的 `Lazy`、`logger.rs:13/40` 的 `OnceCell`），与 `utils.rs:12`、`common.rs`、`rhine.rs`、`notify.rs`、`chiri/scheduler.rs` 已在用的 std `OnceLock` 口径统一。
- **B3（缩范围）** 只处理路径为字面量的调用点：`chiri/scheduler.rs:263-273`（touch_boost 四个字面量路径，去掉 `to_string()` 并配合 `write_nodes` 增加一个接受 `&[(&str, &str)]` 的薄封装或直接内联构造）。**不改 `write_nodes` 签名、不动 `src/scheduler/`**。若核实后收益可忽略，允许该项降级为不动并在报告中注明。

## 涉及文件（Directory Structure）

```
src/
├── Cargo.toml              # [MODIFY] C2：裁剪 tokio features、删除 4 个未使用依赖；K1 可能新增 bytemuck（可选）
├── monitor/
│   ├── cpu_monitor.rs      # [MODIFY] K1（单次 percpu get + CoreState 取值）、K2（写开关 Array）、A6（#[inline]）
│   └── app_detect.rs       # [MODIFY] A2（新增 with_current_package）、B1（ConfigReload 处 Box::new）、C2（lazy_static→LazyLock）
├── chiri/
│   ├── mod.rs              # [MODIFY] A1（三处 .as_str()）、A2（1517 改用闭包入口）、B1（_new_rules 解构适配）
│   ├── affinity.rs         # [MODIFY] A4（删三处多余 clone）、A5（bg/orig_group 改 &'static str）、A5b（核心池缓存）
│   ├── cpu_load_governor.rs# [MODIFY] U3（Arc 双缓冲）、A6（#[inline]）
│   └── scheduler.rs        # [MODIFY] B3（touch_boost 字面量路径，仅此一处）
├── common.rs               # [MODIFY] B1（ConfigReload 装箱）
├── i18n.rs                 # [MODIFY] C2（once_cell::Lazy → std LazyLock）
├── logger.rs               # [MODIFY] C2（once_cell::OnceCell → std OnceLock）
└── utils.rs                # [仅确认] B3 缩范围后 write_nodes 签名保持不变
yumi-ebpf/
└── src/main.rs             # [MODIFY] K1（CoreState 合并 + Pod 实现）、K2（记账开关 Array）
.codebuddy/
├── docs/2026-09-20-rust-perf-report-final.md   # [MODIFY] 标注实际完成情况与 B3/U3 的范围收敛
└── memory/2026-09-20.md    # [APPEND] 本轮实施记录
```

## 实施要点（防回归）

- **验证命令固定**：`cargo +nightly check -p chiri --target aarch64-linux-android`（host target 因 netlink-sys/aya-obj 缺 libc 符号必然失败，不要试图改目标）。
- **穷举调用点禁用 `head_limit`**（曾截断漏点）；改签名类（B1、A5、C2 的 Lazy 迁移）改前先全仓搜引用。
- **K1/K2 盲区兜底**：本机 `cargo check` 覆盖不到 `yumi-ebpf`；提交前必须逐行对照「旧 map 语义 → 新 CoreState 字段」的一一对应关系，并在 CI 构建通过后才能认为前半段落地；真机侧用同一场景录制 status.csv 对比逐核 util 是否一致。
- **数据面自查**：每项改完确认「不产生新的条件分支、不写新的 sysfs 值、不改变任何落盘文本」；A2 的闭包不得跨锁调用 `get_current_package`。
- 提交前 `git status` 核对足迹，改动清单按文件级汇报；完成后同步 `docs/agents/`（若涉及架构/契约变更）与当日 memory 日志。

## 将使用的扩展

- **token-efficient-coding**（用户已指定）：文件头区块索引导航、按需分片读取、局部 Edit 禁整文件重写、工具调用并行、精简汇报。
- **coding-guidelines**：Rust 代码风格与最佳实践，保证 11 项改动与既有代码风格一致。
- **m10-performance**：性能优化专项，复核 K1/K2/U3 的收益与是否引入新的热点。
- **m02-resource**：`Arc` 双缓冲（U3）与智能指针生命周期复核，确保无跨 tick 引用残留。
- **m12-lifecycle**：`OnceLock` / `LazyLock` 惰性初始化迁移口径（C2）。
- **m11-ecosystem**：`Cargo.toml` features 与依赖裁剪的可行性核查（C2），避免裁掉隐式所需 feature。
- **unsafe-checker**：K1 若采用手写 `unsafe impl bytemuck::Pod`，用于审查 `#[repr(C)]` 布局与 SAFETY 注释。