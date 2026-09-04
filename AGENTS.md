# AGENTS.md

> 上次更新时间：2026-09-04
> 最后更新位于此 head 之后：40577b0af15ce66a0875546b1d98d730fa2925af

本文档为 AI 编程助手（Cursor / Claude Code / Trae 等）在本仓库工作时的指导文件。

## 项目概述

**yumi** 是 Android CPU 智能调度控制系统（Magisk/KernelSU 模块），核心是 Rust 守护进程：eBPF 内核探针采集 CPU 调度事件与渲染帧数据，结合 FAS 帧感知调度和 CLG 负载调速器动态调频。

本仓库为 ChiRi（自 imacte/yumi fork，README "Based on imacte/yumi"），调度代码在 `src/chiri/`。`src/scheduler/`（Yumi 调度）即将废弃，仅作为 ChiRi 的基础保留，勿改动其逻辑。新功能、调优只落 `src/chiri/` 和处理器配置 `module/config/{soc}/`。

- 目标平台：Android 8.0+ / AArch64 / 需要 Root

- 许可证：GPL-3.0-or-later

- 版本：见 `module/module.prop` 与 `Cargo.toml`（需保持同步）

## 目录结构

```
src/                  # Rust 守护进程主代码
  monitor/            # 监控层：app_detect / fps_monitor / cpu_monitor / screen_detect / telemetry（两套调度共享）
  scheduler/          # 调度层 Yumi（即将废弃，作为 ChiRi 基础保留、勿动逻辑）：FAS 引擎、CLG 负载调速器
    fas/              # FAS 核心：PID 控制器、帧率档位、frame_pipeline
  chiri/              # ChiRi 调度（发展主线；特定 SoC 触发；含 CLG、akmode 明日方舟特调、touch_detect 触摸升频、affinity 按核亲和/线程迁移、core_ctl 核心在线接管）
  chiri/affinity_blacklist.txt  # 线程亲和黑名单（编译期嵌入：系统关键进程默认名单 + re: 正则；含 com.example 示例；用户/WebUI 不可改）
  common.rs / fas_types.rs / i18n.rs / logger.rs
yumi-ebpf/            # eBPF 探针（bpfel-unknown-none，build-std 编译；独立 workspace，不在根 members；sched_switch + queueBuffer + 遥测计数探针）
xtask/                # 构建脚本（cargo xtask build 完成编译打包 zip）
module/               # Magisk/KernelSU 模块载体（module.prop、customize.sh、service.sh）
  config/             # config.yaml / rules.yaml / i18n (en.ftl / zh.ftl)；<soc>/config.yaml 处理器子目录（各 SoC 自带，8475/8998 参数相同、各自一份）+ normal/akmode.yaml、normal/scenemode.yaml（编译期嵌入）
webui/                # Vue 3 + TypeScript + Vite + Pinia + Vant 管理界面
updateInformation/    # 更新.json 与 changelog
.github/workflows/    # CI：Node 24 + Rust nightly + NDK r29 + cargo-ndk
```

## 技术栈

| 层     | 技术                                                                             |
| ----- | ------------------------------------------------------------------------------ |
| 守护进程  | Rust (edition 2024, nightly), tokio, aya (eBPF), serde\_yaml, inotify, netlink |
| eBPF  | aya 框架，`sched_switch` tracepoint + `queueBuffer` uprobe                        |
| WebUI | Vue 3, TypeScript, Vite, Pinia, Vant, vue-i18n, kernelsu                       |
| 构建    | cargo xtask build（Rust aarch64-linux-android 交叉编译 + webui npm build + 打包）      |

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

- 本地 check 需要 nightly + aarch64-linux-android target + bpf-linker：eBPF 编译用 `-Z build-std`（仅 nightly 支持），build.rs 会构建 yumi-ebpf。yumi 是 Android/Linux 专属 crate，别用 Windows host 目标检查（netlink-sys/aya 无法在 Windows 编译）。Windows 下 bpf-linker 为 `bpf-linker.exe`（build.rs 已按 `cfg!(windows)` 兼容）。

- yumi-ebpf 是 no\_std/no\_main 探针、独立 workspace（根 `Cargo.toml` 的 members 只有 xtask，勿把 yumi-ebpf 加回）：只可用 bpfel 目标检查（`-Z build-std=core`），禁止在带 std 的目标（aarch64-linux-android、Windows host）下编译/检查。探针无法用 std 编译（`unwinding panics are not supported without std`），test 剖面还会与 `#[panic_handler]` 冲突（`duplicate lang item panic_impl`）。根 build.rs 用 `current_dir=yumi-ebpf` 单独构建，IDE（rust-analyzer）在根 workspace 下不检查它。

- 本地开发只做 `cargo check` / WebUI `type-check`；完整产物由 CI（GitHub Actions）生成。不要随意 `cargo build`（需要 NDK 环境），优先静态检查。

- bpf-linker 获取：`build.rs` 的 `ensure_bpf_linker` 依次尝试 PATH 中已有 bpf-linker → OUT\_DIR 缓存 → `cargo install bpf-linker`。CI 通过 GitHub API 下载静态链接 LLVM 的预编译二进制（bpf-linker 0.11 依赖 LLVM 21+，源码编译在 ubuntu runner 上不可行；cargo-binstall 也会回退到源码编译）。eBPF release 编译在 build.rs 内用 `CARGO_PROFILE_RELEASE_OPT_LEVEL=2` 局部覆盖（新版 bpf-linker 已移除 `-Oz`/`-Os`，仅支持 `-O0~O3`，workspace 根的 `opt-level="z"` 会导致链接失败）。Windows 兜底：bpf-linker 源码编译依赖 `os::unix` API，Windows 上无法构建，且本地不承担完整产物构建。`ensure_bpf_linker` 在 Windows 无现成 bpf-linker 时返回跳过错误，`build_ebpf` 捕获后经 `write_ebpf_stub()` 回退占位产物（`ebpf_target/bpfel-unknown-none/{debug,release}/yumi-ebpf`，与 `YUMI_SKIP_EBPF=1` 同路径），保证 rust-analyzer 和本地 `cargo check` 不被阻塞。Windows 上若有 `bpf-linker.exe` 仍正常构建 eBPF，CI 行为不变。

## 代码约定

### 架构与事件流

- Monitor 线程组通过有界 mpsc 事件通道（`DaemonEvent`，`sync_channel` 容量 64，满时 send 阻塞形成背压）解耦数据采集与调度决策，新增监控/调度能力遵循此模式。

- 前台 PID 由 `monitor/mod.rs` 的 `pid_watcher` 线程经 `tokio::sync::watch` 广播，FPS/CPU 监控共享消费，不要各自轮询。

- 调度器双套架构：`scheduler/`（Yumi）与 `chiri/` 二选一，main.rs 按 `common::is_chiri_soc()`（SoC 列表命中）决定启动哪一套。两套互斥消费同一事件通道，Monitor 层共享。ChiRi 是主线，Yumi 即将废弃且逻辑冻结，新功能/调优只落 `src/chiri/`。

