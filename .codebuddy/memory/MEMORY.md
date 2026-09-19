# ChiRi 项目长期记忆

> AI 协作文件统一在 `.codebuddy/`：`AGENTS.md`（仓库根，按 `[tag]` 区块维护，勿通读）、本文件（长期事实，就地更新保持精简）、`YYYY-MM-DD.md`（按日追加）、`docs/`（AI 产出文档）。`mdocs/` 只放项目原有文档。旧 `.workbuddy/` 已废弃。开始工作前先读 `AGENTS.md` 与最近几天日志。
> 精简记录（2026-09-20）：原文 87KB 超注入上限，按「保留全部口径/同步点、压缩叙事」重写；逐次改动的经过留在当日日志与 git。

## 工具链环境（本机）

- Bash 无 coreutils；PowerShell 不回显 stdout。文件统计用 managed node：`C:/Users/Aibeto Zhu/.workbuddy/binaries/node/versions/22.22.2-3/node.exe`（可直接 `node x.ts`）。
- `npm` 被沙箱拦截。本地构建走 node 直驱 vite（chdir webui + `import('file:///…/vite/dist/node/index.js')` + `build({})`，~0.95s）。vite 体积单位 kB；esbuild 产物用**反引号**包字符串，dist 探测要同时匹配 反引号/单/双引号。
- 含反引号的文本必须 Write/Edit 写，不要经 `node -e` 走 shell。
- **检索禁用 `head_limit`**（曾因截断漏写入点）；穷举写出点用 `root\.join\(` 全局检索。
- **`agent-browser` 本机没装**（PATH、`~/.cargo/bin`、`%APPDATA%\npm` 都没有，npm 被拦）→ WebUI 改动无法浏览器走查，只能 `svelte-check` + `vitest` + vite 构建 + dist 字符串探测；汇报必须说明「视觉/交互未实测」。
- Rust 检查必须带目标：`cargo check -p chiri --target aarch64-linux-android`（default host target 会因 netlink-sys/aya-obj 缺 libc 符号失败，非本项目问题）。

## 守护进程契约

来源：`common.rs` / `main.rs` / `logger.rs` / `chiri/*` / `scheduler/*` / `monitor/app_detect.rs`。

### 文件接触点（15 个）

`daemon.lock`（daemon 自持单实例锁，WebUI 不读写；存活判据已改心跳）· `LiveTime.chr`（只读心跳，daemon 每 15s 写本地时间 `MM:SS`，差 >20s 判已停止）· `active_config.chr`（只读，生效 meta 相对路径）· `config/{rel}`（读写）· `rules.yaml` / `special_tuned.yaml` / `fas_whitelist.yaml`（只读，仅 Chiri 生成）· `current_mode.chr`（只读，模式 id）· `logs/daemon.log`（只读，仅本次运行）· `logs/status.csv`（+`.1`）· `logs/watchdog.pid`（读+删）· `rhine.chr`（读写，实验室）+ `rhine-back.chr`（只读快照）· `down.chr`（读写，写了 `down` = 停摆）· `PowerAVG.chr`（只读，功耗参考/平均）。附：`devimp/`、`logd/`、`config/{soc}/`。

读取失败三态必须分开：正常空值 / 合法缺失（非 Chiri 无白名单、Yumi 无 status.csv）/ 读取失败（要在界面明确报错，不能伪装成空值）。

### meta.yaml 写侧

