# AGENTS.md

> 上次更新时间：2026-08-30 12:00:00
> 最后更新位于此 head 之后：40577b0af15ce66a0875546b1d98d730fa2925af

本文档为 AI 编程助手（Cursor / Claude Code / Trae 等）在本仓库工作时的指导文件。

## 项目概述

**yumi** 是 Android CPU 智能调度控制系统（Magisk/KernelSU 模块），核心是 Rust 守护进程，通过 eBPF 内核探针采集 CPU 调度事件与渲染帧数据，结合 FAS 帧感知调度和 CLG 负载调速器动态调频。

**项目方向**：本仓库为 ChiRi（自 imacte/yumi fork，README "Based on imacte/yumi"），调度代码在 `src/chiri/`。`src/scheduler/`（Yumi 调度）即将废弃，仅作为 ChiRi 的基础保留，**勿改动其逻辑**。新功能、调优只落 `src/chiri/` 和处理器配置 `module/config/{soc}/`。

- 目标平台：Android 8.0+ / AArch64 / 需要 Root
- 许可证：GPL-3.0-or-later
- 版本：见 `module/module.prop` 与 `Cargo.toml`（需保持同步）

## 目录结构

```
src/                  # Rust 守护进程主代码
  monitor/            # 监控层：app_detect / fps_monitor / cpu_monitor / screen_detect（两套调度共享）
  scheduler/          # 调度层 Yumi（即将废弃，作为 ChiRi 基础保留、勿动逻辑）：FAS 引擎、CLG 负载调速器
    fas/              # FAS 核心：PID 控制器、帧率档位、frame_pipeline
  chiri/              # ChiRi 调度（发展主线；特定 SoC 触发；含 CLG、akmode 明日方舟特调、touch_detect 触摸升频）
  common.rs / fas_types.rs / i18n.rs / logger.rs
yumi-ebpf/            # eBPF 探针（bpfel-unknown-none，build-std 编译；独立 workspace，不在根 members）
xtask/                # 构建脚本（cargo xtask build 完成编译打包 zip）
module/               # Magisk/KernelSU 模块载体（module.prop、customize.sh、service.sh）
  config/             # config.yaml / rules.yaml / i18n (en.ftl / zh.ftl)；<soc>/config.yaml + akmode.yaml 处理器子目录（各 SoC 自带，8475/8998 参数相同、各自一份）
webui/                # Vue 3 + TypeScript + Vite + Pinia + Vant 管理界面
updateInformation/    # 更新.json 与 changelog
.github/workflows/    # CI：Node 24 + Rust nightly + NDK r29 + cargo-ndk
```

## 技术栈

| 层       | 技术                                                                              |
| -------- | --------------------------------------------------------------------------------- |
| 守护进程 | Rust (edition 2024, nightly), tokio, aya (eBPF), serde_yaml, inotify, netlink     |
| eBPF     | aya 框架，`sched_switch` tracepoint + `queueBuffer` uprobe                        |
| WebUI    | Vue 3, TypeScript, Vite, Pinia, Vant, vue-i18n, kernelsu                          |
| 构建     | cargo xtask build（Rust aarch64-linux-android 交叉编译 + webui npm build + 打包） |

## 常用命令

```bash
# 完整构建（编译 eBPF + 守护进程 + WebUI 并打包模块 zip）
cargo xtask build

# 仅组装模块目录、不打包 zip（CI 用；目录名按 module.prop 动态命名，GitHub 下载时自动压缩）
cargo xtask build --no-pack

# 本地静态检查（验证 yumi crate 本身；需 nightly + aarch64-linux-android target + bpf-linker）
cargo +nightly check -p yumi --target aarch64-linux-android

# WebUI 开发
cd webui && npm install && npm run dev

# WebUI 类型检查
cd webui && npm run type-check
```