- `CHIRI_SOC_HINTS` 在 common.rs 维护，新增机型只追加列表，不要绑定单一型号；同时提供 `config/{片段}/config.yaml` 和核心组区间 `common::chiri_core_ranges()`。匹配只看硬件标识片段，不检查磁盘目录（配置编译进二进制，硬件命中即生效）。

- FAS 暂禁用期间的保留代码（fps\_monitor 整模块、`DaemonEvent` 的 `FrameUpdate`/`pid`/`foreground_max_util`、capacity 权重函数等）统一加 `#[allow(dead_code)]` 并注明"恢复 FAS 时启用"，不要删除。FPS 帧监控（fps\_monitor）仅服务于 FAS 调频，FAS 禁用期间不启动（见 `mod.rs` 注释块）。`foreground_max_util` 仅 FAS 消费，FAS 禁用期间经 `FAS_FG_UTIL_ENABLED=false` 跳过计算（发送 0.0，两套调度器均忽略）；`get_thread_tids/compute_tgid_util/compute_thread_level_util` 加 `#[allow(dead_code)]` 保留（恢复 FAS 时置 true）。

- 负载采样间隔：`cpu_monitor` 的 `SystemLoadUpdate` 按 SoC 参数化，main.rs 按 `is_chiri_soc()` 传入 `start_monitor`→`start_cpu_loop`（ChiRi 160ms / Yumi 200ms，Yumi 200ms 为原值勿改）；akmode 激活时经共享 `Arc<AtomicBool>`（main.rs 创建、`AkmodeGovernor` 接管/释放时置位）切换到 40ms。akmode 消费该负载流做动态限频（档位固定，max 随负载在内核频率表中逐档升降，范围均为硬件上下限）；CLG 消费同一事件流，tick 语义按当前采样间隔（各 rate\_limit\_ticks/smoothing 按各自 tick 调优）。

- **事件循环低开销原则（性能响应零延迟约束下）**：所有性能敏感路径（负载事件/模式切换/触摸升频）都是**推送事件**——`recv_timeout` 被事件到达立即打断，轮询周期只影响非性能的定时任务。据此：① chiri `scheduler_ipc` 用**动态超时**阻塞到「最近一个周期任务 deadline」（telemetry 1s / thermal+亲和 2s / mode file 5s / fast\_lock 重写 5s 各自取 min，上限 `EVENT_POLL_MS=1s`），空闲从每秒 10 次空转降到 \~1 次，勿改回固定 100ms 轮询；② CLG Worker 的 `run()` 超时分支只做触摸窗口清理 + 防篡改重写（1s 粒度），负载/触摸决策全走推送事件即时触发，勿把超时改回 160ms；③ 新增周期任务时**必须把它的 deadline 纳入** `recv_timeout` 的 wait 计算（取 min），否则任务会被拖到最长 1s 粒度；④ 高频 `debug!` 中的 `format!/collect/join` 构造发生在宏求值前，INFO 级别下每 tick 白白分配——用 `log::log_enabled!(log::Level::Debug)` 门控（scheduler-event-load / clg-tick-log / akmode-tick-log / cpu-monitor-tick-log 已做）。

### 日志

- 调试与排障优先用 `debug!`，别全用 info 冲掉有价值的信息；高频路径（频率控制/帧处理）用 25-tick / 60-frame 周期摘要，状态变化（模式、PID、屏幕、attach、档位）即时打点。

- 日志文件只有两个：`logs/daemon.log`（log4rs，范围不变）+ `logs/status.csv`（CSV 宽表 + `type` 列：仅 `snap` 一种行类型，1s 一条，整合原 power/telemetry 全部字段 + **前台包名**（`package` 列，实时取自 `app_detect::get_current_package()`，切换点由相邻行变化体现）+ **充放电状态**（`charge` 列，1s 读一次 `/sys/class/power_supply/battery/status`，归一为 charging/discharging/full/not\_charging，缺失 "-"——电流符号因厂商节点方向不一不可靠）。已无 `fg` 事件行：所有信息都在每秒汇总行内，稳定 1s 一行，不再在包切换时额外写行）。勿再依赖 ModeChange 事件记录前台切换——模式不变时无该事件，这是原 foreground.log 失效的根因。

- status 写入用常驻 append 句柄（`STATUS_WRITER`，每行仅 1 次 write syscall）+ 每 256 行巡检（8MB 轮转 1 备份、被删自愈）。禁止恢复每行 open/stat 的 `append_aux_log` 模式，勿再拆分 power/telemetry/foreground 独立文件。

- 启动日志归档：main 在 `create_dir_all(logs)`/`logger::init` 之前调 `logger::archive_logs_on_startup`——把上一轮整个 `logs/` 原子 rename 为同级 `ziped_<毫秒时间戳>` 临时目录，交一次性子线程 `log_archiver` 打包为 `logd/ziped_<ts>.zip`（手写 stored ZIP，零新依赖，勿引入 zip/flate2）后删除临时目录并自然退出；打包失败保留临时目录并写 ARCHIVE\_FAILED.txt（此时 logger 未 init 无法打点）。必须复制回 `logs/watchdog.pid`——看门狗先于 daemon 启动、WebUI stopScheduler 靠它终止看门狗，归档带走它会导致「关闭调度」失效。

- 开发诊断日志（devimp/）：独立目录 `<模块根>/devimp/`（与 logs/ 平级，**不受日志归档影响**）。每次启动**只创建一个文件** `devimp_<unix 毫秒时间戳>.log`（首次写入惰性创建，进程生命周期不换文件、无 8MB 轮转；软上限 128MB 触顶静默停写）。总开关 `logger::set_devimp_active`（scheduler\_ipc 按 `Config.meta.dev_record` 同步，meta 允许外部修改的字段之一、WebUI「开发记录」开关 + 热重载）；模式名经 `logger::set_devimp_mode` 同步、各行 mode 列自动填充。CSV 宽表 40 列 + `type` 列：`tick`（每决策 tick × 每核心组的调频决策轨迹，CLG Worker 与 akmode on\_load\_update 写入）、`snap`（1s 环境上下文，前台包名**实时取自** **`app_detect::get_current_package()`**——ModeChange 仅模式变化时才有事件，用事件维护会写过期包名）、`place`（低频快照：前台线程的包名/线程名/落点核，全部来自 affinity 缓存——comm 在线程首见 stat 采样时缓存、fg\_cmdline 每轮刷新一次，**零新增文件读**）、`aff`（亲和迁移动作 pin/promote/demote/restore/blacklist\_skip，包名列用缓存 cmdline）、`core`（逐核 util + 钉核计数，每 2 轮一次）、`event`（模式/屏幕/热/配置状态变化）。**共用数据减小开销**：电池/CPU 温度在 1s snap 块读一次存入 `last_batt_temp/last_cpu_temp`，status.csv、devimp snap、thermal（2s）三处共用（thermal 复用 ≤1s 旧值，带回滞的秒级判定无影响）；affinity 的逐核 util/在线位图/线程 comm 均为缓存复用。main 启动时 `logger::devimp_prepare()` 清旧留新（保留最近 10 份）。未开启开关时所有写入点零 IO。

