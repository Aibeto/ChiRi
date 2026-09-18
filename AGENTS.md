# AGENTS.md

> 上次更新时间：2026-09-15
> 最后更新位于此 head 之后：fd9a71ab6c6b4636cb073bcffa96cb468c4993fd

本文档为 AI 编程助手（Cursor / Claude Code / Trae 等）在本仓库工作时的指导文件。

> ** workBuddy 和 CodeBuddy 协作文件已统一到 `.codebuddy/`（2026-09-13）**
>
> - 本文件 `AGENTS.md`：项目**指导**文档，按区块维护（Grep `\[tag\]` 定位，勿通读全文）。
> - `.codebuddy/memory/`：跨会话记忆，注意它是一个**目录**而不是单一文件——
>   `MEMORY.md` 放长期事实（就地更新、保持精简），`YYYY-MM-DD.md` 是按日期追加的工作日志（只追加不重写）。
> - `.codebuddy/docs/`：计划、评估等 AI 产出文档（`mdocs/` 只放项目原有文档，勿把 AI 产出放进去）。
> - 旧的 `.workbuddy/` 已废弃合并，其下只剩重定向说明文件，不再写入任何内容。
>   凡属架构约定、契约数字、命令口径的变更，AGENTS.md 与 `.codebuddy/memory/` 都要同步；
>   具体某次改动的经过只写当日日志。
>   **开始工作前可以考虑互相阅读**：读本文件的同时，考虑读 `.codebuddy/memory/MEMORY.md` 与最近几天的 `YYYY-MM-DD.md`。

> AGENTS.md: [overview] [tree] [stack] [cmds] [convention] [chiri] [hard] [lessons] [maint]（Grep `\[tag\]` 定位区块，勿通读全文）

## [overview] 项目概述

**ChiRi** 是 Android CPU 智能调度控制系统（Magisk/KernelSU 模块），核心是 Rust 守护进程（二进制 `chiri`）：eBPF 内核探针采集 CPU 调度事件与渲染帧数据，结合 FAS 帧感知调度和 CLG 负载调速器动态调频。

本仓库为 ChiRi（自 imacte/yumi fork，README "Based on imacte/yumi"），调度代码在 `src/chiri/`。`src/scheduler/`（Yumi 调度）即将废弃，仅作为 ChiRi 的基础保留，勿改动其逻辑。新功能、调优只落 `src/chiri/` 和处理器配置 `module/config/{soc}/`。

- 目标平台：Android 8.0+ / AArch64 / 需要 Root

- 许可证：GPL-3.0-or-later

- 版本：见 `module/module.prop` 与 `Cargo.toml`（需保持同步）

## [tree] 目录结构

```
src/                  # Rust 守护进程主代码
  monitor/            # 监控层：app_detect / fps_monitor / cpu_monitor / screen_detect / telemetry（两套调度共享）
  scheduler/          # 调度层 Yumi（即将废弃，作为 ChiRi 基础保留、勿动逻辑）：FAS 引擎、CLG 负载调速器
    fas/              # FAS 核心：PID 控制器、帧率档位、frame_pipeline
  chiri/              # ChiRi 调度（发展主线；特定 SoC 触发；含 CLG、akmode 明日方舟特调、fas_manager FAS 帧感知调度、touch_detect 触摸升频、affinity 按核亲和/线程迁移、core_ctl 核心在线接管）
  chiri/affinity_blacklist.yaml  # 线程亲和黑名单（编译期嵌入：系统关键进程默认名单 + re: 正则；含 com.example 示例；用户/WebUI 不可改）
  rhine.rs            # 实验室（ChiRi 专属）：rhine-init.yaml 解析、rhine.chr / rhine-back.chr 读写、启用与还原、运行时监听
  common.rs / fas_types.rs / i18n.rs / logger.rs
yumi-ebpf/            # eBPF 探针（bpfel-unknown-none，build-std 编译；独立 workspace，不在根 members；sched_switch + queueBuffer + 遥测计数探针）
xtask/                # 构建脚本（cargo xtask build 完成编译打包 zip）
module/               # Magisk/KernelSU 模块载体（module.prop、customize.sh、service.sh）
  config/             # config.yaml / rules.yaml / i18n (en.ftl / zh.ftl)；<soc>/config.yaml 处理器子目录（各 SoC 自带，8475/8998 参数相同、各自一份）+ normal/tuned_profiles.yaml、normal/scenemode.yaml、normal/fas.yaml（FAS 白名单）、normal/fas/<配置名>.yaml 每应用 FAS 调优（编译期嵌入）；rhine-init.yaml 实验室模式定义（同样只进二进制）
  rhine.chr           # 实验室状态（对外暴露、可手改；空/只有注释 = 未启用）。rhine-back.chr 由 daemon 生成，不随包
webui/                # Svelte 5(runes) + TypeScript + Vite + ak-ui 管理界面；分层 kernel/shell → contract → data → views，tests/ 为纯逻辑断言（详见 webui/README.md）
updateInformation/    # 更新.json 与 changelog
.github/workflows/    # CI：Node 24 + Rust nightly + NDK r29 + cargo-ndk
```

## [stack] 技术栈

| 层       | 技术                                                                              |
| -------- | --------------------------------------------------------------------------------- |
| 守护进程 | Rust (edition 2024, nightly), tokio, aya (eBPF), serde_yaml, inotify, netlink     |
| eBPF     | aya 框架，`sched_switch` tracepoint + `queueBuffer` uprobe                        |
| WebUI    | Svelte 5 (runes), TypeScript, Vite, ak-ui 1.0.0 (CSS Core), js-yaml, vitest       |
| 构建     | cargo xtask build（Rust aarch64-linux-android 交叉编译 + webui npm build + 打包） |

## [cmds] 常用命令

```bash
# 完整构建（编译 eBPF + 守护进程 + WebUI 并打包模块 zip）
cargo xtask build

# 仅组装模块目录、不打包 zip（CI 用；目录名按 module.prop 动态命名，GitHub 下载时自动压缩）
cargo xtask build --no-pack

# 本地静态检查（验证 chiri crate 本身；需 nightly + aarch64-linux-android target + bpf-linker）
cargo +nightly check -p chiri --target aarch64-linux-android

# WebUI 开发（无 ksu 时自动装载 src/dev/mock-shell 设备替身）
cd webui && npm install && npm run dev

# WebUI 类型检查（svelte-check，已替换旧的 vue-tsc）
cd webui && npm run type-check

# WebUI 纯逻辑断言（契约层与数据层，可脱离真机运行）
cd webui && npm test
```

- 本地 check 需要 nightly + aarch64-linux-android target + bpf-linker：eBPF 编译用 `-Z build-std`（仅 nightly 支持），build.rs 会构建 yumi-ebpf。yumi 是 Android/Linux 专属 crate，别用 Windows host 目标检查（netlink-sys/aya 无法在 Windows 编译）。Windows 下 bpf-linker 为 `bpf-linker.exe`（build.rs 已按 `cfg!(windows)` 兼容）。

- yumi-ebpf 是 no_std/no_main 探针、独立 workspace（根 `Cargo.toml` 的 members 只有 xtask，勿把 yumi-ebpf 加回）：只可用 bpfel 目标检查（`-Z build-std=core`），禁止在带 std 的目标（aarch64-linux-android、Windows host）下编译/检查。探针无法用 std 编译（`unwinding panics are not supported without std`），test 剖面还会与 `#[panic_handler]` 冲突（`duplicate lang item panic_impl`）。根 build.rs 用 `current_dir=yumi-ebpf` 单独构建，IDE（rust-analyzer）在根 workspace 下不检查它。

- 本地开发只做 `cargo check` / WebUI `type-check`；完整产物由 CI（GitHub Actions）生成。不要随意 `cargo build`（需要 NDK 环境），优先静态检查。

- bpf-linker 获取：`build.rs` 的 `ensure_bpf_linker` 依次尝试 PATH 中已有 bpf-linker → OUT_DIR 缓存 → `cargo install bpf-linker`。CI 通过 GitHub API 下载静态链接 LLVM 的预编译二进制（bpf-linker 0.11 依赖 LLVM 21+，源码编译在 ubuntu runner 上不可行；cargo-binstall 也会回退到源码编译）。eBPF release 编译在 build.rs 内用 `CARGO_PROFILE_RELEASE_OPT_LEVEL=2` 局部覆盖（新版 bpf-linker 已移除 `-Oz`/`-Os`，仅支持 `-O0~O3`，workspace 根的 `opt-level="z"` 会导致链接失败）。Windows 兜底：bpf-linker 源码编译依赖 `os::unix` API，Windows 上无法构建，且本地不承担完整产物构建。`ensure_bpf_linker` 在 Windows 无现成 bpf-linker 时返回跳过错误，`build_ebpf` 捕获后经 `write_ebpf_stub()` 回退占位产物（`ebpf_target/bpfel-unknown-none/{debug,release}/yumi-ebpf`，与 `YUMI_SKIP_EBPF=1` 同路径），保证 rust-analyzer 和本地 `cargo check` 不被阻塞。Windows 上若有 `bpf-linker.exe` 仍正常构建 eBPF，CI 行为不变。

## [convention] 代码约定

### 架构与事件流

- Monitor 线程组通过有界 mpsc 事件通道（`DaemonEvent`，`sync_channel` 容量 64，满时 send 阻塞形成背压）解耦数据采集与调度决策，新增监控/调度能力遵循此模式。

- 前台 PID 由 `monitor/mod.rs` 的 `pid_watcher` 线程经 `tokio::sync::watch` 广播，FPS/CPU 监控共享消费，不要各自轮询。

- 调度器双套架构：`scheduler/`（Yumi）与 `chiri/` 二选一，main.rs 按 `common::is_chiri_soc()`（SoC 列表命中）决定启动哪一套。两套互斥消费同一事件通道，Monitor 层共享。ChiRi 是主线，Yumi 即将废弃且逻辑冻结，新功能/调优只落 `src/chiri/`。

- `CHIRI_SOC_HINTS` 在 common.rs 维护，新增机型只追加列表，不要绑定单一型号；同时提供 `config/{片段}/config.yaml` 和核心组区间 `common::chiri_core_ranges()`。匹配只看硬件标识片段，不检查磁盘目录（配置编译进二进制，硬件命中即生效）。

- FAS 已恢复为 ChiRi 专属主线功能（引擎在 src/scheduler/fas/，算法未改；ChiRi 侧 src/chiri/fas_manager.rs 提供解耦多实例生命周期管理，见 ChiRi 调度子系统 FAS 小节）。fps_monitor 仅 `is_chiri_soc() && fas_available()` 时恢复启动，复用共享 pid_watcher 广播——`start_fps_loop` 接收 watch::Receiver，内部经 mpsc 桥接喂给 fps_probe 线程；`DaemonEvent` 的 `FrameUpdate`/`pid`/`foreground_max_util` 已恢复消费。**反偷跑门控**：`fas_active` 共享 `Arc<AtomicBool>`（main.rs 创建，`FasManager::activate` 置位 / `deactivate_active` 清零）贯穿 monitor 与 chiri 两层——fps_monitor_ebpf 线程在标志置位前 1s 周期空转等待（不建 tokio runtime、不加载 eBPF），fps_probe 线程启动后**不挂任何 uprobe**（FpsManager 仅加载 eBPF 无 attach 点即零执行）；置位瞬间从 watch 直接借当前前台 PID 补挂（桥接任务只转发变化值，FAS 应用激活前已在前台时无变化事件可借）；去激活后 fps_probe 主循环下一轮 `switch_pid(0)` 纯 detach（FpsManager 摘探针、清 states、复位 current_pid），回到零开销待机。勿改回「daemon 启动即 attach」——非 FAS 会话（桌面/普通应用）每帧 queueBuffer 都会白付一次探针开销。`FAS_FG_UTIL_ENABLED` 由 const bool 改为运行时 AtomicBool（cpu_monitor `start_cpu_loop` 入口按 `is_chiri_soc() && fas_available()` 置位，Yumi 设备保持 false、行为零变化）；cpu_monitor 的 `get_thread_tids/compute_tgid_util/compute_thread_level_util` 已恢复使用（allow 标注移除）。保留 `#[allow(dead_code)]` 的仅剩两处引擎热重载预留方法（fas/pid.rs `update_coefficients`、fas/policy_mgmt.rs `reload_rules`，注明「per-app 配置编译期嵌入、ConfigReload 不再重载 FAS」）。main.rs 在 chiri_active 门控内导出两个运行时文件：special_tuned.yaml 与 fas_whitelist.yaml（后者每行 `包名:配置名`，供 WebUI 只读）。Yumi 仅恢复 `pub mod fas;` 模块声明供 crate 路径引用，其余 FAS 接线保持注释冻结，Yumi 运行时行为零变化。

- 负载采样间隔：`cpu_monitor` 的 `SystemLoadUpdate` 按 SoC 参数化，main.rs 按 `is_chiri_soc()` 传入 `start_monitor`→`start_cpu_loop`（ChiRi 160ms / Yumi 200ms，Yumi 200ms 为原值勿改）；特调激活时经共享 `Arc<AtomicBool>`（main.rs 创建、`TunedGovernor` 接管/释放时置位）切换到 40ms。特调消费该负载流做动态限频（无档位，max 随负载在内核频率表中逐档升降，范围均为硬件上下限）；CLG 消费同一事件流，tick 语义按当前采样间隔（各 rate_limit_ticks/smoothing 按各自 tick 调优）。

- **事件循环低开销原则（性能响应零延迟约束下）**：所有性能敏感路径（负载事件/模式切换/触摸升频）都是**推送事件**——`recv_timeout` 被事件到达立即打断，轮询周期只影响非性能的定时任务。据此：① chiri `scheduler_ipc` 用**动态超时**阻塞到「最近一个周期任务 deadline」（telemetry 1s / thermal+亲和 2s / mode file 5s / fast_lock 重写 5s 各自取 min，上限 `EVENT_POLL_MS=1s`），空闲从每秒 10 次空转降到 \~1 次，勿改回固定 100ms 轮询；② CLG Worker 的 `run()` 超时分支只做触摸窗口清理 + 防篡改重写（1s 粒度），负载/触摸决策全走推送事件即时触发，勿把超时改回 160ms；③ 新增周期任务时**必须把它的 deadline 纳入** `recv_timeout` 的 wait 计算（取 min），否则任务会被拖到最长 1s 粒度；④ 高频 `debug!` 中的 `format!/collect/join` 构造发生在宏求值前，INFO 级别下每 tick 白白分配——用 `log::log_enabled!(log::Level::Debug)` 门控（scheduler-event-load / clg-tick-log / tuned-tick-log / cpu-monitor-tick-log 已做；FrameUpdate 帧事件 debug 打点同口径门控——FAS 恢复后逐帧 format 真实发生，勿裸打点）。

### 日志

- 调试与排障优先用 `debug!`，别全用 info 冲掉有价值的信息；高频路径（频率控制/帧处理）用 25-tick / 60-frame 周期摘要，状态变化（模式、PID、屏幕、attach、档位）即时打点。

- 日志文件只有两个：`logs/daemon.log`（log4rs，范围不变）+ `logs/status.csv`（CSV 宽表 + `type` 列：仅 `snap` 一种行类型，1s 一条，整合原 power/telemetry 全部字段 + **前台包名**（`package` 列，实时取自 `app_detect::get_current_package()`，切换点由相邻行变化体现）+ **充放电状态**（`charge` 列，1s 读一次 `/sys/class/power_supply/battery/status`，归一为 charging/discharging/full/not_charging，缺失 "-"——电流符号因厂商节点方向不一不可靠）+ **FAS 实测帧率**（`fps` 列，末列；**预留列**——schema 恒定占位，仅 FAS 激活且帧窗口有样本时为实测值，其余一律 "-"，见 logger.rs `STATUS_HEADER` 注释。取值 `FasManager::current_fps()` → `FasController::current_fps()` 的 `fps_window` 窗口均值，与引擎齿轮决策的 avg_fps 同口径；`status_log_snapshot` 收 `Option<f32>` 走 `fmt_num`）。已无 `fg` 事件行：所有信息都在每秒汇总行内，稳定 1s 一行，不再在包切换时额外写行）。新增列一律**追加在末尾**（WebUI `data/status-csv.ts` 的 `STATUS_COLUMNS` 按列数过滤残缺行，插入中间会打乱既有索引、须同步列常量/类型/解析与 mock 与单测）。勿再依赖 ModeChange 事件记录前台切换——模式不变时无该事件，这是原 foreground.log 失效的根因。

- devimp 锁序约定（防死锁）：`DEVIMP_MODE` / `DEVIMP_FG_PKG` / `DEVIMP_TICK_STATE` / `DEVIMP_WRITER` 四把锁**一律不嵌套持有**——WRITER 是写路径汇合点（scheduler_ipc/CLG Worker/亲和线程都会写），其临界区内只做文件 IO 与自身状态修改，绝不获取其他锁；曾在 WRITER 临界区内清 TICK_STATE（包名切换/触顶）已全部移出（`set_devimp_package` 用 `pkg_switched` 标志、`devimp_check` 返回 rotated 在锁外清）。新增写入路径必须遵守，否则反向获取即成环。

- status 写入用常驻 append 句柄（`STATUS_WRITER`，每行仅 1 次 write syscall）+ 每 16 行巡检（8MB 轮转 1 备份、被删自愈；常驻句柄在文件被删后指向孤儿 inode 写入不报错，只能靠巡检发现，16 行 ≈16s 自愈窗口，勿调回 256——会长时间"无文件可读"）。禁止恢复每行 open/stat 的 `append_aux_log` 模式，勿再拆分 power/telemetry/foreground 独立文件。

- `logs/watchdog.pid` 运行时自愈：`logger::ensure_watchdog_pid_file()` 由两套 scheduler_ipc 的 5s 周期块（mode file 重写处）调用——logs/ 被外部删除后 pid 文件随目录消失，stopScheduler 将无法终止看门狗。看门狗 sh 是 daemon 的父进程（`"$DAEMON"` 前台拉起），`getppid()` 即其 PID；仅当文件缺失、ppid>1 且父进程 comm 为 sh/mksh 时重建（防调试直跑/孤儿态误写）。

