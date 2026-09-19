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
  chiri/              # ChiRi 调度（发展主线；特定 SoC 触发；含 CLG、akmode 明日方舟特调、fas_manager FAS 帧感知调度、touch_detect 触摸升频、affinity 按核亲和/线程迁移、core_ctl 核心在线接管、scheduler.rs 一次性系统调整（cpuidle/IO/cpu_boost/内核 Sched 参数，含 DOWN 停摆的快照与还原））
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

