# ChiRi Cursor 环境记忆

> 按环境分文件夹写入：Cursor 的产出只写 `.cursor/`。历史长期事实（工具链口径、守护进程契约）
> 在 `.codebuddy/memory/MEMORY.md`，只读，开工必读。

## 协作约定

- 一切 AI 产出文件（计划、评估、报告等）只写项目文件夹内：本环境文档放 `.cursor/docs/`、记忆放 `.cursor/memory/`；
  工具自动写到项目外的副本（如 Cursor plans 目录）复制回项目后删除原件。
- 架构约定、契约数字、命令口径变更同步 `docs/agents/` 与本文件；具体改动经过写 `.cursor/memory/YYYY-MM-DD.md`。
- CI 跳过口径：`build.yml` 的 `check-skip` 闸——提交信息含 `skip ci`（不分大小写、无需方括号）即跳过该次构建；
  `[skip ci]` 等方括号标记另走 GitHub 原生整 run 跳过；`workflow_dispatch` 手动触发恒运行；提交信息仅提及该字样也会命中。

## 进行中

- daemon-perf-opt 五阶段性能优化计划：`.cursor/docs/daemon-perf-opt-plan.md`
  （Phase 3 前置 `.trae/specs/harden-scenemode-and-screen-detect` 的 hold 三分支）。