- 启动日志归档：main 在 `create_dir_all(logs)`/`logger::init` 之前调 `logger::archive_on_startup`——**先做短会话判定**（见本条末尾），再**扫描并回收模块根下遗留的 `ziped_*` staging 目录**（见本条末尾「遗留回收」），再把上一轮整个 `logs/` 与 `devimp/` 分别原子 rename 为同级 `ziped_<MMDD-HHmmss>` / `ziped_devimp_<MMDD-HHmmss>` 临时目录（目录名经 `unique_staging` 加 `-N` 去重，**同目录 rename 目标必须不存在**，否则 ENOTEMPTY 会让本次整个不归档），交同一个一次性子线程 `log_archiver` 串行打包（遗留优先，本轮次之）为 `logd/<去 ziped_ 前缀名>.tar`（归档命名一律本地时间 MMDD-HHmmss 人眼可辨）；**打包本身由外部脚本 `module/scripts/pack.sh` 完成**（对外暴露的稳定接口、构建流程不得修改——CI 注释剥离已排除 `module/scripts/`；脚本优先用模块自带 `core/bin/tar`（外部引入的二进制）、回退系统 tar；2026-09-17 起不再由 Rust 手写 ZIP，用户要求「引入外部二进制与 shell 处理所有打包事项」）后删除临时目录并自然退出；打包失败保留对应临时目录并写 ARCHIVE_FAILED.txt（此时 logger 未 init 无法打点）。必须复制回 `logs/watchdog.pid`——看门狗先于 daemon 启动、WebUI stopScheduler 靠它终止看门狗，归档带走它会导致「关闭调度」失效。打包完成后子线程执行 **目录预算清理**：`logd/` 与 `devimp/` **各自独立计量**（`enforce_dir_limits` → `enforce_dir_limit`；`LOGD_MAX_BYTES` / `DEVIMP_DIR_MAX_BYTES` = 128MB，删本目录最旧文件到 <96MB，且**目录内最新的一个文件永不删除**）；devimp 触顶换文件时也触发一次同款清理（在 WRITER 锁外调用），防长会话膨胀。**短会话丢弃（2026-09-16 新增）**：`logs/daemon.log` **首行**时间戳 ≈ 上一轮 daemon 启动时刻，距本次启动不足 `SHORT_SESSION_SECS`（30s）即判为短会话——**直接清空 `logs/` 与 `devimp/`、不打包**（崩溃循环每轮只产几行日志，逐轮打包会把 logd/ 塞满空壳归档并淹没真正的现场）。三条硬约束：① **时区**：daemon.log 的 `[YYYY-MM-DD HH:MM:SS]` 是**设备本地时间**（log4rs 1.4 的 `{d(...)}` 走 chrono、默认本地，只有显式传第二个参数 `utc` 才是 UTC），故必须用 `libc::mktime` 解读而非 `timegm`，否则偏一个时区（东八区 −8h）使 30s 判定彻底失效；`tm_isdst = -1` 交给 libc 判 DST。② 只读前 256 字节取首行（daemon.log 可达 50MB，绝不整读）。③ 判定必须在 rename **之前**——rename 后 logs/ 已被换成空目录，读不到上一轮日志；且清空时 **`watchdog.pid` 必须保留**（与归档路径同口径，否则 WebUI「关闭调度」失效）。读不到/解析失败/时钟回拨（差值为负）一律判为「非短会话」，保守走正常归档，绝不因判定失败丢日志。遗留 staging 目录在短会话下**仍照常回收打包**（属更早会话，不是本轮垃圾）。因归档发生在 `logger::init` 之前无日志可打，用 `SHORT_SESSION_DISCARDED` 标志供 main 在 init 后补一条 `main-log-short-session-discarded`。

**「staging 目录遗留 → 日志永远进不了 logd」历史坑（2026-09-16 修复）**：`log_archiver` 是游离线程，进程在打包完成前退出（日志门限 `exit(0)`、panic、被 kill）会把它连带杀掉，被 rename 出去的 `ziped_*` 目录就此留在模块根——它既不在 `logd/` 也不在 `devimp/`，`enforce_dir_limits` 扫不到、后续启动也不认领；而此时 `logs/`/`devimp/` 是上一轮重建的空目录，下一次启动「看起来没有东西可归档」，表现为**特定条件下启动调度不打包**。修法：`archive_on_startup` 在 rename **之前**用 `collect_staging_dirs`（前缀 `ziped_`）收集遗留目录，与本次的一并打包（各自沿用自己目录名、保留原时间戳），空目录直接丢弃；同时 `unique_staging` 给 staging 目录名去重，避免重名 rename 失败导致本次整体不归档。**新增任何「rename 到同级临时目录再异步处理」的流程都必须配同款回收**，否则中断即永久丢失。

**「logd/ 被全清」历史坑（2026-09-15 修复）**：原实现把两目录合并成一个 128MB 总预算并跨目录按 mtime 删除——devimp 单文件软上限 128MB、按数量最多留 20 份（上限约 2.5GB），其膨胀被算到 logd 头上，而 logd 内归档包 mtime 恒旧于本会话正在写的 devimp 文件，于是归档包被优先删光（含启动时刚打好的那个）。勿再合并两目录预算。

- 日志打包门限（2026-09-15）：**事件触发、零额外 syscall**——daemon 写日志只有三条路径（`daemon.log` 的 `SelfHealingAppender`、`status.csv` 的 `status_write_line`、`devimp` 的 `devimp_write_line`），三条都在落盘后调 `logger::note_write(&LOGS_BYTES_WRITTEN | &DEVIMP_BYTES_WRITTEN, "logs/" | "devimp/", n)` 记账（`[loglimit]` 区块）；`logs/` 或 `devimp/` 本会话累计写入 ≥ `LOG_RESTART_THRESHOLD_BYTES`（16MB）即 `std::process::exit(0)`，看门狗 3s 后拉起新进程，在 `logger::init` 之前由 `archive_on_startup` 把两个目录打包进 `logd/`（打包只在启动路径发生，故只能靠重启调度触发；日志逐条落盘不丢数据）。**刻意不遍历目录**：轮转/清理发生前（daemon.log 50MB、status.csv 8MB+备份、devimp 单文件 128MB）累计写入量与目录实际大小一致，且每进程从 0 计起（启动归档已清空两目录）——勿改成定期 readdir，多花 syscall 且状态更差。无看门狗时（`watchdog_pid()` 为 None：调试直跑/孤儿态）**不退出并把计数清零**——退出后无人拉起、调度会永久停止。退出前打点的日志本身也走同一写路径，以 `LOG_RESTARTING` 原子标志防重入。

- 开发诊断日志（devimp/）：独立目录 `<模块根>/devimp/`（与 logs/ 平级，**启动时随 logs/ 一起归档**到 `logd/devimp_<MMDD-HHmmss>.tar`）。**按前台包名分组**：文件名 `devimp_<前台包名>_<MMDD-HHmmss>.log`（本地时间秒级，人眼可辨；同秒重开以 -N 后缀去重）（首次写入惰性创建，整轮未开启 DEV 则不产生文件），scheduler_ipc 每秒经 `logger::set_devimp_package` 同步 `app_detect::get_current_package()`，**包名变化即关闭当前文件、下次写入以新包名+当前时间戳开新文件**；原始包名同时存入 logger 全局，**所有行类型的 package 列自动填充前台包名**（tick/core 不感知包名也带包名；place/event 仅在携带有效包名时覆盖，aff 后台迁移行传 "-" 是刻意不归属前台包，勿改 set_pkg）；空包名（启动初期尚未检测到前台应用）**不触发切文件**继续写当前文件，尚无任何包名时包名段为 `nopkg`（防 `devimp__` 空段）；单文件软上限 128MB **触顶自动换新时间戳文件继续写**（勿改回静默停写）。**写入量控制**：tick 行按 cluster 节流——决策签名（decision/tgt_perf/cur_freq/max_freq/thermal/touch/防抖进度）变化才写、无变化 2s 心跳（`DEVIMP_TICK_HEARTBEAT`，状态在切文件时清零），稳态从 akmode 25 行/s/组 降到 0.5 行/s/组，勿改回每 tick 必写；place/core 低频快照每 `DEVL_ROW_EVERY_ROUNDS=4` 轮（8s）一轮；snap 1s 一行保留（功耗关联锚点）。文件名含包名段、字典序≠时间序，清理（`devimp_prepare` 启动一次 + 触顶换文件后）一律按文件 mtime 排序保留最近 20 份，当前活跃文件不清理。总开关 `logger::set_devimp_active`（scheduler_ipc 按 `Config.meta.dev_record` 同步，meta 允许外部修改的字段之一、WebUI「开发记录」开关 + 热重载）；模式名经 `logger::set_devimp_mode` 同步、各行 mode 列自动填充。CSV 宽表 40 列 + `type` 列：`tick`（每决策 tick × 每核心组的调频决策轨迹，CLG Worker 与 akmode on_load_update 写入）、`snap`（1s 环境上下文，前台包名**实时取自** **`app_detect::get_current_package()`**——ModeChange 仅模式变化时才有事件，用事件维护会写过期包名）、`place`（低频快照：前台线程的包名/线程名/落点核，全部来自 affinity 缓存——comm 在线程首见 stat 采样时缓存、fg_cmdline 每轮刷新一次，**零新增文件读**）、`aff`（亲和迁移动作 pin/promote/demote/restore/blacklist_skip，包名列用缓存 cmdline）、`core`（逐核 util + 钉核计数，与 place 同周期每 4 轮/8s 一次）、`event`（模式/屏幕/热/配置状态变化；含 `kind="fas"` 行，action: activate/deactivate/destroy——FAS 实例生命周期状态变化即时打点，见 FAS 小节）。**共用数据减小开销**：电池/CPU 温度在 1s snap 块读一次存入 `last_batt_temp/last_cpu_temp`，status.csv、devimp snap、thermal（2s）三处共用（thermal 复用 ≤1s 旧值，带回滞的秒级判定无影响）；affinity 的逐核 util/在线位图/线程 comm 均为缓存复用。未开启开关时所有写入点零 IO。

- **时间戳统一为设备本地时间（2026-09-11）**：status.csv / devimp 的 ts 列此前经 `as_secs()%86400` 输出 UTC，与 daemon.log、devimp 文件名（本地时间）相差时区，离线对齐必须人工换算（8550 整夜功耗分析踩坑）。现 `logger::format_now` 经 `libc::localtime_r` 走系统时区（非 unix 回退 UTC），devimp 文件头注释为 `# ts-column=local format_now`。新增任何日志流一律本地时间，勿再引入第二时区。

- devimp 文件头的 `soc/board/model`（2026-09-17 修复）：`model` 等跨分区属性（`ro.product.model` 在 /product、/vendor 分区）**必须走 `common::getprop`**（调 `getprop` 命令拿合并视图），直接读 `/system/build.prop` 会读空、头里显示 `-`；getprop 为空时回退 build.prop 解析。

- devimp `tgtop` 行（2026-09-11）：30s 一轮全系统 top 消耗者快照（cpu_monitor tokio 循环读 `TGID_RUN_TIME` 全 map 增量，top-5/轮），定位待机期「小核 util 长期 60%+」的后台元凶（place 行只覆盖前台线程）。列语义复用：pid/tid=TGID、comm=cmdline 首段（退化 comm）、util_pct=窗口运行占比（**多核并行可 >100%**，如 320%≈3.2 核满载，不 clamp）、max_util=运行时长增量 ms。门控 `chiri_telemetry && devimp_active`，首轮只建基线不出行；基线 map 整体重建（死进程条目随之清除）。新增行类型时同步更新 DEVIMP_HEADER 注释与本条。

- devimp 双实例防护（进程层兜底）：devimp 文件按 `devimp_<pkg>_<毫秒时间戳>.log` 命名，两个 daemon 实例并行时会写两份不同时间戳的同包名文件（status/daemon 日志同理）。shell 侧清理（service.sh/action.sh/WebUI stopScheduler 依赖 logs/watchdog.pid）在 pid 文件丢失时失效——旧看门狗 3s 把 daemon 拉起与新实例并行。main.rs 在一切初始化（含日志归档）之前对 `<模块根>/daemon.lock` 做 `flock(LOCK_EX|LOCK_NB)`，拿锁失败直接 exit(0)（看门狗 3s 重试，旧实例退出后接管）；锁文件放模块根而非 logs/（logs/ 每次启动被整体归档 rename，不可作锁锚点）。fd 故意泄漏持锁至进程退出，内核在进程死亡时自动释放，无需清理逻辑。

- 新增日志 key 同时补 `module/config/i18n/zh.ftl` 与 `en.ftl`，命名格式 `模块-描述`。

### 配置（嵌入、快照与热重载）

- 调优配置编译期嵌入二进制（防篡改）：`common.rs` 用 `include_dir!` **整体**嵌入 `module/config` 目录（`CONFIG_DIR` + `common::embedded_config_file(相对路径)`），`module/rules.yaml` 与 `src/chiri/*.yaml` 仍用 `include_str!` 单文件嵌入。原 config.yaml 已拆分为 **meta.yaml（用户可修改抬头，九个字段严格校验）+ feature.yaml（不可修改调优段，磁盘不落盘）**，各 SoC 目录与默认 config/ 下各一份且必须成对。**新增/删除 yaml 无需改任何 .rs**（放目录即被收录；目录增删由 build.rs 的 `rerun-if-changed=module/config` 触发重编译）。**必需文件缺失直接编译失败**：build.rs `assert_required_configs` 断言 meta/feature/akmode/scenemode/fas/ftl 存在且 SoC 目录 meta/feature 成对，杜绝静默回退代码默认值。

- FAS 配置文件（均编译期嵌入）：`module/config/normal/fas.yaml` 定义 `fas.apps: {包名: 配置名}` 白名单，随 module/config 目录构建期嵌入并经 OnceLock 解析，运行时导出 `fas_whitelist.yaml` 对外只读，用户/WebUI 不可修改；`module/config/normal/fas/<配置名>.yaml`（如 fas/endfield.yaml）为每应用 FAS 调优，编译期嵌入不导出（`FasRulesConfig` 全字段），由 `common::embedded_fas_app_str(配置名)` 按 `normal/fas/<配置名>.yaml` 路径查找（不再有 match arm）。**FAS 游戏两步**：fas.yaml 加一行 → 新建 `normal/fas/<配置名>.yaml`（可复制 `normal/fas-example.yaml` 全字段说明书裁剪——该文件仅文档参考、不被加载）。FAS 配置已脱离 rules.yaml（rules.yaml 的 `fas_rules` 注释模板与 `RulesConfig.fas_rules` 字段保留为 Yumi 冻结区向后兼容）。

- `Config::load(path)`（chiri 与 scheduler 各一份）：feature 段解析嵌入 feature.yaml（磁盘不落盘）；meta 先取嵌入 meta.yaml 默认值（`common::embedded_meta_defaults`），再被磁盘 meta.yaml 覆盖（`common::read_external_meta`）。meta.yaml 严格整体校验（`parse_disk_meta`）：**字段全部可选**（2026-09-18 定稿：`Option<T>` + 缺省 = 沿用二进制内嵌默认，与 rhine 影响项同一「缺省 = 不变更」语义——老文件、精简文件、缺新字段的文件都合法，不会因字段扩充把用户设置判非法整体覆盖）——**出现即校验**：loglevel 限六档、language 限 en/zh、name/author 非空、开关与 power_avg 必须布尔、power_max_w 必须数字（越界回退默认 12 不判非法），未知键仍由 `deny_unknown_fields` 拒绝、`deny_unknown_fields` 拒绝未知键、loglevel 限六档、language 限 en/zh、name/author 非空——必填/类型异常整文件判非法（不再按行提取），由 sync_meta_snapshot 用内嵌默认覆盖修正；`power_max_w` 越界（≤0/>200/非有限）**不判非法**，回退默认 12（单个数笔误不连丢其它设置）。daemon 只消费 loglevel/language/四个开关 + power_avg/nofix/power_max_w（chiri 全量覆盖，scheduler 仅 loglevel+language；name/author 仅 WebUI 展示）。**新增字段要同步**：`MetaYamlFile`/`ExternalMetaOverrides`、`chiri::config::Meta` 字段与 `Config::load` 的两处覆盖合并、四个 meta.yaml 模板（`config/meta.yaml` + 三个 `{soc}/meta.yaml`）、WebUI `contract/meta.ts`（必填进 META_FIELDS、可选进 OPTIONAL_FIELDS + WRITABLE_FIELDS + 类型校验；**可选字段缺行时写入走 `appendTopLevelField` 顶层追加**）与 `tests/contract.test.ts` 的合法样本；`rewrite_meta_toggles` 只管实验室接管的三个开关，新字段不必进。

- meta.yaml 快照自愈：`common::sync_meta_snapshot()` 在 main 启动时与两套 config_watcher **触发热重载前**调用（先于 Config::load）：文件合法 → 跳过写入（防 inotify 成环）；缺失/不可读 → 嵌入原文原子重建（不留警告注释）；任一字段非法 → 嵌入原文整体覆盖 + 文件末尾追加警告注释「上一次修改存在非法字段，已恢复为默认值」+ warn 日志。`get_config_path()` 现指向 meta.yaml（active_config.chr 内容随之变为 "8550/meta.yaml" / "meta.yaml"）；旧版遗留 config.yaml 由 main 启动时清理。**meta.yaml 可选字段 `nofix: true` = 跳过本自愈**（见下条；`common::read_nofix_flag` 必须在自愈之前调用，晚了开关自身先被覆盖；文件缺失/非法按 false 照常自愈）。判定结果经 `common::set_nofix()` 置为进程级标志，**全部覆盖类入口**——main 启动自愈、chiri/scheduler config_watcher 热重载自愈、`sync_rules_snapshot` 的 rules 展示副本还原、webui 资产还原——统一读 `common::nofix_active()` 门控（各入口勿再各自读磁盘）；nofix 期间非法 meta 由加载侧（`read_external_meta`/`Config::load`）自行回退内嵌默认，磁盘文件保持用户原样。