- **本地 check 需要 nightly + aarch64-linux-android target + bpf-linker**：eBPF 编译用 `-Z build-std`（仅 nightly 支持），build.rs 会构建 yumi-ebpf。yumi 是 Android/Linux 专属 crate，别用 Windows host 目标检查（netlink-sys/aya 无法在 Windows 编译）。Windows 下 bpf-linker 为 `bpf-linker.exe`（build.rs 已按 `cfg!(windows)` 兼容）。
- **yumi-ebpf 是 no_std/no_main 探针，独立 workspace**（根 `Cargo.toml` 的 members 只有 xtask，勿把 yumi-ebpf 加回）：只可用 bpfel 目标检查（`-Z build-std=core`），**禁止**在带 std 的目标（如 aarch64-linux-android 或 Windows host）下编译/检查。探针无法用 std 编译（`unwinding panics are not supported without std`），test 剖面还会与 `#[panic_handler]` 冲突（`duplicate lang item panic_impl`）。根 build.rs 用 `current_dir=yumi-ebpf` 单独构建，IDE（rust-analyzer）在根 workspace 下不检查它。
- 本地开发只做 `cargo check` / WebUI `type-check` 验证；完整产物由 CI（GitHub Actions）生成。不要随意 `cargo build`（需要 NDK 环境），优先静态检查。
- **完整构建的 bpf-linker 获取**：`build.rs` 的 `ensure_bpf_linker` 依次尝试 PATH 中已有 bpf-linker → OUT_DIR 缓存 → `cargo install bpf-linker`。CI 通过 GitHub API 下载静态链接 LLVM 的预编译二进制（bpf-linker 0.11 依赖 LLVM 21+，源码编译在 ubuntu runner 上不可行；cargo-binstall 也会回退到源码编译）。eBPF release 编译在 build.rs 内用 `CARGO_PROFILE_RELEASE_OPT_LEVEL=2` 局部覆盖（新版 bpf-linker 已移除 `-Oz`/`-Os`，仅支持 `-O0~O3`，workspace 根的 `opt-level="z"` 会导致链接失败）。**Windows 兜底**：bpf-linker 源码编译依赖 `os::unix` API，Windows 上无法构建，且本地不承担完整产物构建。`ensure_bpf_linker` 在 Windows 无现成 bpf-linker 时返回跳过错误，`build_ebpf` 捕获后经 `write_ebpf_stub()` 回退占位产物（`ebpf_target/bpfel-unknown-none/{debug,release}/yumi-ebpf`，与 `YUMI_SKIP_EBPF=1` 同路径），保证 rust-analyzer 和本地 `cargo check` 不被阻塞。Windows 上若有 `bpf-linker.exe` 仍正常构建 eBPF，CI 行为不变。

## 代码约定