- 新增日志 key 同时补 `module/config/i18n/zh.ftl` 与 `en.ftl`，命名格式 `模块-描述`。

### 配置（嵌入、快照与热重载）

- 调优配置编译期嵌入二进制（防篡改）：`common.rs` 用 `include_str!` 打包 `module/config/{soc}/config.yaml ×3`、默认 `config.yaml`、`normal/akmode.yaml`、`normal/scenemode.yaml` 与 i18n 两个 ftl。运行时一律以 `common::embedded_config_str()`（按 `matched_soc_hint()` 选择）为准，磁盘同名文件只是「自愈快照 + meta 覆盖入口」。

- `Config::load(path)`（chiri 与 scheduler 各一份，替代旧 `from_file`）用嵌入内容做基准，仅从磁盘文件反序列化 meta 段（`common::read_external_meta` 返回 `ExternalMetaOverrides`，只取 loglevel + dev\_record，调优字段与其他 meta 字段即使被篡改也不读入）。

- 快照自愈：`common::sync_config_snapshot()` 在 main 启动时与两套 config\_watcher 热重载后，把「嵌入内容 + 磁盘 meta 覆盖」写回 `get_config_path()`（内容一致时跳过写入防 inotify 成环），磁盘文件被篡改/删除都会被还原/重建。允许外部修改的字段只有两个：meta.loglevel（WebUI 日志等级切换）与 meta.dev\_record（WebUI「开发记录」开关，控制 devimp/ 诊断日志写入，热重载生效；布尔替换必须写裸值——serde\_yaml 把带引号的 "true" 解析为字符串而非 bool；language 等其余内容固定，外部修改无效且会被快照自愈还原）。

- `rules.yaml` 保持磁盘读写（WebUI 全局模式切换、应用性能模式需要）。运行时状态文件（active\_config.txt / current\_mode.txt / special\_tuned.txt 导出 / 日志）不变。

- `matched_soc_hint()` 已不检查磁盘目录存在性。新增配置项需同步更新反序列化结构体与默认值；改 `module/config/` 下的 yaml 源文件后重新编译才生效。**`module/config/config-example.yaml`** **是完整字段说明书（不参与加载），任何新增/删除/改语义的配置段与 meta 字段都必须同步更新它**（含注释说明取值范围与机型差异），否则模板与实际配置脱节。

- 所有加载/热重载入口（main.rs、两套 config\_watcher）统一走 `get_config_path()`，不要硬编码路径。启动时把生效配置的相对路径（如 `8550/config.yaml`，非处理器时 `config.yaml`）写入 `active_config.txt`，WebUI 据此读取同一份文件。

- 热重载链路三处断链已修复，勿回退：

  1. config\_watcher 必须监听生效配置的父目录（`config_path.parent()`），不是固定 `config/` 根目录。inotify 目录监听不递归，ChiRi 生效配置在 `config/{soc}/config.yaml` 子目录，监听根目录收不到 CLOSE\_WRITE/MOVED\_TO，导致 8550/8475/8998 上 WebUI 改 meta.loglevel 热重载完全失效（Yumi 生效配置在根目录，所以历史未暴露）。
  2. `common::sync_config_snapshot` 必须原子写（同目录 tmp 文件 + rename）。直接 `fs::write` 截断覆盖存在窗口期，config\_watcher/WebUI 可能读到半截内容导致 meta 解析失败回退默认（用户日志等级被静默丢弃）；rename 触发 MOVED\_TO 后靠「内容一致跳过写入」防环。
  3. config.yaml 调参热重载须联动运行中的调度器：config\_watcher 重载成功后置 `config_dirty`（`Arc<AtomicBool>`），scheduler\_ipc 循环内 `swap` 消费，亮屏时按当前模式 reload CLG/akmode 并刷新亲和/core\_ctl（息屏不覆盖 Doze，亮屏事件补上）；此前调参要等下次 ModeChange/规则重载才生效。

### i18n

- 守护进程日志用 Fluent，ftl 语言包编译期嵌入二进制（`common::embedded_ftl_str`，zh → zh.ftl、其余回退 en.ftl，磁盘 `module/config/i18n/` 不再被读取）；新增日志 key 改 `module/config/i18n/zh.ftl` 与 `en.ftl` 源文件后重新编译。

- WebUI 用 `webui/src/i18n/locales/`；新增用户可见文案同时提供中英文。

### Rust 风格与资源占用

- release profile 体积优先（`opt-level = "z"`, lto, strip），避免引入重依赖；优先复用现有依赖（serde/anyhow/log/tokio/nix 等），新增第三方库选社区高星、维护活跃的 crate。

- 守护进程跑在 Android 后台，注意内存分配（避免频繁 Vec 分配）、锁粒度和线程唤醒次数。

### 版本与发布

- 发版时同步更新 `module/module.prop`（version/versionCode）、根 `Cargo.toml`（version）、`updateInformation/update.json` 和 `changelog.md`。

- 产物命名以 `module/module.prop` 为准：xtask 读取 `name + version`（配合 git 提交数与日期）生成 zip/目录名（如 `ChiRi-Alpha01-42-20260829-1200`）。CI 用 `cargo xtask build --no-pack` 只组装目录、不预打包，目录名即 GitHub artifact 名，由 GitHub 下载时压缩成同名 `.zip`（避免 `.zip.zip`）。

### WebUI

- 与守护进程通过 kernelsu bridge 交互（见 `webui/src/utils/bridge.ts`），不要硬编码路径；读配置前先读 `active_config.txt` 确定实际生效文件。

- 文件写入用 base64 管道（`echo '<b64>' | base64 -d > path.tmp && mv -f path.tmp path`）避免 shell 特殊字符干扰，必须经临时文件 + 原子 mv，防止直接 `>` 截断时 config\_watcher 读到半截内容导致重载失败；不要用 `echo "${content}"` 拼接。

- 依赖约束：`typescript` 固定 5.x（`~5.9.0`）。TS 7.x 是 Go 原生编译器，不再导出 `lib/tsc`，`vue-tsc` 3.x 无法兼容（type-check 报 `ERR_PACKAGE_PATH_NOT_EXPORTED`），勿升级。

- 不开放 YAML 编辑：`/config` 页只查看生效配置文件（`active_config.txt` 解析路径 + meta 抬头：配置名/作者/日志语言/日志等级）并切换日志等级（`bridge.ts::setLogLevel` 只替换 `meta.loglevel` 行、保留注释与其余内容，config\_watcher 热重载即时生效），另提供「开发记录」开关（`bridge.ts::setDevRecord` 同口径替换 `meta.dev_record` 行，布尔裸值；MockBridge 需同步提供同名方法——`Bridge = isDev ? MockBridge : RealBridge` 是联合类型，缺方法 type-check 报错）；rules.yaml 仅在 WebUI 内部读写（全局模式切换、应用性能模式），不提供整文件编辑。

