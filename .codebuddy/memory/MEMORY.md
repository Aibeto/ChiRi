# ChiRi 项目长期记忆

> AI 协作文件统一在 `.codebuddy/`：`AGENTS.md`（仓库根，按 `[tag]` 区块维护，勿通读）、本文件（长期事实，就地更新保持精简）、`YYYY-MM-DD.md`（按日追加，只追加不重写）、`docs/`（AI 产出文档）。`mdocs/` 只放项目原有文档。旧 `.workbuddy/` 已废弃。开始工作前先读 `AGENTS.md` 与最近几天的日志。

## 工具链环境（本机）

- Bash 无 coreutils（`head`/`sed`/`find`/`uniq`/`dirname`/`cat` 均缺失）；PowerShell 不回显 stdout。文件统计/产物探测用 managed node 绝对路径 `C:/Users/Aibeto Zhu/.workbuddy/binaries/node/versions/22.22.2-3/node.exe`。Node 22.22 可直接 `node x.ts`（类型剥离）。
- `npm` 被沙箱拦截（触发 wsl.exe 黑名单）。本地构建用 node 直驱 vite：`process.chdir('E:/code/ChiRi/webui')` + `import('file:///…/vite/dist/node/index.js')` + `build({})`，约 0.95s。
- vite 体积单位是 kB（1000 字节），要 KiB 用 `Buffer.byteLength`。esbuild 产物用**反引号**包字符串，dist 字符串探测必须同时匹配 反引号/单引号/双引号。
- 含反引号的文本必须用 Write/Edit 写，不要经 `node -e` 走 shell（反引号被当命令替换吞掉）。
- **检索禁用 `head_limit`**：曾因截断漏掉关键写入点。穷举文件写出点用 `root\.join\(` 全局检索。
- **`agent-browser` 本机没有安装**（`agent-browser` 命令不在 PATH，`~/.cargo/bin`、`%APPDATA%\npm` 都没有；npm 被拦也装不了），所以 WebUI 改动的浏览器走查在本机做不了——只能靠 `svelte-check` + `vitest` + vite 构建 + dist 字符串探测，汇报时必须说明「视觉/交互未实测」。（2026-09-16 更正：此前记的「需 `--executable-path`」是过期信息。）

## 守护进程契约（权威要点）

来源：`common.rs` / `main.rs` / `logger.rs` / `chiri/mod.rs` / `chiri/config.rs` / `scheduler/config.rs` / `scheduler/mod.rs` / `monitor/app_detect.rs`。

### 文件接触点（13 个）

`daemon.lock`（探测，唯一权威存活信号）· `active_config.chr`（只读，生效 meta 相对路径）· `config/{rel}`（读写）· `rules.yaml`（只读，无写路径）· `special_tuned.yaml`（只读，仅 Chiri 生成）· `fas_whitelist.yaml`（只读，仅 Chiri 生成）· `current_mode.chr`（只读，模式 id）· `logs/daemon.log`（只读，只含本次运行）· `logs/status.csv`（+`.1`，只读）· `logs/watchdog.pid`（读+删）· `rhine.chr`（读写，实验室状态）· `rhine-back.chr`（只读，实验室原值快照）· `down.chr`（读写，DOWN 停摆状态，写了 `down` = 停摆）。附：`devimp/`、`logd/`、`config/{soc}/`。

### meta.yaml 写侧硬约束

- 严格层 `common::MetaYamlFile`：8 字段全必填 + `#[serde(deny_unknown_fields)]`，多一个未知键整文件判非法（2026-09-16 加 `thread_bind`：**四处要同步**——`MetaYamlFile`/`ExternalMetaOverrides`、四个 meta.yaml 模板（`config/meta.yaml` + 三个 `{soc}/meta.yaml`）、WebUI `contract/meta.ts::META_FIELDS`、`rewrite_meta_toggles` 字段表）
- 取值：`loglevel` ∈ OFF/ERROR/WARN/INFO/DEBUG/TRACE（大小写不敏感，允许成对单层引号）；`language` ∈ zh/en；`name`/`author` trim 后非空；四个开关必须布尔
- 违反后果：`sync_meta_snapshot`（启动时 + 每次热重载前）用内嵌默认整体覆盖整个文件并追加 `# 上一次修改存在非法字段…`，用户其它改动一并丢失
- 可写字段：`loglevel` `language` `dev_record` `fas_enabled` `scenemode_enabled` `thread_bind`
- 双层解析：严格层仅 `sync_meta_snapshot` 用；容错层 `chiri::config::Meta`（6 字段全 `#[serde(default)]`），`scheduler::config::Meta` 只有 loglevel/language。`fas_enabled`/`scenemode_enabled`/`thread_bind` 缺省 true。Yumi 上写这几个字段无害但无效

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

