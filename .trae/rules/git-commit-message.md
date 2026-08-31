---
alwaysApply: false
description: commits rule
scene: git_message
---
# ChiRi 提交信息规范

基于 Conventional Commits 格式。

## 基本格式

```
<type>(<scope>): <subject>

[optional body]

[optional footer(s)]
```

## 提交类型（必填，可使用未列出的类型）

| 类型       | 说明   | 使用场景                                        |
| ---------- | ------ | ----------------------------------------------- |
| `feat`     | 新功能 | 添加新特性、新模块、新能力                      |
| `fix`      | 修复   | 修复 bug、错误、异常行为                        |
| `docs`     | 文档   | README、注释、AGENTS.md 等文档更新              |
| `style`    | 格式   | 代码格式调整、空格、分号等不影响逻辑的改动      |
| `refactor` | 重构   | 既不修复 bug 也不添加功能的代码变更             |
| `perf`     | 性能   | 提升性能的代码改动                              |
| `test`     | 测试   | 添加或修改测试                                  |
| `build`    | 构建   | 构建系统或外部依赖变更（xtask、CI、Cargo.toml） |
| `ci`       | CI     | CI 配置文件和脚本变更                           |
| `chore`    | 杂项   | 其他不修改 src 或 test 的改动                   |
| `revert`   | 回滚   | 回滚之前的提交                                  |

## 作用域

### 主要作用域（必填，可使用未列出的作用域）

| 作用域      | 说明                                        |
| ----------- | ------------------------------------------- |
| `chiri`     | ChiRi 调度模块（src/chiri/）                |
| `scheduler` | Yumi 调度模块（src/scheduler/，通常不改动） |
| `monitor`   | 监控层（src/monitor/）                      |
| `ebpf`      | eBPF 探针（yumi-ebpf/）                     |
| `config`    | 配置相关（module/config/）                  |
| `webui`     | WebUI 界面（webui/）                        |
| `xtask`     | 构建脚本（xtask/）                          |
| `module`    | Magisk/KernelSU 模块载体（module/）         |
| `ci`        | CI 配置（.github/workflows/）               |

### 子作用域（可选，可使用未列出的作用域）

| 作用域           | 说明            |
| ---------------- | --------------- |
| `chiri/akmode`   | 特调模式        |
| `chiri/clg`      | CLG 负载调速器  |
| `chiri/touch`    | 触摸升频        |
| `chiri/fast`     | 极速模式锁频器  |
| `chiri/scene`    | 息屏场景模式    |
| `monitor/fps`    | FPS 监控        |
| `monitor/cpu`    | CPU 监控        |
| `monitor/app`    | 应用检测        |
| `monitor/screen` | 屏幕状态检测    |
| `config/8550`    | 骁龙 8550 配置  |
| `config/8475`    | 骁龙 8475 配置  |
| `config/8998`    | 骁龙 8998 配置  |
| `config/rules`   | rules.yaml 规则 |
| `webui/bridge`   | 内核通信层      |
| `webui/store`    | 状态管理        |
| `webui/config`   | 配置页面        |

## 主题行规则

- 祈使语气（"添加" 不是 "添加了"）
- 末尾不加标点
- 必须使用中文，部分名词可用英文（如 "balance"、"CLG" 等）

## 正文/页脚（可选）

- 正文：说明动机和背景，每行不超过 80 字符
- 页脚：`BREAKING CHANGE:`、`Closes #123`、`Refs #456`

## 常见错误

- ❌ `fix: 修复了 bug` → ✅ `fix(module): 修复守护进程重启失效`
- ❌ `feat: 新功能` → ✅ `feat(chiri/touch): 添加触摸升频支持`
- ❌ `update config` → ✅ `config(8550): 调整平衡模式参数`

## 其他规则

- 使用 /humanizer-zh skill 检查文本是否符合中文规范