- rules.yaml 写入防 null：js-yaml 会把值为 null 的字段序列化成 `app_modes: null`，而 serde\_yaml 无法把 null 反序列化为 HashMap（`#[serde(default)]` 只对缺失字段生效），导致守护进程每次加载 rules.yaml 告警。`bridge.ts::saveRulesConfig` 写盘前移除 null/undefined 的 `app_modes` 键；守护进程侧 `monitor/config.rs` 的 `RulesConfig::app_modes` 用 `deserialize_with`（untagged 枚举）显式兼容 null 为空表，已存在 null 的旧文件也不告警。

## ChiRi 调度子系统

以下子系统仅在命中 `CHIRI_SOC_HINTS` 的 SoC 上生效。全局统一 schedutil：ChiRi 的 CLG 与 akmode 均把内核调速器写为 schedutil（两套调度器各自 `init_policies` 时写 governor、release 时恢复快照）；Yumi 的 `scheduler/cpu_load_governor.rs` 写 `performance`（Yumi 原有行为，勿改）。

### 特调（akmode）

白名单数据：

- 白名单数据在独立文件 `src/chiri/special_tuned.txt`，经 `include_str!` 编译进二进制（common.rs 的 `parse_special_tuned` 解析、OnceLock 缓存），用户/WebUI 不可修改。

- 格式：每行 `匹配器:模式列表(逗号分隔):优先回退模式`。匹配器支持精确包名与 `re:` 前缀正则（忽略大小写用 `(?i)`，匹配器内不能含 ':'）；`special_tuned_entry(pkg)` 先精确匹配（文件顺序）、未命中再按正则条目匹配。每项含可用模式列表 `modes` 与优先回退模式 `fallback`（用户未显式配置则采用 `fallback`）。

- 当前条目：明日方舟国服 `com.hypergryph.arknights`、日服 `com.YoStarJP.Arknights`、正则兜底 `re:(?i)arknights`（覆盖台服/Mod 变体），模式均为 `akmode`（fallback 同）。

- main.rs 启动时把精确条目导出到运行时文件 `special_tuned.txt`（每行 `包名:模式列表(逗号分隔):优先回退模式`，正则条目无法按包名精确查找故不导出）并 info 打点。导出仅 `is_chiri_soc()` 下发生，Yumi 设备不生成该文件。

生效范围与门控：

- 特调体系仅限 ChiRi：`determine_mode` 先判 `is_chiri_soc()`，非 ChiRi SoC 上特调映射一律回退全局模式。

- 只在 chiri 的 `Config` 挂载独立特调字段 `akmode`（`SpecialTunedConfig`）；不要注册进 `get_mode`（只认 CLG 常规模式），也不要注册进 yumi 的 `scheduler/config.rs`。

- 白名单应用始终进特调：前台命中 `special_tuned_entry()` 就返回特调模式，不管 app\_modes/global\_mode 配了什么（`determine_mode` 开头直接判定）。rules.yaml 里给该应用配的普通模式只作为特调起始档（scheduler 侧 `get_ak_initial_tier` 识别）。

- 非白名单应用的模式优先级仍为用户 `app_modes` > `global_mode`；后端门控：非白名单包名映射到特调模式时 warn 并回退 `global_mode`（`app_detect.rs` 的 `determine_mode`）。

调度行为：

- 特调是完全独立调度：`src/chiri/akmode.rs` 的 `AkmodeGovernor` 与 CLG 完全解耦。前台为白名单应用时由 `mod.rs` 的 scheduler\_ipc 先 `cpu_governor.release()` 再 `ak_governor.init_policies()` 接管，退出前台反向释放。

- 特调模式下息屏保持 akmode 接管、不切换 CLG doze（akmode 已统一 schedutil，息屏随负载自然降频省电；`mod.rs` 的 `ScreenStateChange` 分支先判 `is_special_mode`），非特调模式息屏仍走 CLG doze。

- 四档就是全局那套模式档位 powersave/balance/performance/fast（不另起 tier 体系）。档位由 rules.yaml 生效模式决定（明日方舟 app\_modes > global\_mode，`config::mode_to_tier` 换算）；特调期间固定应用、不自动切换档位，用户改 rules.yaml 模式后经 ConfigReload 热重载更新档位。所有档位都能使用硬件最高档位。

- 档位差异仅在升降频策略参数和防抖等待（wait\_ms，每档可不同）。核心组区间随命中 SoC 变化，统一在 `common::chiri_core_ranges()`（8550 little 0-2 / big 3-6 / prime 7；8475 0-3/4-6/7；8998 0-3/4-7 无 prime），akmode 与 CLG 触摸升频共用。每组独立 up\_core\_count/up\_util\_percent/down\_core\_count/down\_util\_percent：核心数为组内绝对个数，yaml 写整数，0 = 组内任一核心命中即触发，写大值如 64 = 关闭该方向判定；占用率写整数百分比，加载时转 0..1。

- 动态限频（schedutil + 负载驱动升降 max）：`AkmodeGovernor` 激活时写 schedutil、min 压到硬件最低、max 设为硬件最高。`on_load_update`（特调 40ms tick）用当前档位策略参数按核心组判定升降，升频优先：升频 = 任一组内达到 up\_core\_count 个核心 util > up\_util\_percent；降频 = 任一组内达到 down\_core\_count 个核心 util < down\_util\_percent（达到 = 组内核心数 >= core\_count，即配置值就是绝对个数）。统计口径：util 恰为 0.0 的核心（离线与整窗空闲的在线核均为 0.0、不可区分）不计入升频 over、但计入降频 under——空闲即低负载，避免挂机/息屏时永不降频。升频前检查实际频率（scaling\_cur\_freq）是否已达当前设定的 max（schedutil 余量），达到才在频率表中升一档；降频直接把 max 降为当前实际频率对应档位（`read_cur_freq` 后 `partition_point` 找 <= 实际频率的最高档，实际不可读回退降一档，绝不高于当前 max），max 上下限均为硬件上下限。升降频带 wait\_ms 防抖（升降后 `after_change_duration_ms` 内减半）。CLG 与 akmode 同构：min 压硬件最低、只调 max。

- 特调参数独立成文件（仅嵌入 normal/，非处理器绑定）：`module/config/normal/akmode.yaml` 定义单特调段 `akmode`，经 `common::embedded_akmode_str()` 编译进二进制，不放在默认 `config.yaml` 里。原处理器目录 `{soc}/akmode.yaml` 绑定已在 4cb4d97 重构中移除（磁盘已无该文件，勿再加回）。嵌入内容解析失败时 `set_akmode_available(false)` → 特调不可用、白名单应用回退 CLG（不再保留旧值用默认参数接管 CPU）。