`reduce` `default` `boost` `vector` `fas` + 特调 modes（`akmode` 游戏 / `playback` 视频播放 / `daily` 日常轻载，后两者 2026-09-17 加）+ `down`（DOWN 停摆期间写的值）。前四档来自 `rules.yaml`（默认 `global_mode: default`，`app_modes` 为空）。`scenemode` 不在取值内——它是独立息屏轴。Yumi 设备不注册特调模式。daemon 停止后 `current_mode.chr` 是陈旧值。

### UI 原理上不可知（表述原则）

- `is_special_tuned_available()` 还取决于嵌入的 `tuned_profiles.yaml` 是否加载成功；`fas_available()` 还要求白名单非空且至少一个应用配置解析成功。白名单命中 ≠ 生效
- 特调模式全集含 `re:` 正则条目，导出的 special_tuned.yaml 只含精确条目，UI 只能近似
- **特调参数组（2026-09-17 加）**：所有特调共用 `TunedGovernor` 连续控制（40ms 按组内最大占用 × headroom 算上限），参数按模式名分派——`Config.tuned_profiles: HashMap<模式名, SpecialTunedConfig>`，`get_tuned_profile(mode)` 取（缺省回退 `akmode` 段），来自 `normal/tuned_profiles.yaml` 的 `tuned_profiles:` 段（**全局共享，所有 SoC 自动生效**）。**命名族（用户 2026-09-17 要求：调度内标识符不得重名、名字要贴切）**：配置/字段/方法统一 `tuned_profile(s)` 词根、可用性标志统一 `special_tuned_*`（`is/set_special_tuned_available`）；**新增标识符前必须先全仓库搜一遍确认不撞**；模式名须与任何既有词不撞——`light` 因与亮色主题（`prefers-color-scheme: light`）/light use 语义撞车被用户否掉，改 `daily`；`video` 改 `playback`（避开录像/摄像头歧义）。**2026-09-17 二次审查后一并改名**：`TunedGovernor` → `TunedGovernor`、`src/chiri/tuned.rs` → `src/chiri/tuned.rs`、`init_policies(mode, cfg)` 增加模式名参数（日志可区分）、i18n 键 `akmode-*` → `tuned-*`（文案去「明日方舟」并带 `{ $mode }`）、`tuned_cooldown_until`/`TUNED_COOLDOWN` → `tuned_cooldown_until`/`TUNED_COOLDOWN`。**唯一保留的 `akmode`**：模式名本身（白名单条目 / tuned_profiles.yaml 的缺省段键 / `is_special_mode` 判定）——它是配置里的模式 key，改了会破坏语义。**新增防错配校验**：`Config::merge_tuned_profiles` 用 `common::special_tuned_mode_names()`（白名单 modes 并集）核对每个模式名都有参数组，缺了打 `tuned-profile-missing` 告警——否则会静默回退 akmode 段（游戏参数：headroom 1.15 + boost 亲和），在省电场景是反效果。全部 5 个取参调用点已改（apply_mode_takeover / ModeChange / 亮屏恢复 / config_dirty / ConfigReload）。`boost_affinity` 字段（默认 true）= 是否走 boost 亲和（cpuset 收窄 big+prime + core_ctl 保大核常在线）：游戏 true；**省电型特调（playback/daily）必须 false**——保大核常在线与贴负载降频相反（空转漏电），且会把前台重线程钉到大核组被低上限压住（`tuned_boost_affinity()` 在 is_boost_mode 与 uclamp_override 两处二次过滤）。新增特调模式 = special_tuned.yaml 加条目 + tuned_profiles.yaml 的 tuned_profiles 段加同名参数组，无需改 .rs。**回退语义**：特调 init 失败时 `clg_cfg_for` 对未注册模式返回 enabled=false（无人接管=系统原状，与 akmode 现有行为一致，刻意不做默认参数接管）。**`util_smoothing`**（参数字段，默认 1.0=不平滑，2026-09-17 加）= 决策负载 EMA 系数：抖动负载下「升频立即执行」被瞬时尖峰反复推高上限、降频又被 hold 拖住，**上限均值反而高于带平滑的 CLG**（日志回放证实）——playback/daily 用 0.35/0.5 滤尖峰，游戏保持 1.0。**降频计时修复（同日）**：`down_target` 只在目标**明显回升**（> down_target+hyst）时重置计时，下探/微调只更新目标——原「档位不等即重置」在抖动下上限卡高位降不下来，连续控制退化成「只升不降」。**参数定稿（回放扫参）**：playback = hr 1.0 / hold 100 / α0.35；daily = hr 1.0 / floor 0.10 / hold 100 / α0.5（hold 300 明显更差；hr 1.05 多 2-4 点均值）
- **8550 整夜日志实测（0916-2350~0917-0435 共 5h，全 default 档）**：亮屏平均 1.35W；bili 视频 101min/1.27W（61% 时间）；launcher 29min/0.89W；游戏 sgameGlobal 13min/2.48W；息屏 0.07-0.09W 已极低。游戏段 65% 时间被 vendor 热限流（cap 70%）而 ChiRi 未参与（cpuT 才 59.6）——压住温度可减少 vendor 限流。wakeups p90 ~2500/s（突发风暴，应用行为）。**⛔ 列语义（2026-09-17 复盘修正，之前理解错）**：devimp tick 的 `cur_freq_khz` = **CLG 设置的动态上限（scaling_max_freq）**，不是实际频率；`max_freq_khz` = 硬件最高（tuned.rs L381 注释为证）。所以「bili little 1595M」= CLG 给的上限 1595M，实际频率被 schedutil 取在 ≤1595M，**数据里没有实际频率**。CLG 上限/(mu×hw) 实测：little 1.12-1.18、big 1.16-1.45、**prime 1.81-2.46（冗余最大但 prime 是单核，功耗占比低）**——CLG 压上限其实很积极。**回放模拟（analyze5-7，忠实复刻 akmode 算法）**：无平滑的 akmode 在抖动负载下上限均值**反而高于 CLG**（1.1-1.4×）；EMA α0.35 + hr 1.0 + hold 100 才能把 little/big 压到 CLG 的 **0.90-0.96（省 4-10%）**；prime 任何参数都压不动（≥0.99）。**结论：调度层剩余空间就是这几个点（亮屏省个位数 %），「平均降一半」必须靠屏幕/刷新率/解码器层——非调度可及**。**日志列坑（复读数据必看）**：`batt_current_ma` 恒 0/1（用 batt_power_w）；`fps` 列全空（未采集）；wakeups/migrations 是**累计值**；`util_pct` 恒空（真实占用在 `max_util`）；23:39-23:46 qq 段 15-21W 但 CPU 仅 46°C（传感器异常，忽略）。分析脚本 `devimpbin/8550/analyze{1..7}.mjs` 可对后续新日志复跑
- 表述原则：UI 只陈述已知事实，不对可用性下结论；生效态以 `current_mode.chr` 观测值为准