- **二进制内嵌 WebUI 资产还原（2026-09-18，防界面篡改）**：`build.rs` 的 `[webui-embed]` 段在构建期把 `webui/dist` 全量生成 `include_bytes!` 清单（`OUT_DIR/webui_assets.rs`；dist 缺失 → `EMBEDDED=false` 自动降级，xtask 构建顺序「先 webui 后 core」保证打包产物带全量资产）；`src/webui_asset.rs::restore_webroot` 在 daemon 启动（logger 初始化后）把内嵌副本与模块 `webroot/` 逐文件比对，**只覆盖缺失或不一致的文件**（被篡改/删除的界面文件回出厂；内容一致跳过省擦写；多余文件不删）。与 meta 自愈同属「二进制内容对外部文件的覆盖类操作」，`nofix: true` 一并跳过并打 `main-nofix-skip` 日志；还原条数打 `main-webui-restored`。rhine 实验改写 meta 是用户主动操作，不受 nofix 影响；WebUI 无 nofix 写入口（只能手改 meta.yaml）。

- `rules.yaml` 嵌入二进制且只读：编译期 `include_str!` 打包（`common::embedded_rules_str`/`embedded_rules()`），运行时**一律读嵌入值**（monitor/chiri/scheduler 初始加载 + app_detect 热重载共 4 处均改读嵌入）；磁盘 `rules.yaml` 仅是 `main.rs::sync_rules_snapshot` 启动时复制出的对外展示副本（内容一致跳过写），被篡改不影响调度行为、下次启动还原。全局模式、应用性能模式等均由模块随附维护，WebUI 无写入入口。

- 功能总开关（config.yaml meta 段，缺省 true，热重载即时生效）：`fas_enabled`、`scenemode_enabled` 与 `thread_bind`（**线程摆放总闸**：false = 关闭线程功能——`Config::load` 里与机型内嵌的 `Affinity.enabled` / `CoreCtl.enabled` 取「与」，关掉后 `AffinityManager::apply` 直接 `release()`（逐线程恢复全核 + cpuset/uclamp 快照回写）、`CoreCtlManager` 回 Normal（恢复 min_cpus/online 快照），即「把绑定分配全部改成全核心」；落地时机是 config_dirty 分支的 `apply_affinity_and_corectl`，2s 周期块再兜一次）。字段在 chiri `Meta`（`#[serde(default = "crate::utils::default_true")]`），`Config::load` 读盘 meta 覆盖后经 `common::set_fas_enabled`/`set_scenemode_enabled` 同步到进程级原子标志（覆盖 main 启动 + chiri config_watcher 热重载两条路径）。fas_enabled=false ⇒ `fas_available()` 恒 false（determine_mode 不产生 fas、FAS 监测线程不启动），chiri 主循环 fas 块开头注销运行中实例并按屏幕状态恢复 balance/doze；scenemode_enabled=false ⇒ 息屏不进入 scenemode，已激活的下个 tick 退出并恢复 affinity/core_ctl 快照 + doze 配置。高频路径只读原子量，不许在 tick 内读磁盘。快照自愈会保留这两个 meta 字段（meta.yaml 覆盖已含）。WebUI「配置」页提供三个总闸开关（`src/views/ConfigView.svelte` → `state.setDraft` → `contract/meta.ts::writeMetaFields`；被实验室接管的项按 `data/lab.ts::LAB_TAKEOVER` 置灰并说明原因）：先落草稿、由「保存」做单次读-改-写（顶层行替换 + tmp→rename），写后回读实际值（守护进程可能判定非法并整体重置）；dev mock 走同一条链路。

- 功耗口径开关（meta.yaml `power_avg`，默认 false，2026-09-18 加）：控制 ChiRi 1s 状态采样写模块根 `PowerAVG.chr` 的口径——false = 参考值（(旧值×10+新值)/11 递推，偏历史——上次留存值先乘 10 再参与，单次异常采样只占 1/11；**样本含息屏**）；true = 累计平均（(旧值×次数+新值)/(次数+1)，等权全史；**样本仅亮屏放电**）。计算紧跟在 status.csv 行写入之后**顺序**执行（`logger::power_avg_update`，不另起并行）；留存值/次数是 logger 的进程级静态（调度线程 panic 重启不丢、daemon 重启清零；文件只是输出记录、不读回）。WebUI 在「电池读数」二级页直写该字段（不走草稿，写后立即热重载），状态页第一卡片右侧按当前口径读 PowerAVG.chr 展示（缺失/空显示 —）。配套 `power_max_w`（meta.yaml 可选字段，默认 12，写入侧限 0–200）：状态页耗电仪表盘（官方 ak-gauge，进度环 = 当前值/满量程）的换算基准，**在「电池读数」二级页可改**（数字输入 + 1..=200 校验，写后回读）；daemon 不参与换算、仅透传配置。

- 对外暴露审计（落盘最小化）：磁盘只保留有外部读取方的文件——meta.yaml（WebUI 读写）、rhine.chr（WebUI 读写 + 用户手改）、down.chr（WebUI 读写 + 用户手改，DOWN 停摆状态）、rhine-back.chr（daemon 写、WebUI 只读展示）、PowerAVG.chr（daemon 写、WebUI 只读展示，功耗参考/平均；**启动清空**——只反映本次运行，上一次的残值不当读数）、LiveTime.chr（daemon 写、WebUI 只读展示，存活心跳）、rules.yaml / fas_whitelist.yaml / special_tuned.yaml / current_mode.chr / active_config.chr（WebUI 只读）。feature.yaml、normal/tuned_profiles.yaml、normal/scenemode.yaml、normal/fas\*.yaml、**rhine-init.yaml** 无任何读取方：不落盘、不监听，xtask 打包时从模块包移除（仅存于二进制）；customize.sh 热更新备份/恢复只针对 meta.yaml。i18n ftl 仅嵌入消费，暂仍随包（后续可同样移除）。

- 存活心跳（LiveTime.chr，2026-09-18 重写判据）：daemon 侧 `logger::write_live_time` 由 main.rs 的独立线程 `live_time` 每 `LIVE_TIME_INTERVAL_SECS`(15s) 写一次当前**本地时间** `MM:SS`（原子写、失败静默）——不搭任一调度循环：两套调度器（chiri / yumi）共用同一心跳，语义是**进程级存活**（与旧 flock 判据一致，调度线程卡死不带停心跳）。WebUI 侧 `contract/daemon.ts::probeLiveness` 读该文件并与本机时间比差（解析与容差在 `data/live-time.ts`）：差值 > `LIVE_TIME_TOLERANCE_SECONDS`(20s) 判「已停止」，**文件缺失同判**，读失败/内容非法才报错显示「无法判定」。差值按 1 小时取模（文件只有分秒、无小时）——超过半小时的旧值会落进 (1800,3600) 区间，不会被误判成新鲜。取代此前 `flock -n daemon.lock` 探测：不再依赖 toybox 是否带 flock applet（旧实现在缺失时只能显示「无法判定」），也不再需要「取锁后立刻释放」的约束；daemon.lock 仍由 daemon 自持（单实例），WebUI 不再读写它。dev mock 在**读取时刻**动态生成心跳内容（`?daemon=stopped` 写 8 分钟前），静态播种会在预览打开 20s 后自然过期。

- 常驻状态通知（`src/notify.rs`，2026-09-18 加）：daemon 用 shell 工具投递**常驻**通知（`/system/bin/cmd notification post -S bigtext -t <标题> -c chiri-status -o chiri-status <正文>`；直接 spawn `cmd` 而不走 `sh -c`，参数不经 shell 解析、无引号风险），由 ChiRi 1s 循环每 `INTERVAL_SECS`(5s) 调一次——循环只组装内容并**非阻塞投递**到 notify 自己的线程（`sync_channel(1)` + `try_send`：投递线程还在忙就丢本次、下个周期再送，调度循环绝不等待 `cmd`；**内容与上次成功投递完全一致时跳过**（省一次进程创建）、失败冷却、进程创建都在该线程内，见 `[dispatch]`/`[worker]`）。内容：标题 = 前台包名（空则 `notify-title-fallback` 兜底）；正文**单行**、参数间用 ` · ` 分隔（`notify::SEPARATOR`）：模式 · 家族 · 子模式 · 温度 · 功耗——**各参数只出值、不带「模式：」这类字段标签**（用户口径），温度写成 `38.5/45.2 °C`（电池/CPU，缺测写 `-`），功耗带 `W`（**只出数值**，是参考值还是平均值随 meta.yaml `power_avg`——即电池读数页里的口径开关，通知里不标注口径）。模式展示名与家族派生与 WebUI `data/mode.ts` 同口径：CLG/实验室/FAS/down 的展示名就是 id 字面量，**特调直接用模式 id**（用户口径：类名「特调/Tuned」不含新模式信息，删掉；`akmode`、`playback` 这类 id 才是具体模式），只有息屏场景与未注册值有本地化名（`notify-mode-scenemode`/`notify-mode-unknown`），特调集合取编译期嵌入 `special_tuned.yaml` 的 modes 并集（`OnceLock` 缓存）；**家族与子模式都按信息量去重**（与模式卡片同口径）：家族只在 `clg`/`lab`/`stardust` 出现（特调与 `Special`、未知与 `UNKNOWN` 是同一个词，同时出现等于重复两次），子模式只在它与展示名不同时出现（CLG 与实验室档的展示名就是 id）。投递命令行三级候选（带渠道 + 常驻 → 去渠道 → 最简形式，各 ROM 旗标支持度不同）：单条失败记 debug、**全部失败**记一条 warn（本进程一次）并进入 **60s 冷却**（避免通知不可用的机型每 5s 起进程）。**关闭调度时 daemon 是被 SIGKILL 杀死的、没有清理时机**，故由 WebUI `contract/daemon.ts::stopScheduler` 在杀进程后追加 `cmd notification post -d chiri-status` 撤销（best-effort、失败静默）。**开关**：meta.yaml `notify`（默认 true）——false 时不投递，且 daemon 自己撤销已投递的通知（`notify::cancel`：首轮无条件，清上一次运行的残留；此后幂等），WebUI 可修改项里有对应开关（走草稿 + 保存）。Yumi 未接入（无 1s 状态采样与功耗数据）。

- 对外写文件防 panic：新增的 `common::write_file_no_panic`（tmp + rename，失败回退 try_write_file）全程无 unwrap/expect；`sync_rules_snapshot` 写失败时补建父目录重写一次，重写仍无效则 warn 并直接跳过——**任何对外文件写入都不允许 panic 击穿启动流程**，`sync_meta_snapshot` 同口径。

- `matched_soc_hint()` 已不检查磁盘目录存在性。新增配置项需同步更新反序列化结构体与默认值；改 `module/config/` 下的 yaml 源文件后重新编译才生效。**`module/config/config-example.yaml`** **是完整字段说明书（不参与加载），任何新增/删除/改语义的配置段与 meta 字段都必须同步更新它**（含注释说明取值范围与机型差异），否则模板与实际配置脱节。

- 所有加载/热重载入口（main.rs、两套 config_watcher）统一走 `get_config_path()`，不要硬编码路径。启动时把生效配置的相对路径（如 `8550/meta.yaml`，非处理器时 `meta.yaml`）写入 `active_config.chr`，WebUI 据此读取同一份文件。

- 热重载链路四处断链已修复，勿回退：
  1. config_watcher 必须监听生效配置的父目录（`config_path.parent()`），不是固定 `config/` 根目录。inotify 目录监听不递归，ChiRi 生效配置在 `config/{soc}/meta.yaml` 子目录，监听根目录收不到 CLOSE_WRITE/MOVED_TO，导致 8550/8475/8998 上 WebUI 改 meta.loglevel 热重载完全失效（Yumi 生效配置在根目录，所以历史未暴露）。
  2. `common::sync_meta_snapshot` 必须原子写（write_file_no_panic：同目录 tmp 文件 + rename）。直接 `fs::write` 截断覆盖存在窗口期，config_watcher/WebUI 可能读到半截内容导致 meta 解析失败回退默认（用户日志等级被静默丢弃）；rename 触发 MOVED_TO 后靠「内容合法跳过写入」防环。
  3. meta.yaml 调参热重载须联动运行中的调度器：config_watcher 重载成功后置 `config_dirty`（`Arc<AtomicBool>`），scheduler_ipc 循环内 `swap` 消费，亮屏时按当前模式 reload CLG/akmode 并刷新亲和/core_ctl（息屏不覆盖 Doze，亮屏事件补上）；此前调参要等下次 ModeChange/规则重载才生效。
  4. **inotify 实例必须跨重载复用，且必须按文件名过滤**（2026-09-16，改用 `utils::DirWatcher`，已删 `utils::watch_path`）。旧写法每轮 `Inotify::init → add_watch → read_events_blocking → 返回即 drop`，两轮之间无 watch 在册，此期间到达的事件被内核直接丢弃；且目录级 watch 不过滤文件名，WebUI「先写 `meta.yaml.webui.tmp` 再 `mv` 覆盖」产生的 CLOSE_WRITE(tmp) 会让监听器在**覆盖前**醒来、读到旧内容（日志级别保持旧值），而真正的 MOVED_TO(meta.yaml) 恰好落在重载窗口内被丢弃——现象是「日志显示重载成功但改动不生效」。现 `DirWatcher` 常驻一个 inotify，`wait_change(file_name)` 只认目标文件的 CLOSE_WRITE/MOVED_TO，命中后 100ms 静默 + 清空积压（与 `app_detect::watch_config_file` 同口径）；实例异常时置 None 下轮重建。新增任何目录级 watch 一律复用它，勿再写「每轮重建 inotify」。

### 实验室（rhine，ChiRi 专属）

