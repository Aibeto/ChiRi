# AGENTS.md

> 上次更新时间：2026-09-20

本文档为 AI 编程助手（Cursor / Claude Code / Trae 等）在本仓库工作时的指导文件。
2026-09-20 起按块拆分：本文件只作**索引引导**，正文位于 `docs/agents/` 子块文件。

> ** workBuddy 和 CodeBuddy 协作文件已统一到 `.codebuddy/`（2026-09-13）**
>
> - `docs/agents/*.md`：本文件拆分出的**指导正文**（原 AGENTS.md 各区块，Grep `\[tag\]` 定位，勿通读全文）。
> - `.codebuddy/memory/`：跨会话记忆，注意它是一个**目录**而不是单一文件——
>   `MEMORY.md` 放长期事实（就地更新、保持精简），`YYYY-MM-DD.md` 是按日期追加的工作日志（只追加不重写）。
> - `.codebuddy/docs/`：计划、评估等 AI 产出文档（`mdocs/` 只放项目原有文档，勿把 AI 产出放进去）。
> - 旧的 `.workbuddy/` 已废弃合并，其下只剩重定向说明文件，不再写入任何内容。
>   凡属架构约定、契约数字、命令口径的变更，`docs/agents/` 与 `.codebuddy/memory/` 都要同步；
>   具体某次改动的经过只写当日日志。
>   **开始工作前可以考虑互相阅读**：读本文件的同时，考虑读 `.codebuddy/memory/MEMORY.md` 与最近几天的 `YYYY-MM-DD.md`。

## 区块索引

| 区块 | 子块文件 | 内容 |
|---|---|---|
| `[overview]` `[tree]` `[stack]` `[cmds]` | [docs/agents/01-overview.md](docs/agents/01-overview.md) | 项目概述、目录结构、技术栈、常用命令 |
| `[convention]` | [docs/agents/02-convention.md](docs/agents/02-convention.md) | 代码约定：架构事件流 / 日志 / 配置嵌入与热重载 / 实验室 / i18n / Rust 风格 / 版本发布 / WebUI |
| `[chiri]` | [docs/agents/03-chiri.md](docs/agents/03-chiri.md) | ChiRi 调度子系统：特调 / FAS / CLG 语义 / Thermal / 触摸升频 / 息屏省电 / 极速 / 亲和迁移 / core_ctl / 遥测 |
| `[hard]` `[lessons]` | [docs/agents/04-hard-lessons.md](docs/agents/04-hard-lessons.md) | 硬性约束、经验教训 |
| `[maint]` | [docs/agents/05-maint.md](docs/agents/05-maint.md) | 本文档维护要求（每次会话结束前评估更新） |

快速定位：`Grep '\[tag\]' docs/agents/` 打到具体子块文件再读对应小节，勿通读全文。