- 严格层 `common::MetaYamlFile` + `deny_unknown_fields`；**2026-09-18 定稿：字段全部可选**（`Option<T>`，缺省 = 沿用内嵌默认），**出现即校验**（loglevel∈OFF/ERROR/WARN/INFO/DEBUG/TRACE 大小写不敏感、language∈zh/en、name/author trim 后非空、开关键布尔、power_max_w 数字且越界回退 12）。
- 可写字段：`loglevel` `language` `dev_record` `fas_enabled` `scenemode_enabled` `thread_bind` `power_avg` `notify`（+ 电池 `oplus_chg` `oplus_dual_cell` `voltage_double` `current_double` `voltage_divisor` `current_divisor`，旧键 `unit_divisor` 仍接收兜底）。
- 非法后果：`sync_meta_snapshot`（启动 + 每次热重载前）用内嵌默认整份覆盖并追加 `# 上一次修改存在非法字段…`，用户其它改动一并丢失。
- **新增字段同步点**：`MetaYamlFile`/`ExternalMetaOverrides` + `chiri::config::Meta` 与 `Config::load` 的覆盖合并 + 四个 meta.yaml 模板 + WebUI `contract/meta.ts`（META_FIELDS/WRITABLE_FIELDS/布尔列表）+ `tests/contract.test.ts` 样本。**nofix 门控**（进程级 `NOFIX`，`read_nofix_flag` 必须早于自愈）管他自己与 sync。
- 双层解析：严格层仅 sync 用；容错层 chiri::config::Meta（`#[serde(default)]`）、scheduler::config::Meta（只 loglevel/language）。`fas_enabled`/`scenemode_enabled`/`thread_bind` 缺省 true。

### 存活判定与关闭

- daemon `flock(LOCK_EX|LOCK_NB)` 模块根 `daemon.lock`，拿不到即 `exit(0)`；fd 故意泄漏持锁到退出。锁被持有 ⇔ 有活跃 daemon（探测须取到即释放）。`pidof` 只作弱信号。
- 关闭顺序：**先按 `logs/watchdog.pid` 杀看门狗 → 再杀 chiri**；pidfile 缺失时只杀 bin 会被 3s 拉起；杀完 flock 复核。
- 看门狗：`service.sh`/`action.sh` 前台 while 跑 daemon，任何方式退出都重启（退避 3→10→30→60s，活过 60s 归零）。触发：main/monitor_core panic、chiri 调度线程连续 panic >5 次、日志门限 `exit(0)`；监控子线程 panic 由 `spawn_guarded` 落盘后 `exit(1)`。硬挂无探测。

### 热重载

- `config_watcher` 监听**生效配置的父目录**（inotify 不递归），`CLOSE_WRITE|MOVED_TO`，`utils::DirWatcher` **跨重载复用同一实例 + 按文件名过滤**，命中后 100ms 静默 + 清空积压。同目录 tmp→rename 原子替换受契约支持（WebUI 后缀 `.webui.tmp`，避开 daemon 的 `<name>.tmp`）。
- 链路：事件 → `sync_meta_snapshot` → `Config::load` → `update_level` → 语言变更 `load_language` → `apply_system_tweaks`（**DOWN 期间跳过，见下**）+ 置 config_dirty。
- **多节点 / 周期性写入口径（2026-09-18 定稿）**：一律走 `utils::write_nodes(items, what)`——逐节点失败 debug、全部失败才 warn、成功复位；**告警键按操作切分**（两操作共用一键会互相清记录）；**不做 `Path::exists()` 预判**（写入失败即证据）。
- 外部命令（`Command::new`）一律丢 stdout/stderr（`Stdio::null()`），否则工具用法文本灌进 daemon.log。

### 模式 id 值域

`reduce` `default` `boost` `vector` `fas` + 特调（`akmode` 游戏 / `playback` 视频；`daily` 组已移除）+ `down`（停摆期写的值）。`scenemode` 不产生模式值（独立息屏轴）。daemon 停止后 `current_mode.chr` 是陈旧值。
- **UI 家族**：clg / special / lab / **down** / **stardust**（=scenemode 家族位）/ fas / unknown；同步面 = `data/mode.ts` + 两语 locale + `tests/i18n.test.ts` + AGENTS。down 是独立家族（不是 stardust）。
- **二进制名 chiri（原 yumi）**：Cargo 包名 / 产物 / `core/bin/chiri` / `DAEMON_PATH` / 进程名（各脚本 killall、customize 的 pidof）/`WebUI stopScheduler`/`app_detect` 黑名单/i18n/`.vscode`/mock 日志都要一致。**刻意保留**：`yumi-ebpf`、`YUMI_SKIP_EBPF`、`src/scheduler/`（Yumi 调度器）、设备形态枚举 `'yumi'`、README 上游 fork 引用。
- 档位段名必须三侧同名：`feature.yaml` 段名 + `chiri/config.rs` + `scheduler/config.rs` 的 `Modes` 字段与 `get_mode`（后者无 deny_unknown_fields，漏改会静默忽略整段）。