- 特调接管失败冷却：`AkmodeGovernor::init_policies` 返回 `bool`（无可用 cluster 即 false）。scheduler\_ipc 在三个特调接管入口（亮屏恢复/ModeChange/ConfigReload）检查返回值，失败时置 `akmode_cooldown_until = now + 300s` 并 warn 打点 `scheduler-akmode-cooldown`，5 分钟内该特调模式不再触发、改由 CLG 接管（冷却中特调模式名保持，息屏 doze/亮屏恢复均走 CLG 分支），冷却结束后经 ConfigReload 或下次 ModeChange 自然恢复重试。

WebUI 侧：

- 双套流动：`bridge.ts::isChiri`（active\_config 为处理器子目录 `config/{soc}/config.yaml` 时真）判定设备类型，`stores/scheduler.ts` 存 `isChiri` 态。

- 特调在 WebUI 为只读标注：`getSpecialTuned` 读白名单，仅 `isChiri && specialTuned[pkg]` 时在应用列表显示「特调：{fallback}」标签（Yumi 设备不显示）；不提供专属特调模式选项、不做特调清理/重扫修复，档位切换与普通应用一致（写 rules.yaml 的 `app_modes`/`global_mode`，特调起始档由 scheduler 侧识别）。应用列表提供「重新扫描」按钮（扫描中禁用防并发）。

- 当前模式读取与自愈：`bridge.ts::getCurrentMode` 读 `current_mode.txt`（空文件回退 `balance`）；守护进程启动时写一次初始模式、模式切换时写入，并常态每 5 秒重写一次（两套 scheduler\_ipc 均实现），防止文件被意外清空/删除后 WebUI 读不到状态。

### CLG 调频语义（动态上限制）

- CLG 不再锁频 min=max，改为 schedutil 动态上限：接管时把 `scaling_min_freq` 一次性压到硬件最低，之后只写 `scaling_max_freq`（CLG 决定性能上限），schedutil 在 \[硬件最低, 上限] 内按瞬时负载自主调频，空闲间隙内核微秒级降频到地板，消除锁频方案"采样间隔（160ms）内频率下不来"的空转发热（8550 实测主要热源）。上限语义下 `perf_floor/perf_ceil/perf_init` 约束的是上限（空闲实际频率由 schedutil 决定），`perf_floor` 允许被热保护击穿。

- 多线程按核心组独立调度：每个 cpufreq policy 由一个独立的 `CoreGroupWorker` 线程管理，持有各自的 `ClusterState`（频率档位、`FastWriter`、`current_perf`、防抖计数器等），线程内自主完成决策 + 写频，核心组之间完全并行无锁。`CpuLoadGovernor`（`cpu_load_governor.rs`）是 Worker 线程管理器：`init_policies` 枚举系统 cpufreq policy、为每个 policy spawn Worker 线程（写 schedutil governor、min 压硬件最低、max 按 perf\_init 设初始上限）；`release` 停止所有 Worker（Worker 退出前自行恢复系统原始状态）；`reload_config` 停旧 Worker + 用新配置 spawn 新 Worker（等价轻量 init，current\_perf 重置到新 perf\_init）。

- scheduler\_ipc 通过 `on_load_update(&core_utils)` 将负载数据广播给所有 Worker（非阻塞 `try_send`，通道满则丢弃本 tick），Worker 线程内自主决策 + 写频，无需外部 flush。

- 升频 = 平滑抬高上限：ceiling 提升不强制频率跳变，无需读 `scaling_cur_freq` 做余量检查（旧 `clg-up-skipped` 机制已随锁频语义删除）。降频 = 一步到位降上限：防抖确认（`down_rate_limit_ticks`，极低负载命中 `down_fast_threshold` 免防抖）后直接写目标档（`ratio_of_freq` 同步 current\_perf），降 ceiling 只收窄 schedutil 区间、不会把实际频率抬上去。已删除废弃参数 `smoothing_down / slow_down_scale / down_fast_mult`。

- 热切换配置重放 perf\_init：`reload_config` 重置 current\_perf 到新 perf\_init 并立即写频，避免息屏 doze/scenemode 期间 current\_perf 掉到 0 后亮屏恢复原模式时频率要从地板缓慢爬升数秒（表现为"亮屏了还卡在 scenemode 低频"）；模式变更 / ConfigReload 热重载同理。

### Thermal 热保护（ChiRi 专属）

- chiri `Config` 顶层 `Thermal` 段（`ThermalGuardConfig`，`from_file` 时 normalize）。电池温度为主参考、CPU 温度仅极端参考：电池温度（`/sys/class/power_supply/battery/temp`，缺失打点 `clg-thermal-no-battery` 后退化为仅 CPU）反映整机持续发热且不随游戏瞬时负载抖动，阈值 41/45°C；CPU 温度阈值 75/85°C——大型游戏中 CPU 由内核 95°C 级温控兜底，软件压制只在极端热时参与。

- 压制带豁免档 `free_above`（默认 0.80，normalize 保证 >= soft\_perf\_cap）：Worker flush 中 `current_perf < free_above` 才 `min(cap)` 钳制（cap 允许击穿 perf\_floor）。持续高负载经平滑抬升越过豁免档后不再回落钳制，任何温度下都能到达硬件最高频，过热兜底交给系统/内核温控；但压制意图保留：中低负载区间仍被压在 cap 以下，减少发热积累。

- scheduler\_ipc 启动时探测一次传感器（CPU `find_cpu_temp_path()` 缺失打点 `clg-thermal-no-sensor` 静默降级），事件循环内每 2s（`THERMAL_CHECK_INTERVAL`）采样双温度，逐传感器三级判定（>= 硬限压 hard\_cap（默认 0.40）；>= 软限压 soft\_cap（默认 0.70）；回落到 软限-hysteresis 才解除；回滞带内压制中先退软限档防阶跃）后取两者较小值。cap 或豁免档变化时经 `cpu_governor.set_thermal_limits(cap, free_above)`（f32 bit pattern 存 `AtomicU32`，启动即同步一次豁免档）下发，debug 打点 `clg-thermal-cap`（含电池/CPU 温度，缺失显示 "-"）。

- 仅作用于 CLG 接管的模式，fast/akmode 不受影响。

### 触摸升频（事件驱动，ChiRi 专属）

- `touch_detect.rs` 线程读 `/dev/input/event*`（`libc::poll` + 解析 64 位 `input_event` 24 字节，`BTN_TOUCH==1` 或 `ABS_MT_TRACKING_ID>=0` 判定触摸按下），触摸按下时经独立事件通道 `mpsc::sync_channel::<()>` 发送事件（非阻塞 `try_send`）。