- **三个文件各管一件事**：`module/config/rhine-init.yaml` 是模式定义（编译期嵌入、xtask 剔除，不落盘）；`rhine.chr`（模块根，对外暴露可手改）只存状态——内容就是模式 key，空文件或只有注释 = 未启用；`rhine-back.chr`（模块根，daemon 生成）是套用前一刻的原值快照，还原完即删除。**rhine-back.chr 同时是「上次启用过实验室」的唯一信号**——开机时 service.sh 已清空 rhine.chr，光看它看不出要不要还原。
- **影响项语义：缺省 = 不变更**。rhine-init.yaml 里每个模式下列可选五项：`global_mode`（运行时覆盖 rules 的 global_mode 兜底）、`fas_enabled` / `scenemode_enabled` / `thread_bind`（改写生效 meta.yaml）、`special_tuned`（运行时开关，false = 所有特调判定失效）。只有写出值的项才被套用。当前 `vector`（界面显示 Vector Breakthrough）四项齐全，`contingency`（Contingency Contract）/ `babel`（Babel）/ `frozen`（Frozen）是**空映射的预留条目**——WebUI 显示为「预留」且不给启用入口，将来把影响项填进去就能上线，两侧代码都不用改。
- **两层落地是能力边界决定的，不是风格选择**：fas/scenemode 有 meta.yaml 载体（`Config::load` 会同步到 `FAS_ENABLED` / `SCENEMODE_ENABLED` 原子标志），thread_bind 同样有载体（`Config::load` 里与机型内嵌的 `Affinity.enabled` / `CoreCtl.enabled` 取「与」，见 `chiri/config.rs`），所以直接改写文件，WebUI 开关才能同步显示为关；global_mode 在 rules.yaml 里是编译期嵌入、special_tuned 白名单是 `include_str!` 嵌入，写文件没用，只能走运行时覆盖（`common::lab_global_mode()` / `common::lab_special_tuned_disabled()`，仿 `FAS_ENABLED` 范式，高频路径只读原子量）。
- **改写 meta.yaml 只能顶层行替换，绝不能 serde 重排**：`MetaYamlFile` 是 8 字段必填 + 3 可选 + deny_unknown_fields，整文件重排会吃掉用户注释，漏一个字段就会被 `sync_meta_snapshot` 判非法并整体覆盖。用 `common::replace_top_level_bool` + `rewrite_meta_toggles`（三个开关**一次写盘** = 一次热重载事件；分两次写会让「只关了一半」的中间态真的被执行一遍）。口径与 WebUI `contract/meta.ts::replaceTopLevelField` 一致，两边不要各写一套。
- **先还原、后套用**（`rhine::converge`）：任何时候都先按 rhine-back.chr 还原，再读 rhine.chr 决定要不要套用。顺序反了的话，模式 A 切 B 时快照里记的「原值」会变成 A 改过的值，还原一次比一次偏。启动期（main.rs 首次 `Config::load` 之前）走同一条路径，三种场景都自洽：开机只剩快照 → 只还原；不停机重启调度 → 先还原再重新套用（快照刷新）；平时两者都不在 → 什么都不做。快照非法时用 `common::embedded_meta_defaults()` 兜底（三个总闸回内嵌默认、运行时覆盖全清），最坏是回到出厂设定，而不是卡在退不出的半覆盖态。
- **兜底创建与非法重置**：`rhine.chr` 缺失就地补建内置模板（`include_str!("../module/rhine.chr")`，只有注释，用户一眼看到该写什么）；内容非法（非单行标量 / 未知 key）覆盖为同一份模板并 warn。读不到、解析失败一律按「未启用」处理，绝不因判定失败丢状态。
- **监听与即时收敛**：`src/chiri/mod.rs` 起 `rhine_watcher` 线程，用 `utils::DirWatcher` 盯模块根的 `rhine.chr`，写入即套用/还原（与 meta.yaml 热重载同语义）。套用/还原后调 `monitor::app_detect::request_mode_refresh()`（进程级 `FORCE_MODE_REFRESH`，在 app_detection_loop 里与规则热重载那条 force_refresh 通道合并消费），当前模式当场重算——否则用户在游戏里点了启用还得切一次前台才看到效果。
- **只改 global_mode 兜底**：`determine_mode` 里被替换的只有兜底值，FAS 白名单与 `app_modes` 的优先级不变，用户显式配过模式的应用仍按自己的规则走。
- **运行时锁定：启用后必须重启设备才能关闭**。套用成功就在 tmpfs 上写 `chiri-labs.lock`（`LOCK_DIRS` = `/tmp` → `/dev` 按序探测可写目录；两处都不可写时只在启动 warn 一次并放弃锁定，不因此禁用功能——Android 根目录不一定有 /tmp）。**标记存在 = 本次开机后启用过**，于是 `converge` 在锁定期间既不做还原也不接受关闭，且**锁定期间以 `rhine.chr` 的写法为意图来源**：文件写了模式就按它生效（换模式不拦，标记第一行同步过去——否则文件与实际生效的模式会长期各说各话且无人知），文件被清空/改坏才当关闭请求处理（写回标记里的模式 + 留痕 + warn），只做 `reassert`（装回 meta 的三个开关与运行时覆盖），**绝不重写 rhine-back.chr**——此刻 meta 已是实验室改过的值，重写会把改过的值记成原值，之后每还原一次偏一次。快照真的丢了才用内嵌默认补一份（`write_fallback_backup`）。重启后 tmpfs 清空 + service.sh 已删 rhine.chr，才走正常还原。`action.sh` / WebUI「重启调度」不删标记，所以那之后实验室仍是启用态（停止的是调度进程，不是设备）。
- **强制关闭（保留字 `off`，2026-09-16 新增）**：`rhine.chr` 里写 `off` = 用户明示「无视风险」，`converge` 看到它就**清掉标记并落到正常还原路径**（还原 meta 三开关 → 清运行时覆盖 → 刷新模式，即「恢复原地调度」），随后把 rhine.chr 写回内置模板（未启用）——**无论此刻有没有锁定标记**：重启后标记已随 tmpfs 消失，但用户写的 `off` 还留在模块目录里，非锁定态不写回界面就会永远停在「已提交强制关闭」这个中间态；写回自身的事件不会引发第二轮收敛（那时 `next == current == None`）。与「写空内容」的区别正在这里：空内容 = 普通关闭请求，锁定期间被挡回；`off` = 强制关闭。三条硬约束：① `off` **必须在 YAML 解析之前做字面量比较**（`is_force_off`：忽略大小写、允许成对引号）——`off` 在 YAML 1.1 里是布尔字面量，各实现口径不一，交给类型推断会出现「界面认、守护进程判非法」的裂缝；② `off` 是**保留字**，模式 key 不得取这个名字；③ 日志统一走 `log_converged`（按 `Converged::forced` 打点）——启动期那次收敛早于 `logger::init`，在 `converge` 里打会直接丢。WebUI 侧：锁定期间「关闭」按钮**不再禁用**，改为弹 `ConfirmSheet` 摊开风险（`lab.force.*`），确认后 `state.svelte.ts::forceDisableLab()` 写 `off` 并等 400ms 再回读（守护进程收敛要几十毫秒）；`data/lab.ts` 用 `LAB_FORCE_OFF` 解析出 `{ kind: 'force-off' }`，`labWarnings` 对该态**不报** lock-drifted / no-backup（预期中间态）。
- **篡改痕迹与界面告警**：标记第一行是模式名，其后每行 `# 代号` 是异常记录（`lock_note` 追加，去重、最多 8 条）：`state-reset`（rhine.chr 内容非法被重置）、`close-rejected`（关闭请求被挡回）、`backup-rebuilt`（快照丢失被内置默认补上）。**写代号而不是现成文案**：留痕可能发生在 `load_language` **之前**（启动期收敛），那时 i18n bundle 还是空的、`t()` 只返回裸 key，标记里就会留下一串 i18n key；WebUI 用 `lab.lock.note.*` 映射成文案，未收录的代号原样显示（便于跟进新增代号）。WebUI 读标记 + 一致性检查（`data/lab.ts::labWarnings` 给出稳定代号 `state-invalid` / `lock-drifted` / `no-backup`），命中就在实验室页顶部出警示条「检测到运行配置被改动，建议立即重启设备」，并把痕迹原文附在 detail 里。痕迹随 tmpfs 在重启后消失，与处置方式一致。**改动运行配置的代码路径都要记得留痕**，否则界面看不到、用户只会觉得"莫名其妙不对了"。
- **DOWN 停摆（2026-09-16 新增）**：`down.chr`（模块根，对外暴露可手改）写着保留字 `down` 时，调度**释放全部接管**——CLG / akmode / FAS / fast_lock / affinity / core_ctl / scenemode 一并交回系统，只往 `current_mode.chr` 写一次 `down` 且**不再覆盖它**（覆盖就等于退出停摆）；采集与日志照常（eBPF / status.csv / daemon.log / devimp），`app_detect` 继续跑，所以 status.csv 仍记前台包名。用途是记录「ChiRi 不工作时」设备自己的调度情况。与 `rhine.chr` 同款：内容即状态、缺失补建模板（读不到按「不停摆」处理，绝不因状态文件异常自关调度），且文件在磁盘上，**重启后仍保持停摆**，只能改文件解除。实现放在调度循环（`src/chiri/mod.rs`）而不是监听线程——governor 对象归那个线程独占；监听只在 `src/down.rs`（`down_watcher`）维护一个原子标志。三处必须跳过，漏一处就出问题：① 5s `current_mode.chr` 自愈（**只推进计时器不落盘**，否则 `wait` 恒为 0 会变成 100% CPU 忙循环）；② config_dirty 下发与 2s 亲和/热保护块；③ 事件 `match msg` 之前 `if halted { continue; }`（事件照收但不处理，**不能放在 recv 之前**，否则跳过阻塞收包同样忙循环）。退出时删掉 `current_mode.chr`、把内存模式复位到 `down_resume_mode`，并**按该模式重新接管**（CLG / vector 锁频 / 亲和 / core_ctl 重建；特调与 FAS 交给后续事件，口径同 panic 自愈后的重建）——只复位内存模式是不够的：ModeChange 只在「模式真的变了」时才重接管，前台应用没换就永远停在释放态，而且不会有任何异常提示。同理**启动即停摆时启动块要跳过 `apply_affinity_and_corectl`**：`halted` 初值就是 `is_down()`，循环里那次「进入停摆」分支永远不会执行，写进去的布局谁都不会收回去。WebUI 侧：`data/down.ts` + `contract/down.ts` + 配置页「高级设置」面板（直接提交，不走 meta 草稿），`data/mode.ts` 给 `down` 一个 `kind: 'down'`（独立家族，2026-09-18 定稿——**不是 stardust**；显示名就是 id、danger 信号，总览页无需改动即呈现）。
- **模式家族与实验室静态分组（2026-09-17 重构）**：CLG（reduce/default/boost 兜底）/ **stardust**（scenemode；2026-09-18 家族位在 UI 注册——`ModeKind: 'stardust'` + i18n `mode.family.stardust`，**不并入 CLG**）/ **down**（DOWN 停摆，独立家族 `mode.family.down`，**不是 stardust**）/ **rhine**（vector/contingency/babel 仅实验室）四个家族。**contingency（危机合约）**：CPU+GPU 全核最高频（GPU 由 `src/chiri/gpu.rs::GpuGuard` 启动探测 devfreq 节点——Adreno kgsl-3d0 / 名字含 gpu/kgsl/mali 的 devfreq，用 available_frequencies 取硬件上限，探测不到跳过+warn；lock 时快照 min/max 写 hw_max，release 恢复）+ **performance 调速器**（`src/chiri/governor.rs::GovernorGuard`：快照各 policy 的 scaling_governor → 写 performance，release 逐 policy 恢复；读不到原值的 policy 跳过不接管；启动时 `cleanup_residue()` 把 SIGKILL 残留的 performance 恢复 schedutil——performance 不是任何厂商默认调速器；**部分机型 governor 节点只读写不进，锁频不能只靠它**）+ **极速锁频**（2026-09-17 修复「除小核外其他组锁不到最高频」：contingency 复用 `fast::FastLock`——min=max=硬件最高、每 5s 重写防篡改，与 vector 同口径，不依赖 governor 节点）+ 停线程迁移 + 后台进程压 0-1 两颗小核、前台/顶部全核。**babel（规整化）**：停线程迁移，后台→小核、前台/顶部→大核+超大核、系统进程(system-background)→大核。两者经 rhine-init 影响项（global_mode=自身 + fas/scenemode/special_tuned=false）驱动，行为在 `apply_affinity_and_corectl` 的 lab 分支实现（`AffinityManager::lab_static_apply`：停迁移 + 组级 cpus 写入，原值记内部快照、退出恢复；周期重入即纠偏）；governor/GPU/极速锁频由 `sync_lab_governor_gpu` 在启动/ModeChange/DOWN 进出/FAS tick 接管/亮屏恢复/panic 收尾与重建各入口同步（幂等；contingency 分支含 `fast_lock.init()`）。**进入 lab 前必须先释放其它调频接管**（CLG 的 tick/防篡改会压 scaling_max_freq、其 release 会恢复 schedutil 覆盖 performance；亮屏路径由通用接管块释放，息屏路径在 sync 前兜底）——2026-09-17 修复。**lab 静态分组与普通亲和同受 `meta.thread_bind` 总闸约束**（`apply_affinity_and_corectl` 的 lab 分支按 `config.affinity.enabled` 门控，关时走 `lab_static_deactivate`）——同日修复「线程调整关不掉」。**stardust 家族语义**：scenemode 激活期间停线程迁移、全部 cpuset 恢复全核、仅压 CLG 频率上限（apply 的 scenemode 分支 release + core_ctl 回 NONE；原 prime 整簇下线已废弃，`core_ctl.scenemode_offline` 配置字段停用仅保留兼容）。FAS 单实例 + 15s 延迟退出见 FAS 小节。**customize.sh 热更新后不再自动重启调度**，要求用户手动 Action 启动。
- **开机关闭**：`service.sh` 的 `[lab-reset]` 块在启动看门狗之前 `rm -f "$MODDIR/rhine.chr"`，只删它、不碰 rhine-back.chr（那是还原信号）。`action.sh` 与 WebUI 的「重启调度」刻意不删——实验室状态在设备重启前一直保留。
- **WebUI 侧不读 rhine-init.yaml**（它不对外暴露）：模式 key 与文案在 WebUI 硬编码，即 `webui/src/data/lab.ts::LAB_MODE_KEYS` 与 `i18n/locales/{zh,en}.ts` 的 `lab.mode.*`。**新增实验室模式要同时改四处**：rhine-init.yaml、LAB_MODE_KEYS、两套 locale、`LAB_TAKEOVER`（接管表：该模式会写哪些 meta 开关——**被接管的开关在配置页置灰不可切换**，因为锁定期间守护进程的 `reassert` 会把它们按定义写回去，手改只会「看起来生效」；`tests/lab.test.ts` 有刚性断言兜底，改 rhine-init.yaml 忘了同步这张表会红）。`parseLabState` 必须与 `src/rhine.rs::parse_state` 同口径（含保留字 `off`：`LAB_FORCE_OFF` ↔ `is_force_off`），否则会出现「界面说已启用、守护进程判非法并重置」的裂缝。**key 直接点明它实际做了什么**（`vector` = 全局模式切 vector 档、关 FAS/scenemode/特调；`contingency` / `babel` / `frozen` = 预留），界面显示名与 key 解耦、走 i18n `lab.mode.*`；显示名**一律用英文官方译名，中英两套 locale 同值（2026-09-17 用户要求，模式名不分语言）**：矢量突破 = Vector Breakthrough（官方简称 VEC）、危机合约 = Contingency Contract、巴别塔 = Babel、frozen 沿用 Frozen；中文只留在 `lab.mode.*.detail` 的描述里（2026-09-16 改名：`fastmode`→`vector`、`socremode`→`contingency`，并新增两个预留条目；**旧 key 不兼容**，rhine.chr 里残留旧名会被判非法重置为未启用）。新增模式的显示名同样用官方译名，不要自造。

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

- 分层（上层只依赖下层接口）：`kernel/ksu.ts`（原生注入的 ksu.\* 桥，逐 API 能力探测、缺失时降级）→ `kernel/shell.ts`（可注入 `ShellRunner`，dev 预览与单测注入 `dev/mock-shell.ts`）→ `contract/`（paths 路径安全校验 / read 三分类读取 / meta 读写 / **lab 实验室读写** / daemon 心跳（LiveTime.chr）存活探测与关闭调度 / sources 设备形态与只读源）→ `data/`（纯解析：status-csv 22 列、daemon-log、mode、whitelists、rules、apps、**lab**）→ `views/` 四屏 + 二级页。不硬编码路径；读配置前先读 `active_config.chr` 确定实际生效文件（`contract/sources.ts`）。

- 路由分两级（`router.svelte.ts`）：`VIEW_IDS` 四项进底部导航，二级视图（当前只有 `lab` 实验室，从配置页底部进入）不占导航位但同样有 hash（`#/lab`），刷新与后退行为一致；导航高亮用 `navOwner()` 回落到父视图（`SUB_OF`）。新增二级页要同时改 `RouteId`、`ROUTE_IDS`、`SUB_OF` 与 `App.svelte` 的视图分支。

- 实验室页（`views/LabView.svelte`）只摆事实：`rhine.chr` 里写了什么、`current_mode.chr` 观测到什么，两者分开显示、不做因果推断（实验室只改全局模式兜底，前台应用自己的规则仍可能覆盖它）。模式列表在 WebUI 侧硬编码（`data/lab.ts::LAB_MODE_KEYS`），英文名用明日方舟官方译名，与 rhine-init.yaml 的同步义务见「实验室（rhine）」小节。

- `vite.config.ts` 的 `chiri-embedded-config` 插件在构建期把 `module/config/**`（排除 feature.yaml 与 _-example.yaml，mock 不读取）、`module/rules.yaml`、`src/chiri/_.yaml`嵌入为虚拟模块`virtual:chiri-config`（`files`键为相对仓库根的路径），**仅供`dev/mock-shell.ts`无设备预览取仓库默认值**（新增/删除 yaml 无需改 WebUI 代码；mock 的设备形态由 URL 参数`?soc=chiri|yumi` 指定，配置默认值取自嵌入副本，不要在 mock 里手写配置内容或 SoC 列表）。**嵌入副本不是设备权威值**：meta.yaml（{soc}/meta.yaml）是用户可修改文件，WebUI 真实路径必须经 shell 读`active_config.chr` 指向的设备文件（`contract/` 不引用该虚拟模块），不得用嵌入副本覆盖或展示其设备现状。

- 文件写入用 base64 管道（`printf '%s' <b64> | base64 -d > <path>.webui.tmp && mv -f <path>.webui.tmp <path>`）避免 shell 特殊字符干扰；临时文件后缀固定 `.webui.tmp`（与 daemon 自身的 `<name>.tmp` 区分防互踩），必须经临时文件 + 原子 mv，防止直接 `>` 截断时 config_watcher 读到半截内容导致重载失败；不要用 `echo "${content}"` 拼接。

- 样式分层（2026-09-18 起全面直接复用 ak-ui 官方类）：全局 `app.css` 承担 token 品牌映射、`[fonts]`、`[utilities]`（`u-*` 工具类）、`[signal-map]`（`data-signal`→`--signal` 单点映射）与 `[ak-adapt]`（官方原语的深色/几何适配，**只改颜色与表面，几何/状态/排版用官方**）。组件外壳一律用官方类：`ak-card`/`ak-dialog`/`ak-status`/`ak-segmented`/`ak-choice--switch`/`ak-notice`/`ak-progress`/`ak-gauge`/`ak-form-stack`/`ak-field`/`ak-tag*`；自研只剩 `.btn` 适配器（官方按钮 150×50 固定 + 重阴影 + 硬编码浅色 modifier）与 `.kv`/`.log*`/`.snapshot*`/`.terminal*`/`.nav*`/`.commit*`（上游无对应原语）。**改样式先查 app.css 是否已有**；新增官方类适配进 `[ak-adapt]`，不要在组件里重复画皮。表格内距必须写在 `th/td`（`tr` 的 padding 被忽略）；`.ak-choice` 是 inline-grid，开关组必须包 `.ak-form-stack` 才是一行一个；**`.u-stack` 必须显式 `grid-template-columns: minmax(0, 1fr)`**——网格项默认 `min-width: auto`，nowrap 日志这类宽内容会把整列/整页撑出屏幕（子块自己的 overflow 才是滚动出口）；`.u-scroll` 自带显式细滚动条（WebView overlay 滚动条一滑即隐）；回到底部按钮只在滑离底部时渲染，且用 `.pane{position:relative}` + `absolute` 锚在所属滚动子块内（禁止 `position: fixed` 悬浮）；左上角「关闭」走管理器 `ksu.exit`（`kernel/ksu.ts::exitApp`），仅在缺失该 API 时才 `window.close()` + 历史后退（只做后退会变成「返回上一页」）。`--ak-font-command` 走无衬线系统栈（2026-09-18 用户要求；内置 ChiRi Serif 子集保留在 `--ak-font-serif`、暂无引用）。

- 依赖现状：type-check 走 `svelte-check`（旧 vue-tsc 及其 typescript 5.x 固定约束已随重构移除，现为 ^6.0.3）；`svelte` 5（runes）+ `@yunyoujun/ak-ui` 锁定 1.0.0（CSS Core，`.ak-*` 类名与 `--ak-*` token 属 1.x 契约承诺）。