### UI 原理上不可知

`is_special_tuned_available()` 还取决于嵌入 `tuned_profiles.yaml` 是否加载成功；`fas_available()` 要求白名单非空且至少一个 app 配置解析成功——白名单命中 ≠ 生效。UI 只陈述已知事实，生效态以 `current_mode.chr` 观测值为准。

### 日志归档与量级

- `daemon.log` 单文件 ≤50MB + 3 备份；行格式 `[YYYY-MM-DD HH:MM:SS] [LEVEL] [module] msg`（本地时间），模块剥 crate 前缀。
- `status.csv` ≤8MB + `.1`，每秒一行 **22 列**（末列 `fps` 仅 FAS 激活时有值）；新增列一律追加末尾。列全精度写入、显示层 `toFixed(1)`。
- 两者都不可整读。`devimp_<pkg>_<ts>.log`（Chiri 专属）。
- 每次启动：先判短会话（首行时间距启动 <30s → 清空不打包；解析失败按非短会话），再回收遗留 staging 并把上轮打包进 `logd/`（`pack.sh` 走外部 tar，**不得改**；导出=logd→tar→**gzip**→删中间产物，无 gzip 保留未压缩 tar）。
- 预算：`logd/`、`devimp/` 各自 >128MB 只删本目录最旧到 <96MB（最新一个永不删）。打包门限：写路径记账 ≥16MB 即 `exit(0)` 交给看门狗归档。

### 功耗参考/平均（PowerAVG）

- 链路：status.csv 行后顺序 `power_avg_update`，更新进程级静态 + 原子写一行两位小数；文件是输出、**不读回**， daemon 重启从零且**启动时清空**。
- 口径：参考 `(旧×10+新)/11`；平均 `(旧×次数+新)/(次数+1)`（次数两模式共享）。
- 样本：`charge_state=="discharging" && (!use_average || is_screen_on)`（曾互换写反）。跳过时按原因变化打一次 `power-avg-skip` 并好后清空——**周期性采样的静默跳过必须留痕**。

### 实验室（rhine）

- 三文件：`rhine-init.yaml`（嵌入，不落盘、WebUI 不读）/ `rhine.chr`（对外可手改，内容即 key）/ `rhine-back.chr`（daemon 写的原值快照，存在 = 上轮启用过）。影响项缺省 = 不变更；`global_mode` 走运行时覆盖，三开关改写 meta.yaml（用 `replace_top_level_bool` + `rewrite_meta_toggles`，**绝不 serde 重排**）。
- `rhine::converge` **先还原后套用**；覆盖层在 `common.rs`（`LAB_GLOBAL_MODE` + `LAB_SPECIAL_TUNED_DISABLED`），只改兜底不动白名单/app_modes 优先级。
- **运行时锁定**：启用不改自用——套用成功写 `/tmp/chiri-labs.lock`（退 `/dev`），标记存在期间不还原、以 rhine.chr 为意图来源做 reassert（**绝不重写 rhine-back**）。重启 tmpfs 空 + service.sh 删 rhine.chr 才还原。
- **保留字 `off`** = 强制关闭（UI 走风险确认），必须**在 YAML 解析前字面量比较**（YAML 1.1 里 off 是布尔）。
- 篡改痕迹：标记首行模式名 + 其后 `# 代号` 异常记录（代号而非文案，因可能早于 `load_language`）；WebUI `labWarnings` 映射。
- WebUI 模式列表硬编码（`LAB_MODE_KEYS` + `lab.mode.*` + `LAB_TAKEOVER` + rhine-init 四处同步）；`LAB_ENABLEABLE` = vector/contingency/babel。key 说明实际做了什么；**2026-09-17 起 zh 用中文名 + 彩蛋、en 用英文名**，勿再按旧约「修正」。