### 日志归档与量级

- `daemon.log`：单文件上限 **50 MB**，保留 3 个备份 `daemon.1~3.log`。行格式 `[YYYY-MM-DD HH:MM:SS] [LEVEL] [module] msg`（本地时间）
- `status.csv`：上限 **8 MB**，一个 `.1` 备份，每秒一行、**22 列**，三到五万行。列序 timestamp,type,mode,package,charge,screen_on,batt_temp,cpu_temp,thermal_cap_pct,thermal_free_pct,clg_active,psi_cpu_some,psi_io_some,psi_mem_some,gpu_busy_pct,batt_voltage_v,batt_current_ma,batt_power_w,wakeups,migrations,freq_trans,fps。首行表头、`type` 恒 `snap`、缺测 `-`、`screen_on`/`clg_active` 为 0/1、`package` 可为空。数据源 `src/logger.rs`
  - `fps` 是 2026-09-16 新增的**预留列**（末列）：仅 FAS 激活且帧窗口有样本时为实测值（窗口均值，与齿轮决策 avg_fps 同口径），否则恒 `-`。新增列一律追加末尾——WebUI 按列数过滤残缺行，插入中间会打乱索引
- 两者都不能整读，必须限量，历史只在 zip 里。`devimp/`：Chiri 专属，`devimp_<pkg>_<MMDD-HHmmss>.log`
- 每次 daemon 启动：**先判短会话**——`logs/daemon.log` 首行时间戳（**本地时间**，log4rs 的 `{d}` 走 chrono 默认本地；用 `mktime` 解析，勿用 `timegm`）距本次启动 <30s 则直接清空 `logs/`（保留 `watchdog.pid`）与 `devimp/`、不打包；读不到/解析失败/时钟回拨一律保守按「非短会话」处理。随后回收模块根遗留的 `ziped_*` staging 目录（2026-09-16：进程在打包完成前退出会让它们永久滞留、且下次启动因 `logs/`/`devimp/` 为空而「无东西可归档」＝启动调度不打包），再把上一轮 `logs/` 与 `devimp/` 打包为 `logd/ziped_<ts>.zip`、`logd/devimp_<ts>.zip`；`watchdog.pid` 复制回新 `logs/`。staging 目录名经 `unique_staging` 去重（rename 目标已存在 → ENOTEMPTY → 本次整体不归档）
- 预算清理：`logd/` 与 `devimp/` **各自独立计量**，各自 > 128MB 只删本目录最旧文件到 < 96MB，目录内最新一个文件永不删除。旧实现两目录合并成一个总预算、跨目录按 mtime 删，会把 logd 归档包删光
- 打包门限：**事件触发、零额外 syscall**。三条写路径（daemon.log / status.csv / devimp）落盘后经 `logger::note_write` 记账，任一目录本会话累计 ≥ 16MB 即 `exit(0)`，看门狗 3s 后拉起走启动归档。无看门狗（调试直跑）不退出、计数清零。刻意不遍历目录

### 读取失败三态

正常空值（`app_modes` 为空）／合法缺失（非 Chiri 无白名单文件、status.csv 在 Yumi 不生成）／读取失败（命令失败、`active_config.chr` 陈旧指向不存在文件）。三者必须分开，最后一种要在界面明确报错，不能伪装成空值。

