# ChiRi 项目长期记忆

> AI 协作文件统一在 `.codebuddy/`：`AGENTS.md`（仓库根，按 `[tag]` 区块维护，勿通读）、本文件（长期事实，就地更新保持精简）、`YYYY-MM-DD.md`（按日追加，只追加不重写）、`docs/`（AI 产出文档）。`mdocs/` 只放项目原有文档。旧 `.workbuddy/` 已废弃。开始工作前先读 `AGENTS.md` 与最近几天的日志。

## 工具链环境（本机）

- Bash 无 coreutils（`head`/`sed`/`find`/`uniq`/`dirname`/`cat` 均缺失）；PowerShell 不回显 stdout。文件统计/产物探测用 managed node 绝对路径 `C:/Users/Aibeto Zhu/.workbuddy/binaries/node/versions/22.22.2-3/node.exe`。Node 22.22 可直接 `node x.ts`（类型剥离）。
- `npm` 被沙箱拦截（触发 wsl.exe 黑名单）。本地构建用 node 直驱 vite：`process.chdir('E:/code/ChiRi/webui')` + `import('file:///…/vite/dist/node/index.js')` + `build({})`，约 0.95s。
- vite 体积单位是 kB（1000 字节），要 KiB 用 `Buffer.byteLength`。esbuild 产物用**反引号**包字符串，dist 字符串探测必须同时匹配 反引号/单引号/双引号。
- 含反引号的文本必须用 Write/Edit 写，不要经 `node -e` 走 shell（反引号被当命令替换吞掉）。
- **检索禁用 `head_limit`**：曾因截断漏掉关键写入点。穷举文件写出点用 `root\.join\(` 全局检索。
- 本机只有 Edge，`agent-browser` 需 `--executable-path`；`wait --load networkidle` 在 Vite HMR 下永不完成，改固定等待 + `eval` 读 DOM。

## 守护进程契约（权威要点）

来源：`common.rs` / `main.rs` / `logger.rs` / `chiri/mod.rs` / `chiri/config.rs` / `scheduler/config.rs` / `scheduler/mod.rs` / `monitor/app_detect.rs`。

### 文件接触点（10 个）

`daemon.lock`（探测，唯一权威存活信号）· `active_config.chr`（只读，生效 meta 相对路径）· `config/{rel}`（读写）· `rules.yaml`（只读，无写路径）· `special_tuned.yaml`（只读，仅 Chiri 生成）· `fas_whitelist.yaml`（只读，仅 Chiri 生成）· `current_mode.chr`（只读，模式 id）· `logs/daemon.log`（只读，只含本次运行）· `logs/status.csv`（+`.1`，只读）· `logs/watchdog.pid`（读+删）。附：`devimp/`、`logd/`、`config/{soc}/`。

### meta.yaml 写侧硬约束

- 严格层 `common::MetaYamlFile`：7 字段全必填 + `#[serde(deny_unknown_fields)]`，多一个未知键整文件判非法
- 取值：`loglevel` ∈ OFF/ERROR/WARN/INFO/DEBUG/TRACE（大小写不敏感，允许成对单层引号）；`language` ∈ zh/en；`name`/`author` trim 后非空；三个开关必须布尔
- 违反后果：`sync_meta_snapshot`（启动时 + 每次热重载前）用内嵌默认整体覆盖整个文件并追加 `# 上一次修改存在非法字段…`，用户其它改动一并丢失
- 可写字段：`loglevel` `language` `dev_record` `fas_enabled` `scenemode_enabled`
- 双层解析：严格层仅 `sync_meta_snapshot` 用；容错层 `chiri::config::Meta`（5 字段全 `#[serde(default)]`），`scheduler::config::Meta` 只有 loglevel/language。`fas_enabled`/`scenemode_enabled` 缺省 true。Yumi 上写这三个字段无害但无效

### 存活判定与关闭

- daemon 用 `flock(LOCK_EX|LOCK_NB)` 独占模块根 `daemon.lock`，拿不到就 `exit(0)`；fd 故意泄漏持锁到进程退出
- 锁被持有 ⇔ 有活跃 daemon。`pidof yumi` 是弱信号（同名/僵尸进程命中），只能定位 PID。探测必须取到就释放（`flock -n FILE true`），否则 daemon 下次启动拿不到锁直接退出。真机依赖 toybox/busybox `flock`，不可用时 UI 显示「无法判定」
- 关闭顺序：**先按 `logs/watchdog.pid` 杀看门狗 → 再杀 `yumi`**。pidfile 缺失时只杀 yumi 会被看门狗 3s 拉起。杀完用 flock 复核锁已释放。恢复路径：Action（`action.sh`）或重启
- 看门狗：`service.sh`/`action.sh` 的 `while` 循环**前台**跑 daemon，任何方式结束都重启（退避 3→10→30→60s，活过 60s 归零）。daemon 侧触发重启的是 main/monitor_core panic、chiri 调度线程连续 panic >5 次、日志门限 `exit(0)`；监控子线程 panic 由 `spawn_guarded` 落盘后 `exit(1)`；硬挂（死锁）无存活超时探测