### DOWN 停摆

- 判据 `down.chr`（模块根，对外可手改；写了 `down` = 停摆，空/注释/其它 = 正常），`src/down.rs` 维护进程级 `AtomicBool` + `down_watcher`（掩码含 DELETE，删文件也算解除；缺失补建模板，读不到按不停摆）。
- 行为：释放全部接管（CLG/特调/FAS/fast_lock/affinity/core_ctl/scenemode），`current_mode.chr` 写一次 `down` 且**不再覆盖**（投影保持）；eBPF/status.csv/daemon.log/devimp 照常，app_detect 照常判前台。目的是记录「ChiRi 不工作时」的系统基线。
- 实现在调度循环（governor 归它独占）。必须跳过的三处：5s 模式文件自愈（只推进计时，否则忙循环）、config_dirty 下发、2s 亲和/热保护块；事件在 `match msg` 前 `if halted { continue }`（**不能放在 recv 之前**）。**屏幕事件例外**：停摆期只吸收 ScreenStateChange 更新本地状态。devimp 开关同步与停摆无关（必须在门控之外）。
- 退出：删模式文件 + 模式回到**实时判定**（`app_detect::last_determined_mode()` 优先，空则回退快照），**并立即全量重建接管**（含 fas/特调重建）；`last_load_event` 复位；panic 自愈重建分支同样含 `vector → fast_lock.init()`。
- **2026-09-20 补漏（「开了 DOWN 调度仍在被改」）**：`apply_system_tweaks`（cpuidle / IO / cpu_boost 输入升频 / 内核 Sched 参数）不在任何 governor 快照链里，此前完全无停摆感知（启动时先于 DOWN 判定下发、热重载由另一线程无条件重放）。现：`CpuScheduler` 加 `[snapshot]`（首次写前记原值、重复下发不覆盖、IO `queue/scheduler` 剥方括号）+ `restore_system_tweaks()`，进入 DOWN 还原、退出补发；判定收口在**唯一入口** `apply_system_tweaks` 内部（调用方跨线程，拿不到 `halted`）并用静态闸 `tweaks_gate()` 与还原互斥、闸内复查标志（否则有「判完就被切进 DOWN」的窗口）；down_watcher/on_startup 上移到首次 tweaks 之前；循环内 fas tick/1s 兜底/触摸/fast_lock.tick 与 panic 重建块改显式 `!halted`。**教训**：凡「写 sysfs 但不属于某个 governor」的面，新增时必须自己接停摆语义（详见 `docs/agents/04-hard-lessons.md`）。
- **未提交 WIP（用户 2026-09-20，勿擅自改）**：① feature.yaml `Sched` 段（8550/8475/8998 默认开启 `sched_migration_cost_ns=200000`、`sched_nr_migrate=27`），按 `SCHED_ALLOWED_PARAMS` 白名单写 `/proc/sys/kernel/*`；**root 兜底 feature.yaml 无该段（默认关）**，与三 SoC 不一致。② 子进程名（`com.x:push`）**一律预处理归一为主包名**（用户口径：子进程本质上就是那个包），归一后走原本算法：前台名在唯一入口 `app_detect::set_current_package` 归一、规则键在 `deserialize_app_modes` 归一（冲突保留后者 + `rule-key-conflict` 告警）；`determine_mode` 就是精确查表，**不要**再加「完整名→剥后缀」回退链（会让两套粒度语义并存）。③ `module/system.prop` 的 `persist.sys.horae.enable=1→0` 与上述无关，提交前确认。