- scheduler\_ipc 每次醒来先 drain `touch_rx`，收到事件即 `on_touch()` 更新共享 `AtomicTouchState`（f32 性能比 floor 以 bit pattern 存入 `AtomicU32`，窗口时长与写入时刻毫秒数各存一个 `AtomicU32`），并广播空负载包唤醒全部 Worker（经 Worker 负载通道 `try_send(Vec::new())`，recv\_timeout 即时返回、Worker 只 flush 不决策）。大核 Worker 在本次 flush 中读取共享状态、把性能上限抬到 `touch_boost_floor`，不等待下一个 160ms 负载决策 tick（触摸延迟 ≈ `EVENT_POLL_MS=100ms` + 唤醒 flush，不含 Worker tick 间隔）。

- 配置项 `touch_boost_enabled/ms/tiers` 每模式独立，`enabled=false` 即关闭（normalize 会把 ms 置 0）。

- 屏蔽系统触摸升频：`chiri/scheduler.rs::apply_disable_touch_boost` 写 0 到 `/sys/module/cpu_boost/parameters/` 的 `input_boost_enabled / sched_boost_on_input / input_boost_ms / boost_ms`（按存在性尝试，无节点静默跳过）；`start_scheduler_thread` 启动时即调用一次 `apply_system_tweaks()`。

### 息屏省电与屏幕状态

- scenemode（息屏超时省电）：chiri `Config` 的 `scenemode`（Mode 段，默认 `enabled:true`、`perf_ceil` 极低封顶、`up_threshold=1.0` 不主动升频）与顶层 `scene_mode_delay_secs`（默认 300s=5 分钟）。`mod.rs` 息屏时记录 `screen_off_at`，`SystemLoadUpdate` 分支在息屏超时且非特调模式下一次性把 CLG 热切到 scenemode（scenemode 未启用则 release 回系统默认），亮屏自动恢复原模式。**进入 scenemode 时同步应用离线核**（`CoreCtl.scenemode_offline` 门控，见 core\_ctl 章节：小核全开 + 大核/prime 下线 + 调度服务专用小核自钉 + 抑制 boost），CPU 侧待机功耗大幅下降；**little 持续顶满上限则饱和退回 powersave 并 300s 冷却**。

- 息屏 doze 天花板 0.30：`ScreenStateChange(false)` 生成的 doze 配置 `perf_ceil` 钳到 0.30（原 0.40），配合动态上限后后台突发（sync/JobScheduler）借力受限、空闲间隙照常降到地板频，压制"口袋发热"；5 分钟后 scenemode 进一步压到 0.15。

- cpuidle governor 默认启用 menu：8550/8475/8998 的 `config.yaml` 将 `CpuIdleScalingGovernor` 置 true、`CpuIdle.current_governor` 置 "menu"（menu 按预期空闲时长选最深 C 状态，降低空闲功耗），内核无 menu 时写入失败静默跳过、无副作用。

- 屏幕状态自愈校验：uevent 可能漏报/误报（开机早期背光未就绪、长时间息屏后唤醒、netlink 缓冲溢出），导致 `screen_state_arc` 锁死在错误状态——亮屏仍为 false 时 scenemode 计时器被误触发、且无 ScreenStateChange(true) 无法退出。`screen_detect.rs::verify_screen_state` 由 app\_detect 主循环每轮调用，直接读 `/sys/class/backlight` 的 `bl_power==0`（回退 `actual_brightness>0`，与 uevent 分支同口径）校正 arc，无背光节点则静默跳过。

### 极速模式（fast）

- fast 模式用专属锁频器、不读 yaml、停用 CLG：`src/chiri/fast.rs` 的 `FastLock` 与 CLG 完全独立，fast 模式下由 `mod.rs` 的 scheduler\_ipc 先 `cpu_governor.release()` 再 `fast_lock.init()` 接管。

- `FastLock::init()` 遍历 `get_cpu_policies()`、快照原始状态、写 schedutil governor、把所有 cluster 的 `scaling_min_freq/scaling_max_freq` 都锁到含 boost 的硬件最高频（min=max=hw\_max）；`tick()` 每 5 秒重写一次 hw\_max 防止系统/厂商守护进程篡改；`release()` 恢复接管前的 governor/min/max。

- `mod.rs` 事件循环中 `fast_lock.tick()` 在每次 `recv_timeout` 唤醒时调用；模式切换/息屏 doze/亮屏恢复/看门狗超时/panic 收尾均正确 release fast\_lock。

- 8550/8475/8998 的 `config.yaml` 已移除 `fast:` 段（`Config.fast` 字段保留，`get_mode("fast")` 仍返回 `Some`，仅用于模式校验），akmode 的 fast 档（动态限频）不受影响。

### CPU 亲和与线程迁移（Affinity，ChiRi 专属）

- `src/chiri/affinity.rs` 的 `AffinityManager`（消费 `SysPathExist` 已探测但此前无人使用的 cpuset/cpuctl 能力位）。

- boost 模式（performance/fast/特调，判定口径统一在 `chiri/mod.rs::is_boost_mode`）下：`/dev/cpuset/top-app`、`foreground` 的 `cpus` 收窄到大核+超大核（区间用 `common::chiri_core_ranges()`，不硬编码）；`background/system-background/restricted` 压到小核。

- 可选写 `/dev/cpuctl/top-app/cpu.uclamp.min`（配置 `Affinity.top_app_uclamp_min_pct`，默认 0 关闭——uclamp.min 会让 schedutil 独立于 CLG 抬频，避免与动态上限语义打架）。

- 可选写 `/dev/cpuctl/top-app/cpu.uclamp.max`（配置 `Affinity.top_app_uclamp_max_pct`，任务级性能上限钳制、EAS 原生感知，比 scaling\_max\_freq 硬顶更细）。按机型配置：8550/8475 配 85，8998 内核 4.4 配 0 关闭。运行时内核版本识别兜底纠正：`kernel_version()` 解析 `/proc/sys/kernel/osrelease`，< 5.3（uclamp 主线引入版本）或节点缺失或写入回读无效（防厂商半成品 backport 静默忽略）任一不满足即置 Unsupported 永久跳过并 warn 打点 `affinity-uclamp-unavailable`，判定结果缓存避免重复探测。

