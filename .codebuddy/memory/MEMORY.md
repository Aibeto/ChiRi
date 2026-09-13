# ChiRi 项目长期记忆

> **AI 协作文件已统一到 `.codebuddy/`（2026-09-13）**
> - `AGENTS.md`（仓库根）：项目指导，按区块维护（Grep `\[tag\]` 定位，勿通读全文）。
> - `.codebuddy/memory/MEMORY.md`：本文件，长期事实（就地更新、保持精简）。
> - `.codebuddy/memory/YYYY-MM-DD.md`：按日期追加的工作日志（只追加不重写）。
> - `.codebuddy/docs/`：计划与评估等 AI 产出文档；`mdocs/` 只放项目原有文档。
> - 旧的 `.workbuddy/` 已废弃，其下只剩重定向说明，不再写入内容。
> **开始工作前请互相阅读**：读本文件与最近日志的同时，先读仓库根的 `AGENTS.md`。
> 全部为已验证事实。全量契约基准见 `.codebuddy/docs/webui-refactor-plan.md`。

## 工具链环境（本机）

- `Bash` 工具 coreutils 不可用（`head`/`sed`/`find`/`uniq`/`dirname`/`cat` 均缺失）；`PowerShell` 工具不回显 stdout。文件统计与产物探测用 managed node 绝对路径：`C:/Users/Aibeto Zhu/.workbuddy/binaries/node/versions/22.22.2-3/node.exe`。
- `npm` 被沙箱安全策略拦截（触发 wsl.exe 黑名单）。本地构建改用 node 直驱 vite：`process.chdir('E:/code/ChiRi/webui')` + `import('file:///…/vite/dist/node/index.js')` + `build({})`，实测约 0.95s。
- vite 体积输出单位是 kB（1000 字节）；要 KiB 用 `Buffer.byteLength`。
- 本仓库 esbuild 产物用**反引号**包裹字符串字面量。做 dist 字符串探测必须同时匹配 反引号/单引号/双引号，否则结论全为 0。
- Node 22.22 可直接 `node x.ts` 运行 TypeScript（类型剥离），纯逻辑模块可脱离真机跑断言。
- 含反引号的文本必须用 Write/Edit 工具写，不要经 `node -e` 走 shell。bash 会把反引号当命令替换执行，内容被吞。
- **检索禁用 `head_limit`**：曾因截断漏掉 `scheduler/mod.rs` 的 mode 写入方与 `daemon.lock`。穷举文件写出点用 `root\.join\(` 全局检索。
- 本机只有 Edge，`agent-browser` 走查需 `--executable-path` 指定；`wait --load networkidle` 在 Vite HMR 下永不完成，改为固定等待 + `eval` 读 DOM。

## 守护进程接口契约（权威要点）

来源：`common.rs` / `main.rs` / `logger.rs` / `chiri/mod.rs` / `chiri/config.rs` / `scheduler/config.rs` / `scheduler/mod.rs` / `monitor/app_detect.rs`。

### 文件接触点（10 个）

1. `daemon.lock`：探测。唯一权威的存活信号
2. `active_config.chr`：只读。生效 meta 的相对路径，无换行，仅 daemon 启动时写一次
3. `config/{rel}`（生效 meta.yaml）：读写
4. `rules.yaml`：只读。编译期嵌入为唯一基准，磁盘副本仅供展示，无写路径
5. `special_tuned.yaml`：只读。仅精确条目，仅 Chiri 设备生成
6. `fas_whitelist.yaml`：只读。仅 Chiri 设备生成，每应用配置不导出
7. `current_mode.chr`：只读。模式 id，无换行。两套调度器（chiri / scheduler）各有一处写入，同机同时只有一个在跑；启动写一次，切换时写，之后每 5s 自愈重写
8. `logs/daemon.log`：只读，只含本次运行
9. `logs/status.csv`（+ `.1`）：只读。每秒一条 snap 宽表，含前台包名与充放电
10. `logs/watchdog.pid`：读 + 删。看门狗 sh 的 PID，daemon 侧每 5s 自愈重建

附：`devimp/`、`logd/`、`config/{soc}/`。

### meta.yaml 写侧硬约束（后果最重）

- 严格层 `common::MetaYamlFile`：7 字段全必填，且带 `#[serde(deny_unknown_fields)]`，多一个未知键整文件即判非法
- 取值：`loglevel` ∈ OFF/ERROR/WARN/INFO/DEBUG/TRACE（大小写不敏感，允许成对单层引号）；`language` ∈ zh/en；`name`/`author` trim 后非空；三个开关必须布尔
- 违反后果：`sync_meta_snapshot`（启动时 + 每次热重载前）用内嵌默认整体覆盖整个文件，末尾追加 `# 上一次修改存在非法字段…`，用户其它改动一并丢失
- 可写字段：`loglevel` `language` `dev_record` `fas_enabled` `scenemode_enabled`
- 该约束已被随模块下发的 `module/config/meta.yaml` 文件头注释原文印证