- 状态页（OverviewView）底部的「导出历史归档」：把 `logd/`（历次重启的日志归档）经 `module/scripts/pack.sh export` 打成 **tar.gz** 到 `/sdcard/Download`（**2026-09-18 起压缩格式为 gzip——用户要求，xz 在设备上太慢**；设备无 gzip 时保留未压缩 `.tar`，UI 如实告知，**不要静默改名**），随附两条提示——不含本次运行正在写的日志、导出期间别关调度/重启设备。进度：tar 带 `-v`，逐文件输出写进 `/dev/chiri_export.progress`，脚本先 `wc -l` 出总数写 `/dev/chiri_export.total`；每轮探测把「已处理行数 / 总数」当百分比（封顶 99）、`.part`/`.tar`/`.tar.gz` 的体积当已归档量一起读回来（一次 exec 读三个数，不为进度多跑一趟）。文件少时百分比粒度粗，所以进度文案同时给「已压缩 X MB」。失败标记（含退出码：3=logd 不可进、4=打包/压缩失败、5=无归档）与进度文件都放 `/dev`（tmpfs 必在）而不是 `/sdcard`。**`/sdcard/Download` 由系统提供，不要 `mkdir` 它**：真不可写（未挂载/只读）时脚本立即写失败标记，前端 1.5 秒内报出而不是干等超时。三条实现约束：① **后台执行**：命令 `nohup sh <脚本> &` 立刻返回，前端 1.5s 一轮轮询（`contract/export.ts::pollExport`，上限 4 分钟），前台 await 会被 ksu 桥超时掐断、看起来也像卡死；② **打包逻辑全部在 pack.sh**（对外暴露的稳定接口，构建流程不得修改——CI 注释剥离已排除 `module/scripts/`；Rust 启动归档与 WebUI 导出共用）；③ **`pack.sh export` 的 dest_base 参数是不带扩展名的基准名**（脚本自己生成 `<base>.tar` 再压成 `<base>.tar.gz`）——调用方若传带 `.tar` 的名字会得到 `logd_X.tar.tar.gz`，轮询永远找不到目标、卡在最后一格且「已压缩 0.0 MB」；`TOTAL` 计数必须与 `tar -v` 同口径（**含根目录条目 `./`，= 文件数 + 1**），否则进度出现 11/10；④ 轮询判定靠三个标记：`.tar.gz` 存在=完成（压缩）、`.tar` 存在且无 `$FAIL.mode`=完成（无 gzip 回退）、`$FAIL` 存在=按退出码分类——`$FAIL.mode` 是本轮压缩中的标记，脚本开始时必须连同旧残留一起清理（否则上次中断的残留会让本轮无 gzip 导出被判「压缩中」直到轮询超时）。
- meta.yaml 是唯一可写配置：草稿 + 保存覆盖 7 字段（language/loglevel/dev_record/fas_enabled/scenemode_enabled/thread_bind/notify），电池读数八项（`power_avg`、`power_max_w`、`oplus_chg`、`oplus_dual_cell`、`voltage_double`、`current_double`、`voltage_divisor`、`current_divisor`）走「电池读数」二级页直写（`state.setPowerAvg` / `state.setBatteryFields`，同样经 `writeMetaFields`，不走草稿）；`contract/meta.ts::writeMetaFields` 做单次读-改-写（`replaceTopLevelField` 顶层行替换、保留注释与其余内容 + tmp→rename；tmp+rename 会产生两个 inotify 事件，故整段一次写入、绝不做多次写入；**可选字段整行缺失时走 `appendTopLevelField` 顶层追加**），写后回读实际值——daemon 对 meta 整文件严格校验（**字段全部可选**：缺字段沿用二进制内嵌默认，只有类型不符/取值不在白名单/未知键才判非法，`deny_unknown_fields`），非法即用内嵌默认整体覆盖；「配置」页交互为草稿 + 保存（`state.svelte.ts::setDraft`），config_watcher 热重载即时生效。name/author 仅展示；`nofix` 只手改。dev mock 走同一条链路（ShellRunner 注入），无 MockBridge 联合类型约束。

- 禁止 WebUI 修改性能模式（rules.yaml 只读）：契约层无任何 rules.yaml 写路径，不提供全局模式切换与为 App 指定模式/删除规则的入口；应用列表仅展示特调/FAS/现存 app_modes 只读标签，提供「重新扫描」按钮（扫描中禁用防并发）。需要调整时改 `module/rules.yaml` 源文件重新编译。

- rules.yaml 只读后的 null 兼容：守护进程侧 `monitor/config.rs` 的 `RulesConfig::app_modes` 仍用 `deserialize_with`（untagged 枚举）显式兼容 null 为空表，历史遗留的 `app_modes: null` 旧文件也不告警。

- 特调/FAS 标签（只读）：`data/whitelists.ts` 解析 `special_tuned.yaml`（仅精确条目，导出文件不含正则）与 `fas_whitelist.yaml`（每行 `包名:配置名`，解析异常回空表），仅 `deviceKind==='chiri'` 时应用列表显示标签、`yumi` 显示「不适用」；FAS 仅白名单驱动、非用户可选模式；当前模式为 fas 时经 `data/mode.ts` 派生展示文案。白名单命中 ≠ 实际生效（`fas_available()` 另有 UI 不可见的条件），界面只陈述已知事实，生效以 `current_mode.chr` 观测值为准。

## [chiri] ChiRi 调度子系统

以下子系统仅在命中 `CHIRI_SOC_HINTS` 的 SoC 上生效。全局统一 schedutil：ChiRi 的 CLG 与 akmode 均把内核调速器写为 schedutil（两套调度器各自 `init_policies` 时写 governor、release 时恢复快照）；Yumi 的 `scheduler/cpu_load_governor.rs` 写 `performance`（Yumi 原有行为，勿改）。

### 特调（akmode）

白名单数据：

- 白名单数据在独立文件 `src/chiri/special_tuned.yaml`，经 `include_str!` 编译进二进制（common.rs 的 `parse_special_tuned` 解析、OnceLock 缓存），用户/WebUI 不可修改。

- 格式：每行 `匹配器:模式列表(逗号分隔):优先回退模式`。匹配器支持精确包名与 `re:` 前缀正则（忽略大小写用 `(?i)`，匹配器内不能含 ':'）；`special_tuned_entry(pkg)` 先精确匹配（文件顺序）、未命中再按正则条目匹配。每项含可用模式列表 `modes` 与优先回退模式 `fallback`（用户未显式配置则采用 `fallback`）。

- 当前条目：明日方舟国服 `com.hypergryph.arknights`、日服 `com.YoStarJP.Arknights`、正则兜底 `re:(?i)arknights`（覆盖台服/Mod 变体），模式均为 `akmode`（fallback 同）；**playback 组**（视频播放，2026-09-17 8550 日志分析新增）：`tv.danmaku.bili` + `re:(?i)(bilibili|danmaku\.bili)`，模式 `playback`。**daily 组已于 2026-09-17 晚上机实测后移除**（桌面/聊天/工具属高频短交互，特调收益低、侵入高——「不是所有程序都需要特调」；移除条目留档在 special_tuned.yaml 注释里，特调只保留长稳态场景）。新增特调模式 = 本表加条目 + `normal/tuned_profiles.yaml` 的 `tuned_profiles` 段加同名参数组，无需改 .rs。

- main.rs 启动时把精确条目导出到运行时文件 `special_tuned.yaml`（每行 `包名:模式列表(逗号分隔):优先回退模式`，正则条目无法按包名精确查找故不导出）并 info 打点。导出仅 `is_chiri_soc()` 下发生，Yumi 设备不生成该文件。

生效范围与门控：

- 特调体系仅限 ChiRi：`determine_mode` 先判 `is_chiri_soc()`，非 ChiRi SoC 上特调映射一律回退全局模式。

- 只在 chiri 的 `Config` 挂载独立特调字段 `akmode`（缺省段：游戏特调兼未注册模式的回退）与 `tuned_profiles: HashMap<模式名, SpecialTunedConfig>`（**特调参数组**，2026-09-17 加：`normal/tuned_profiles.yaml` 的 `tuned_profiles:` 段，`get_tuned_profile(mode)` 按模式名取、缺省回退 `akmode` 段，**全部调用点唯一的取参入口**——不要再加第二个取参方法）；不要注册进 `get_mode`（只认 CLG 常规模式），也不要注册进 yumi 的 `scheduler/config.rs`。

- 白名单应用始终进特调：前台命中 `special_tuned_entry()` 就返回特调模式，不管 app_modes/global_mode 配了什么（`determine_mode` 开头直接判定）。rules.yaml 里给该应用配的普通模式只作为特调起始档（scheduler 侧 `get_ak_initial_tier` 识别）。

- 非白名单应用的模式优先级仍为用户 `app_modes` > `global_mode`；后端门控：非白名单包名映射到特调模式时 warn 并回退 `global_mode`（`app_detect.rs` 的 `determine_mode`）。

调度行为：

- 特调是完全独立调度：`src/chiri/tuned.rs` 的 `TunedGovernor`（akmode / playback / daily 共用）与 CLG 完全解耦。前台为白名单应用时由 `mod.rs` 的 scheduler_ipc 先 `cpu_governor.release()` 再 `ak_governor.init_policies()` 接管，退出前台反向释放。**全部特调模式共用这一套连续控制**（每 40ms 按组内最大占用 × headroom 直接算上限，升频立即执行），差异只在参数组；**`boost_affinity`**（参数字段，默认 true）= 是否走 boost 类亲和（cpuset 收窄 big+prime + core_ctl 保大核常在线）——游戏保响应 true，省电型特调（playback/daily）必须 false：保大核常在线与「贴负载降频」相反（空转漏电），且会把前台重线程钉到大核组被低上限压住。`is_boost_mode` 命中的特调再经 `tuned_boost_affinity()` 二次过滤。**`util_smoothing`**（参数字段，默认 1.0=不平滑）= 决策负载 EMA 系数：抖动负载下「升频立即执行」会被瞬时尖峰反复推高上限（降频又被 hold 拖住），上限均值反而高于带平滑的 CLG（2026-09-17 日志回放证实），playback/daily 用 0.35/0.5 滤尖峰，游戏保持 1.0。**降频计时重置语义（同日修复）**：down_target 只在目标**明显回升**（> down_target + hyst）时重置计时，下探/微调只更新目标——原「档位不等即重置」在抖动下让上限卡高位降不下来（回放中平滑 util 都降不动），连续控制退化成「只升不降」。

- 特调模式下息屏保持 akmode 接管（akmode 已统一 schedutil，息屏随负载自然降频省电）；非特调走 CLG doze → 超时 scenemode（见「息屏省电与屏幕状态」——2026-09 曾短暂暂停后**已恢复**，屏幕状态正常驱动息屏/亮屏切换；旧的「[已暂停] 屏幕状态不驱动调度」描述已删除）。

- **模式家族（2026-09-17 重申）**：CLG = reduce/default/boost（兜底档；2026-09-16 由 powersave/balance/performance/fast 改名，id 与 feature.yaml 段名/rules global_mode/current_mode.chr 同名，显示名走 i18n `mode.*`）；**stardust = scenemode（独立息屏轴，不产生模式值；2026-09-18 起在 UI 家族体系注册 `mode.family.stardust`——不管实际运行中看不看得到都占位，不并入 CLG）**；**down（停摆布尔）是独立家族 `mode.family.down`（kind `'down'`），不属于 stardust**；**rhine = vector/contingency/babel（仅实验室**，rhine.chr 经 global_mode 覆盖驱动）。CLG 档位语义：reduce 较 default 升频更保守（up 0.85）降频更激进（down 0.60、上限压 0.70）；default 为默认兼参考基线；boost 升频更激进（up 0.65）降频略消极（down 0.40、rate_limit 5）、headroom 1.40 给线程/核心更大余量。档位由 rules.yaml 生效模式决定（明日方舟 app_modes > global_mode）；特调期间固定应用；所有档位都能使用硬件最高档位。

- 档位差异仅在升降频策略参数和防抖等待（wait_ms，每档可不同）。核心组区间随命中 SoC 变化，统一在 `common::chiri_core_ranges()`（8550 little 0-2 / big 3-6 / prime 7；8475 0-3/4-6/7；8998 0-3/4-7 无 prime），akmode 与 CLG 触摸升频共用。每组独立 up_core_count/up_util_percent/down_core_count/down_util_percent：核心数为组内绝对个数，yaml 写整数，0 = 组内任一核心命中即触发，写大值如 64 = 关闭该方向判定；占用率写整数百分比，加载时转 0..1。

- 动态限频（schedutil + 负载驱动升降 max）：`TunedGovernor` 激活时写 schedutil、min 压到硬件最低、max 设为硬件最高。`on_load_update`（特调 40ms tick）用当前档位策略参数按核心组判定升降，升频优先：升频 = 任一组内达到 up_core_count 个核心 util > up_util_percent；降频 = 任一组内达到 down_core_count 个核心 util < down_util_percent（达到 = 组内核心数 >= core_count，即配置值就是绝对个数）。统计口径：util 恰为 0.0 的核心（离线与整窗空闲的在线核均为 0.0、不可区分）不计入升频 over、但计入降频 under——空闲即低负载，避免挂机/息屏时永不降频。升频前检查实际频率（scaling_cur_freq）是否已达当前设定的 max（schedutil 余量），达到才在频率表中升一档；降频直接把 max 降为当前实际频率对应档位（`read_cur_freq` 后 `partition_point` 找 <= 实际频率的最高档，实际不可读回退降一档，绝不高于当前 max），max 上下限均为硬件上下限。升降频带 wait_ms 防抖（升降后 `after_change_duration_ms` 内减半）。CLG 与 akmode 同构：min 压硬件最低、只调 max。

- 特调参数独立成文件（仅嵌入 normal/，非处理器绑定）：`module/config/normal/tuned_profiles.yaml` 定义缺省段 `akmode` + 按模式名分派的 `tuned_profiles` 段，经 `common::embedded_tuned_profiles_str()` 编译进二进制，不放在默认 `config.yaml` 里。原处理器目录 `{soc}/akmode.yaml` 绑定已在 4cb4d97 重构中移除（磁盘已无该文件，勿再加回）。嵌入内容解析失败时 `set_special_tuned_available(false)` → 特调不可用、白名单应用回退 CLG（不再保留旧值用默认参数接管 CPU）。

- 特调接管失败冷却：`TunedGovernor::init_policies(mode, cfg)` 返回 `bool`（无可用 cluster 即 false）。scheduler_ipc 在三个特调接管入口（亮屏恢复/ModeChange/ConfigReload）检查返回值，失败时置 `tuned_cooldown_until = now + 300s` 并 warn 打点 `scheduler-tuned-cooldown`，5 分钟内该特调模式不再触发、改由 CLG 接管（冷却中特调模式名保持，息屏 doze/亮屏恢复均走 CLG 分支），冷却结束后经 ConfigReload 或下次 ModeChange 自然恢复重试。

WebUI 侧：

- 设备形态三态：`contract/sources.ts::deviceKind()` 依据 active_config 是否指向处理器子目录判定 `chiri` / `yumi` / `unknown`；读不到时是 `unknown`（界面显示「无法判定」，不替用户下结论）。状态落在 `src/state.svelte.ts`。

- 特调/FAS 在 WebUI 为只读标注：`data/whitelists.ts` 解析白名单，仅 `chiri` 机型显示「特调 / FAS」标签（`yumi` 显示「不适用」）；不提供专属特调模式选项、不做特调清理/重扫修复。应用性能模式已禁止在 WebUI 指定（rules.yaml 只读），应用列表仅展示特调/FAS/现存 app_modes 标签，提供「重新扫描」按钮（扫描中禁用防并发）。

- 当前模式读取与自愈：`contract/sources.ts::readCurrentModeRaw()` 读 `current_mode.chr`（内容为模式名原样字节、无换行无空格）；守护进程启动时写一次、模式切换时写、并常态每 5 秒**无条件**重写一次（两套 scheduler_ipc 各一份实现），因此**不能用 mtime 判断模式变化**；守护进程停止后文件是陈旧值，界面须结合存活探测呈现。文件为空/缺失时界面显示「未知 · 尚未产生模式记录」，与「模式名不在已注册档位内」区分开。

### FAS 帧感知调度（单实例 + 延迟进出，ChiRi 专属）

- 白名单与模式判定：`module/config/normal/fas.yaml` 精确包名→配置名；`determine_mode` 优先级 = FAS 白名单 > 特调 > app_modes > global_mode；rules.yaml 中 "fas" 映射视为非法（`app-detect-fas-rejected` / `app-detect-fas-global-rejected` 告警回退）；`fas_available()` = 白名单非空且至少一个应用配置解析成功（fps_monitor/FAS_FG_UTIL 联动门控见「架构与事件流」）。

- **单实例 + 延迟进出（2026-09-17 重构，原多实例已废弃）**：`src/chiri/fas_manager.rs` 的 `FasManager` 同一时刻至多一个白名单应用被接管（`instance: Option<FasInstance>`）。白名单前台 → `activate`（FasController.load_policies 快照频率 + **GovernorGuard 把各 policy 的 `scaling_governor` 切 performance**，原值快照待恢复）；失去前台不立即退出 → `request_delayed_exit` 进入 **15s 延迟期**（`FasRulesConfig.deactivate_delay_secs`，normalize 夹 1..=600，`fas-example.yaml` 可配）：mode 保持 fas、FAS 仍持有接管（频率停在最后状态、governor=performance），期间切回白名单应用由 activate 无缝续期；到期由 1s 遥测块的 `tick()` 完成退出（reset_all_freqs + clear_game + governor 按快照恢复），并按延迟期记住的目标模式（`pending_mode_after_fas`，fas→X 的 ModeChange 被延迟时记录）重新接管——contingency/babel 走 apply 的 lab 分支，其余走 `apply_mode_takeover`。ModeChange 的 fas→非fas 分支因此**不再立即切换**（拦截在 mode_clone 更新之前，避免「文件写 default、FAS 还持着频率」的中间态）。C5 收尾 deactivate_all（DOWN/panic/进程收尾/fas_enabled=false 热重载）立即全退。事件路由：FrameUpdate/SystemLoadUpdate/温度只喂活跃实例。

- 同模式热切换：新增 `DaemonEvent::PackageSwitch { package_name, pid }`（app_detect 主循环在模式不变且 ChiRi 且前台包变化时发射；Yumi 设备不发射、Yumi 调度器空 arm）。fas→fas 切换序列 = deactivate_active → activate → fas_affinity_hook；另有 1s 遥测兜底（事件丢失/启动即 fas 自愈：非活跃且冷却外且白名单命中 → 三 governor release 后 activate；启动残留 fas 模式且前台非白名单且三 governor 均不活跃 → CLG default 自愈）。兜底块仅亮屏时执行——息屏时 FAS 必须保持释放，否则会把息屏前旧包名拉回 FAS、绕过 CLG doze。

- FAS 模式下 CLG/特调/vector 全部暂停（三 governor release 后接管）；激活失败（load_policies 后无可用 policy）→ 300s 冷却（`FAS_COOLDOWN`，镜像 TUNED_COOLDOWN）+ CLG default 回退；进入 fas 的回退分支也先释放三 governor（可能从特调/极速切入）。

