# AGENTS.md

> 上次更新时间：2026-08-29
> 每次会话结束后按文末「AGENTS.md 维护要求」评估，若本文件发生变更，请同步更新上面的日期，便于快速判断是否过时。

本文件为 AI 编程助手（Cursor / Claude Code / Trae 等）在本仓库工作时提供指导。

## 项目概述

**yumi** 是一个 Android CPU 智能调度控制系统（Magisk/KernelSU 模块），核心为 Rust 守护进程，通过 eBPF 内核探针采集 CPU 调度事件与渲染帧数据，结合 PID 控制 FAS 帧感知调度与 CPU 负载调速器（CLG）动态调频。

- 目标平台：Android 8.0+ / AArch64 / 需要 Root
- 许可证：GPL-3.0-or-later
- 版本：见 `module/module.prop` 与 `Cargo.toml`（需保持同步）

## 目录结构

```
src/                  # Rust 守护进程主代码
  monitor/            # 监控层：app_detect / fps_monitor / cpu_monitor / screen_detect
  scheduler/          # 调度层（默认 Yumi）：FAS 引擎、CLG 负载调速器
    fas/              # FAS 核心：PID 控制器、帧率档位、frame_pipeline
  chiri/              # Chiri 专用调度器（特定 SoC 触发；复制自 scheduler/，待定制）
  common.rs / fas_types.rs / i18n.rs / logger.rs
yumi-ebpf/            # eBPF 探针（bpfel-unknown-none，build-std 编译）
xtask/                # 构建脚本（cargo xtask build 完成编译打包 zip）
module/               # Magisk/KernelSU 模块载体（module.prop、customize.sh、service.sh）
  config/             # config.yaml / rules.yaml / i18n (en.ftl / zh.ftl)
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

# WebUI 开发
cd webui && npm install && npm run dev

# WebUI 类型检查
cd webui && npm run type-check
```

- 本地开发通常只做 `cargo check` / WebUI `type-check` 验证；完整产物由云端 CI（GitHub Actions）生成。
- 不要随意执行 `cargo build`（需要 NDK 环境），优先静态检查。
- 完整构建的隐藏依赖：`build.rs` 的 `ensure_bpf_linker` 依次复用「PATH 中已有 bpf-linker」→「OUT_DIR 缓存」→ `cargo install bpf-linker` 兜底。CI 通过 GitHub API 解析官方 release asset 直接下载静态链接 LLVM 的预编译二进制（bpf-linker 0.11 依赖 LLVM 21+，源码编译在 ubuntu runner 上不可行；cargo-binstall 的 fallback 也会回退到源码编译）。eBPF 的 release 编译在 build.rs 内用 `CARGO_PROFILE_RELEASE_OPT_LEVEL=2` 局部覆盖（新版 bpf-linker 内嵌 LLVM 已移除 `-Oz`/`-Os`，仅支持 `-O0~O3`，workspace 根的 `opt-level="z"` 会导致链接失败）。

## 代码约定

1. **架构**：Monitor 线程组通过有界 mpsc 事件通道（`DaemonEvent`，`sync_channel` 容量 64，满时 send 阻塞形成背压）解耦数据采集与调度决策，新增监控/调度能力遵循此模式。前台 PID 由 `monitor/mod.rs` 的单一 `pid_watcher` 线程经 `tokio::sync::watch` 广播，FPS/CPU 监控共享消费，不要各自重复轮询。FPS 帧监控（`fps_monitor`）仅服务于 FAS 调频，FAS 禁用期间不启动（见 `mod.rs` 中注释块）。**调度器为双套架构**：默认 `scheduler/`（Yumi CLG）与 `chiri/`（Chiri 专用）二选一，main.rs 按 `common::is_chiri_soc()`（特定 SoC 列表命中）决定启动哪一套；两套互斥消费同一事件通道，Monitor 层共享。`CHIRI_SOC_HINTS` 在 common.rs 维护，新增机型只追加列表，不要绑定单一型号。
2. **日志**：调试与排障优先使用 `debug!`（勿全部用 info 污染信息日志）；频率控制/帧处理等高频路径用 25-tick / 60-frame 周期摘要，状态变化（模式、PID、屏幕、attach、档位）即时打点。新增日志 key 必须同时补充 `module/config/i18n/zh.ftl` 与 `en.ftl`，key 命名 `模块-描述`。
3. **配置**：运行时配置默认走 `module/config/config.yaml`（CLG/模式参数）与 `rules.yaml`（FAS/模式映射参数），支持热重载；新增配置项需同步更新反序列化结构体与默认值。配置由 main 启动时解析一次并以 `Arc<RwLock<Config>>` 共享给对应调度器（勿重复读取），热重载由该调度器的 config_watcher 覆写同一共享实例。两套调度各持自己的 Config 类型（`scheduler::config` 与 `chiri::config`）。**按 SoC 独立配置**：命中 `CHIRI_SOC_HINTS` 且存在 `config_{命中片段}.yaml` 时，经 `common::get_config_path()` 优先加载该独立文件，否则回退 `config.yaml`；所有加载/热重载入口（main.rs、两套 config_watcher）必须统一走 `get_config_path()`，勿硬编码路径。守护进程启动时把生效配置文件名写入 `active_config.txt`，WebUI 据此读写同一份文件。
4. **i18n**：守护进程日志基于 Fluent（`module/config/i18n/en.ftl` / `zh.ftl`），WebUI 基于 `webui/src/i18n/locales/`；新增用户可见文案必须同时提供中英文。
5. **Rust 风格**：release profile 为极致体积优化（`opt-level = "z"`, lto, strip），避免引入重依赖；优先复用现有依赖（serde/anyhow/log/tokio/nix 等），新增第三方库选择社区高星、维护活跃的 crate。
6. **资源占用敏感**：守护进程运行于 Android 后台，注意内存分配（避免频繁 Vec 分配）、锁粒度和线程唤醒次数。
7. **版本同步**：发版时同步更新 `module/module.prop`（version/versionCode）、根 `Cargo.toml`（version）、`updateInformation/update.json` 与 `changelog.md`。
8. **WebUI**：与守护进程通过 kernelsu bridge 交互（见 `webui/src/utils/bridge.ts`），勿直接硬编码路径；读取配置文件前先读 `active_config.txt` 确定实际生效文件。文件写入用 base64 管道（`echo '<b64>' | base64 -d > path.tmp && mv -f path.tmp path`）规避 shell 特殊字符解释，**必须经临时文件 + 原子 mv**，避免直接 `>` 截断时被守护进程 config_watcher 读到半截内容导致重载失败；不要用 `echo "${content}"` 拼接。

## 硬性约束

- 不修改 CI 配置的构建流程（`.github/workflows/build.yml`），除非明确要求。
- eBPF 程序目标为 `bpfel-unknown-none`，改动需确保可在 CI 环境交叉编译通过。
- 保持与 KernelSU/Magisk 模块规范的兼容（`module/` 目录结构、`service.sh` 启动流程）。

## AGENTS.md 维护要求（重要）

**每次对话结束前，必须回顾本次会话内容，评估是否需要更新本文件：**

- 新增/删除/重构了模块、目录或关键文件 → 更新「目录结构」
- 引入了新的依赖、工具链或构建命令 → 更新「技术栈」「常用命令」
- 确立了新的代码约定、架构决策或踩坑经验 → 更新「代码约定」（可新增「经验教训」小节）
- 发现本文件描述与实际代码不符 → 立即修正

若本次对话未产生需要沉淀的变化，可不更新，但必须经过此评估步骤。
