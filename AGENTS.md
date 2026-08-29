# AGENTS.md

> 上次更新时间：2026-08-29

本文件为 AI 编程助手在本仓库工作时提供指导。

## 项目概述

**yumi** 是 Android CPU 智能调度控制系统（Magisk/KernelSU 模块），核心为 Rust 守护进程，通过 eBPF 采集 CPU 调度事件与渲染帧数据，结合 PID 控制 FAS 帧感知调度与 CLG 动态调频。

- 目标平台：Android 8.0+ / AArch64 / Root
- 许可证：GPL-3.0-or-later
- 版本：`module/module.prop` 与 `Cargo.toml` 同步

## 核心架构

```
src/                  # Rust 守护进程
  monitor/            # 监控层：app_detect / fps_monitor / cpu_monitor / screen_detect
  scheduler/          # 调度层（Yumi）：FAS 引擎、CLG 调速器
  chiri/              # Chiri 专用调度器（特定 SoC）
yumi-ebpf/            # eBPF 探针（独立 workspace）
module/               # Magisk/KernelSU 模块载体
  config/             # 配置文件 + 处理器子目录
webui/                # Vue 3 管理界面
```

技术栈：Rust (nightly) + tokio + aya(eBPF) + Vue 3 + TypeScript + Vite

## 常用命令

```bash
cargo xtask build                                    # 完整构建
cargo +nightly check -p yumi --target aarch64-linux-android  # 静态检查
cd webui && npm run dev                              # WebUI 开发
cd webui && npm run type-check                       # WebUI 类型检查
```

**重要**：

- 本地开发只做 `cargo check`，不要 `cargo build`（需 NDK）
- eBPF 是独立 workspace，勿加回根 members
- Windows 下检查需用 `aarch64-linux-android` 目标

## 代码约定

1. **架构**：Monitor 通过 mpsc 事件通道解耦，调度器双套架构（Yumi/Chiri）按 `is_chiri_soc()` 选择
2. **配置**：走 `config.yaml` + `rules.yaml`，支持热重载，按处理器独立配置
3. **日志**：调试用 `debug!`，高频路径周期摘要，状态变化即时打点
4. **i18n**：新增文案必须中英文双语
5. **WebUI**：通过 kernelsu bridge 交互，文件写入用 base64 管道 + 原子 mv
6. **特调白名单**：`SPECIAL_TUNED_MODES` 编译进二进制，仅 ChiRi SoC 生效

## 硬性约束

- 不修改 CI 构建流程
- eBPF 目标 `bpfel-unknown-none`，确保交叉编译通过
- 保持 KernelSU/Magisk 模块规范兼容

## 维护要求

每次会话结束前评估是否需要更新本文件（目录结构、技术栈、代码约定等）。若无需变更则跳过。