1. **架构**：Monitor 线程组通过有界 mpsc 事件通道（`DaemonEvent`，`sync_channel` 容量 64，满时 send 阻塞形成背压）解耦数据采集与调度决策，新增监控/调度能力遵循此模式。前台 PID 由 `monitor/mod.rs` 的 `pid_watcher` 线程经 `tokio::sync::watch` 广播，FPS/CPU 监控共享消费，不要各自轮询。FPS 帧监控（`fps_monitor`）仅服务于 FAS 调频，FAS 禁用期间不启动（见 `mod.rs` 注释块）。**调度器双套架构**：`scheduler/`（Yumi CLG）与 `chiri/`（ChiRi 专用）二选一，main.rs 按 `common::is_chiri_soc()`（SoC 列表命中）决定启动哪一套。两套互斥消费同一事件通道，Monitor 层共享。**主次关系**：ChiRi 是主线，Yumi 即将废弃且逻辑冻结（勿改 `src/scheduler/` 与默认 `config.yaml`），新功能/调优只落 `src/chiri/`。`CHIRI_SOC_HINTS` 在 common.rs 维护，新增机型只追加列表，不要绑定单一型号（同时提供 `config/{片段}/config.yaml` 和核心组区间 `common::chiri_core_ranges()`，按「片段命中且目录存在」匹配）。**FAS 暂禁用期间的保留代码**（fps_monitor 整模块、`DaemonEvent` 的 `FrameUpdate`/`pid`/`foreground_max_util`、capacity 权重函数等）统一加 `#[allow(dead_code)]` 并注明"恢复 FAS 时启用"，**不要删除**。**负载采样间隔**：`cpu_monitor` 的 `SystemLoadUpdate` 按 SoC 参数化——main.rs 按 `is_chiri_soc()` 传入 `start_monitor`→`start_cpu_loop`（ChiRi 160ms / Yumi 200ms，Yumi 200ms 为原值勿改）；akmode 激活时经共享 `Arc<AtomicBool>`（main.rs 创建、`AkmodeGovernor` 接管/释放时置位）切换到 40ms。akmode 消费该负载流做**动态限频**（档位固定，max 随负载在内核频率表中逐档升降，范围均为硬件上下限）；CLG 消费同一事件流，tick 语义按当前采样间隔（各 rate_limit_ticks/smoothing 按各自 tick 调优）。
2. **日志**：调试与排障优先用 `debug!`，别全用 info 冲掉有价值的信息；高频路径（频率控制/帧处理）用 25-tick / 60-frame 周期摘要，状态变化（模式、PID、屏幕、attach、档位）即时打点。新增日志 key 同时补 `module/config/i18n/zh.ftl` 与 `en.ftl`，命名格式 `模块-描述`。
3. **配置**：运行时配置走 `module/config/config.yaml`（CLG/模式参数）和 `rules.yaml`（FAS/模式映射参数），支持热重载；新增配置项同步更新反序列化结构体与默认值。配置由 main 启动时解析一次，以 `Arc<RwLock<Config>>` 共享给对应调度器，热重载由 config_watcher 覆写同一实例。两套调度各持自己的 Config 类型（`scheduler::config` 与 `chiri::config`）。**按处理器独立配置**：命中 `CHIRI_SOC_HINTS` 时，`common::get_config_path()` 优先加载 `config/{命中片段}/config.yaml`，否则回退 `config/config.yaml`。匹配规则：片段命中且目录存在（`common::matched_soc_hint`），防止多 SoC 并存时误用其它机型配置。**特调/息屏场景配置路径回退链**：`get_akmode_path()` 与 `get_scenemode_path()` 均返回 `Option<PathBuf>`——先找 `config/{命中片段}/akmode.yaml`（或 `scenemode.yaml`），不存在回退 `config/normal/` 同名文件（与 SoC 目录同级的通用配置），都没有返回 `None`。akmode：`Config::from_file` 的 `merge_akmode()` 在路径为 `None`（SoC 与 normal 均缺失）时置特调不可用（`AKMODE_AVAILABLE` false），白名单应用回退 CLG；scenemode：`merge_scenemode()` 路径为 `None` 时保持 serde 默认值（不告警）。所有加载/热重载入口（main.rs、两套 config_watcher）统一走 `get_config_path()`，不要硬编码路径。启动时把生效配置的相对路径（如 `8550/config.yaml`，非处理器时 `config.yaml`）写入 `active_config.txt`，WebUI 据此读取同一份文件。
4. **i18n**：守护进程日志用 Fluent（`module/config/i18n/en.ftl` / `zh.ftl`），WebUI 用 `webui/src/i18n/locales/`；新增用户可见文案同时提供中英文。
5. **Rust 风格**：release profile 体积优先（`opt-level = "z"`, lto, strip），避免引入重依赖；优先复用现有依赖（serde/anyhow/log/tokio/nix 等），新增第三方库选社区高星、维护活跃的 crate。
6. **资源占用敏感**：守护进程跑在 Android 后台，注意内存分配（避免频繁 Vec 分配）、锁粒度和线程唤醒次数。
7. **版本同步**：发版时同步更新 `module/module.prop`（version/versionCode）、根 `Cargo.toml`（version）、`updateInformation/update.json` 和 `changelog.md`。**产物命名以 `module/module.prop` 为准**：xtask 读取 `name + version`（配合 git 提交数与日期）生成 zip/目录名（如 `ChiRi-Alpha01-42-20260829-1200`），CI 用 `cargo xtask build --no-pack` 只组装目录、不预打包，目录名即 GitHub artifact 名，由 GitHub 下载时压缩成同名 `.zip`（避免 `.zip.zip`）。
8. **WebUI**：与守护进程通过 kernelsu bridge 交互（见 `webui/src/utils/bridge.ts`），不要硬编码路径；读配置前先读 `active_config.txt` 确定实际生效文件。文件写入用 base64 管道（`echo '<b64>' | base64 -d > path.tmp && mv -f path.tmp path`）避免 shell 特殊字符干扰，**必须经临时文件 + 原子 mv**，防止直接 `>` 截断时 config_watcher 读到半截内容导致重载失败；不要用 `echo "${content}"` 拼接。**依赖约束**：`typescript` 固定 5.x（`~5.9.0`）——TS 7.x 是 Go 原生编译器，不再导出 `lib/tsc`，`vue-tsc` 3.x 无法兼容（type-check 报 `ERR_PACKAGE_PATH_NOT_EXPORTED`），勿升级。**WebUI 不开放 YAML 编辑**：`/config` 页只查看生效配置文件（`active_config.txt` 解析路径 + meta 抬头：配置名/作者/日志语言/日志等级）并切换日志等级（`bridge.ts::setLogLevel` 只替换 `meta.loglevel` 行、保留注释与其余内容，config_watcher 热重载即时生效）；rules.yaml 仅在 WebUI 内部读写（全局模式切换、应用性能模式），不提供整文件编辑。**rules.yaml 写入防 null**：js-yaml 会把值为 null 的字段序列化成 `app_modes: null`，而 serde_yaml 无法把 null 反序列化为 HashMap（`#[serde(default)]` 只对缺失字段生效），会导致守护进程每次加载 rules.yaml 告警——`bridge.ts::saveRulesConfig` 写盘前移除 null/undefined 的 `app_modes` 键；守护进程侧 `monitor/config.rs` 的 `RulesConfig::app_modes` 用 `deserialize_with`（untagged 枚举）显式兼容 null 为空表，已存在 null 的旧文件也不告警。
9. **内部特调白名单（ChiRi 专属）**：特定应用的专用模式定义在 `common.rs` 的 `SPECIAL_TUNED_MODES`，每项含包名、可用模式列表 `modes` 与优先回退模式 `fallback`（同包名多模式时，用户未显式配置则采用 `fallback`）。白名单编译进二进制，不随 rules.yaml 下发，用户/WebUI 不可修改。**特调体系仅限 ChiRi**：只在命中 `CHIRI_SOC_HINTS` 的 SoC 上生效（`determine_mode` 先判 `is_chiri_soc()`），非 ChiRi SoC 上特调映射一律回退全局模式；只在 chiri 的 `Config` 挂载独立特调字段 `akmode`（`SpecialTunedConfig`），**不要**注册进 `get_mode`（`get_mode` 只认 CLG 常规模式），也**不要**注册进 yumi 的 `scheduler/config.rs`。模式确定：**白名单应用始终进特调**（前台命中 `SPECIAL_TUNED_MODES` 就返回特调模式，不管 app_modes/global_mode 配了什么，`determine_mode` 开头直接判定）；rules.yaml 里给该应用配的普通模式只作为特调起始档（scheduler 侧 `get_ak_initial_tier` 识别）；非白名单应用的模式优先级仍为用户 `app_modes` > `global_mode`，后端门控：非白名单包名映射到特调模式时 warn 并回退 `global_mode`（`app_detect.rs` 的 `determine_mode`）。**特调是完全独立调度**：`src/chiri/akmode.rs` 的 `AkmodeGovernor` 与 CLG 完全解耦，前台为白名单应用（明日方舟）时由 `mod.rs` 的 `scheduler_ipc` 先 `cpu_governor.release()` 再 `ak_governor.init_policies()` 接管，退出前台反向释放；**特调模式下息屏保持 akmode 接管、不切换 CLG doze**（akmode 已统一 schedutil，息屏随负载自然降频省电，`mod.rs` 的 `ScreenStateChange` 分支先判 `is_special_mode`），非特调模式息屏仍走 CLG doze。四档就是全局那套模式档位 powersave/balance/performance/fast（不另起 tier 体系），**档位由 rules.yaml 生效模式决定**（明日方舟 app_modes > global_mode，`config::mode_to_tier` 换算），**特调期间固定应用、不自动切换档位**（用户改 rules.yaml 模式后经 ConfigReload 热重载更新档位）；档位差异仅在升降频策略参数（核心组区间随命中 SoC 变化，统一在 `common::chiri_core_ranges()`：8550 little 0-2 / big 3-6 / prime 7、8475 0-3/4-6/7、8998 0-3/4-7 无 prime，akmode 与 CLG 触摸升频共用；每组独立 up_core_count/up_util_percent/down_core_count/down_util_percent，核心数为组内绝对个数、yaml 写整数、0 = 组内任一核心命中即触发、写大值如 64 = 关闭该方向判定，占用率写整数百分比、加载时转 0..1）和防抖等待（wait_ms，每档可不同），**所有档位都能使用硬件最高档位**。**全局统一 schedutil**：CLG 与 akmode 均把内核调速器写为 schedutil（两套调度器各自 `init_policies` 时写 governor、release 时恢复快照，yumi 的 `scheduler/cpu_load_governor.rs` 同样改 schedutil）。**特调动态限频（schedutil + 负载驱动升降 max）**：`AkmodeGovernor` 激活时写 schedutil、min 压到硬件最低、max 设为硬件最高；`on_load_update`（特调 40ms tick）用当前档位策略参数按核心组判定升降（升频 = 任一组内达到 up_core_count 个核心 util > up_util_percent；降频 = 任一组内达到 down_core_count 个核心 util < down_util_percent，升频优先，达到 = 组内核心数 >= core_count，即配置值就是绝对个数；统计口径：util 恰为 0.0 的核心（离线与整窗空闲的在线核均为 0.0、不可区分）不计入升频 over、但计入降频 under——空闲即低负载，避免挂机/息屏时永不降频），**升频前检查实际频率（scaling_cur_freq）是否已达当前设定的 max**（schedutil 余量），达到才在频率表中升一档；**降频直接把 max 降为当前实际频率对应档位**（`read_cur_freq` 后 `partition_point` 找 <= 实际频率的最高档，实际不可读回退降一档，绝不高于当前 max）——max 上下限均为硬件上下限。升降频带 wait_ms 防抖（升降后 `after_change_duration_ms` 内减半）。CLG 仍锁 min=max（schedutil 在锁频点无调频空间，频率由 CLG 决定）。**特调参数独立成文件，且与处理器绑定**：`module/config/{命中片段}/akmode.yaml` 定义单特调段 `akmode`（8475 与 8998 参数相同、各自一份），缺失时回退 `config/normal/akmode.yaml`（与 SoC 目录同级的通用配置）；**SoC 与 normal 均缺失 → 特调不可用、白名单应用回退 CLG**（不再保留旧值用默认参数接管 CPU），不放在默认 `config.yaml` 里。**特调接管失败冷却**：`AkmodeGovernor::init_policies` 返回 `bool`（无可用 cluster 即 false）；`mod.rs` 的 scheduler_ipc 在三个特调接管入口（亮屏恢复/ModeChange/ConfigReload）检查返回值，失败时置 `akmode_cooldown_until = now + 300s` 并 warn 打点 `scheduler-akmode-cooldown`，**5 分钟内该特调模式不再触发，改由 CLG 接管**（冷却中特调模式名保持，息屏 doze/亮屏恢复均走 CLG 分支），冷却结束后经 ConfigReload 或下次 ModeChange 自然恢复重试。守护进程启动时把白名单导出到 `special_tuned.txt`（每行 `包名:模式列表(逗号分隔):优先回退模式`）并 info 打点，导出**仅 Chiri 模式**（`is_chiri_soc()`）下发生，Yumi 设备不生成该文件。WebUI 为双套流动：`bridge.ts::isChiri`（active_config 为处理器子目录 `config/{soc}/config.yaml` 时真）判定设备类型，`stores/scheduler.ts` 存 `isChiri` 态；特调在 WebUI 为**只读标注**——`getSpecialTuned` 读白名单，仅 `isChiri && specialTuned[pkg]` 时在应用列表显示「特调：{fallback}」标签（Yumi 设备不显示），**不提供专属特调模式选项、不做特调清理/重扫修复**，档位切换与普通应用一致（写 rules.yaml 的 `app_modes`/`global_mode`，特调起始档由 scheduler 侧识别）。应用列表提供「重新扫描」按钮（扫描中禁用防并发）。**当前模式读取与自愈**：`bridge.ts::getCurrentMode` 读 `current_mode.txt`（空文件回退 `balance`）；守护进程启动时写一次初始模式、模式切换时写入，并**常态每 5 秒重写一次**（两套 scheduler_ipc 均实现），防止文件被意外清空/删除后 WebUI 读不到状态。
10. **CLG 调频语义（ChiRi 8550 优化）**：CLG 仍锁 min=max（schedutil 在锁频点无调频空间，频率由 CLG 决定）。**多线程按核心组独立调度**：每个 cpufreq policy 由一个独立的 `CoreGroupWorker` 线程管理，持有各自的 `ClusterState`（频率档位、`FastWriter`、`current_perf`、防抖计数器等），线程内自主完成决策 + 写频，核心组之间完全并行无锁。`CpuLoadGovernor`（`cpu_load_governor.rs`）是 Worker 线程管理器：`init_policies` 枚举系统 cpufreq policy、为每个 policy spawn Worker 线程（写入 schedutil governor 并按 perf_init 锁频）；`release` 停止所有 Worker（Worker 退出前自行恢复系统原始状态）；`reload_config` 停旧 Worker + 用新配置 spawn 新 Worker（等价轻量 init，current_perf 重置到新 perf_init）。scheduler_ipc 通过 `on_load_update(&core_utils)` 将负载数据广播给所有 Worker（非阻塞 `try_send`，通道满则丢弃本 tick），Worker 线程内自主决策 + 写频，**无需外部 flush**。**升频带 schedutil 余量检查（按需升频）**：升频分支先读 `scaling_cur_freq`，实际频率未追平当前锁定频率时忽略本次升频（debug `clg-up-skipped`），等硬件自然爬升后再升；**降频是直接降频**：防抖确认（`down_rate_limit_ticks`，极低负载命中 `down_fast_threshold` 免防抖）后一步到位写目标档，目标不高于当前实际频率（`ratio_of_freq` 同步 current_perf）。已删除废弃参数 `smoothing_down / slow_down_scale / down_fast_mult`。**触摸升频（事件驱动，ChiRi 专属）**：`touch_detect.rs` 线程读 `/dev/input/event*`（`libc::poll` + 解析 64 位 `input_event` 24 字节，`BTN_TOUCH==1` 或 `ABS_MT_TRACKING_ID>=0` 判定触摸按下），触摸按下时经独立事件通道 `mpsc::sync_channel::<()>` 发送事件（非阻塞 `try_send`）；scheduler_ipc 每次醒来先 drain `touch_rx`，收到事件即 `on_touch()` 更新共享 `AtomicTouchState`（跨线程原子状态：f32 性能比 floor 以 bit pattern 存入 `AtomicU32`，窗口时长与写入时刻毫秒数各存入一个 `AtomicU32`）并**广播空负载包唤醒全部 Worker**（经 Worker 负载通道 `try_send(Vec::new())`，recv_timeout 即时返回、Worker 只 flush 不决策），大核 Worker 在本次 flush 中读取共享状态、提升性能下限到 `touch_boost_floor`，**不等待下一个 160ms 负载决策 tick**（触摸延迟 ≈ `EVENT_POLL_MS=100ms` + 唤醒 flush，不含 Worker tick 间隔）。配置项 `touch_boost_enabled/ms/tiers` 每模式独立，`enabled=false` 即关闭（normalize 会把 ms 置 0）。**屏蔽系统触摸升频**：`chiri/scheduler.rs::apply_disable_touch_boost` 写 0 到 `/sys/module/cpu_boost/parameters/` 的 `input_boost_enabled / sched_boost_on_input / input_boost_ms / boost_ms`（按存在性尝试，无节点静默跳过）；`start_scheduler_thread` 启动时即调用一次 `apply_system_tweaks()`。**数据获取优化**：常规采样按 SoC 参数化（ChiRi 160ms / Yumi 200ms，由 main.rs 按 `is_chiri_soc()` 传入 `start_monitor`→`start_cpu_loop`）；`foreground_max_util` 仅 FAS 消费，FAS 禁用期间经 `FAS_FG_UTIL_ENABLED=false` 跳过计算（发送 0.0，两套调度器均忽略），`get_thread_tids/compute_tgid_util/compute_thread_level_util` 加 `#[allow(dead_code)]` 保留（恢复 FAS 时置 true）。**scenemode（息屏超时省电）**：chiri `Config` 新增 `scenemode`（Mode 段，默认 `enabled:true`、`perf_ceil` 极低封顶、`up_threshold=1.0` 不主动升频）与顶层 `scene_mode_delay_secs`（默认 300s=5 分钟）；`mod.rs` 息屏时记录 `screen_off_at`，`SystemLoadUpdate` 分支在息屏超时且非特调模式下一次性把 CLG 热切到 scenemode（scenemode 未启用则 release 回系统默认），亮屏自动恢复原模式。**屏幕状态自愈校验**：uevent 可能漏报/误报（开机早期背光未就绪、长时间息屏后唤醒、netlink 缓冲溢出）导致 `screen_state_arc` 锁死在错误状态——亮屏仍为 false 时 scenemode 计时器被误触发、且无 ScreenStateChange(true) 无法退出；`screen_detect.rs::verify_screen_state` 由 app_detect 主循环每轮调用，直接读 `/sys/class/backlight` 的 `bl_power==0`（回退 `actual_brightness>0`，与 uevent 分支同口径）校正 arc，无背光节点则静默跳过。**热切换配置重放 perf_init**：`cpu_load_governor.rs::reload_config` 停旧 Worker + 用新配置 spawn 新 Worker（current_perf 重置到新 perf_init 并立即写频），避免息屏 doze/scenemode 期间 `current_perf` 掉到 0 后亮屏恢复原模式时频率要从地板缓慢爬升数秒（表现为"亮屏了还卡在 scenemode 低频"）；模式变更 / ConfigReload 热重载同理。**极速模式（fast）专属锁频器、不读 yaml、停用 CLG**：`src/chiri/fast.rs` 的 `FastLock` 与 CLG 完全独立，fast 模式下由 `mod.rs` 的 scheduler_ipc 先 `cpu_governor.release()` 再 `fast_lock.init()` 接管；`FastLock::init()` 遍历 `get_cpu_policies()`、快照原始状态、写 schedutil governor、把所有 cluster 的 `scaling_min_freq/scaling_max_freq` 都锁到含 boost 的硬件最高频（`min=max=hw_max`）；`tick()` 每 5 秒重写一次 hw_max 防止系统/厂商守护进程篡改；`release()` 恢复接管前的 governor/min/max。`mod.rs` 事件循环中 `fast_lock.tick()` 在每次 `recv_timeout` 唤醒时调用；模式切换/息屏 doze/亮屏恢复/看门狗超时/panic 收尾均正确 release fast_lock。8550/8475/8998 的 `config.yaml` 已移除 `fast:` 段（`Config.fast` 字段保留，`get_mode("fast")` 仍返回 `Some`，仅用于模式校验），akmode 的 fast 档（动态限频）不受影响。