- 线程层（按核心粒度放置，`pin_foreground_threads=true` 启用，每 2s 再平衡一轮、前台 PID/boost 变化立即触发）——**开销控制优先，不做逐线程每轮读 stat**：

  - 前台（fg\_pid 由 app\_detect 提供）：每轮 1 次 `read_dir /proc/<pid>/task`，仅对**新增**线程读一次 stat 判定关键/建档；存量线程的 home 合法性仅用缓存的逐核 util 与在线位图判断。关键线程（tid==pid 或 comm 命中 RenderThread/GLThread/GameThread/UnityMain/UnityGfxDeviceW）→ prime（无 prime 的 SoC 取 big），普通 → big；boost + 亮屏才钉，否则恢复全核；已消失线程下一轮即清理释放钉核计数。

  - 后台动态亲和（**不把后台全压小核**——小核过载能效灾难）：忙线程 promote 到 big、回落 demote。候选 TID 从三个后台 cpuset 的 `tasks` 文件读取（**不做 /proc 全量枚举**），每 2 轮刷新、按游标分片每轮只深扫 64 个；窗口 util = ticks 差分，**两窗防抖**（上次采样忙且本次仍忙、期间采到低负载即清标记）即 promote，不依赖采样间隔。promote 先把 TID 移入 top-app（cpuset v1 按 TID 记账）再按当前核心占用选核钉定，orig 组缓存供 demote/清理迁回。已 promote 线程每 2 轮复查：util 连续 3 次 < 5% → demote；线程仍忙（≥5%）且所在核心 util > 70%（`CORE_OVERLOAD_UTIL`）→ 换低占用 big 核（`bg_overload`，带 4s 迁移防抖）。过载重钉统一门控：**仅当目标核本身 util ≤ 70% 才迁**，选回同核/所有候选核都过载时静止（整体高载交给 CLG 上限与温控处理），杜绝边际收益微小的无谓搬动与两核间乒乓。**后台迁移仅亮屏**。

  - 在线核位图每 4 轮读一次并缓存（核热插拔不频繁）；devimp core 行每 2 轮一次。

  - **多应用快速切换**：每轮再平衡开头按 pid 归属立即清理旧前台线程（`pid>0 且 ≠ 当前 fg_pid` → 解钉恢复全核），不等 30s 失联——否则旧应用转后台后（Android 会短暂把它留在 top-app/foreground cpuset）其单核掩码与大核相交继续生效，8550 仅一颗 prime 会让新旧前台关键线程同核互踩，连续切换还会令 core\_pinned 计数漂移累积。过滤器幂等、零文件 IO；后台 promote 线程（pid==0）不受影响。模式变化的切换经 ModeChange → force 立即重平衡；同模式切换（无事件）依赖 2s 周期块发现，钉核延迟 ≤2s（可接受，切换清理在同一轮完成，新前台钉核时 core\_pinned 已准确）。

  - 选核算法：score = 逐核 util（最近 SystemLoadUpdate 快照，即核心当前占用）+ 钉核计数 × 0.2，取核池 ∩ 在线核最低分核（离线核 util 恒 0.0、与空闲不可区分，钉到「唯一允许核为离线」会让线程有效可运行集为空而冻结，必须排除）；home 核过载（核 util > 70%，`CORE_OVERLOAD_UTIL`，前后台共用）或 home 离线在 4s 防抖窗口外重钉，选回同核不写避免空迁移。

  - 掩码用 `mem::zeroed::<cpu_set_t>()` 分配保证对齐（`vec![0u8]` 强转是未对齐指针的脆弱实践），`libc::CPU_SET` 置位并带容量边界防护。

  - 稳态（前台线程集不变、后台空闲）单轮 ≈ 1 read\_dir + 0\~64 stat + 低频辅助文件读——相对「逐线程每轮全读 stat」约省 >50% 文件 I/O。

- 黑名单（全部编译嵌入、不外透）：`src/chiri/affinity_blacklist.txt` 经 include\_str! 打包（common.rs 的 `parse_affinity_blacklist` 解析、OnceLock 缓存），格式：每行一条精确进程名/包名或 `re:` 前缀正则（同特调白名单语法，含 com.example 注释示例），文件内含**系统进程默认名单**（system\_server/surfaceflinger/logd/lmkd/zygote 等，勿删）；内置兜底：空 cmdline（内核线程）与 `/` 开头（native 二进制路径）一律黑名单。命中进程全部线程保持全核、不做任何迁移，`is_affinity_blacklisted` 另在逐线程层面兜底（防应用进程内的系统服务线程被误迁）。

- normal/doze 布局：top-app/foreground/uclamp 恢复快照，后台保持压小核；boost 退出/开关关闭/调度线程收尾 `release()` 全量还原（已钉线程恢复全核，经 `demote_tid_group` 写回进程当前 cpuset 组）。同模式 App 切换不发 ModeChange 事件，靠 scheduler\_ipc 2s 周期刷新兜底重迁移（manager 内部按 KIND/PID/boost 去重）。

- 配置段 `Affinity`（enabled/top\_app\_uclamp\_min\_pct/top\_app\_uclamp\_max\_pct/pin\_foreground\_threads），三份 SoC yaml 已带。

### core\_ctl 核心在线接管（ChiRi 专属）

- `src/chiri/core_ctl.rs` 的 `CoreCtlManager`，**三态状态机**（None/Boost/Scenemode，`set_power_state(boost, scenemode)` 统一入口，内部去重可被 2s 周期安全调用；切换时先退出旧状态恢复快照再进入新状态）。调度线程收尾 `release()` 按当前状态恢复。

- **Boost**：把各 cluster 的 core\_ctl `min_cpus` 抬到全组常在线（防厂商热插拔与 ChiRi 调频打架），退出恢复快照；只动 min\_cpus 不动 max\_cpus/busy 阈值。

- **Scenemode 离线核（息屏深度省电）**：`CoreCtl.scenemode_offline` 门控（8550/8475 true，8998 内核 4.4 默认 false）。进入 scenemode 时先解除 boost（min\_cpus 抬着会让厂商 core\_ctl 重新拉起被下线的核——两者互斥由 `apply_affinity_and_corectl` 保证），下线目标由 `scenemode_targets()` 计算：**小核全开**（保证 ≥3 个常驻待命核响应电源键等 PMIC 事件；不足 3 个从大核按编号从小到大补足），**大核簇 + prime 整簇下线**消除空转漏电流；逐核写 online=0 回读验证，失败跳过（warn）。**守护进程自身全部线程钉到 1 个专用小核**（编号最大的 little——CPU0 承担最多中断家务），后台任务堵塞不了调度服务；退出 scenemode 解除自钉。维持期每 2s 纠偏（重新下线被外部拉起的核）；退出按快照恢复 online（带回读+重试）。scenemode 下无其它 ChiRi 钉核线程，与离线无冲突。

- **scenemode 饱和退出**：little 簇 max\_util 持续 10s ≥ 70%（`SCENEMODE_SAT_UTIL/SECS`，util 是忙时占比与频率无关，饱和即真饱和）→ 视为后台负载压不死小核：一次性退回 powersave 的 CLG 配置 + 立即恢复全部在线核 + 解除自钉，并进入 **300s 冷却**（`SCENEMODE_COOLDOWN`，期间 scenemode 入口被门控不得重进，防反复拉锯）；冷却结束后息屏条件仍满足则自然重进。

- 为什么选核排除离线核而不"按需唤醒"：唤醒大核要拉电压轨/重建 L2，为后台线程点亮大核净亏能；直接写 online 会与厂商热插拔守护进程打架（对方再下线，ping-pong）。需要更多在线核时的正确姿势是抬 core\_ctl min\_cpus（Boost 态）。scenemode 是唯一反向使用 online 写入的场景（目标恰恰是让大核睡死，同时小核全开保住待命响应）。