### 实验室（rhine，2026-09-16 新增）

- 三个文件：`module/config/rhine-init.yaml`（模式定义，编译期嵌入 + xtask `BIN_ONLY` 剔除，不落盘、WebUI 不读）、`rhine.chr`（模块根，对外暴露可手改，内容即模式 key，空/只有注释 = 未启用）、`rhine-back.chr`（模块根，daemon 生成的原值快照，还原完删除；**它存在 = 上次启用过实验室**，开机时 rhine.chr 已被 service.sh 清空，只有它能触发还原）
- 影响项：缺省 = 不变更。`global_mode`（运行时覆盖 rules 的 global_mode 兜底）· `fas_enabled` / `scenemode_enabled` / `thread_bind`（**改写生效 meta.yaml**，WebUI 开关同步显示为关）· `special_tuned`（运行时开关，false = 全部特调判定失效）。为什么分两种落地：global_mode 在 rules.yaml 里编译期嵌入、special_tuned 白名单是 `include_str!`，写文件没用；fas/scenemode/thread_bind 有 meta.yaml 载体才能靠文件生效
- `thread_bind`（2026-09-16 新增，线程摆放总闸）：meta 字段，`Config::load` 里与机型内嵌 `Affinity.enabled` / `CoreCtl.enabled` 取「与」→ 关闭时 `AffinityManager::apply` 走 `release()`（逐线程恢复全核 + cpuset/uclamp 快照回写）、`CoreCtlManager` 回 Normal（恢复 min_cpus/online 快照）＝「所有绑定分配改成全核心」。落地靠 config_dirty 的 `apply_affinity_and_corectl`（2s 周期块兜底）；affinity.rs / core_ctl.rs 本身不用改
- 运行时覆盖层在 `common.rs`：`LAB_GLOBAL_MODE: Mutex<Option<String>>` + `LAB_SPECIAL_TUNED_DISABLED: AtomicBool`，接入 `determine_mode` 的兜底取值与 `special_tuned_entry` / `is_special_mode` 两处短路（另两个特调判定由 `special_tuned_entry` 派生，自动失效）。**只改兜底**：FAS 白名单与 `app_modes` 优先级不变
- 改写 meta.yaml 用 `common::replace_top_level_bool` + `rewrite_meta_toggles`（顶层行替换 + 原子写 + 三开关一次写盘），**绝不能 serde 重排**（8 字段全必填 + deny_unknown_fields，漏字段就被 sync_meta_snapshot 整体覆盖）
- 流程 `rhine::converge`：**先还原后套用**（顺序反了会让快照记到已改过的值）。启动期在 main 首次 `Config::load` 之前跑，覆盖三种场景：开机只剩快照→只还原；不停机重启→先还原再套用；平时→无操作。快照非法 → `embedded_meta_defaults()` 兜底 + warn
- `rhine.chr` 缺失 → 补建内置模板（`include_str!("../module/rhine.chr")`）；非法（非单行标量 / 未知 key）→ 覆盖成同一模板 + warn。监听用 `utils::DirWatcher` 盯模块根，套用/还原后调 `monitor::app_detect::request_mode_refresh()`（进程级 `FORCE_MODE_REFRESH`，与规则热重载的 force_refresh 通道合并消费）让当前模式当场收敛
- **运行时锁定（2026-09-16 加）：启用后必须重启设备才能关闭**。套用成功即在 tmpfs 写 `chiri-labs.lock`（`/tmp` → `/dev` 按序探测可写目录，都不可写则 warn 一次并放弃锁定，不阻断功能）；标记存在 = 本次开机后启用过，`converge` 锁定期间**不还原、不接受关闭**，且**以 rhine.chr 写法为意图来源**（写了别的模式 → 按它生效并把标记首行同步过去，换模式不拦；被清空/改坏 → 写回 + 留痕 + warn），只做 `reassert`（装回 meta 开关与运行时覆盖，**绝不重写 rhine-back.chr**，否则会把改过的值记成原值）。快照真丢了才用内嵌默认补（`write_fallback_backup`）。重启后 tmpfs 空 + service.sh 已删 rhine.chr，才走正常还原
- **强制关闭（保留字 `off`，2026-09-16 加）**：rhine.chr 写 `off` = 用户明示「无视风险」→ 清掉标记 + 落到正常还原路径（还原 meta 三开关 → 清运行时覆盖 → 刷新模式 = 「恢复原地调度」），随后文件写回内置模板（未启用）。与「写空内容」的区别正在此：空内容 = 普通关闭请求（锁定期间被挡回），`off` = 强制关闭。三条硬约束：① `off` **必须在 YAML 解析之前做字面量比较**（`is_force_off`，忽略大小写/允许成对引号）——`off` 在 YAML 1.1 里是布尔字面量、各实现口径不一，交给类型推断会出现「界面认、守护进程判非法」的裂缝；② `off` 是保留字，模式 key 不得取名 off；③ 日志走 `log_converged`（启动期那次收敛早于 `logger::init`）。WebUI：锁定中「关闭」**不再禁用** → `ConfirmSheet` 风险确认 → `state.svelte.ts::forceDisableLab()` 写 off 并等 400ms 回读；`labWarnings` 对 `force-off` 态不报 lock-drifted / no-backup
- **篡改痕迹**：标记首行是模式名，其后每行 `# 代号` 是异常记录（`lock_note`，去重、最多 8 条；代号 `state-reset` / `close-rejected` / `backup-rebuilt`）——**写代号而非现成文案**，因为留痕可能早于 `load_language`（`t()` 那时只返回裸 key）；WebUI 读标记 + `data/lab.ts::labWarnings`（代号 `state-invalid` / `lock-drifted` / `no-backup`）→ 页面顶部警示条「检测到运行配置被改动，建议立即重启设备」+ 痕迹（`lab.lock.note.*` 映射，未收录的原样显示）。痕迹随重启消失，与处置一致
- `service.sh` 的 `[lab-reset]` 在起看门狗前 `rm -f rhine.chr`（只删它）；`action.sh` 与 WebUI「重启调度」不删（也不删锁定标记，所以那之后实验室仍是启用态）

