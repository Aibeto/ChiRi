# chiri-webui

ChiRi 内核调度模块的 WebUI（KernelSU / Magisk 模块管理界面）。
Svelte 5（runes）+ TypeScript + Vite + ak-ui（CSS Core），无 UI 框架依赖。

## 分层

```
ksu.exec → kernel/shell（可注入） → contract（契约层） → data（数据层） → views（界面）
                  ↘ dev/mock-shell：无设备时的替身（浏览器预览 / 自动化走查）
```

| 目录 | 职责 |
| --- | --- |
| `src/kernel/` | KernelSU 桥（`ksu.ts`）与可注入的 `ShellRunner`（`shell.ts`） |
| `src/contract/` | 与守护进程的接口：路径与安全校验、三分类读取、meta 读写、存活探测、关闭调度 |
| `src/data/` | 纯解析：status.csv（21 列）、daemon.log、白名单、rules、模式派生、应用标签 |
| `src/views/` | 四屏：状态总览 / 配置 / 应用与规则 / 日志 |
| `src/components/` | 面板、状态占位、开关、分段切换、确认弹层 |
| `src/i18n/` | 界面文案（界面语言与 daemon 日志语言互不影响） |
| `tests/` | 契约层与数据层的纯逻辑断言（vitest，可脱离真机运行） |

## 脚本

```sh
npm install          # 安装依赖
npm run dev          # 本地预览（无 ksu 时自动装载设备替身）
npm run build        # 产出 dist（xtask 拷进模块 webroot/）
npm run type-check   # svelte-check
npm test             # vitest
```

## 与守护进程的契约要点

- **存活判据**：`daemon.lock` 的 flock 探测（`flock -n FILE true`，取锁后立刻释放）；不用 `pidof`。
  绝不能删除 `daemon.lock`（flock 是 inode 级，删除会让新实例另起 inode 加锁 → 双实例）。
- **只读接触点**：`active_config.chr`、`current_mode.chr`、`rules.yaml`、`special_tuned.yaml`、
  `fas_whitelist.yaml`、`module.prop`、`logs/**`。
- **唯一可写**：`meta.yaml` 的 5 个字段（language / loglevel / dev_record / fas_enabled / scenemode_enabled）。
  写入做「顶层行替换 + tmp→rename」的单次读-改-写；写后回读，因为守护进程会在文件非法时整体重置。
- **平台差异**：`status.csv`、`devimp/`、`special_tuned.yaml`、`fas_whitelist.yaml` 仅 ChiRi 机型产生，
  非 ChiRi 与「无法判定」在界面上是两种不同的表达。
- **日志一律取尾部窗口**（daemon.log 单文件 50MB、status.csv 8MB），禁止整文件读取。

## 无设备预览

浏览器里没有 `ksu` 注入时自动装载 `src/dev/mock-shell.ts`，用构建期嵌入的仓库配置 + 生成数据模拟设备。
URL 参数可切换形态，便于走查空态 / 错误态：

```
?soc=chiri|yumi        设备形态（影响白名单与 status.csv 是否存在）
?state=normal|empty|error   正常 / 守护进程从未启动 / 读取真实失败
?daemon=running|stopped     存活探测结果
```
