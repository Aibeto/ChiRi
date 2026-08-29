# AGENTS.md

> 上次更新时间：2026-08-30 15:30:00
> 最后更新位于此 head 之后：bf7cef41de9feac2e74a742471c317f0a1b04a12

本文件为 AI 编程助手（Cursor / Claude Code / Trae 等）在本仓库工作时提供指导，由 AI 自主生成。

## 项目概述

**yumi** 是一个 Android CPU 智能调度控制系统（Magisk/KernelSU 模块），核心为 Rust 守护进程，通过 eBPF 内核探针采集 CPU 调度事件与渲染帧数据，结合 PID 控制 FAS 帧感知调度与 CPU 负载调速器（CLG）动态调频。

**项目方向**：本仓库为 **ChiRi**（自 imacte/yumi fork，README "Based on imacte/yumi"），**ChiRi 是发展主线**（8550 等 Chiri 目标 SoC，调度位于 `src/chiri/`）；**Yumi 调度（`src/scheduler/`）即将废弃**，作为 ChiRi 的基础保留，**勿改动其逻辑**——新功能、调优一律落在 `src/chiri/` 与处理器配置 `module/config/{soc}/`。

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
  config/             # config.yaml / rules.yaml / i18n (en.ftl / zh.ftl)；<soc>/config.yaml + akmode.yaml 处理器子目录
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

- **本地 check 的环境依赖**：eBPF 编译用 `-Z build-std`（仅 nightly 支持），且 build.rs 会构建 yumi-ebpf，因此 `cargo check` 需要 nightly 工具链 + `aarch64-linux-android` target + 可用的 bpf-linker。yumi 为 Android/Linux 专属 crate，切勿用 Windows host 目标检查（netlink-sys/aya 无法在 Windows 编译）。bpf-linker 在 Windows 下为 `bpf-linker.exe`（build.rs 已按 `cfg!(windows)` 兼容）。
- **yumi-ebpf 是 no_std/no_main 探针，独立 workspace**（根 `Cargo.toml` 的 members 只有 xtask，勿把 yumi-ebpf 加回）：只可用 bpfel 目标检查（`-Z build-std=core`），**禁止**在带 std 的目标（如 aarch64-linux-android 或 Windows host）下对它编译/检查——探针既无法用 std 目标编译（`unwinding panics are not supported without std`），test 剖面还会与 `#[panic_handler]` 冲突（`duplicate lang item panic_impl`）。根 build.rs 用 `current_dir=yumi-ebpf` 单独构建它。IDE（rust-analyzer）在根 workspace 下不会检查它。

- 本地开发通常只做 `cargo check` / WebUI `type-check` 验证；完整产物由云端 CI（GitHub Actions）生成。
- 不要随意执行 `cargo build`（需要 NDK 环境），优先静态检查。
- 完整构建的隐藏依赖：`build.rs` 的 `ensure_bpf_linker` 依次复用「PATH 中已有 bpf-linker」→「OUT_DIR 缓存」→ `cargo install bpf-linker` 兜底。CI 通过 GitHub API 解析官方 release asset 直接下载静态链接 LLVM 的预编译二进制（bpf-linker 0.11 依赖 LLVM 21+，源码编译在 ubuntu runner 上不可行；cargo-binstall 的 fallback 也会回退到源码编译）。eBPF 的 release 编译在 build.rs 内用 `CARGO_PROFILE_RELEASE_OPT_LEVEL=2` 局部覆盖（新版 bpf-linker 内嵌 LLVM 已移除 `-Oz`/`-Os`，仅支持 `-O0~O3`，workspace 根的 `opt-level="z"` 会导致链接失败）。**Windows 本机兜底**：bpf-linker 源码编译依赖 `os::unix` API，无法在 Windows 构建，且本地不承担完整产物构建（由云端 CI 生成），因此 `ensure_bpf_linker` 在 Windows 上无现成 bpf-linker（PATH/OUT_DIR 均无）时直接返回跳过错误，`build_ebpf` 捕获后经 `write_ebpf_stub()` 回退占位产物（`ebpf_target/bpfel-unknown-none/{debug,release}/yumi-ebpf`，与 `YUMI_SKIP_EBPF=1` 同路径），保证 IDE rust-analyzer（`.vscode/settings.json` 的 check 命令未设 YUMI_SKIP_EBPF）与本地 `cargo check` 不被阻塞；Windows 上若有 `bpf-linker.exe` 仍正常构建 eBPF，CI（Linux）行为不变。

