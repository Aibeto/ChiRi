# 项目方向沉淀计划：ChiRi 主线 / Yumi 即将废弃勿动

## Summary

把用户确立的项目方向约束沉淀进 [AGENTS.md](file:///e:/code/ChiRi/AGENTS.md)：

- **ChiRi 是发展主线**（自 imacte/yumi fork，README "Based on imacte/yumi"）；
- **Yumi 即将废弃，但作为基础保留，勿动其逻辑**；
- 同时审计工作区现有改动，确认未违反该约束。

本次为**纯文档沉淀 + 审计**，不修改任何代码逻辑。

---

## Current State Analysis

1. **AGENTS.md 项目概述仍是 Yumi 主视角**：L10 开篇 "**yumi** 是一个 Android CPU 智能调度控制系统…"，未体现 ChiRi 主线和 Yumi 废弃方向。
2. **目录结构主次未标明**：`src/scheduler/`（默认 Yumi CLG）与 `src/chiri/`（Chiri 专用）并列，未标注"Yumi 废弃 / 勿动"。
3. **README 已是 ChiRi 主视角**（用户已改）：L3 "# ChiRi 调度"，L310 "Based on imacte/yumi"——README 与 AGENTS.md 表述不一致。
4. **上一轮已铺垫的约束**：`注意只动 ChiRi，yumi 不要动` 已通过采样参数化落实（Yumi 恢复 200ms 原值），本计划把该原则正式沉淀为项目规则。

---

## Proposed Changes

### 1. [AGENTS.md](file:///e:/code/ChiRi/AGENTS.md) 项目概述
- 在现有概述后补充一段方向声明：
  "**项目方向**：本仓库为 **ChiRi**（自 imacte/yumi fork，Based on imacte/yumi），**ChiRi 是发展主线**（8550 等 Chiri 目标 SoC，调度位于 `src/chiri/`）；**Yumi 调度（`src/scheduler/`）即将废弃**，作为 ChiRi 的基础保留，**勿改动其逻辑**——新功能、调优一律落在 `src/chiri/` 与处理器配置 `module/config/{soc}/`。"

### 2. [AGENTS.md](file:///e:/code/ChiRi/AGENTS.md) 目录结构
- `src/scheduler/` 标注 "（Yumi 调度，即将废弃，作为 ChiRi 基础保留、勿动逻辑）"；
- `src/chiri/` 标注 "（ChiRi 调度，发展主线）"。

### 3. [AGENTS.md](file:///e:/code/ChiRi/AGENTS.md) 硬性约束（新增一条）
- "**Yumi 逻辑冻结**：不修改 `src/scheduler/` 与默认 `module/config/config.yaml` 的调度逻辑与参数行为（Yumi 即将废弃，作为 ChiRi 基础保留）；确需修复时先与 ChiRi 对齐、最小改动。共享层（`src/monitor/`、`main.rs`）如需为 ChiRi 适配（如采样参数化），必须保持 Yumi 运行时行为不变（Yumi 常规采样 200ms 原值勿再改动）。"

### 4. [AGENTS.md](file:///e:/code/ChiRi/AGENTS.md) 代码约定 1（双套架构）
- 在"调度器为双套架构"句末补充主次关系：ChiRi 为发展主线、Yumi 即将废弃且逻辑冻结。

### 5. [AGENTS.md](file:///e:/code/ChiRi/AGENTS.md) 时间戳
- 更新"上次更新时间"。

### 6. 审计（不落文件，结论写入最终回复）
- 复核 `git diff`：`src/scheduler/` 与默认 `module/config/config.yaml` 零改动；共享层改动（cpu_monitor/main/monitor 采样参数化）保持 Yumi 200ms 原值，属"为 ChiRi 适配且不动 Yumi 行为"，符合约束。

---

## Assumptions & Decisions

1. "勿动其逻辑" = 不改 `src/scheduler/` 与默认 `config.yaml` 的调度逻辑与参数行为；共享层为 ChiRi 适配时须保持 Yumi 行为（Yumi 200ms 原值），上一轮已如此落实。
2. 本次仅文档沉淀 + 审计，不涉及代码/配置/yaml 改动。
3. README 已是 ChiRi 主视角，本次不动 README（避免与你已编辑的内容冲突）。

## Verification

1. `git diff --stat`：确认本次仅 AGENTS.md 改动；`src/scheduler/`、默认 `module/config/config.yaml` 无改动。
2. 人工核对：AGENTS.md 概述/目录结构/硬性约束/约定 1 均体现"ChiRi 主线、Yumi 废弃勿动"。
3. 纯文档改动，无编译影响。