## ChiRi 调度要点

- **动态限频**：CLG 与特调同构——写 schedutil、min 压硬件最低、只调 max；按核心组 `up/down_core_count|util_percent` 判定，util=0 不计升频、计入降频；抖动 wait_ms 防抖。
- **特调参数组**：`Config.tuned_profiles: HashMap<模式名, SpecialTunedConfig>`，`get_tuned_profile(mode)` 缺省回退 `akmode` 段；新增模式 = special_tuned.yaml 条目 + tuned_profiles 同名参数组（无需改 .rs）；缺参数组由 `merge_tuned_profiles` 打 `tuned-profile-missing`（否则静默回退游戏参数）。**唯一保留的 `akmode` 是模式名本身**。命名族：`tuned_profile(s)` 词根、`special_tuned_*` 标志，新增标识符前必须全仓库搜重名。`boost_affinity`=boost 类亲和（游戏 true，省电型必须 false）；`util_smoothing`=负载 EMA（抖动下无平滑的上限均值反而高于 CLG）。
- **降频计时**：`down_target` 只在目标明显回升（>+hyst）时重置计时，下探只更新目标。
- **白名单 `re:` 解析**：必须 `strip_prefix("re:")` 再按 `:` 切分（否则正则条目从未命中且模式名被污染）。特调全集含正则条目，导出的 yaml 只含精确条目 → UI 只能近似。
- **FAS 单实例 + 延迟退出**：activate 时 GovernorGuard 切 performance；失前台 `request_delayed_exit` 持策略 15s（`deactivate_delay_secs` 夹 1..=600），到期 1s tick 完成退出并按 `pending_mode_after_fas` 重接管；ModeChange 的 fas→非fas 拦截在 mode_clone 更新前。息屏与 FAS 完全解耦（息屏不再释放）。
- **帧指标口径**：eBPF 只有一个 uprobe（`Surface::queueBuffer`），`frame_delta_ns` = **相邻帧间隔**（非单帧耗时）；必须只投喂新产生的帧；曾因单条重复投喂把时间尺度拉长 ~6 倍。
- **息屏轴**：scenemode 停迁移 + cpuset 全核 + 只压 CLG 上限；判定走屏幕**投票仲裁**（`OFF_QUORUM=2`，只统计有效读数）。
- **后台降权**：`AffinityConfig.background_uclamp_max_pct`（默认 50）钳后台/受限组 `cpu.uclamp.max`（不含 system-background）；`affinity_blacklist.yaml` 含 SystemUI/桌面（命中即保持全核）。
- `thread_bind` 是线程摆放总闸（与机型内嵌开关取与），关闭 = 全部绑定改全核。

## 8550 实测结论

- 整夜 5h default 档：亮屏均 1.35W；bili 1.27W；游戏 2.48W；息屏 0.07-0.09W。游戏段 65% 时间被 vendor 热限流（cap 70%）而 ChiRi 未参与 → 压温度可减少 vendor 限流。
- **devimp 列语义**：`cur_freq_khz` = CLG 设的**动态上限**（scaling_max_freq），`max_freq_khz` = 硬件最高，**日志里没有实际频率**；`batt_current_ma` 值其实是安培（历史列名）；wakeups/migrations 是累计值；`fps` 列全空。
- 回放结论：无平滑特调在抖动负载下上限均值反而高于 CLG；EMA + hr 1.0 才能把 little/big 压到 0.90-0.96（prime 压不动）。**调度层剩余空间是个位数 %，「平均降一半」必须靠屏幕/刷新率/解码器层。**

## WebUI