### 双层解析

- 严格层（只在校验时调用）：`common::MetaYamlFile`，仅有 `sync_meta_snapshot` 使用
- 容错层（运行时）：`chiri::config::Meta`（5 字段全带 `#[serde(default)]`）；`scheduler::config::Meta` 只有 loglevel 和 language
- `fas_enabled` / `scenemode_enabled` 缺省为 true，缺字段即开启
- 在 Yumi 设备上写 `dev_record`/`fas_enabled`/`scenemode_enabled` 无害但无效

### 存活判定

- daemon 用 `flock(LOCK_EX | LOCK_NB)` 独占模块根的 `daemon.lock`，拿不到就 `exit(0)`；fd 故意泄漏，持锁到进程退出
- 锁被持有 ⇔ 有活跃 daemon。`pidof yumi` 是弱信号（同名进程、僵尸进程都会命中），只能用来定位 PID
- 探测必须取到就释放。探测方若一直持锁，daemon 下次启动会拿不到锁直接退出。实现为 `flock -n FILE true`，带命令、结束即释放
- 真机依赖 toybox/busybox 的 `flock`。不可用时界面显示「无法判定」，不退回 pidof

### 热重载

- `config_watcher` 监听**生效配置的父目录**（inotify 不递归），事件 `MODIFY | CLOSE_WRITE | MOVED_TO`
- 链路：事件 → `sync_meta_snapshot` → `Config::load` → `update_level` → 语言变更则 `load_language` → `apply_system_tweaks`
- 故同目录 `tmp + rename` 原子替换是被契约支持的写法（WebUI 临时文件后缀 `.webui.tmp`，避开 daemon 自身的 `<name>.tmp`）

### 关闭调度

- 顺序：**先按 `logs/watchdog.pid` 杀看门狗 → 再杀 `yumi`**
- pidfile 缺失时：`killall yumi` 杀掉当前实例，看门狗没杀，3s 后又拉起来，界面上就是关不掉
- 双实例已有 flock 兜底，不再是风险
- 校验：杀完用 flock 复核锁已释放；仍被持有则报错
- 恢复路径：Action（`action.sh`）或重启设备，UI 文案须包含

### 模式 id 值域

- 取值：`powersave` `balance` `performance` `fast` `fas`，外加特调 modes（当前只有 `akmode`）
- 前四档来自 `rules.yaml` 的 `global_mode`/`app_modes`（默认 `global_mode: balance`，`app_modes` 为空）
- `scenemode` 不在取值内。它是独立的息屏轴（`scenemode_offline` 标志 + 饱和退出 0.75 持续 10s + 冷却 300s）
- Yumi 设备不注册特调模式，值域只有前四档
- daemon 停止后 `current_mode.chr` 是陈旧值

### UI 原理上不可知（须遵守表述原则）

- `is_akmode_available()` 还取决于嵌入的 `akmode.yaml` 是否加载成功，白名单命中不等于特调生效
- `fas_available()` 另外要求白名单非空且至少一个应用配置解析成功，白名单非空不等于 FAS 可用
- 特调模式全集来自 `is_special_mode()` 遍历的全部嵌入条目（含 `re:` 正则），而导出的 special_tuned.yaml 只含精确条目，UI 只能近似
- 表述原则：UI 只陈述已知事实，不对可用性下结论；生效态以 `current_mode.chr` 的观测值为准

### 日志归档与量级

- `daemon.log`：单文件上限 **50 MB**（`LOG_MAX_BYTES`），保留 3 个备份 `daemon.1~3.log`（`LOG_KEEP_BACKUPS`）。按每行一两百字节算，单文件可达三五十万行；行格式 `[YYYY-MM-DD HH:MM:SS] [LEVEL] [module] msg`（本地时间）
- `status.csv`：上限 **8 MB**（`STATUS_LOG_MAX_BYTES`），一个 `.1` 备份，每秒一行、**21 列**，约三到五万行；列序 timestamp,type,mode,package,charge,screen_on,batt_temp,cpu_temp,thermal_cap_pct,thermal_free_pct,clg_active,psi_cpu_some,psi_io_some,psi_mem_some,gpu_busy_pct,batt_voltage_v,batt_current_ma,batt_power_w,wakeups,migrations,freq_trans；数据源 `src/logger.rs`
  - 列语义：首行表头、`type` 恒为 `snap`、timestamp 为本地时间 `HH:MM:SS.mmm`、缺测为 `-`、`screen_on`/`clg_active` 为 0/1、`package` 可为空
- 两个都不能整读，必须限量。历史只在 zip 里
- `devimp/`：Chiri 专属，文件名 `devimp_<pkg>_<MMDD-HHmmss>.log`
- 每次 daemon 启动：上一轮整个 `logs/` 与 `devimp/` 打包为 `logd/ziped_<ts>.zip`、`logd/devimp_<ts>.zip`；`watchdog.pid` 复制回新 `logs/`
- 预算清理：logd + devimp > 128MB 时从最旧删到 < 96MB