- 移除的调度（FAS 活跃期间豁免）：ChiRi 热保护仅当 `mode=="fas" && fas_mgr.is_active()` 时跳过（fas 模式但实例未活跃——息屏已释放/冷却/初始化失败——照常生效，否则 CLG doze 期间失去热保护；FAS 活跃时温度由 FasManager 每 3s 独立读传感器喂引擎内部限温，endfield 配置 core_temp_threshold=0 即关闭；1s 遥测温度读数保留）；触摸升频不参与（fas 非 boost，`is_boost_mode` 不含 fas）；scenemode 进入判定按 `!fas_mgr.is_active()` 门控（FAS 活跃即不进入——息屏保持接管后 fas 模式天然屏蔽 scenemode，FAS 失效后正常进入）；config_dirty/ConfigReload 的 CLG 分支对 fas 模式守卫（FAS 配置编译期嵌入静态）。

- 息屏释放已完全移除（2026-09，原 C4 doze 更早已废弃删除）：原 `ScreenStateChange(false)` 在非特调分支统一 `ak/vector release` + （fas 活跃时）`fas_mgr.deactivate_active()` → 走 CLG doze → scenemode 全局接管。现 FAS 与屏幕状态完全解耦：**FAS 激活/去激活仅由前台包名驱动**，息屏保持接管（fas 活跃时跳过 doze）、boost 布局全时段生效、1s 兜底与 FrameUpdate 喂帧无 is_screen_on 门控、ModeChange 息屏 fas 特殊分支删除。仅 FAS 失效（初始化冷却/异常）后息屏才走 doze → scenemode：亮屏恢复分支的 fas arm 对失效场景激活 CLG default（唯一恢复路径），1s 兜底在冷却结束后重新激活 FAS（activate 复用保留实例 + apply_freqs，activate 前 release 全部 governor，且若 scenemode 激活先恢复全部在线核）。勿改回「息屏 enter_doze 写低频锁、亮屏 exit_doze 恢复」：锁屏前台无帧事件时 apply_freqs 恢复路径不触发（且被 freq_hold_frames 挡 2 拍），全簇锁死最低频表现为亮屏 0.2fps。

- 线程亲和预留接口：`fas_affinity_hook(affinity_mgr, corectl_mgr, active, fg_pid)` 在 chiri/mod.rs，FAS 激活/去激活时调用；当前职责 = FAS 激活期放开 top-app uclamp.max 为 100（快照还原；boost 进入按机型写入的 85 会钳制 EAS 对重线程的 capacity 视图、抑制 prime 放置，与 FAS 让 prime 承载负载相悖——8475 实测 prime 空转 45%；FAS 激活期 min=max 锁频绕过 schedutil，85 对调频无效仅剩放置负效应；去激活时若 boost 已退出则跳过还原，由 restore 链归位避免把 boost 值泄漏到 normal；8998 内核 4.4 走兜底探测永久跳过），后续 FAS 线程钉核扩展也在此实现，无需再改事件循环接线。**同一「放开 100」机制已与特调（akmode）共用**：方法改名 `set_boost_uclamp_override`，由 `apply_affinity_and_corectl` 按模式同步 uclamp 归属（特调 → `Some(true)` 本函数负责、fas → `None` 让出给本 hook、其余 boost → `Some(false)` 保证不残留 100）；8550 实测 arknights 特调下 big 饱和 47.6% 而 prime 仅 37%，正是 85 抑制 prime 放置的同一现象。

- 调度能力范围（相对 CLG 的扩展仅频率维度）：per-policy min=max 锁频（PolicyController，1.5s 回读校验防内核覆写）+ 容量加权分频 + util 软封顶 + PID/Jank 帧级响应；FAS 自身不做 cpuset/uclamp/core_ctl——线程摆放复用 boost 布局（fas 亮屏入 boost，见亲和章节），uclamp.max 放开经 fas_affinity_hook（特调同机制但经 `apply_affinity_and_corectl` 按模式同步，见上条）。

### CLG 调频语义（动态上限制）

- CLG 不再锁频 min=max，改为 schedutil 动态上限：接管时把 `scaling_min_freq` 一次性压到硬件最低，之后只写 `scaling_max_freq`（CLG 决定性能上限），schedutil 在 \[硬件最低, 上限] 内按瞬时负载自主调频，空闲间隙内核微秒级降频到地板，消除锁频方案"采样间隔（160ms）内频率下不来"的空转发热（8550 实测主要热源）。上限语义下 `perf_floor/perf_ceil/perf_init` 约束的是上限（空闲实际频率由 schedutil 决定），`perf_floor` 允许被热保护击穿。

- 多线程按核心组独立调度：每个 cpufreq policy 由一个独立的 `CoreGroupWorker` 线程管理，持有各自的 `ClusterState`（频率档位、`FastWriter`、`current_perf`、防抖计数器等），线程内自主完成决策 + 写频，核心组之间完全并行无锁。`CpuLoadGovernor`（`cpu_load_governor.rs`）是 Worker 线程管理器：`init_policies` 枚举系统 cpufreq policy、为每个 policy spawn Worker 线程（写 schedutil governor、min 压硬件最低、max 按 perf_init 设初始上限）；`release` 停止所有 Worker（Worker 退出前自行恢复系统原始状态）；`reload_config` 停旧 Worker + 用新配置 spawn 新 Worker（等价轻量 init，current_perf 重置到新 perf_init）。

- scheduler_ipc 通过 `on_load_update(&core_utils)` 将负载数据广播给所有 Worker（非阻塞 `try_send`，通道满则丢弃本 tick），Worker 线程内自主决策 + 写频，无需外部 flush。

- 升频 = 平滑抬高上限：ceiling 提升不强制频率跳变，无需读 `scaling_cur_freq` 做余量检查（旧 `clg-up-skipped` 机制已随锁频语义删除）。降频 = 一步到位降上限：防抖确认（`down_rate_limit_ticks`，极低负载命中 `down_fast_threshold` 免防抖）后直接写目标档（`ratio_of_freq` 同步 current_perf），降 ceiling 只收窄 schedutil 区间、不会把实际频率抬上去。已删除废弃参数 `smoothing_down / slow_down_scale / down_fast_mult`。

- 热切换配置重放 perf_init：`reload_config` 重置 current_perf 到新 perf_init 并立即写频，避免接管间隙 current_perf 掉到 0 后频率要从地板缓慢爬升数秒（息屏 doze/scenemode 切换同样经此热切换恢复，见「息屏省电与屏幕状态」）；模式变更 / ConfigReload 热重载同理。

### Thermal 热保护（ChiRi 专属）

- chiri `Config` 顶层 `Thermal` 段（`ThermalGuardConfig`，`from_file` 时 normalize）。电池温度为主参考、CPU 温度仅极端参考：电池温度节点 `/sys/class/power_supply/battery/temp` 的**原始刻度单位因内核/厂商而异，禁止硬编码除数**——`utils::detect_battery_temp_scale()` 启动时预识别一次，按「唯一落进 5~60°C 合理窗口」的解释定档（0.1°C / 毫摄氏度 / 直读 °C，多解时 tenths 优先），结果经 `OnceLock` 缓存，CLG 热保护（`utils::read_battery_temp_celsius`）与 FAS 温度护栏（`utils::battery_temp_divisor`，FasManager 每次刷新现取、勿再固化）共用同一结论。历史事故：CLG 曾硬编码 /1000 而 FAS 硬编码 /10 两处口径不一致，8550 实测恒读 0.4°C（真实约 20~50°C），电池软/硬限（41/45°C）永不触发、主参考彻底失效；探测不出（节点缺失/读数未就绪）时退化为仅 CPU 温度并打 `clg-thermal-no-battery` / `battery-temp-scale-unknown`。该节点反映整机持续发热且不随游戏瞬时负载抖动，阈值 41/45°C；CPU 温度阈值 90/95°C——soc_max 等合成温区在大型游戏高负载下常态 85°C+，阈值过低（旧值 75/85）会让 8475 实测 84% 时间处于压制、比官调更卡，软件压制只在接近内核 ~95°C 温控起控点时参与。

- 温度输入必须滤波（`mod.rs::TempFilter`，1s snap 块内对原始读数应用）：物理范围门（电池 -10..70 / CPU 5..110）+ 毛刺丢弃（相对上次输出跳变 >10/12°C 视为 soc_max 合成温区切换热点，实测 2s 内 91.5→57.2°C）+ 3 样本中值 + 斜率限制（电池 ±1 / CPU ±3°C/样本）。不滤波会让热保护 cap 以 ~8s 周期在 40/70/100 间 bang-bang 振荡，每次跳变把 current_perf 砸回低位再缓慢爬升，高负载游戏周期性掉帧。

- 压制解除带斜坡（`THERMAL_UNPRESS_STEP=0.15`）：压制加深立即生效，解除方向每采样周期（2s）最多恢复 0.15，8s 级渐进；立即全量解除会让温度马上反弹再触发深压，重现振荡。

- 压制带豁免档 `free_above`（默认 0.80，normalize 保证 >= soft_perf_cap）：Worker flush 中 `current_perf < free_above` 才 `min(cap)` 钳制（cap 允许击穿 perf_floor）。持续高负载经平滑抬升越过豁免档后不再回落钳制，任何温度下都能到达硬件最高频，过热兜底交给系统/内核温控；但压制意图保留：中低负载区间仍被压在 cap 以下，减少发热积累。

- scheduler_ipc 启动时探测一次传感器（CPU `find_cpu_temp_path()` 缺失打点 `clg-thermal-no-sensor` 静默降级），事件循环内每 2s（`THERMAL_CHECK_INTERVAL`）采样双温度，逐传感器三级判定（>= 硬限压 hard_cap（默认 0.40）；>= 软限压 soft_cap（默认 0.70）；回落到 软限-hysteresis 才解除；回滞带内压制中先退软限档防阶跃）后取两者较小值。cap 或豁免档变化时经 `cpu_governor.set_thermal_limits(cap, free_above)`（f32 bit pattern 存 `AtomicU32`，启动即同步一次豁免档）下发，debug 打点 `clg-thermal-cap`（含电池/CPU 温度，缺失显示 "-"）。

- 仅作用于 CLG 接管的模式，fast/akmode 不受影响。

### 触摸升频（事件驱动，ChiRi 专属）

- `touch_detect.rs` 线程读 `/dev/input/event*`（`libc::poll` + 解析 64 位 `input_event` 24 字节，`BTN_TOUCH==1` 或 `ABS_MT_TRACKING_ID>=0` 判定触摸按下），触摸按下时经独立事件通道 `mpsc::sync_channel::<()>` 发送事件（非阻塞 `try_send`）。

- scheduler_ipc 每次醒来先 drain `touch_rx`，收到事件即 `on_touch()` 更新共享 `AtomicTouchState`（f32 性能比 floor 以 bit pattern 存入 `AtomicU32`，窗口时长与写入时刻毫秒数各存一个 `AtomicU32`），并广播空负载包唤醒全部 Worker（经 Worker 负载通道 `try_send(Vec::new())`，recv_timeout 即时返回、Worker 只 flush 不决策）。大核 Worker 在本次 flush 中读取共享状态、把性能上限抬到 `touch_boost_floor`，不等待下一个 160ms 负载决策 tick（触摸延迟 ≈ `EVENT_POLL_MS=100ms` + 唤醒 flush，不含 Worker tick 间隔）。

- 配置项 `touch_boost_enabled/ms/tiers` 每模式独立，`enabled=false` 即关闭（normalize 会把 ms 置 0）。

- 屏蔽系统触摸升频：`chiri/scheduler.rs::apply_disable_touch_boost` 写 0 到 `/sys/module/cpu_boost/parameters/` 的 `input_boost_enabled / sched_boost_on_input / input_boost_ms / boost_ms`（按存在性尝试，无节点静默跳过）；`start_scheduler_thread` 启动时即调用一次 `apply_system_tweaks()`。

### 息屏省电与屏幕状态

- scenemode（息屏超时省电，2026-09 曾短暂暂停后恢复）：chiri `Config` 的 `scenemode`（Mode 段，默认 `enabled:true`、`perf_ceil` 极低封顶、`up_threshold=1.0` 不主动升频）与顶层 `scene_mode_delay_secs`（默认 300s=5 分钟）。`mod.rs` 息屏时记录 `screen_off_at`，`SystemLoadUpdate` 分支在息屏超时且**非特调、无任意 FAS 实例**（`fas_mgr.has_any_instance()`，活跃与后台保留实例都算存在——CLG 绝不能接管 FAS，保留实例 60s TTL reap 后自动放行）时一次性把 CLG 热切到 scenemode（scenemode 未启用则 release 回系统默认），亮屏自动恢复原模式。**进入 scenemode 时同步应用离线核**（`CoreCtl.scenemode_offline` 门控，见 core_ctl 章节：**小核+大核全开常驻低频**（频率上限由 scenemode CLG 配置压制）+ prime 下线 + **专用小核独占**给调度服务 + 抑制 boost），CPU 侧待机功耗大幅下降；**常驻簇（小核∪大核）持续顶满上限则饱和退回 powersave 并 300s 冷却**。

- 息屏 doze 天花板 0.30：`ScreenStateChange(false)` 生成的 doze 配置 `perf_ceil` 钳到 0.30（原 0.40），配合动态上限后后台突发（sync/JobScheduler）借力受限、空闲间隙照常降到地板频，压制"口袋发热"；5 分钟后 scenemode 进一步压到 0.15。特调与 FAS（活跃时）息屏保持接管、跳过 doze（见 FAS 小节）。

- **FAS 息屏省电已完全移除（2026-09，删除而非暂停）**：FAS 与屏幕状态完全解耦——① `ScreenStateChange(false)` 不再释放 FAS（fas 活跃时息屏走"保持接管"分支，与特调同语义；仅 FAS 失效后走 doze）；② 亮屏恢复分支的 fas arm 对 **FAS 失效场景激活 CLG default**（300s 冷却期内 1s 兜底被 `fas_cooldown` 门控短路，此处是退出 doze/scenemode 低性能配置的唯一恢复路径；FAS 活跃时空操作——CLG 绝不能接管 FAS 已接管的 CPU）；③ ModeChange 息屏进入 fas 的特殊分支删除；④ fas 全时段 boost（`apply_affinity_and_corectl` 不再按 screen_on 回退 normal）；⑤ FAS 兜底激活与 FrameUpdate 喂帧的 is_screen_on 门控删除；⑥ **FAS 优先于 scenemode**：兜底激活 FAS 前若 `scene_mode_active` 先退出 scenemode（恢复全部在线核，守卫「FAS 实例存在 ⇒ 非 scenemode」不变量），scenemode 入口按 `has_any_instance()` 门控。`scheduler-fas-screen-release` i18n key 已随代码删除。

- cpuidle governor 默认启用 menu：8550/8475/8998 的 `config.yaml` 将 `CpuIdleScalingGovernor` 置 true、`CpuIdle.current_governor` 置 "menu"（menu 按预期空闲时长选最深 C 状态，降低空闲功耗），内核无 menu 时写入失败静默跳过、无副作用。

- 屏幕状态自愈校验：uevent 可能漏报/误报（开机早期背光未就绪、长时间息屏后唤醒、netlink 缓冲溢出，或机型根本不广播屏幕类 uevent——现代内核已无 early_suspend/late_resume power uevent，leds/backlight 亮度变化多数驱动也不发 KOBJ_CHANGE），导致 `screen_state_arc` 锁死在错误状态。`screen_detect.rs::verify_screen_state` 由 app_detect 主循环每轮调用，读检测源 sysfs 校正 arc。**检测源按可靠性优先级自动扫描并缓存**：① `/sys/class/backlight`（bl_power/actual_brightness 三分支：`bl_power==0`→亮（FB 权威亮屏信号）；非 0 **不可信**——部分 DRM 面板驱动息屏写 FB_BLANK 后亮屏不清零，以 `actual_brightness>0` 为准；bl 不可读回退亮度。勿改回「bl_power==0 即亮、非 0 即灭」的两分支）；② `/sys/class/leds/*backlight*`（MTK 等无 backlight class 机型，`brightness>0` 即亮——**此前这类机型屏幕状态完全读不到的主因**，2026-09 修复）；③ `/sys/class/graphics/fb0/blank`（0=亮）。源发现/无源/读失败各 info/warn 打点（`screen-detect-source-found` / `screen-detect-no-source` / `screen-detect-read-failed`），「屏幕状态读不到」时从 daemon.log 即可确认设备实际可用的源；缓存源连续 8 次全不可读时退役并切换下一个候选（见下方两阶段判定）。