### 热重载

- `config_watcher` 监听**生效配置的父目录**（inotify 不递归），`CLOSE_WRITE | MOVED_TO`，用 `utils::DirWatcher` **跨重载复用同一个 inotify 实例 + 按文件名过滤**（2026-09-16 修复：旧 `utils::watch_path` 每轮重建 inotify，重载窗口内到达的事件被内核丢弃；又不过滤文件名，被 WebUI 的 `*.webui.tmp` 提前唤醒、读到覆盖前的旧内容 → 「重载成功但改动不生效」）。命中后 100ms 静默 + 清空积压，与 `app_detect::watch_config_file`（rules.yaml）同口径
- 链路：事件 → `sync_meta_snapshot` → `Config::load` → `update_level` → 语言变更则 `load_language` → `apply_system_tweaks`
- 故同目录 `tmp + rename` 原子替换受契约支持（WebUI 临时后缀 `.webui.tmp`，避开 daemon 自身的 `<name>.tmp`）

### 模式 id 值域

`powersave` `balance` `performance` `fast` `fas` + 特调 modes（当前只有 `akmode`）。前四档来自 `rules.yaml`（默认 `global_mode: balance`，`app_modes` 为空）。`scenemode` 不在取值内——它是独立息屏轴。Yumi 设备不注册特调模式。daemon 停止后 `current_mode.chr` 是陈旧值。

### UI 原理上不可知（表述原则）

- `is_akmode_available()` 还取决于嵌入的 `akmode.yaml` 是否加载成功；`fas_available()` 还要求白名单非空且至少一个应用配置解析成功。白名单命中 ≠ 生效
- 特调模式全集含 `re:` 正则条目，导出的 special_tuned.yaml 只含精确条目，UI 只能近似
- 表述原则：UI 只陈述已知事实，不对可用性下结论；生效态以 `current_mode.chr` 观测值为准

### 日志归档与量级

- `daemon.log`：单文件上限 **50 MB**，保留 3 个备份 `daemon.1~3.log`。行格式 `[YYYY-MM-DD HH:MM:SS] [LEVEL] [module] msg`（本地时间）
- `status.csv`：上限 **8 MB**，一个 `.1` 备份，每秒一行、**22 列**，三到五万行。列序 timestamp,type,mode,package,charge,screen_on,batt_temp,cpu_temp,thermal_cap_pct,thermal_free_pct,clg_active,psi_cpu_some,psi_io_some,psi_mem_some,gpu_busy_pct,batt_voltage_v,batt_current_ma,batt_power_w,wakeups,migrations,freq_trans,fps。首行表头、`type` 恒 `snap`、缺测 `-`、`screen_on`/`clg_active` 为 0/1、`package` 可为空。数据源 `src/logger.rs`
  - `fps` 是 2026-09-16 新增的**预留列**（末列）：仅 FAS 激活且帧窗口有样本时为实测值（窗口均值，与齿轮决策 avg_fps 同口径），否则恒 `-`。新增列一律追加末尾——WebUI 按列数过滤残缺行，插入中间会打乱索引
- 两者都不能整读，必须限量，历史只在 zip 里。`devimp/`：Chiri 专属，`devimp_<pkg>_<MMDD-HHmmss>.log`
- 每次 daemon 启动：先回收模块根遗留的 `ziped_*` staging 目录（2026-09-16：进程在打包完成前退出会让它们永久滞留、且下次启动因 `logs/`/`devimp/` 为空而「无东西可归档」＝启动调度不打包），再把上一轮 `logs/` 与 `devimp/` 打包为 `logd/ziped_<ts>.zip`、`logd/devimp_<ts>.zip`；`watchdog.pid` 复制回新 `logs/`。staging 目录名经 `unique_staging` 去重（rename 目标已存在 → ENOTEMPTY → 本次整体不归档）
- 预算清理：`logd/` 与 `devimp/` **各自独立计量**，各自 > 128MB 只删本目录最旧文件到 < 96MB，目录内最新一个文件永不删除。旧实现两目录合并成一个总预算、跨目录按 mtime 删，会把 logd 归档包删光
- 打包门限：**事件触发、零额外 syscall**。三条写路径（daemon.log / status.csv / devimp）落盘后经 `logger::note_write` 记账，任一目录本会话累计 ≥ 16MB 即 `exit(0)`，看门狗 3s 后拉起走启动归档。无看门狗（调试直跑）不退出、计数清零。刻意不遍历目录