## 代码约定

1. **架构**：Monitor 线程组通过有界 mpsc 事件通道（`DaemonEvent`，`sync_channel` 容量 64，满时 send 阻塞形成背压）解耦数据采集与调度决策，新增监控/调度能力遵循此模式。前台 PID 由 `monitor/mod.rs` 的单一 `pid_watcher` 线程经 `tokio::sync::watch` 广播，FPS/CPU 监控共享消费，不要各自重复轮询。FPS 帧监控（`fps_monitor`）仅服务于 FAS 调频，FAS 禁用期间不启动（见 `mod.rs` 中注释块）。**调度器为双套架构**：默认 `scheduler/`（Yumi CLG）与 `chiri/`（Chiri 专用）二选一，main.rs 按 `common::is_chiri_soc()`（特定 SoC 列表命中）决定启动哪一套；两套互斥消费同一事件通道，Monitor 层共享。**主次关系**：ChiRi 为发展主线，Yumi 即将废弃且逻辑冻结（勿改 `src/scheduler/` 与默认 `config.yaml` 行为），新功能/调优只落 `src/chiri/`。`CHIRI_SOC_HINTS` 在 common.rs 维护，新增机型只追加列表，不要绑定单一型号。**FAS 暂禁用期间的保留代码**（fps_monitor 整模块、`DaemonEvent` 的 `FrameUpdate`/`pid`/`foreground_max_util`、capacity 权重函数等）统一加 `#[allow(dead_code)]` 并注明"恢复 FAS 时启用"，**不要删除**——恢复时直接取消注释即可。**负载采样间隔动态切换**：`cpu_monitor` 的 `SystemLoadUpdate` 常规采样按 SoC 参数化——由 main.rs 按 `is_chiri_soc()` 传入 `start_monitor`→`start_cpu_loop`（ChiRi 160ms / 非 Chiri（Yumi）200ms，Yumi 200ms 为 akmode 改造前原有值，勿再改动）；明日方舟特调（akmode）激活时经共享 `Arc<AtomicBool>` 标志（main.rs 创建、`AkmodeGovernor` 接管/释放时置位）切换到 40ms。akmode 消费该负载流做**动态限频**（档位固定不切换，max 随负载在内核频率表中逐档升降，范围均为硬件上下限）；CLG 消费同一事件流，tick 语义按当前采样间隔（CLG 各 rate_limit_ticks/smoothing 参数按各自 tick 调优：ChiRi 160ms / Yumi 200ms）。
2. **日志**：调试与排障优先使用 `debug!`（勿全部用 info 污染信息日志）；频率控制/帧处理等高频路径用 25-tick / 60-frame 周期摘要，状态变化（模式、PID、屏幕、attach、档位）即时打点。新增日志 key 必须同时补充 `module/config/i18n/zh.ftl` 与 `en.ftl`，key 命名 `模块-描述`。
3. **配置**：运行时配置默认走 `module/config/config.yaml`（CLG/模式参数）与 `rules.yaml`（FAS/模式映射参数），支持热重载；新增配置项需同步更新反序列化结构体与默认值。配置由 main 启动时解析一次并以 `Arc<RwLock<Config>>` 共享给对应调度器（勿重复读取），热重载由该调度器的 config_watcher 覆写同一共享实例。两套调度各持自己的 Config 类型（`scheduler::config` 与 `chiri::config`）。**按处理器独立配置**：命中 `CHIRI_SOC_HINTS` 时，经 `common::get_config_path()` 优先加载处理器子目录 `config/{命中片段}/config.yaml`，否则回退 `config/config.yaml`；特调模式段在 `common::get_akmode_path()` 返回的同一处理器目录 `akmode.yaml`（与主配置同目录、随处理器绑定）。所有加载/热重载入口（main.rs、两套 config_watcher）必须统一走 `get_config_path()`，勿硬编码路径。守护进程启动时把生效配置相对 config 目录的路径（如 `8550/config.yaml`，非处理器时 `config.yaml`）写入 `active_config.txt`，WebUI 据此拼回同一份文件。
4. **i18n**：守护进程日志基于 Fluent（`module/config/i18n/en.ftl` / `zh.ftl`），WebUI 基于 `webui/src/i18n/locales/`；新增用户可见文案必须同时提供中英文。
5. **Rust 风格**：release profile 为极致体积优化（`opt-level = "z"`, lto, strip），避免引入重依赖；优先复用现有依赖（serde/anyhow/log/tokio/nix 等），新增第三方库选择社区高星、维护活跃的 crate。
6. **资源占用敏感**：守护进程运行于 Android 后台，注意内存分配（避免频繁 Vec 分配）、锁粒度和线程唤醒次数。
7. **版本同步**：发版时同步更新 `module/module.prop`（version/versionCode）、根 `Cargo.toml`（version）、`updateInformation/update.json` 与 `changelog.md`。**产物命名以 `module/module.prop` 为准**：xtask 读取其 `name + version`（配合 git 提交数与日期）生成 zip/目录名（如 `ChiRi-Alpha01-42-20260829-1200`），CI 用 `cargo xtask build --no-pack` 只组装目录、不预打包，目录名即 GitHub artifact 名，由 GitHub 下载时统一压缩成同名 `.zip`（避免 `.zip.zip`）。
8. **WebUI**：与守护进程通过 kernelsu bridge 交互（见 `webui/src/utils/bridge.ts`），勿直接硬编码路径；读取配置文件前先读 `active_config.txt` 确定实际生效文件。文件写入用 base64 管道（`echo '<b64>' | base64 -d > path.tmp && mv -f path.tmp path`）规避 shell 特殊字符解释，**必须经临时文件 + 原子 mv**，避免直接 `>` 截断时被守护进程 config_watcher 读到半截内容导致重载失败；不要用 `echo "${content}"` 拼接。**依赖约束**：`typescript` 固定在 5.x（`~5.9.0`）——TS 7.x 为 Go 原生编译器，不再导出 `lib/tsc`，`vue-tsc` 3.x 无法兼容（type-check 报 `ERR_PACKAGE_PATH_NOT_EXPORTED`），勿升级。
9. **内部特调白名单（ChiRi 专属）**：特定应用的专用模式定义在 `common.rs` 的 `SPECIAL_TUNED_MODES`，每项含包名、可用模式列表 `modes` 与优先回退模式 `fallback`（同包名多模式时，用户未显式配置则采用 `fallback`）。白名单编译进二进制，不随 rules.yaml 下发，用户/WebUI 不可修改。**特调体系仅限 ChiRi**：只在命中 `CHIRI_SOC_HINTS` 的 SoC 上生效（`determine_mode` 先判 `is_chiri_soc()`），非 ChiRi SoC 上特调映射一律回退全局模式；只在 chiri 的 `Config` 挂载独立特调字段 `akmode`（`SpecialTunedConfig`），**不要**注册进 `get_mode`（`get_mode` 只认 CLG 常规模式），也**不要**注册进 yumi 的 `scheduler/config.rs`。模式确定：**白名单应用始终进特调**（前台命中 `SPECIAL_TUNED_MODES` 就返回特调模式，不管 app_modes/global_mode 配了什么，`determine_mode` 开头直接判定）；rules.yaml 里给该应用配的普通模式只作为特调起始档（scheduler 侧 `get_ak_initial_tier` 识别）；非白名单应用的模式优先级仍为用户 `app_modes` > `global_mode`，后端门控：非白名单包名映射到特调模式时 warn 并回退 `global_mode`（`app_detect.rs` 的 `determine_mode`）。**特调为完全独立调度**：`src/chiri/akmode.rs` 的 `AkmodeGovernor` 与 CLG 完全解耦，前台为白名单应用（明日方舟）时由 `mod.rs` 的 `scheduler_ipc` 先 `cpu_governor.release()` 再 `ak_governor.init_policies()` 接管，退出前台反向释放；**特调模式下息屏保持 akmode 接管、不切换 CLG doze**（akmode 已统一 schedutil，息屏随负载自然降频省电，`mod.rs` 的 `ScreenStateChange` 分支先判 `is_special_mode`），非特调模式息屏仍走 CLG doze。四档就是全局那套模式档位 powersave/balance/performance/fast（不另起 tier 体系），**档位由 rules.yaml 生效模式决定**（明日方舟 app_modes > global_mode，`config::mode_to_tier` 换算），**特调期间固定应用、不自动切换档位**（用户改 rules.yaml 模式后经 ConfigReload 热重载更新档位）；档位差异仅在升降频策略参数（8550 硬编码：little 0-2 / big 3-6 / prime 7，每组独立 up_core_count/up_util_percent/down_core_count/down_util_percent，核心数为组内绝对个数、yaml 写整数、0 = 组内任一核心命中即触发、写大值如 64 = 关闭该方向判定，占用率写整数百分比、加载时转 0..1）和防抖等待（wait_ms，每档可不同），**所有档位都能使用硬件最高档位**。**全局统一 schedutil**：CLG 与 akmode 均把内核调速器写为 schedutil（两套调度器各自 `init_policies` 时写 governor、release 时恢复快照，yumi 的 `scheduler/cpu_load_governor.rs` 同样改 schedutil）。**特调动态限频（schedutil + 负载驱动升降 max）**：`AkmodeGovernor` 激活时写 schedutil、min 压到硬件最低、max 设为硬件最高；`on_load_update`（特调 40ms tick）用当前档位策略参数按核心组判定升降（升频 = 任一组超过 up_core_count 个核心 util > up_util_percent；降频 = 任一组超过 down_core_count 个核心 util < down_util_percent，升频优先），**升频前检查实际频率（scaling_cur_freq）是否已达当前设定的 max**（schedutil 余量），达到才在频率表中升一档；**降频直接把 max 降为当前实际频率对应档位**（`read_cur_freq` 后 `partition_point` 找 <= 实际频率的最高档，实际不可读回退降一档，绝不高于当前 max）——max 上下限均为硬件上下限。升降频带 wait_ms 防抖（升降后 `after_change_duration_ms` 内减半）。CLG 仍锁 min=max（schedutil 在锁频点无调频空间，频率由 CLG 决定）。**特调参数独立成文件，且与处理器绑定**：`module/config/{命中片段}/akmode.yaml` 定义单特调段 `akmode`，chiri 的 `Config::from_file` 在 `merge_akmode()` 中经 `common::get_akmode_path()` 合并（读取/解析失败 warn 保留旧值），不放在默认 `config.yaml` 里。守护进程启动时把白名单导出到 `special_tuned.txt`（每行 `包名:模式列表(逗号分隔):优先回退模式`）并 info 打点，导出**仅 Chiri 模式**（`is_chiri_soc()`）下发生，Yumi 设备不生成该文件。WebUI 为双套流动：`bridge.ts::isChiri`（active_config 为处理器子目录 `config/{soc}/config.yaml` 时真）判定设备类型，`stores/scheduler.ts` 存 `isChiri` 态；`getSpecialTuned` 只读展示的「特调：{模式}」标签、模式动作单的专属特调选项（置顶、仅白名单应用可见）、重扫后的 `pruneSpecialTunedRules` 清理，均仅在 `isChiri` 下激活（`isChiri && specialTuned[pkg]`），Yumi 设备完全不显示特调 UI。应用列表提供「重新扫描」按钮（扫描中禁用防并发），扫描完成后清理 rules.yaml 中非白名单/非法特调映射。
10. **CLG 调频语义（ChiRi 8550 优化）**：CLG 仍锁 min=max（schedutil 在锁频点无调频空间，频率由 CLG 决定）。**事件驱动升降频（backset 统一写频）**：`cpu_load_governor.rs` 的 `on_load_update`（ChiRi 采样 160ms，由 main.rs 按 SoC 传入）只做**决策**（更新各 cluster `current_perf`，不写 sysfs）；写频统一走 `flush()`（backset 入口：触摸地板钳位 + clamp + `find_nearest_freq` + `write_freq` 去重），由 scheduler_ipc 在每次 CLG 决策后、以及每次触摸事件时调用；`release()`/`init_policies` 会清空触摸状态。**升频带 schedutil 余量检查（按需升频）**：升频分支先读 `scaling_cur_freq`，实际频率未追平当前锁定频率时忽略本次升频（debug `clg-up-skipped`），等硬件自然爬升后再升；**降频为直接降频**：防抖确认（`down_rate_limit_ticks`，极低负载命中 `down_fast_threshold` 免防抖）后一步到位写目标档，目标不高于当前实际频率（`ratio_of_freq` 同步 current_perf）。已删除废弃参数 `smoothing_down / slow_down_scale / down_fast_mult`。**触摸升频（事件驱动，ChiRi 专属）**：`touch_detect.rs` 线程读 `/dev/input/event*`（`libc::poll` + 解析 64 位 `input_event` 24 字节，`BTN_TOUCH==1` 或 `ABS_MT_TRACKING_ID>=0` 判定触摸按下），触摸按下时经独立事件通道 `mpsc::sync_channel::<()>` 发送事件（非阻塞 `try_send`）；scheduler_ipc 每次醒来先 drain `touch_rx`，收到事件即 `on_touch()`（把大核 3-6 性能下限抬 `touch_boost_tiers` 档、设 `touch_boost_until`）+ `flush()` 立即写频，**不等待下一个 160ms 决策 tick**（触摸延迟 ≤ `EVENT_POLL_MS=100ms`）。配置项 `touch_boost_enabled/ms/tiers` 每模式独立，`enabled=false` 即关闭（normalize 会把 ms 置 0）。**屏蔽系统触摸升频**：`chiri/scheduler.rs::apply_disable_touch_boost` 写 0 到 `/sys/module/cpu_boost/parameters/` 的 `input_boost_enabled / sched_boost_on_input / input_boost_ms / boost_ms`（按存在性尝试，无节点静默跳过）；`start_scheduler_thread` 启动时即调用一次 `apply_system_tweaks()`。**数据获取优化**：常规采样按 SoC 参数化（ChiRi 160ms / Yumi 200ms，由 main.rs 按 `is_chiri_soc()` 传入 `start_monitor`→`start_cpu_loop`）；`foreground_max_util` 仅 FAS 消费，FAS 禁用期间经 `FAS_FG_UTIL_ENABLED=false` 跳过计算（发送 0.0，两套调度器均忽略），`get_thread_tids/compute_tgid_util/compute_thread_level_util` 加 `#[allow(dead_code)]` 保留（恢复 FAS 时置 true）。**scenemode（息屏超时极致省电）**：chiri `Config` 新增 `scenemode`（Mode 段，默认 `enabled:true`、`perf_ceil` 极低封顶、`up_threshold=1.0` 不主动升频）与顶层 `scene_mode_delay_secs`（默认 300s=5 分钟）；`mod.rs` 息屏时记录 `screen_off_at`，`SystemLoadUpdate` 分支在息屏超时且非特调模式下一次性把 CLG 热切到 scenemode（scenemode 未启用则 release 回系统默认），亮屏自动恢复原模式。Fast 模式由 `perf_floor/ceil/init=1.0` 锁定硬件最高频。