- 技术栈 Svelte 5 runes + TS strict + Vite + `@yunyoujun/ak-ui@1.0.0` + vitest（旧 Vue/Vant/Pinia 全移除）；命令 `npm run dev/build/type-check/test`。
- 分层 `kernel`（ksu 桥 + 可注入 ShellRunner）→ `contract` → `data` → `views`/`components`；`src/dev/mock-shell.ts` 为替身。
- hash 路由分两级（`VIEW_IDS` 四项导航，二级如 `#/lab`/`#/battery` 用 `navOwner()` 回落）；自实现 runes i18n（插值 `{name}`）。
- **样式**：优先直接用 ak-ui 官方原语（Panel→ak-card、ConfirmSheet→ak-dialog、LiveStatus→ak-status、Segmented→ak-segmented、ToggleField→ak-choice--switch、StateBox→ak-notice、进度→ak-progress/`ak-gauge`、字段→ak-form-stack+ak-field），适配集中在 `app.css [ak-adapt]`。保留自研：`.btn*`、`.kv`、`.u-*` 工具、`.log*`/`.snapshot*`/`.terminal*`/`.nav*`/`.commit*`。
- **级联硬规则**：组件局部类（带 svelte hash）可安全覆盖全局工具类，不依赖顺序；纯工具类之间 `app.css` 内 `.u-note` 必须排在 `.u-mt-*` 之前。新增通用样式前先查是否已有。
- **i18n 双语**：zh 是唯一语义源，改文案两边同步；`i18n.test.ts` 不校验语义（英文漂移测不出）。风格：无句尾句号、不用破折号、避免 AI 腔、句尾不留句号。**UI 文案只留信息量**（重复 hint 只留一处、无信息量 toast 不写、纯派生显示不单列）。
- **信号色成套**：带语义强调色必须一套用到底（不得「红条配蓝字」）。
- **交互定值**：日志/快照限高 60vh 子滚动 + 自动跟随底部（用户滑离即退出，滚回不自动恢复，点「回到底部」才恢复）；状态码/日志页每秒自刷新 + 失败 toast 节流；`.u-stack` 必须 `grid-template-columns: minmax(0,1fr)`；回到底部按钮**只在滑离时渲染**、相对定位锚在子块内；关闭用 `ksu.exit`。
- 读取文件前必须 `[ -f ]` 守卫（重定向错误 shell 级，`2>/dev/null` 盖不住）。导出 history：`pack.sh` + `/sdcard/Download`（系统目录不要 mkdir），前端轮询产物；压缩用 gzip（xz 在设备上太慢）。

## 构建契约

`cargo xtask build` 先跑 `webui/npm run build` 再拷 `webui/dist` 到模块 `webroot/`；硬约束 `base:'./'` + `type="module"`。CI Node 24。`module.prop` id = `chiri`。webui dist 由根 `build.rs` 嵌入（`restore_webroot` 启动时补齐，缺 dist 则降级）。

## 仓库约定（用户明确）

- `mdocs/` 只放项目原有文档；AI 产出放 `.codebuddy/docs/`（已 gitignore）；`.codebuddy/memory/` 未忽略。
- 评估与准备 ≠ 批准开工。没明确说"开始改"就不建不改源码；改动前 `git status` 核对足迹，汇报给文件级清单。
- **只改任务范围内的东西，不「顺手修」**：未提交改动、被注释的代码可能是用户 WIP 中间态（2026-09-16 踩过 `mode.ts` 的 `descKey` 注释）。检查报错若指向用户正在编辑的文件，只报告、不动手。
- 不碰别人正在编辑的文件（如 `OverviewView.svelte`、`module/module.prop`），汇报要区分「我改的」与「工作区里已有的」。
- 常驻技能 **humanizer-zh** 与 **token-efficient-coding**：中文去 AI 腔；文件头必须 `//! x.rs - 区块索引: [a] [b]`（ASCII 方括号），先读头部再定位、单任务 Read ≤200 行、Edit 局部改禁整文件重写、工具调用并行、shell 输出过滤、回复精简给 diff 不给全文。
