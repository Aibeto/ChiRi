# ChiRi WebUI 重构计划

## 目标

换掉 WebUI 的 Vue 技术栈，用 TypeScript + Svelte 5 + ak-ui 重写。

这是一次完全重构，旧 webui 的代码和逻辑都不保留，重构完成后整体替换掉。唯一的硬要求是接口业务正常，页面结构、路由、视觉、代码组织都不迁就旧实现。

验收看新实现是否符合守护进程的接口契约，不看它是否与旧版行为一致。

## 契约基准

这是唯一硬约束。内容取证自 daemon 源码。

### 接触点

| 路径 | 方向 | 说明 |
|---|---|---|
| `daemon.lock` | 探测 | 存活判据 |
| `active_config.chr` | 只读 | 生效 meta 的相对路径，无换行，只在 daemon 启动时写 |
| `config/{rel}` | 读写 | 生效的 meta.yaml |
| `rules.yaml` | 只读 | 无写路径。另有 `yumi_scheduler`、`dynamic_enabled`、`ignored_apps` 三个开关目前界面上看不到 |
| `special_tuned.yaml` | 只读 | 仅精确包名条目，仅 Chiri 设备生成 |
| `fas_whitelist.yaml` | 只读 | 仅 Chiri 设备生成 |
| `current_mode.chr` | 只读 | 模式 id，无换行，两个写入方，每 5 秒自愈重写 |
| `module.prop` | 只读 | 模块名、版本、作者 |
| `logs/daemon.log` | 只读 | 只含本次运行 |
| `logs/status.csv`（及 `.1`） | 只读 | 每秒一条，22 列 |
| `logs/watchdog.pid` | 读、删 | 看门狗 sh 的 PID |
| `devimp/` | 目录 | 开发记录日志，与 `dev_record` 开关呼应 |
| `logd/` | 目录 | 归档 zip |
| `rhine.chr` | 读写 | 实验室状态：内容就是模式 key（`contingency` / `vector`），空文件或只有注释 = 未启用。用户手改同样生效，不用重启调度 |
| `rhine-back.chr` | 只读 | 实验室套用前一刻的原值快照，写在模块根，还原完由 daemon 删除。存在即表示「上次启用过实验室」 |
| `/tmp/chiri-labs.lock`（回退 `/dev/chiri-labs.lock`） | 只读 | 实验室锁定标记：存在 = 本次开机后启用过，运行时关不掉，必须重启设备。首行是模式名，其后的 `#` 行是 daemon 留下的异常痕迹（界面据此提示「运行配置被改动，建议重启」）。落在 tmpfs 上，重启即消失 |
| `action.sh` | 探测存在 | 关闭调度后的恢复入口 |

### 写 meta.yaml

7 个字段全部必填：`name` `author` `language` `loglevel` `dev_record` `fas_enabled` `scenemode_enabled`。

结构上拒绝未知字段，多一个键就整文件非法。`loglevel` 限六档、`language` 限 `zh`/`en`、`name` 与 `author` 不能为空、三个开关必须是布尔。

非法时 daemon 会用内嵌默认值覆盖整个文件，用户改过的其他字段一起丢。所以写的时候只改目标字段的值，不动别的键。

可写字段有五个，其中 `language` 旧版当成只读，实际写下去会触发 daemon 热重载语言。

### 写 rhine.chr

内容是一个 YAML 标量：启用写模式 key 加换行，关闭写空内容（不删文件）。守护进程侧
`src/rhine.rs::parse_state` 先剥空行与 `#` 注释行，再要求单行标量且 key 在 `rhine-init.yaml`
里定义过，否则判非法并覆盖成内置模板。WebUI 的 `data/lab.ts::parseLabState` 必须同口径。

模式名中文与英文都在 WebUI 侧硬编码（`data/lab.ts` + `i18n/locales/*.ts`），英文取明日方舟官方译名：

| key | 中文 | 英文 |
| --- | --- | --- |
| `contingency` | 危机合约 | Contingency Contract |
| `vector` | 矢量突破 | Vector Breakthrough |