- **两阶段息屏判定 + 节点退役 + 恒亮屏兜底（2026-09）**：① 正常情况只读主检测节点（省开销，`SCREEN_SOURCE` 缓存）；② 主节点报**息屏**且 arc 为 ON（翻转尝试）时触发全节点投票（`tally_screen_nodes`：fb0/blank + 全部 backlight + 全部背光 leds，跳过已退役节点，各源读数统一走 `read_screen_state`；**读数不可得的节点不计票、也不否决**）——**两个有效节点报息屏即确认**（`OFF_QUORUM=2`；有效节点只有一个时按一票算，否则机型只暴露一个节点就永远进不了息屏；一个有效读数都没有时不改判）；有节点报亮屏却凑不齐息屏票才不确认（warn `screen-off-vetoed`，每 episode 一条）；节点退役/切换**延迟 15s**（`INCONSISTENCY_SWITCH_DELAY`，`INCONSISTENT_SINCE` 计时器）：单次不一致可能是瞬时毛刺，**持续不一致 15s 才退役主节点**切换下一个候选（切换时 warn `screen-detect-node-switched`，新主节点重新计时）；息屏票够即确认，同时清掉不一致计时与驳回计数；**稳态（无翻转）快速返回不扫描**。③ 节点缺失/持续不可读（8 次 ≈8~12s）→ 同样退役并直接切下一个节点。④ **全部候选退役（耗尽）**→ **error 级打点 `screen-detect-nodes-exhausted`**（比 warn 高一级，提示所有节点均不正确或矛盾）+ 进入**恒亮屏模式**（`ALWAYS_ON`）：不再读取任何节点、不再接受息屏事件，arc 强制校正为 ON（若原为 OFF 会经 app_detect 转发 ScreenStateChange(true) 恢复亮屏安全态），verify 直接返回零 IO。⑤ **稳态息屏定时复核**（`LAST_OFF_REVIEW` 计时器，每 `OFF_REVIEW_INTERVAL`=10s 一次）：翻转复核只盖「亮→息」瞬间，主节点失真（息屏不清零/读数卡死）会让 arc **永久滞留 OFF**——scenemode/prime 下线无法退出，亮屏使用中超大核异常离线；复核发现「息屏票凑不齐且至少有节点报亮屏」→ 退役主节点切换（同样走 15s 持续不一致门控）+ 强制改判亮屏；票型仍是息屏（例如 2 OFF + 1 ON）则只清计时器，不受单个亮屏节点扰动。所有息屏事件生产者（verify 自愈、power/backlight/leds uevent）共用 `update_state_if_changed`，策略天然全覆盖。**复核/否决扫描必须在拿 arc 锁之前**（耗尽路径 enter_always_on 经 update_state_if_changed 写 arc，持锁会死锁）。仲裁口径（2026-09-18 用户要求「两个节点报息屏即确认」）取代了旧的「任一节点亮屏即驳回」：单个失真节点（如 `bl_power` 陈旧非零）不再挡住息屏，代价是两票同时失真会误判息屏。代价权衡仍是刻意的：false-ON 只损失节电，false-OFF 会让息屏机制在亮屏期间误触发卡死设备；恒亮屏兜底（宁可不节电，不可误判息屏）不变。

- 屏幕事件双源直推：`monitor_screen_state_uevent` 收到 power（early_suspend/late_resume）、backlight KOBJ_CHANGE 或 leds backlight KOBJ_CHANGE（仅认名字含 backlight 的 leds，跳过通知灯/按键灯）且状态确实变化时**直接 send `DaemonEvent::ScreenStateChange`**（纯推送），不再依赖 app_detect 轮询转发；app_detect 的 verify+轮询转发保留为 uevent 漏报时的自愈兜底。双源可能对同一次屏幕切换各发一次事件，两套调度器的 ScreenStateChange 分支开头都有 `screen_on == is_screen_on` 去重守卫（状态未变只打点）——新增屏幕事件生产点时必须维持该守卫。

- **驳回 episode 退役（2026-09-11 补丁）**：15s 连续不一致门控存在死锁——主节点自身失真（息屏仍报亮，如某 8550 面板 panel1-backlight）时，verify 每轮读到 ON 都会清 `INCONSISTENT_SINCE`，15s 永远攒不满 → 节点永不退役、arc 永久钉死 ON（实测整夜 0 条 screen,off、scenemode 全程未进入 → 大核带电+小核上限全开）。现增加 `VETO_EPISODES` 计数兜底：每 60s 最多记 1 次「息屏被驳回」episode（`VETO_EPISODE_MIN_GAP` 防同屏连发 uevent 重复计数），1800s 窗口（`VETO_EPISODE_WINDOW`）内累计 ≥3 次（`VETO_RETIRE_EPISODES`）即退役主节点；ON 读数**不**清零，仅全节点一致 OFF 或退役换节点时复位（防连锁误退役逐个耗尽候选）。

- **scenemode 长息屏兜底（2026-09-11 补丁）**：进入门槛 `standby_max >= SCENEMODE_SAT_UTIL(0.75)` 在后台常驻负载下会整夜拒绝进入（实测小核 util 长期 60-70%、峰值触 0.75）。现息屏时长 ≥4×`scene_mode_delay_secs` 时绕过负载门槛进入 scenemode——短息屏仍按原门槛防「进→10s 饱和退出→300s 冷却」拉锯；门槛的防拉锯语义只对短息屏成立。

### 极速模式（fast）

- vector 档用专属锁频器、不读 yaml、停用 CLG：`src/chiri/fast.rs` 的 `FastLock` 与 CLG 完全独立，vector 档下由 `mod.rs` 的 scheduler_ipc 先 `cpu_governor.release()` 再 `fast_lock.init()` 接管（**六个入口都不能漏**：启动 / 亮屏恢复 / ModeChange / ConfigReload / DOWN 退出 / panic 自愈——vector 不注册 CLG 参数，漏掉就是频率零接管且无任何告警）。

- `FastLock::init()` 遍历 `get_cpu_policies()`、快照原始状态、写 schedutil governor、把所有 cluster 的 `scaling_min_freq/scaling_max_freq` 都锁到含 boost 的硬件最高频（min=max=hw_max）；`tick()` 每 5 秒重写一次 hw_max 防止系统/厂商守护进程篡改；`release()` 恢复接管前的 governor/min/max。

- `mod.rs` 事件循环中 `fast_lock.tick()` 在每次 `recv_timeout` 唤醒时调用；模式切换/息屏 doze/亮屏恢复/看门狗超时/panic 收尾均正确 release fast_lock。

- 8550/8475/8998 的 `feature.yaml` 没有 `vector:` 段（该档由 `fast_lock` 硬锁最高频、不读 CLG 参数；共享 `config/feature.yaml` 里有 vector 段，是代码默认值的来源）；akmode 已改为**无档位连续控制**（见 `normal/tuned_profiles-example.yaml`），不受影响。

### CPU 亲和与线程迁移（Affinity，ChiRi 专属）

- `src/chiri/affinity.rs` 的 `AffinityManager`（消费 `SysPathExist` 已探测但此前无人使用的 cpuset/cpuctl 能力位）。

- boost 类模式（boost/vector/特调，判定口径统一在 `chiri/mod.rs::is_boost_mode`；fas 模式亮屏在 `apply_affinity_and_corectl` 调用点同样并入 boost——FAS 只管调频、线程摆放沿用 boost 布局，息屏 FAS 释放后回退 normal，与 akmode 息屏保持 boost 不同）下：`/dev/cpuset/top-app`、`foreground` 的 `cpus` 收窄到大核+超大核（区间用 `common::chiri_core_ranges()`，不硬编码）；`background/system-background/restricted` 压到小核。

- 可选写 `/dev/cpuctl/top-app/cpu.uclamp.min`（配置 `Affinity.top_app_uclamp_min_pct`，默认 0 关闭——uclamp.min 会让 schedutil 独立于 CLG 抬频，避免与动态上限语义打架）。

- 可选写 `/dev/cpuctl/top-app/cpu.uclamp.max`（配置 `Affinity.top_app_uclamp_max_pct`，任务级性能上限钳制、EAS 原生感知，比 scaling_max_freq 硬顶更细）。按机型配置：8550/8475 配 85，8998 内核 4.4 配 0 关闭。运行时内核版本识别兜底纠正：`kernel_version()` 解析 `/proc/sys/kernel/osrelease`，< 5.3（uclamp 主线引入版本）或节点缺失或写入回读无效（防厂商半成品 backport 静默忽略）任一不满足即置 Unsupported 永久跳过并 warn 打点 `affinity-uclamp-unavailable`，判定结果缓存避免重复探测。

- **后台降权（2026-09-18 加；配置 `Affinity.background_uclamp_max_pct`，默认 50，0 = 关闭）**：写 `background` 与 `restricted` 两组的 `cpu.uclamp.max`（**刻意不含 system-background**——系统后台含媒体/音频等服务，画中画、后台播放等可感知场景保守跳过），把后台任务的 util 需求钳低（50 = 512）——EAS 放置与 schedutil 频率随之回落、优先落小核，**不禁止使用大核**（空闲时仍会被 EAS 调度上去）。boost/normal 两态在 `apply` 汇合点持续写（「始终压低后台、给 UI/视频让路」），幂等；`release()`/关闭总闸时写回内核默认 `max`。目标形态：UI（top-app/foreground 收窄 + 线程钉核）与视频（playback 特调 / 前台进程天然在 foreground）优先，后台运算/内容计算只做软降权。与下一条后台 promote 的关系：promote 仍会把「忙」后台线程送 big（能效兜底），但其 uclamp 需求已被压住，占核权重有限——如需彻底禁止后台升核，把 promote 门槛收紧或加开关（未做）。

- 线程层（按核心粒度放置，`pin_foreground_threads=true` 启用，每 2s 再平衡一轮、前台 PID/boost 变化立即触发）——**开销控制优先，不做逐线程每轮读 stat**：
  - 前台（fg_pid 由 app_detect 提供）：每轮 1 次 `read_dir /proc/<pid>/task`，仅对**新增**线程读一次 stat 判定关键/建档；存量线程的 home 合法性仅用缓存的逐核 util 与在线位图判断（合法范围 = prime ∪ big 全部性能核，含溢出落点）。**主池/溢出两级选核**（`pick_core_pref`）：关键线程（tid==pid 或 comm 命中 RenderThread/GLThread/GameThread/UnityMain/UnityGfxDeviceW 白名单）主池 = prime、溢出 = big（超大核全被钉才回落大核，UnityMain 等白名单线程直接绑定超大核）；普通线程主池 = big、溢出 = prime（**大核钉满后自动启用超大核**，修复"多个重负载挤单一大核而超大核闲置"）。boost + 亮屏才钉，否则恢复全核；已消失线程下一轮即清理释放钉核计数。
  - **default（非 boost）轻量前台保护**：8550 实测日常 default 模式下后台压小核 + EAS 把前台任务也堆在小核，little p50=88~99% 排队而 prime p50≈0% 空转、大核半闲（后台 promote 只救后台组线程，前台关键线程无人救援）。little 高水位（`LITTLE_HIGH_WATER` 0.70，迟滞 <`KEY_BIND_RELEASE_WATER` 0.50 解除，boost/息屏强制解除）时把前台关键线程组绑定到 big∪prime（reason `normal_press`），不钉单核、不占钉核计数。**非关键前台线程按窗口 util 升核**：压力窗口内每轮限量采样（`FG_SCAN_WINDOW` 32，游标轮转，窗口外零采样零写入），连续两窗 util ≥ `FG_BUSY_UTIL_PCT`(30%) 绑到 big∪prime（reason `normal_busy`），跌到 `FG_BUSY_RELEASE_UTIL_PCT`(15%) 以下连续 `DEMOTE_STREAK` 轮才回落——绑定/释放水位构成滞回，避免 5~30% 的中等负载线程长期滞留性能核反而抬高功耗（勿把释放水位改回 `DEMOTE_UTIL_PCT` 的 5%）。组绑定状态用单一枚举 `GroupBind`（None/Key/Busy）表达，绑/放只改一个字段，勿改回 `group_pinned + busy_bound` 两个需手工同步的布尔量。两窗忙判定由 `busy_window_update` 供前台/后台共用，勿各写一份；boost 布局接管时无缝衔接（掩码一致）。

  - 后台动态亲和（**不把后台全压小核**——小核过载能效灾难）：忙线程 promote 到 big、回落 demote。候选 TID 从三个后台 cpuset 的 `tasks` 文件读取（**不做 /proc 全量枚举**），每 2 轮刷新、按游标分片每轮只深扫 64 个；窗口 util = ticks 差分，**两窗防抖**（上次采样忙且本次仍忙、期间采到低负载即清标记）即 promote，不依赖采样间隔。promote 先把 TID 移入 top-app（cpuset v1 按 TID 记账）再按当前核心占用选核钉定（`pick_core_pref`：big 有未钉核只看 big、钉满溢出 prime，与前台普通线程同口径——游戏场景 big 是关键线程主场，后台忙线程不挤占但也不排队）。已 promote 线程每 2 轮复查：util 连续 3 次 < 5% → demote；线程仍忙（≥5%）且所在核心 util > 70%（`CORE_OVERLOAD_UTIL`）→ 换低占用核（`bg_overload`，带 4s 迁移防抖）。过载重钉统一口径（前台 `home_overload` / 后台 `bg_overload` 共用）：**候选 = 全性能池（big∪prime）最低分核 + 双滞回**（分数差 ≥ `OVERLOAD_MARGIN` 且目标核 util 严格低于 home）+ 反跳回冷却（`RETURN_COOLDOWN` 16s 禁回刚迁离核）。旧「big 池内选核 + 目标核 util ≤ 70% 硬门槛」在整体高载时必然静止——8475 实测 FAS 下大核 86-90% 排队、prime 空转 45%、26 分钟 85 次 `overload_hold`；全员过载时把负载摊向低载核（含 prime）仍有收益，乒乓由防抖+冷却兜底。**后台迁移仅亮屏**。

  - 在线核位图每 4 轮读一次并缓存（核热插拔不频繁）；devimp core 行每 2 轮一次。

  - **多应用快速切换**：每轮再平衡开头按 pid 归属立即清理旧前台线程（`pid>0 且 ≠ 当前 fg_pid` → 解钉恢复全核），不等 30s 失联——否则旧应用转后台后（Android 会短暂把它留在 top-app/foreground cpuset）其单核掩码与大核相交继续生效，8550 仅一颗 prime 会让新旧前台关键线程同核互踩，连续切换还会令 core_pinned 计数漂移累积。过滤器幂等、零文件 IO；后台 promote 线程（pid==0）不受影响。模式变化的切换经 ModeChange → force 立即重平衡；同模式切换（无事件）依赖 2s 周期块发现，钉核延迟 ≤2s（可接受，切换清理在同一轮完成，新前台钉核时 core_pinned 已准确）。

  - 选核算法：score = 逐核 util（最近 SystemLoadUpdate 快照，即核心当前占用）+ 钉核计数 × 0.4（`PINNED_WEIGHT`，权重过弱会让 util 快照滞后时第二个重线程挤入"看似空闲"的核造成同核轮转卡顿），取候选 ∩ 在线核最低分核（离线核 util 恒 0.0、与空闲不可区分，钉到「唯一允许核为离线」会让线程有效可运行集为空而冻结，必须排除）；home 核过载（核 util > 70%，`CORE_OVERLOAD_UTIL`，前后台共用）或 home 离线在 **8s** 重钉防抖（`REPIN_DEBOUNCE`，promote/demote 保持 4s `MIN_MIGRATE_INTERVAL` 不影响负载响应）窗口外重钉，选回同核不写避免空迁移。

  - 掩码用 `mem::zeroed::<cpu_set_t>()` 分配保证对齐（`vec![0u8]` 强转是未对齐指针的脆弱实践），`libc::CPU_SET` 置位并带容量边界防护。

  - 稳态（前台线程集不变、后台空闲）单轮 ≈ 1 read_dir + 0\~64 stat + 低频辅助文件读——相对「逐线程每轮全读 stat」约省 >50% 文件 I/O。

- 黑名单（全部编译嵌入、不外透）：`src/chiri/affinity_blacklist.yaml` 经 include_str! 打包（common.rs 的 `parse_affinity_blacklist` 解析、OnceLock 缓存），格式：每行一条精确进程名/包名或 `re:` 前缀正则（同特调白名单语法，含 com.example 注释示例），文件内含**系统进程默认名单**（system_server/surfaceflinger/logd/lmkd/zygote 等，勿删）；内置兜底：空 cmdline（内核线程）与 `/` 开头（native 二进制路径）一律黑名单。命中进程全部线程保持全核、不做任何迁移，`is_affinity_blacklisted` 另在逐线程层面兜底（防应用进程内的系统服务线程被误迁）。

- normal/doze 布局：top-app/foreground/uclamp 恢复快照，后台保持压小核 + bg uclamp 降权持续（两态都写）；boost 退出/开关关闭/调度线程收尾 `release()` 全量还原（已钉线程恢复全核，经 `demote_tid_group` 写回进程当前 cpuset 组）。同模式 App 切换不发 ModeChange 事件，靠 scheduler_ipc 2s 周期刷新兜底重迁移（manager 内部按 KIND/PID/boost 去重）。

- 配置段 `Affinity`（enabled/top_app_uclamp_min_pct/top_app_uclamp_max_pct/pin_foreground_threads/background_uclamp_max_pct），三份 SoC yaml 已带（背景降权默认 50；root feature.yaml 无 Affinity 段、走 serde 默认）。

### core_ctl 核心在线接管（ChiRi 专属）

- `src/chiri/core_ctl.rs` 的 `CoreCtlManager`，**三态状态机**（None/Boost/Scenemode，`set_power_state(boost, scenemode)` 统一入口，内部去重可被 2s 周期安全调用；切换时先退出旧状态恢复快照再进入新状态）。调度线程收尾 `release()` 按当前状态恢复。**NONE 与 BOOST 稳态都重试 `offlined` 残留核**（每 2s）——只重试 NONE 的话，scenemode 退出恢复失败后用户随即进入 boost（fas/performance 全时段 boost），prime 会整场游戏保持离线（超大核异常离线实测根因之一，2026-09 修复）。

- **Boost**：把各 cluster 的 core_ctl `min_cpus` 抬到全组常在线（防厂商热插拔与 ChiRi 调频打架），退出恢复快照；只动 min_cpus 不动 max_cpus/busy 阈值。

- **Scenemode 离线核（[已暂停] 息屏深度省电）**：`CoreCtl.scenemode_offline` 门控（8550/8475 true，8998 内核 4.4 默认 false）。进入 scenemode 时先解除 boost（min_cpus 抬着会让厂商 core_ctl 重新拉起被下线的核——两者互斥由 `apply_affinity_and_corectl` 保证），下线目标由 `scenemode_targets()` 计算：**小核 + 大核全开常驻**（频率上限由 scenemode CLG 配置统一压制），**仅 prime 整簇下线**消除空转漏电流；逐核写 online=0 回读验证，失败跳过（warn），已在 offlined 中的核防重复登记。**独占一颗小核给调度服务**（编号最大的 little——三步实现：① `affinity::exclude_core_from_cpusets` 把该核从全部业务 cpuset 组（top-app/foreground/background/system-background/restricted）的 cpus 移除，其他进程/新进程（继承组掩码）均不可调度到该核；② 自身全部线程移入 cpuset 根组（根组含全部在线核，sched_setaffinity 才不会被原组掩码二次过滤）；③ 全线程自钉到该核）；设备无 /dev/cpuset 时降级为仅自钉。维持期每 2s 纠偏：重新下线被外部拉起的核 + `exclude_core_from_cpusets` 重写被框架 CpusetManager 加回保留核的组（快照只记首次原始值，防框架中间值覆盖）。**退出恢复 `restore_online` 失败的核必须保留在 offlined 中由 STATE_NONE 分支每 2s 重试**——此前失败即 clear、状态机回 NONE 再无重试路径，写回被内核拒绝的核永久离线；全部恢复后才释放独占（cpuset 快照还原 + 自身线程移回原组 + 解除自钉），重新下线前先把残留核拉回在线（否则 online=0 被跳过记录、核永远失去恢复登记）。