## 硬性约束

- 不修改 CI 构建流程（`.github/workflows/build.yml`），除非明确要求。
- eBPF 程序目标为 `bpfel-unknown-none`，改动需确保 CI 环境交叉编译通过。
- 保持 KernelSU/Magisk 模块规范兼容（`module/` 目录结构、`service.sh` 启动流程）。
- **Yumi 逻辑冻结**：不修改 `src/scheduler/` 与默认 `module/config/config.yaml` 的调度逻辑与参数行为（Yumi 即将废弃）；确需修复时先与 ChiRi 对齐、最小改动。共享层（`src/monitor/`、`main.rs`）如需为 ChiRi 适配，必须保持 Yumi 运行时行为不变（常规采样 200ms 原值勿改）。

## 经验教训

- **含 `std::ops::Range<usize>` 字段的结构体别 `derive(Copy)`**：CI（`-Z build-std` + nightly）编译 `common::CoreGroupRanges` 时报 E0204（字段不实现 Copy），即便标准库中 `Range<usize>` 实现了 Copy。按值 move 或显式 `clone()` 即可，用 `#[derive(Debug, Clone)]` 够了，不要加 Copy。
- **日志文件被删不能崩**：`src/logger.rs` 用自实现 `SelfHealingAppender`（替换 log4rs `RollingFileAppender`），每次写入按路径 `create+append` 重新打开，`daemon.log` 被外部删除会自动重建；循环轮转与锁上锁全程 `Result`/剥除 poison，绝不 `unwrap`。别改回 log4rs 滚动追加器。
- **WebUI 手动重启必须 `setsid`**：`bridge.ts::restartDaemon` 用 `killall -9 yumi; sleep 1; setsid "$MODDIR/service.sh" </dev/null >/dev/null 2>&1 &`。`ksu.exec` 返回会清理执行 shell 的进程组，直接 `nohup ... &` 拉起的守护进程会被一并杀掉。`setsid` 新开会话 + 重定向标准流后脱离 exec 进程组存活，等效手动执行 service.sh。

## AGENTS.md 维护要求

**每次对话结束前，回顾本次会话内容，评估是否需要更新本文件：**

- 新增/删除/重构了模块、目录或关键文件 → 更新「目录结构」
- 引入了新的依赖、工具链或构建命令 → 更新「技术栈」「常用命令」
- 确立了新的代码约定或踩坑经验 → 更新「代码约定」（可新增「经验教训」）
- 文件描述与实际代码不符 → 立即修正

没有需要沉淀的变化时跳过，但必须经过评估。