### DOWN 停摆（2026-09-16 新增）

- 状态文件 `down.chr`（模块根，对外暴露可手改，`include_str!` 内置 `module/down.chr` 模板作默认内容）：写了保留字 `down` = 停摆，空/只有注释/其它 = 正常。**与 rhine.chr 同款：内容即状态**；文件在磁盘上 → **重启设备后仍保持停摆**，只能改文件解除
- 判据在 `src/down.rs`（`[consts] [state] [flag] [watch]`）：`read_down` 解析 + 缺失补建模板（读不到按「不停摆」处理，绝不因状态文件异常自关调度）；进程级 `AtomicBool`，`down_watcher` 线程盯模块根热切换
- 行为：停摆期间调度**释放全部接管**（CLG / akmode / FAS / fast_lock / affinity / core_ctl / scenemode），只往 `current_mode.chr` 写一次 `down` 并且**不再覆盖它**（覆盖 = 退出停摆）；采集与日志照常（eBPF / status.csv / daemon.log / devimp），`app_detect` 继续跑所以 status.csv 仍记前台包名。用途：记录「ChiRi 不工作时」设备自己的调度情况，做 A/B 对比
- 实现在调度循环（`chiri/mod.rs`）里，不在监听线程：governor 对象归那个线程独占。三处必须跳过——5s `current_mode.chr` 自愈（只推进计时器不落盘，否则忙循环）、config_dirty 下发、2s 亲和块与热保护块；事件 `match msg` 前 `if halted { continue; }`（消费事件但不下发，避免 `continue` 跳过阻塞 recv 造成 100% CPU）
- 退出：删掉 `current_mode.chr` + 内存模式回到 `down_resume_mode`，**并立即按该模式重新接管**（CLG / vector 锁频 / 亲和 / core_ctl 重建，特调与 FAS 交给后续事件）——2026-09-17 审查修：只复位内存模式不够，ModeChange 只在「模式真的变了」时才重接管，前台应用没换就永远停在释放态且无任何提示。三个配套要点：① `down_resume_mode` 要在**进入停摆时**用当时的实际模式覆盖（启动缺省值可能已被 app_modes/特调/FAS 改掉）；② 退出时 `last_load_event = Instant::now()`，否则「负载源超时」看门狗会把刚重建的接管立刻判 stale 释放；③ **panic 自愈的重建分支同样要补 `vector → fast_lock.init()`**（vector 不注册 CLG 参数，漏掉就是频率零接管且无告警）。同理**启动即停摆时启动块必须跳过 `apply_affinity_and_corectl`**：`halted` 初值就是 `is_down()`，循环里那次「进入停摆」分支永远不执行，写进去的布局没人收
- WebUI：`data/down.ts`（解析，与 daemon 同口径）+ `contract/down.ts`（读写，tmp→rename）+ 配置页「高级设置」面板开关（`state.svelte.ts` 的 `[down]` 段，直接提交不走 meta 草稿）+ `data/mode.ts` 的 `kind: 'down'`（显示名 `down`、danger 信号，总览页自动呈现）
- **删除也是解除**（2026-09-16 审查后补）：`utils::DirWatcher` 默认掩码只有 `CLOSE_WRITE | MOVED_TO`，删文件不唤醒。为此加了 `DirWatcher::new_with_mask`，down 的监听用 `CLOSE_WRITE | MOVED_TO | DELETE`（配置/实验室链路保持默认掩码，它们删了会被自愈补建）。删掉 down.chr → 唤醒 → 读到缺失 → 补建模板 + 返回 false → 解除；补建那次事件不会引发第二轮（prev 已是 false）
- 已知不修：停摆期间屏幕事件被丢弃，`is_screen_on` 会陈旧；退出后靠 app_detect 的双源 verify 自愈（一个周期内纠正）
- **key 说明实际做了什么**（用户要求）：`vector` = 全局模式切 vector 档 + 关 FAS/scenemode/特调；`contingency` / `babel` / `frozen` = 预留（2026-09-16 改名：`fastmode`→`vector`、`socremode`→`contingency`，新增两个预留；**旧 key 不兼容**，残留会被判非法重置）。界面显示名与 key 解耦，走 i18n `lab.mode.*`；**2026-09-17 用户 WIP 推翻「中英同值英文」约定**：zh 现在用中文显示名（矢量突破/危機契約）+ 彩蛋 detail（小心地滑！/将数据压向极限…/巴别塔的恶灵/未获得访问许可），en 保持英文名（Vector Breakthrough/Contingency Contract）+ 对应英文彩蛋（Slippery floor! 等），各语言显示各自语言，勿再按旧约定「修正」zh。三个预留条目（影响项空映射）WebUI 显示「预留」且不给启用入口
- **⛔ 用户 WIP 不可擅自恢复（用户 2026-09-17 明令）**：用户手工注释/回退/简化的代码（如 `mode.ts` 的 `CLG_CATALOG` 刻意注释掉 `descKey`——CLG 档位**不显示描述**是有意的 UI 决策）不是「遗留问题」，审查时只能**报告**，修复前必须先问。教训：我曾把它当 svelte-check 报错的根因擅自恢复，被用户严厉制止。正确做法：保持 WIP 原样，只适配**我写的引用方**（describeMode 对 CLG 档返回空 descKey、UI 对空描述条件渲染），两边都绿
- WebUI 侧模式列表硬编码在 `webui/src/data/lab.ts::LAB_MODE_KEYS` + 两套 locale 的 `lab.mode.*`；新增模式要同时改 rhine-init.yaml、LAB_MODE_KEYS、locale、**`LAB_TAKEOVER`** 四处。`parseLabState` 必须与 `src/rhine.rs::parse_state` 同口径——**具体坑（2026-09-17 审查修）**：daemon 先剥整行注释（`#` 开头）、再把余下行交 serde_yaml，所以 ① 行内 ` #` 注释要由 WebUI 自己剥（`vector # x` daemon 视作 vector）；② 引号用 `trim_matches` 逐边剥（`"vector'` 也认），且**剥完不 trim 内侧**（`" vector "` 两边都判非法）。多 trim 一次就会裂成「界面说已启用、守护进程判非法」。`data/down.ts` 同源同坑（`" down "` 两边都必须判「不是停摆」）
- **接管表 `LAB_TAKEOVER`（2026-09-16 加，2026-09-17 更新）**：每个模式会写的 meta 开关（vector/contingency/babel = fas_enabled + scenemode_enabled，frozen 预留 = 空；与 rhine-init.yaml 影响项一一对应，单测真读 yaml 比对）。配置页据此把被接管的开关**置灰不可切换**并在 hint 里点名原因（`config.labTakenOver`）；判定优先 `app.labMode`、文件读不出模式时用锁标记里的模式兜底（那一刻 daemon 正按标记 reassert，置灰不能松）。`LAB_ENABLEABLE`（2026-09-17）= vector/contingency/babel（frozen 预留不给入口）。`commitLab` 成功后会清掉被接管字段的草稿，避免「待提交」标记骗人。**两个已修的坑（2026-09-17）**：① 单测那条「刚性断言」原先写死字面量、改 yaml 不会红 → 改成真读 `module/config/rhine-init.yaml` 比对（模式 key 集合 + 每模式的 meta 影响项）；② `ConfigView` 冷启动直接落 `#/config` 时 `app.isChiri` 还是 false（`loadOverview` 未 await）→ `loadLab` 被跳过、置灰失效 → 必须 await 后再判
- **改名/改字段必须两侧同步（2026-09-17 修）**：`module/config/feature.yaml` 的档位段名与 `src/chiri/config.rs`、**`src/scheduler/config.rs` 的 `Modes` 字段名 + `get_mode` 匹配值**都要同名——scheduler 侧漏改会让该段被静默忽略（`Modes` 没有 `deny_unknown_fields`），CLG 拿不到档位参数、回退默认（`fast` → `vector` 那次就是这么漏的）。**改名只针对「模式档位 key」**：`chiri/fast.rs` + `chiri/mod.rs` 的 `fast_lock`（极速档锁频器）、CLG 参数的 `down_fast_threshold` / `down_fast_mult`、i18n 的 `fast-*`（`[Fast]` 锁频器日志）、utils 的 `FastWriter`（sysfs 写入器，FAS/CLG 共用）全部保持原名。**唯一的例外（用户 2026-09-17 要求）**：FAS 内部的 `fast_decay_*` 与档位无关、留着会和「极速档」混淆，已改名 `steady_decay_*`（字段 + `d_sd_*` 默认值函数 + `apply_steady_decay` + `module/config/normal/fas-example.yaml` 的 key + README.en.md 参数表）
- **模式家族（2026-09-17 重构）**：CLG = reduce/default/boost（兜底档）；**stardust = scenemode/down（仅分组概念，不产生新模式值）**；rhine = vector/contingency/babel（仅实验室）。**contingency（危机合约）**：CPU+GPU 全核最高频（`src/chiri/gpu.rs::GpuGuard` 启动探测 devfreq——Adreno kgsl-3d0/名字含 gpu/kgsl/mali，available_frequencies 取硬件上限，探测不到跳过+warn）、performance 调速器（`src/chiri/governor.rs::GovernorGuard`：快照 scaling_governor → performance，release 恢复；启动 `cleanup_residue()` 清 SIGKILL 残留——performance 非任何厂商默认）、停线程迁移、后台压 0-1 小核、前台/顶部全核。**babel（规整化）**：停迁移，后台→小核、前台/顶部→大核+超大核、系统进程→大核。两者经 rhine-init（global_mode=自身 + fas/scenemode/special_tuned=false）驱动，行为在 `apply_affinity_and_corectl` 的 lab 分支（`AffinityManager::lab_static_apply/deactivate`：停迁移 + 组级 cpus，原值内部快照恢复，周期重入即纠偏）；governor/GPU 由 `sync_lab_governor_gpu` 在启动/ModeChange/DOWN 进出/FAS tick 接管/panic 收尾与重建同步（幂等）。**stardust 语义**：scenemode 期间停迁移、全部 cpuset 全核、仅压 CLG 频率上限（apply 的 scenemode 分支 release + core_ctl 回 NONE；`core_ctl.scenemode_offline` 字段停用仅保留兼容）。**customize.sh 热更新后不再自动重启调度**，要求手动 Action 启动
- **FAS 单实例 + 延迟进出（2026-09-17 重构，原多实例已废弃）**：`FasManager` 同一时刻至多一个白名单应用接管；activate 时 **GovernorGuard 把各 policy 调速器切 performance**（原值快照）；失去前台 `request_delayed_exit` 进 **15s 延迟期**（`FasRulesConfig.deactivate_delay_secs`，normalize 夹 1..=600）：mode 保持 fas、FAS 仍持有接管，期间切回白名单 activate 无缝续期；到期 1s tick 完成退出并按 `pending_mode_after_fas`（fas→X 的 ModeChange 被延迟时记录）重新接管——ModeChange 的 fas→非fas 分支**拦截在 mode_clone 更新之前**（避免「文件写 default、FAS 还持着频率」中间态）。`has_any_instance()` 保留但语义 = 活跃（延迟期内仍拦 scenemode）；reap/TTL 60s 已删