## 硬性约束

- 不修改 CI 配置的构建流程（`.github/workflows/build.yml`），除非明确要求。
- eBPF 程序目标为 `bpfel-unknown-none`，改动需确保可在 CI 环境交叉编译通过。
- 保持与 KernelSU/Magisk 模块规范的兼容（`module/` 目录结构、`service.sh` 启动流程）。
- **Yumi 逻辑冻结**：不修改 `src/scheduler/` 与默认 `module/config/config.yaml` 的调度逻辑与参数行为（Yumi 即将废弃，作为 ChiRi 基础保留）；确需修复时先与 ChiRi 对齐、最小改动。共享层（`src/monitor/`、`main.rs`）如需为 ChiRi 适配（如采样参数化），必须保持 Yumi 运行时行为不变（Yumi 常规采样 200ms 原值勿再改动）。

## 经验教训

- **日志文件被删不能崩**：`src/logger.rs` 用自实现 `SelfHealingAppender`（替换 log4rs `RollingFileAppender`），每次写入都按路径 `create+append` 重新打开，`daemon.log` 被外部删除会自动重建；循环轮转与锁上锁全程 `Result`/剥除 poison，绝不 `unwrap`，杜绝日志路径 panic 打崩守护进程。勿改回 log4rs 滚动追加器。
- **WebUI 手动重启必须 `setsid`**：`bridge.ts::restartDaemon` 用 `killall -9 yumi; sleep 1; setsid "$MODDIR/service.sh" </dev/null >/dev/null 2>&1 &` 拉起。因 `ksu.exec` 返回会清理执行 shell 的进程组，过去直接 `nohup ... &` 后台拉起的守护进程会被一并杀掉（“只杀没起”）。`setsid` 新开会话 + 重定向标准流后脱离 exec 进程组存活，等效手动执行 service.sh。

## AGENTS.md 维护要求（重要）

**每次对话结束前，必须回顾本次会话内容，评估是否需要更新本文件：**

- 新增/删除/重构了模块、目录或关键文件 → 更新「目录结构」
- 引入了新的依赖、工具链或构建命令 → 更新「技术栈」「常用命令」
- 确立了新的代码约定、架构决策或踩坑经验 → 更新「代码约定」（可新增「经验教训」小节）
- 发现本文件描述与实际代码不符 → 立即修正

若本次对话未产生需要沉淀的变化，可不更新，但必须经过此评估步骤。