- cluster 发现：遍历 `get_cpu_policies()` → related\_cpus 首个 CPU 的 `/sys/devices/system/cpu/cpuN/core_ctl`（每 policy 一份，天然去重），惰性枚举一次；无节点打点 `corectl-unavailable` 后保持空表（scenemode 直接 sysfs 离线**不依赖** core\_ctl 节点，仅受独立配置门控）。

- 配置段 `CoreCtl`（enabled/scenemode\_offline）。

### 遥测数据源（Telemetry，ChiRi 专属）

- `src/monitor/telemetry.rs` 的 `telemetry_loop` 线程（1s 轮询，仅 `is_chiri_soc()` 时由 monitor/mod.rs 启动）读 PSI（`/proc/pressure/{cpu,io,memory}` 的 some avg10）、GPU busy%（`/sys/class/kgsl/kgsl-3d0/gpu_busy_percentage` → MTK `/sys/kernel/ged/hal/gpu_loading`，缺失为 None）、电池电流/电压。

- 电池功耗读取 OPlus 私有节点优先：`/sys/class/oplus_chg/battery/bcc_parms` 存在时（启动探测一次，info 打点 `telemetry-oplus-bcc`）走 BCC 实时数据（逗号分隔，下标 6 = 电芯1电压、8 = 电流、11 = 电芯2电压非零时取双节之和），量级启发式归一 µV/µA 并做物理合理范围校验（2–6V / ±30A），解析失败回退标准节点。OPlus 内核标准 power\_supply 节点（current\_now/voltage\_now）约每 10s 才刷新，1s 精度采样必须绕开。

- 结果写入进程级共享原子量 `telemetry()`（f32 bit pattern 存 `AtomicU32`，与热保护/触摸状态同口径），不占事件通道容量。

- eBPF 扩展探针（`yumi-ebpf/src/main.rs` 的 `handle_sched_wakeup`/`handle_sched_migrate_task`/`handle_cpufreq_transition`，PerCpuArray 计数）由 `cpu_monitor.rs` 仅在 ChiRi 上可选挂载（内核缺 tracepoint 时 warn 一次跳过，不影响主探针），每 2s 读累计值取增量发 `DaemonEvent::BpfStats`（Yumi 设备不发送；Yumi scheduler match 里的空 arm 仅为枚举完备性）。

- chiri scheduler\_ipc 以 `TELEMETRY_LOG_INTERVAL=1s` 消费：写 `logs/status.csv`（logger.rs `status_log_snapshot`，1s 一行，功耗精度 1s）+ 20s 一条 debug 摘要 `telemetry-summary`。BpfStats 不刷新 CLG 看门狗心跳（探针失效不影响负载源判定）。

## 硬性约束

- 不修改 CI 构建流程（`.github/workflows/build.yml`），除非明确要求。

- eBPF 程序目标为 `bpfel-unknown-none`，改动需确保 CI 环境交叉编译通过。

- 保持 KernelSU/Magisk 模块规范兼容（`module/` 目录结构、`service.sh` 启动流程）。

- Yumi 逻辑冻结：不修改 `src/scheduler/` 与默认 `module/config/config.yaml` 的调度逻辑与参数行为（Yumi 即将废弃）；确需修复时先与 ChiRi 对齐、最小改动。共享层（`src/monitor/`、`main.rs`）如需为 ChiRi 适配，必须保持 Yumi 运行时行为不变（常规采样 200ms 原值勿改）。

- 不允许在未经许可的情况下主动chuang'jian

## 经验教训

- 含 `std::ops::Range<usize>` 字段的结构体别 `derive(Copy)`：CI（`-Z build-std` + nightly）编译 `common::CoreGroupRanges` 时报 E0204（字段不实现 Copy），即便标准库中 `Range<usize>` 实现了 Copy。按值 move 或显式 `clone()` 即可，用 `#[derive(Debug, Clone)]` 够了，不要加 Copy。

- 日志文件被删不能崩：`src/logger.rs` 用自实现 `SelfHealingAppender`（替换 log4rs `RollingFileAppender`），每次写入按路径 `create+append` 重新打开，`daemon.log` 被外部删除会自动重建；循环轮转与锁上锁全程 `Result`/剥除 poison，绝不 `unwrap`。别改回 log4rs 滚动追加器。

- 看门狗崩溃自愈 + 进程模型：`service.sh`（开机）/`action.sh`（手动启动）用 `nohup sh -c` 拉起统一定义的看门狗循环，启动时把自己 PID 写入 `logs/watchdog.pid`。循环内 `"$DAEMON"` 崩溃返回非 0 仍会 `sleep 3` 自动重启；仅当存在卸载标记 `$MODDIR/.uninstalling`（`uninstall.sh` 开头 touch）或主进程二进制被删时才 `break` 退出。旧写法 `"$1" || exit 0` 在 yumi 崩溃（返回非 0）时会直接让看门狗退出、无法自愈，禁止复用。

- WebUI「关闭调度」而非重启：`bridge.ts::stopScheduler` 先按 `logs/watchdog.pid` kill 掉看门狗（防止其把主进程再拉起），再 `killall -9 yumi`，实现彻底停止调度；恢复需点击模块 Action（`action.sh` 手动启动）或重启设备。不要在 ksu.exec 里用 `nohup ... &` 后台拉起——`ksu.exec` 返回会清理执行 shell 的进程组，直接拉起会被一并杀掉；要启动调度一律调用模块自身的 service.sh/action.sh（内含 `nohup`，且 disable\_boost 幂等）。

- 模块热更新必须 `exit 1` 收尾：customize.sh 的热更新分支把新文件直接 cp 进已安装模块目录 `/data/adb/modules/chiri` 并重启服务后，必须轮询确认 daemon（`pidof`/`pgrep yumi`，最多 \~6s）存活，然后无条件 `exit 1` 按报错退出。若 `return 0` 让安装器继续"完整安装"，会再覆盖一遍模块目录并写 update 标记，管理器随即识别为"模块更新"提示重启、隐藏 WebUI/Action；`exit 1` 使安装器按失败中止（清理暂存目录、不碰已热替换的目录），管理器不感知更新。管理器显示"安装失败"是预期行为，脚本内已双语提示；服务未启动时同样 exit 1 并提示重启设备走完整安装。

## AGENTS.md 维护要求

每次对话结束前，回顾本次会话内容，评估是否需要更新本文件：

- 新增/删除/重构了模块、目录或关键文件 → 更新「目录结构」

- 引入了新的依赖、工具链或构建命令 → 更新「技术栈」「常用命令」

- 确立了新的代码约定或踩坑经验 → 更新「代码约定」（可新增「经验教训」）

- 文件描述与实际代码不符 → 立即修正

没有需要沉淀的变化时跳过，但必须经过评估。