### 读取失败三态

正常空值（`app_modes` 为空）／合法缺失（非 Chiri 无白名单文件、status.csv 在 Yumi 不生成）／读取失败（命令失败、`active_config.chr` 陈旧指向不存在文件）。三者必须分开，最后一种要在界面明确报错，不能伪装成空值。

## FAS 帧指标口径（2026-09-16 核实）

- eBPF 只有一个探针：`uprobe` 挂在 `/system/lib64/libgui.so` 的 `android::Surface::queueBuffer`，每次只记录 `bpf_ktime_get_ns()`（帧提交时刻），无帧开始/结束配对
- 因此 FAS 的 `frame_delta_ns` = **相邻两次 queueBuffer 的时间间隔（帧间隔 / frame time）**，不是单帧生成耗时（GPU/CPU 渲染一帧的 duration）
- 推出：`current_fps = 1e9 / frame_delta_ns`；`actual_ms` 与 `1000/target_fps` 的 budget 比较；`heavy_frame_threshold_ms`、EMA、PID 误差全部建立在帧间隔语义上
- 过滤：`< min_frame_ns()`（= `1e9 / max_gear() / 2`）丢弃；`> fixed_max_frame_ms` 丢弃。fps_monitor 侧另有硬门 `MIN_FRAME_NS=1ms` / `MAX_FRAME_NS=200ms`，窗口 144 帧
- 投喂语义：只投喂本轮**真实新产生的**帧间隔（pending 队列全量取走），不重复投喂最新一条

## WebUI（2026-09-13 重写完成）

- 技术栈：Svelte 5 (runes) + TypeScript (`strict` + `noUncheckedIndexedAccess`) + Vite + `@yunyoujun/ak-ui@1.0.0`（CSS Core，218 类名 / 63 token，属 1.x 契约承诺）+ vitest。旧 Vue3/Vant/Pinia/vue-i18n/vue-router 已全部移除，无兼容层
- 分层：`src/kernel`（ksu 桥 + 可注入 `ShellRunner`）→ `src/contract`（paths/errors/read/meta/daemon/sources）→ `src/data`（status-csv、daemon-log、mode、whitelists、rules、module-info、apps）→ `src/views`（四屏）+ `src/components`。`src/dev/mock-shell.ts` 为无 ksu 时的设备替身，`?soc=chiri|yumi`、`?state=normal|empty|error`、`?daemon=running|stopped` 切换形态
- 自写约 40 行 hash 路由；自实现 runes i18n（界面语言与 `meta.language` 解耦）；`.svelte.ts` 做共享状态免 Pinia
- 命令：`npm run dev` / `build` / `type-check`（svelte-check，替代 vue-tsc）/ `test`
- 实施定调：① 不引入 headless 库，交互用原生元素配 ak-ui token；② ak-ui 按钮/表单硬编码浅色（150×50、`#f3f4ef`），在 `[data-ak-ui]` 作用域内映射回 `--ak-*`；③ 配置改动先落草稿再一次性提交，一次写入触发一次全量热重载，写后回读；④ 设备形态分 chiri/yumi/unknown，读不到不下结论
- 未引入 `kernelsu` npm 包，用仓库内原有本地桥（TS 重写为 `src/kernel/ksu.ts`）。真机注意：`ksu.moduleInfo().moduleDir` 未在真机取证，路径异常先查这里；真机 `hasKsu()` 为 true，mock 不激活

## 构建契约

`cargo xtask build` 内跑 `webui/npm run build`，再把 `webui/dist` 原样拷进模块 `webroot/`。硬约束 `base: './'` 与 `type="module"`。CI：Node 24 + `npm install`。`module.prop` id = `chiri`。

## 仓库约定（用户明确）

- `mdocs/` 只放项目原有文档（`socList.md`、`updateWith.md`）。AI 产出的评估/计划/报告一律放 `.codebuddy/docs/`（已 gitignore）；`.codebuddy/memory/` 未忽略，是否提交由用户定
- 评估与准备不等于批准开工。用户没明确说"开始改"，就不创建也不修改源码文件。改动前先 `git status` 核对足迹，汇报时给文件级清单
- WebUI 重构是彻底重写：不保留旧 webui 代码和逻辑，接口业务正常即可