## FAS 帧指标口径（2026-09-16 核实）

- eBPF 只有一个探针：`uprobe` 挂在 `/system/lib64/libgui.so` 的 `android::Surface::queueBuffer`，每次只记录 `bpf_ktime_get_ns()`（帧提交时刻），无帧开始/结束配对
- 因此 FAS 的 `frame_delta_ns` = **相邻两次 queueBuffer 的时间间隔（帧间隔 / frame time）**，不是单帧生成耗时（GPU/CPU 渲染一帧的 duration）
- 推出：`current_fps = 1e9 / frame_delta_ns`；`actual_ms` 与 `1000/target_fps` 的 budget 比较；`heavy_frame_threshold_ms`、EMA、PID 误差全部建立在帧间隔语义上
- 过滤：`< min_frame_ns()`（= `1e9 / max_gear() / 2`）丢弃；`> fixed_max_frame_ms` 丢弃。fps_monitor 侧另有硬门 `MIN_FRAME_NS=1ms` / `MAX_FRAME_NS=200ms`，窗口 144 帧
- 投喂语义：只投喂本轮**真实新产生的**帧间隔（pending 队列全量取走），不重复投喂最新一条

## WebUI（2026-09-13 重写完成）

- 技术栈：Svelte 5 (runes) + TypeScript (`strict` + `noUncheckedIndexedAccess`) + Vite + `@yunyoujun/ak-ui@1.0.0`（CSS Core，218 类名 / 63 token，属 1.x 契约承诺）+ vitest。旧 Vue3/Vant/Pinia/vue-i18n/vue-router 已全部移除，无兼容层
- 分层：`src/kernel`（ksu 桥 + 可注入 `ShellRunner`）→ `src/contract`（paths/errors/read/meta/daemon/sources）→ `src/data`（status-csv、daemon-log、mode、whitelists、rules、module-info、apps）→ `src/views`（四屏）+ `src/components`。`src/dev/mock-shell.ts` 为无 ksu 时的设备替身，`?soc=chiri|yumi`、`?state=normal|empty|error`、`?daemon=running|stopped` 切换形态
- 自写约 40 行 hash 路由，**分两级**：`VIEW_IDS` 四项进底部导航，二级视图（当前只有 `lab` 实验室，从配置页底部进）不占导航位但同样有 hash（`#/lab`），导航高亮用 `navOwner()` 回落到父视图；自实现 runes i18n（界面语言与 `meta.language` 解耦，插值语法是 `{name}` 不是 fluent 的 `{ $name }`）；`.svelte.ts` 做共享状态免 Pinia
- 状态页底部的「导出历史日志」（2026-09-16）：`contract/export.ts` 后台 `nohup sh -c '…' &` 打包 devimp → `/sdcard/Download/devimp_<MMDD-HHmmss>.tar.xz`，前端 1.5s 轮询产物（上限 4 分钟）。xz 三级回退（`tar -J` → `busybox tar -J` → gzip，回退产物是 `.tar.gz`，UI 如实告知）；进度靠 `tar -v` 逐文件输出（`/dev/chiri_export.progress` 行数 ÷ `/dev/chiri_export.total`，封顶 99）+ `.part` 大小作已压缩量，一次 exec 读回三个数；排除 mtime 最新的 `*.log`（= 本次运行那个，daemon 还在写）；失败标记/排除列表放 `/dev` 而非 `/sdcard`（后者可能写不进去，会干等超时）。**`/sdcard/Download` 是系统自带目录，不要 mkdir**：不可写时三种 tar 都会失败并落到 `exit 4` 立即报错。视图：`OverviewView` 在「关闭调度」卡片之上
- 实验室页（`views/LabView.svelte`）只摆事实：「实验室模式（来自 rhine.chr）」与「设备当前模式（来自 current_mode.chr）」分两处显示、不做因果推断。ak-ui 的 `.ak-notice` 是**浅色底**，深色主题下不要用，提示块用 `--ak-surface-*` + `--ak-signal-action` 竖条自己拼
- 命令：`npm run dev` / `build` / `type-check`（svelte-check，替代 vue-tsc）/ `test`
- 实施定调：① 不引入 headless 库，交互用原生元素配 ak-ui token；② ak-ui 按钮/表单硬编码浅色（150×50、`#f3f4ef`），在 `[data-ak-ui]` 作用域内映射回 `--ak-*`；③ 配置改动先落草稿再一次性提交，一次写入触发一次全量热重载，写后回读；④ 设备形态分 chiri/yumi/unknown，读不到不下结论
- 未引入 `kernelsu` npm 包，用仓库内原有本地桥（TS 重写为 `src/kernel/ksu.ts`）。真机注意：`ksu.moduleInfo().moduleDir` 未在真机取证，路径异常先查这里；真机 `hasKsu()` 为 true，mock 不激活