- **scenemode 饱和退出**：常驻簇（小核∪大核）max_util 持续 10s ≥ 70%（`SCENEMODE_SAT_UTIL/SECS`，util 是忙时占比与频率无关，饱和即真饱和）→ 视为后台负载压不死常驻核：一次性退回 reduce 的 CLG 配置 + 立即恢复全部在线核 + 释放独占小核，并进入 **300s 冷却**（`SCENEMODE_COOLDOWN`，期间 scenemode 入口被门控不得重进，防反复拉锯）；冷却结束后息屏条件仍满足则自然重进。

- 为什么选核排除离线核而不"按需唤醒"：唤醒大核要拉电压轨/重建 L2，为后台线程点亮大核净亏能；直接写 online 会与厂商热插拔守护进程打架（对方再下线，ping-pong）。需要更多在线核时的正确姿势是抬 core_ctl min_cpus（Boost 态）。scenemode 是唯一反向使用 online 写入的场景（目标恰恰是让 prime 睡死，小核+大核常驻保住待命响应）。

- cluster 发现：遍历 `get_cpu_policies()` → related_cpus 首个 CPU 的 `/sys/devices/system/cpu/cpuN/core_ctl`（每 policy 一份，天然去重），惰性枚举一次；无节点打点 `corectl-unavailable` 后保持空表（scenemode 直接 sysfs 离线**不依赖** core_ctl 节点，仅受独立配置门控）。

- 配置段 `CoreCtl`（enabled/scenemode_offline）。

### 遥测数据源（Telemetry，ChiRi 专属）

- `src/monitor/telemetry.rs` 的 `telemetry_loop` 线程（1s 轮询，仅 `is_chiri_soc()` 时由 monitor/mod.rs 启动）读 PSI（`/proc/pressure/{cpu,io,memory}` 的 some avg10）、GPU busy%（`/sys/class/kgsl/kgsl-3d0/gpu_busy_percentage` → MTK `/sys/kernel/ged/hal/gpu_loading`，缺失为 None）、电池电流/电压。

- 电池读数来源（2026-09-18 重构，用户口径）：**默认走 Android 标准节点**（`/sys/class/power_supply/battery/{current_now,voltage_now}`，ABI = µV/µA）；meta `oplus_chg`（默认 false）打开后**优先读 OPlus 私有节点** `/sys/class/oplus_chg/battery/bcc_parms`（逗号分隔，**0 基下标**：下标 6 = 电芯0电压、8 = 电流、11 = 电芯1电压，即第 7/9/12 个字段；字段按 mV/mA 解读、×1000 存成与标准节点一致的 µV/µA 口径），节点不存在或字段缺失才回退标准节点（打点 `telemetry-oplus-bcc` / `-bcc-missing` / `-bcc-unusable`）。**旧的量级启发式（mV/V、mA/A 自动识别）与物理范围门（2–6V / ±30A）已删除**——单位不匹配交给「单位校准」，代码不再猜。OPlus 内核的标准 power_supply 节点约每 10s 才刷新，1s 精度采样必须绕开（私有节点存在的理由）。
- 双电芯与单位校准（同一批 meta 字段）：`oplus_dual_cell`（默认 false，私有节点路径）电压取「下标 6 + 11」之和；`voltage_double` / `current_double`（默认 false，**仅标准节点路径**）分别把电压 / 电流 ×2（双电芯机型上标准节点常只报单节/单芯值）——**与 `oplus_chg` 互斥**：`chiri::config::Config::load` 推给遥测层时强制 `!oplus_chg && x`，WebUI 打开私有节点时在同一笔写入里清掉这两项并置灰（`state.setBatteryFields` 内联合并，避免「私有开 + 倍压还开着」的中间态被热重载读到）。`voltage_divisor` / `current_divisor`（各默认 1000，须 > 0）：**全链没有内置换算**（2026-09-18 用户要求「去除所有内置校准」），`节点原始值 ÷ 校准值` 直接就是输出——电压得 V、电流得 mA，`batt_power_w` 仍是 |mA| × V。校准值随节点单位填：**标准节点（µV/µA）1e6 / 1000，私有节点 bcc_parms（mV/mA）1000 / 1**（私有节点路径原先的 ×1000 归一与标准节点路径的 ÷1000（旧 ABI 步）都已删除，两张基准不再强行统一，全部由这两个值决定）——`Telemetry::batt_voltage_v` / `batt_current_ma` 两个访问器执行，`batt_power_w` 仍是 |mA| × V（W = V×A 保持自洽）。**分开的理由**：节点的电压与电流未必同时错单位，共用一个值会让功率按平方变化、也说不清是谁的锅。旧键 `unit_divisor`（曾把两者合一）仍接收，等价于只设电压校准（`MetaYamlFile` 保留该字段 + `parse_disk_meta` 取 `.or(...)`，避免老文件被 `deny_unknown_fields` 判非法整份重置）。六个字段都是「出现即校验、缺省沿用内嵌默认」，经 `telemetry::set_battery_options` 落到进程级原子量（1s 遥测线程与各消费点读它，不每轮解析 YAML）。

- 结果写入进程级共享原子量 `telemetry()`（f32 bit pattern 存 `AtomicU32`，与热保护/触摸状态同口径），不占事件通道容量。

- eBPF 扩展探针（`yumi-ebpf/src/main.rs` 的 `handle_sched_wakeup`/`handle_sched_migrate_task`/`handle_cpufreq_transition`，PerCpuArray 计数）由 `cpu_monitor.rs` 仅在 ChiRi 上可选挂载（内核缺 tracepoint 时 warn 一次跳过，不影响主探针），每 2s 读累计值取增量发 `DaemonEvent::BpfStats`（Yumi 设备不发送；Yumi scheduler match 里的空 arm 仅为枚举完备性）。

- chiri scheduler_ipc 以 `TELEMETRY_LOG_INTERVAL=1s` 消费：写 `logs/status.csv`（logger.rs `status_log_snapshot`，1s 一行，功耗精度 1s）+ 20s 一条 debug 摘要 `telemetry-summary`。BpfStats 不刷新 CLG 看门狗心跳（探针失效不影响负载源判定）。

- BCC 失效可见性（2026-09-11）：`bcc_parms` 下标 6/8（电压/电流）缺失或非整数时此前 `?` 静默回退标准节点，「BCC 从未生效、功耗列一直来自 10s 缓存节点」完全不可见（8550 实测 batt_current_ma 99% 在 ±5、74% 为 0）。现 warn 一次 `telemetry-bcc-unusable`（去重）后回退，便于从 daemon.log 确认功耗列口径。

## [hard] 硬性约束

- 不修改 CI 构建流程（`.github/workflows/build.yml`），除非明确要求。

- eBPF 程序目标为 `bpfel-unknown-none`，改动需确保 CI 环境交叉编译通过。

- 保持 KernelSU/Magisk 模块规范兼容（`module/` 目录结构、`service.sh` 启动流程）。

- Yumi 逻辑冻结：不修改 `src/scheduler/` 与默认 `module/config/config.yaml` 的调度逻辑与参数行为（Yumi 即将废弃）；确需修复时先与 ChiRi 对齐、最小改动。共享层（`src/monitor/`、`main.rs`）如需为 ChiRi 适配，必须保持 Yumi 运行时行为不变（常规采样 200ms 原值勿改）。

- 不允许在未经许可的情况下主动创建git提交

- 所有任务除例外外都需要调用 /token-efficient-coding 这个SKILL，如果找不到这个SKILL则终止任务并提示

## [lessons] 经验教训

- 含 `std::ops::Range<usize>` 字段的结构体别 `derive(Copy)`：CI（`-Z build-std` + nightly）编译 `common::CoreGroupRanges` 时报 E0204（字段不实现 Copy），即便标准库中 `Range<usize>` 实现了 Copy。按值 move 或显式 `clone()` 即可，用 `#[derive(Debug, Clone)]` 够了，不要加 Copy。

- 日志文件被删不能崩：`src/logger.rs` 用自实现 `SelfHealingAppender`（替换 log4rs `RollingFileAppender`），每次写入按路径 `create+append` 重新打开，`daemon.log` 被外部删除会自动重建；循环轮转与锁上锁全程 `Result`/剥除 poison，绝不 `unwrap`。别改回 log4rs 滚动追加器。

- FAS 帧投喂必须全量：fps_monitor 的 `latest_frametime()` 单条投喂会把 100ms 窗口内 6 帧（60fps）丢 5 帧，所有「帧数」常数（confirm/cooldown/decay 阈值、freq_hold_frames）的时间尺度被拉长 ~6 倍，PID/防抖/jank 全面钝化（平均帧↓、1%Low 崩塌）；且无新帧时同一条陈旧 delta 被重复投喂，静态 UI 上一条 heavy 帧 ~1s 即可刷出假 loading（perf 钳 0.60~0.70，纯 UI 卡顿）。现按 ProbeState.pending 队列逐帧 try_send 全量投喂，通道满时丢弃（EMA 可容忍，绝不阻塞 fps_probe）。

- FAS governor 快照/恢复：load_policies 把每 policy 的 scaling_governor 改写为 performance 配合 min=max 锁频，PolicyController 必须在 new 时快照接管前 governor、reset()（deactivate 路径）时恢复——否则 performance 泄漏给后续 CLG/akmode/系统调频，所有模式功耗拉满、低负载降频失效（用户实测「FAS 用一次之后功耗不降」的根因）。

- FasManager 复用路径不显式 reset_runtime（其为 pub(super) 引擎内部方法）：依赖引擎 app_switch_gap 语义——去激活时已 clear_game，复活后首个真实帧的帧间隔跨越失前台时长、必然超过 app_switch_gap_ms，引擎 handle_early_exit 内部自动 reset_runtime 并落 app_switch_resume_perf；手动补 reset 反而破坏该语义。

- 看门狗崩溃自愈 + 进程模型：`service.sh`（开机）/`action.sh`（手动启动）用 `setsid sh -c ... &`（setsid 不可用时回退 `nohup sh -c ... &`）拉起统一定义的看门狗循环，启动时把自己 PID 写入 `logs/watchdog.pid`。**两个分支都必须 `&` 后台化**——setsid 前台执行会阻塞拉起脚本直到 daemon 退出（看门狗 while 循环存活期永不返回，action.sh 卡在 stopped 之后、"daemon restarted." 打不出来的根因），且脚本退出时向作业补 `disown` 防 SIGHUP。循环内 `"$DAEMON"` 在前台运行，进程以任何方式结束（正常退出 / panic 展开退出 / 被信号杀）都会返回并重启；仅当存在卸载标记 `$MODDIR/.uninstalling`（`uninstall.sh` 开头 touch）或主进程二进制被删时才 `break` 退出。**崩溃退避（2026-09-15）**：每轮用 `date +%s` 量 daemon 存活时长，退出用时 <60s 判为异常短命（启动即崩），sleep 按 3→10→30→60s 递增封顶，活过 60s 或 `date` 不可用时回到 3s——此前固定 `sleep 3` 在启动即崩时会形成重启风暴（日志/IO 放大）。**daemon 侧哪些失败能触发看门狗重拉**（2026-09-15 核查）：main 线程 panic、monitor_core panic（main 末尾 `join().unwrap()` 把 panic 带进 main）、chiri 调度线程连续 panic >`SCHEDULER_IPC_RESTART_MAX`(5) 次后的 `exit(1)`、日志门限 `exit(0)` 都终止进程；**监控子线程**（screen/config/pid/fps/cpu/telemetry，经 `monitor/mod.rs::spawn_guarded` 派生）panic 后落盘并 `exit(1)`——此前它们是游离线程，panic 只死线程、进程照活，看门狗永远感知不到（fps 死后 FAS 无帧、CLG 超时退回原生调频）；正常返回（通道关闭等）不受 spawn_guarded 影响。仍未覆盖的是「daemon 硬挂（死锁/死循环）」：看门狗只监控进程存活，无存活超时探测。旧写法 `"$1" || exit 0` 在 yumi 崩溃（返回非 0）时会直接让看门狗退出、无法自愈，禁止复用。`service.sh` 开头的清理必须先按 `logs/watchdog.pid` kill 旧看门狗再 `killall chiri`（脚本被重新执行——模块热更新/管理器重载——时只 killall 会留下旧看门狗，3s 后与新看门狗各拉起一个 daemon，形成双实例并行写两份 devimp/status 日志）；`action.sh` 已有同口径清理。

- **热更新必须先杀看门狗再复制二进制，且复制结果必须显式校验**：`customize.sh` 热更新只 killall chiri 不杀看门狗时，看门狗的 3s 周期复活可落在 killall 与 `cp` 之间的窗口——`cp` 覆盖**运行中的可执行文件**会 `ETXTBSY` 失败，且 `cp -r ... 2>/dev/null` 把它静默吞掉：模块目录残留旧版本，热更新后手动重启调度（action.sh 拉的是 live 目录的二进制）仍跑旧版，直到重启设备被 KSU 的 `modules_update` 覆盖才固化。修复四件套：① 复制前按 `logs/watchdog.pid` 杀旧看门狗；② killall 统一 `-9` 并轮询确认 daemon 退出（pidof，≤5s，D 状态进程对 SIGKILL 也要等 IO 返回）；③ `core/bin/chiri` 单独 `cp` 并校验退出码，失败恢复配置备份、重启旧版服务保持调度连续并明确告知；④ **`MODDIR` 赋值后必须 `export`**——安装器环境可能已把 MODDIR 导出为 staging（modules_update），仅局部赋值子进程看不到，service.sh 的 `[ -z "$MODDIR" ]` 会继承错位路径（看门狗从 staging 拉起 daemon、日志/pidfile/锁写入 staging，KSU 清理后调度静默死亡）。顺带修正 `chmod 755 "$MODDIR/yumi"` 的错误路径（二进制在 `core/bin/chiri`）。**热更新路径（成功与失败）都必须以 `exit 1` 结束**——exit 0 会让 KSU 把模块归为「待更新」，重启前 Action/WebUI 全部禁用（脚本 MSG_HOT_UPDATE_HINT 注明的预期行为）。

- WebUI「关闭调度」而非重启：`contract/daemon.ts::stopScheduler` 先按 `logs/watchdog.pid` kill 掉看门狗（防止其把主进程再拉起），再 `killall -9 yumi`（缺失时回退 pkill），实现彻底停止调度；恢复需点击模块 Action（`action.sh` 手动启动）或重启设备。不要在 ksu.exec 里用 `nohup ... &` 后台拉起——`ksu.exec` 返回会清理执行 shell 的进程组，直接拉起会被一并杀掉；要启动调度一律调用模块自身的 service.sh/action.sh（内含 `nohup`，且 disable_boost 幂等）。

- 模块热更新必须 `exit 1` 收尾：customize.sh 的热更新分支把新文件直接 cp 进已安装模块目录 `/data/adb/modules/chiri` 并重启服务后，必须轮询确认 daemon（`pidof`/`pgrep yumi`，最多 \~6s）存活，然后无条件 `exit 1` 按报错退出。若 `return 0` 让安装器继续"完整安装"，会再覆盖一遍模块目录并写 update 标记，管理器随即识别为"模块更新"提示重启、隐藏 WebUI/Action；`exit 1` 使安装器按失败中止（清理暂存目录、不碰已热替换的目录），管理器不感知更新。管理器显示"安装失败"是预期行为，脚本内已双语提示；服务未启动时同样 exit 1 并提示重启设备走完整安装。

- 没有要求或引用的情况下默认视所有log和csv分析文件都是过时的、错误的、具有误导性的，不能参考或用于分析。但是如果引用或指出了需要参考文件夹则需要查看里面的所有文件，无论你认为有没有必要。

- **暂停功能用 `// [PAUSED]` 注释入口/触发点而非删除**：需求方明确「功能以后大概率还要加回来」时适用（实例：息屏节电 doze + scenemode，2026-09 暂停后同日恢复，恢复 = grep "PAUSED" 解开标记块）。变量声明随触发点一并注释、调用点实参保留恒值以防扩散改动；若需求变为「完全移除」（如 FAS 息屏省电），则删除代码并同步清理 i18n key 与文档，不留注释残骸。

- **shell 命令读文件前必须守卫**：`wc -c < 缺失文件`（或任何 `< file`）的输入重定向错误由 shell 直接打到 stderr——命令上的 `2>/dev/null` 覆盖不到，真机 mksh 会把整条命令判为失败（实例：导出进度轮询把「产物尚未生成」误报成「查询导出进度失败」）。统一写法：`[ -f path ] && …` 守卫再读，变量给初值 0；WebUI 侧对「非零退出但输出可用」宽容处理。

- **整文件覆盖（Write）前必须确认目标不是已跟踪文件**：本轮为嵌 WebUI 资产直接整文件重写了已有 `build.rs`，丢掉 eBPF 构建与 `assert_required_configs` 断言——cargo check 静默通过（ebpf 占位产物已存在、断言消失不会报错），靠 diff 审查才抓回。写整文件前先 `git status` 该路径 / 读原文件；向既有文件加功能一律局部 Edit。cargo/类型检查通过 ≠ 构建脚本语义完整。

## [maint] AGENTS.md 维护要求

每次对话结束前，回顾本次会话内容，评估是否需要更新本文件：

- 新增/删除/重构了模块、目录或关键文件 → 更新「目录结构」

- 引入了新的依赖、工具链或构建命令 → 更新「技术栈」「常用命令」

- 确立了新的代码约定或踩坑经验 → 更新「代码约定」（可新增「经验教训」）

- 文件描述与实际代码不符 → 立即修正

没有需要沉淀的变化时跳过，但必须经过评估。