`vector` 是预留条目（守护进程侧影响项为空映射），WebUI 显示为「预留」且不给启用入口。

### 几个必须知道的点

`daemon.lock` 由 daemon 用 `flock` 独占，抢不到就退出。锁被持有等于有 daemon 在跑。`pidof` 是弱信号，只能用来定位 PID。探测时必须拿到锁后立刻释放，否则下次 daemon 启动会失败。

关闭调度要先按 `watchdog.pid` 杀看门狗，再杀 `yumi`。pidfile 丢了的话 killall 杀得掉当前实例但看门狗会把它拉起来，表现为关不掉。

`current_mode.chr` 的取值只有 `reduce`、`default`、`boost`、`vector`、`fas` 和特调 id。`scenemode` 不在其中，它是独立的息屏轴。

读取失败要分三种：正常空值、契约允许的缺失（比如非 Chiri 设备没有白名单文件）、真正的失败。第三种要报错，不能吞成空值。

连着改多个开关时，读文件、改一行、写回去这个过程之间可能被插队，把前一次改动抹掉；而且每次写入都会触发 daemon 全量热重载。怎么处理留到实现阶段定。

### 日志的量级

两个日志文件都有上限，但都不小，不能整读。

`daemon.log` 单文件上限 50 MB，另留三个备份（`daemon.1~3.log`）。按每行一两百字节算，单文件可以到三五十万行。

`status.csv` 上限 8 MB，一个 `.1` 备份，每秒一行、22 列，大约三到五万行。

所以读的时候必须限量，界面上也要说明展示的是最近一段而不是全部。另一个约束是每次 daemon 启动会把上一轮整个 `logs/` 与 `devimp/` 打包进 `logd/ziped_<ts>.zip` 与 `logd/devimp_<ts>.zip`，历史只存在于 zip 里。

### 界面的表述边界

有三件事 WebUI 拿不到：特调是否真的可用、FAS 是否真的可用、特调模式的完整集合。它们都取决于 UI 看不到的内部状态。

所以界面上只写它确实知道的事，比如"已配置"，不写"已生效"。要表达生效状态，以 `current_mode.chr` 的观测值为准。

## 选型

TypeScript、Svelte 5、ak-ui（引 CSS Core，用 `.ak-*` 类名）、svelte-i18n，路由自己写一小段。类型检查换成 `svelte-check`。

构建侧不动：仍是 Vite，仍由 `cargo xtask build` 调 `npm run build` 后把 `webui/dist` 拷进模块，`base: './'` 和 `type="module"` 两个硬约束保持。CI 不变。

## 路线

分四步，只定顺序不定做法。

1. 契约层：把与设备交互的部分做完，读、写、存活探测、关闭调度。纯逻辑，不依赖界面。
2. 数据层：模式、日志、应用列表这些从契约数据算出来的东西。
3. 界面：接入 ak-ui，页面和组件。
4. 验收：按下面三层走完，然后整体替换掉旧实现。

前两步做完之前不做界面。逻辑和渲染分开推进，出问题的来源容易定位。

## 验收

契约层和数据层是纯逻辑，可以脱离真机用 node 跑断言，重点覆盖路径校验、meta 字段改写、白名单解析、模式派生、日志切分这些。

操作层必须在真机验证：改配置后回读、非法值能否被识别、关闭调度是否真的成功、探测锁之后 daemon 还能正常启动。

界面层真机走查，看各页面的空、缺失、错误、禁用状态，键盘可达，触控面积，以及边到边布局。日志页还要看大文件下的响应。

## 边界

不保留旧 webui 的代码和逻辑，也没有兼容旧结构的义务。

不为展示类元素引入 headless UI 库。

不用 `pidof` 当存活判据。

不保留旧版读失败一律吞掉的写法。

不在界面上对可用性下结论。

日志不做整文件加载。

## 待确认

以下几项我按默认值写，不反对就照此执行：ak-ui 用 `@next` 并锁死版本号；风格强度用 `system`，日志页可局部用 `terminal`；页面划成状态总览、配置读写、应用与规则、日志四屏。

契约基准这一节有没有读错或漏掉的地方。前三版都出过错，值得再看一遍。