## 构建契约

`cargo xtask build` 内跑 `webui/npm run build`，再把 `webui/dist` 原样拷进模块 `webroot/`。硬约束 `base: './'` 与 `type="module"`。CI：Node 24 + `npm install`。`module.prop` id = `chiri`。

## 仓库约定（用户明确）

- `mdocs/` 只放项目原有文档（`socList.md`、`updateWith.md`）。AI 产出的评估/计划/报告一律放 `.codebuddy/docs/`（已 gitignore）；`.codebuddy/memory/` 未忽略，是否提交由用户定
- 评估与准备不等于批准开工。用户没明确说"开始改"，就不创建也不修改源码文件。改动前先 `git status` 核对足迹，汇报时给文件级清单
- **只改任务范围内的东西，不「顺手修」**。工作区里未提交的改动、被注释掉的代码、看起来不完整的片段，都可能是用户正在进行的重构中间态（2026-09-16 踩过：`webui/src/data/mode.ts` 里 `CLG_CATALOG.descKey` 被用户刻意注释掉，我按类型检查报错把注释取消了，被指出后还原）。`svelte-check` / `cargo check` 报错若指向用户正在编辑的文件，只报告、不动手；确实需要动时先问
- 同目录下别人正在编辑的文件（如本次的 `webui/src/views/OverviewView.svelte`、`module/module.prop`）不要碰，汇报时明确区分「我改的」与「工作区里已有的」
- WebUI 重构是彻底重写：不保留旧 webui 代码和逻辑，接口业务正常即可
- 两个常驻技能（2026-09-16 用户挂载）：**humanizer-zh** 与 **token-efficient-coding**
  - 中文写作去掉 AI 腔：不写「此外/值得注意的是/这不仅仅是…而是…/…的证明/不断演变的格局」，少破折号、少粗体、不三段式、不用通用积极结尾；句子长短交错，直接给结论，承认不确定性时说人话
  - 编码省 token：文件头必须有 `//! xxx.rs - 区块索引: [a] [b]`（ASCII 方括号，禁用全角），Grep 命中锚点再定位；先 `Read(limit=30)` 看头部索引判断相关性，单任务累计 Read ≤200 行；用 Edit 局部改（old_string 带区块标记 + 唯一标识），**禁止 Write 整文件重写**；独立工具调用并行发起；shell 输出一律过滤/截断；对外回复 ≤60 字、只答所问、改动给 diff 不给全文
