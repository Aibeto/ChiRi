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
| `src/contract/` | 与守护进程的接口：路径与安全校验、三分类读取、meta 读写、实验室状态读写、存活探测、关闭调度 |
| `src/data/` | 纯解析：status.csv（22 列，末列 fps 为 FAS 预留列）、daemon.log、白名单、rules、模式派生、应用标签、实验室状态 |
| `src/views/` | 四屏：状态总览 / 配置 / 应用与规则 / 日志；配置页下挂二级页「实验室」 |
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
- **可写接触点**：`meta.yaml` 的 6 个字段（language / loglevel / dev_record / fas_enabled /
  scenemode_enabled / thread_bind），`rhine.chr`（实验室状态，写模式 key、空内容或保留字
  `off` 强制关闭），以及 `down.chr`（DOWN 停摆，写 `down` 开、写空内容关）。
  两者都做「tmp→rename」的原子写；meta 还需顶层行替换以保住注释，且都要写后回读——守护进程会在
  文件非法时整体重置。
- **实验室（`#/lab`）**：状态文件 `rhine.chr` 不存在或只有注释都算未启用（不是错误）；
  关闭实验室写空内容而不是删文件；锁定期间关闭要写保留字 `off`（强制关闭，走确认弹层）。
  模式列表在 `data/lab.ts` 里硬编码，英文名取明日方舟官方译名
  模式名一律用英文（中英两套 locale 同值、不分语言）：Vector Breakthrough / Contingency
  Contract / Babel / Frozen；中文只出现在各模式的 `.detail` 描述里。
- **实验室接管**：`data/lab.ts::LAB_TAKEOVER` 列出各模式会写的 meta 开关（与 rhine-init.yaml
  同步，单测有刚性断言），配置页把被接管的开关**置灰不可切换**并在提示里点名原因。
- **导出历史日志**（状态页底部）：设备上后台跑 tar（xz 不可用自动退 gzip），前端只轮询产物，
  不阻塞界面。`/sdcard/Download/devimp_<MMDD-HHmmss>.tar.xz`；本次运行正在写的文件会被排除。
- **实验室锁定**：`/tmp/chiri-labs.lock`（回退 `/dev/chiri-labs.lock`，与守护进程同序探测）存在
  即表示本次开机后启用过，运行时关不掉——关闭按钮禁用并提示需重启设备。标记里的 `#` 行是守护
  进程留下的异常痕迹，配合 `labWarnings()` 的一致性检查（rhine.chr 非法 / 与锁定态不一致 /
  `rhine-back.chr` 缺失）在页面顶部出「建议立即重启」的警示条。
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