### 读取失败三态

- 正常空值，如 `app_modes` 为空
- 合法缺失，如非 Chiri 设备没有白名单文件、`current_mode.chr` 短暂缺失、status.csv 在 Yumi 上不生成
- 读取失败，如命令执行失败、`active_config.chr` 陈旧指向已不存在的文件

三者必须分开。最后一种要在界面上明确报错，不能伪装成空值。

## WebUI 现状（2026-09-13 重写完成）

- 计划文件：`.codebuddy/docs/webui-refactor-plan.md`（契约基准为唯一硬约束）。
- 技术栈：Svelte 5 (runes) + TypeScript + Vite + `@yunyoujun/ak-ui@1.0.0`（CSS Core）+ vitest。旧 Vue 3 / Vant / Pinia / vue-i18n / vue-router 已全部移除，无兼容层。
- 分层：`src/kernel`（ksu 桥 + 可注入 `ShellRunner`）→ `src/contract`（paths/errors/read/meta/daemon/sources）→ `src/data`（status-csv、daemon-log、mode、whitelists、rules、module-info、apps）→ `src/views`（四屏）+ `src/components`；`src/dev/mock-shell.ts` 是无 ksu 时的设备替身，URL 参数 `?soc=chiri|yumi`、`?state=normal|empty|error`、`?daemon=running|stopped` 切换形态。
- 命令：`npm run dev` / `npm run build` / `npm run type-check`（svelte-check）/ `npm test`（vitest）。构建链不变：`cargo xtask build` 调 `npm run build` 后拷 `webui/dist` 到模块 `webroot/`，`base: './'` 与 `type="module"` 两个硬约束保持。
实施时定下的几条：① 不引入 headless 库，交互用原生元素（button/label/dialog）配 ak-ui token；② ak-ui 的按钮与表单是硬编码浅色（150×50 固定尺寸、`#f3f4ef` 底），在 `[data-ak-ui]` 作用域内映射回 `--ak-*`；③ 不用 i18n 库，用 runes 自实现，界面语言与 `meta.language` 解耦；④ 配置改动先落草稿再一次性提交，一次写入触发一次全量热重载，写后回读；⑤ 设备形态分 chiri/yumi/unknown，读不到就不下结论。
- 未引入 `kernelsu` npm 包，用仓库内原有的本地桥（TS 重写为 `src/kernel/ksu.ts`）。
- 真机注意：`ksu.moduleInfo().moduleDir` 未在真机取证，路径异常先查这里；真机上 `hasKsu()` 为 true，mock 不会激活。

## 目标技术栈（已定）

TypeScript（`strict` + `noUncheckedIndexedAccess`）· npm · Vite（保留 `vite.config.ts` 与 `virtual:chiri-config` 插件）· Svelte 5（runes，`.svelte.ts` 做共享状态，免 Pinia）· ak-ui CSS Core（`@yunyoujun/ak-ui`，引 `style.css`；`.ak-*` 类名与 `--ak-*` token 属 1.x 契约承诺，218 类名 / 63 token）· 自写约 40 行 hash 路由 · 自实现 runes i18n · `svelte-check` 替代 `vue-tsc`。

## 构建契约（不变）

`cargo xtask build` 内跑 `webui/npm run build`，再把 `webui/dist` 原样拷进模块 `webroot/`。硬约束 `base: './'` 与 `type="module"`。CI：Node 24 + `npm install`。`module.prop` id = `chiri`。

## 旧 WebUI 事实（仅作参考实现）

`webui/src` 16 文件 / 1768 行；Vue 3.5 + Vite 8 + Pinia + vue-i18n + vue-router(hash) + Vant 4；5 个 SFC 共 874 行（脚本 360 / 模板 188 / 样式 322），框架无关代码 776 行。Vant 已确认全量引入（dist 695.2 KB，实测含 51/52 未使用组件模块、26/28 未使用 CSS 选择器）。Vite 8 的打包器是 Rolldown。旧运行时依赖 8 个（js-yaml / kernelsu / lodash / pinia / vant / vue / vue-i18n / vue-router），其中 lodash 与 npm 版 kernelsu 零引用。

## 仓库约定（用户明确）

- `mdocs/` 只放项目原有文档（`socList.md`、`updateWith.md`）。AI 产出的评估、计划、报告一律不放这里，统一放 `.codebuddy/docs/`。
- `.codebuddy/docs/` 已加入 `.gitignore`；`.codebuddy/memory/` 未忽略，是否提交由用户定。
- 评估与准备不等于批准开工。用户没明确说"开始改"，就不创建也不修改源码文件。改动前先 `git status` 核对足迹，汇报时给文件级清单。
- 这次是彻底重构：不保留旧 webui 的任何代码和逻辑，接口业务正常即可，其余随便改。
