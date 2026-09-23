# ChiRi 长期记忆

> 本目录为 CodeBuddy 环境记忆（2026-09-23 起协作文件按环境分文件夹，见 `AGENTS.md`）：`AGENTS.md`（仓库根 `[tag]` 索引，勿通读）、本文件（长期事实，就地更新）、`YYYY-MM-DD.md`（按日追加）、`docs/`（AI 产出）。`mdocs/` 只放项目原有文档。旧 `.workbuddy/` 已废弃。开工前读 `AGENTS.md` 与最近几天日志。2026-09-23 前的历史条目保留于此，各环境只读；Cursor 的记忆在 `.cursor/memory/`。

## 工具链（本机）

- 无 coreutils；PowerShell 不回显 stdout。统计用 managed node：`C:/Users/Aibeto Zhu/.workbuddy/binaries/node/versions/22.22.2-3/node.exe`（`node x.ts`）。
- `npm` 被沙箱拦；构建用 node 直驱 vite（chdir webui + `import('file:///…/vite/dist/node/index.js')` + `build({})`）。vite 单位 kB；esbuild 用反引号包串，dist 探测需同配反引号/单/双引号；含反引号文本必须 Write/Edit 写。
- **检索禁用 `head_limit`**；穷举写出点用 `root\.join\(`。
- **`agent-browser` 没装** → WebUI 改动只能 svelte-check+vitest+vite 构建+dist 探测，汇报须声明「视觉/交互未实测」。
- Rust 检查带目标：`cargo check -p chiri --target aarch64-linux-android`（host target 因 netlink-sys/aya-obj 缺 libc 符号失败）。
- **eBPF 检查口径**：本机 build.rs 无 bpf-linker → `write_ebpf_stub` 占位（日志「⚠ 跳过 eBPF 编译」才是真相，「✅ 编译成功」是假象）。探针源码本机唯一验证：`cd yumi-ebpf && cargo +nightly check --target bpfel-unknown-none -Z build-std=core`；verifier/真产物靠 CI。

## 守护进程契约

来源：`common.rs`/`main.rs`/`logger.rs`/`chiri/*`/`scheduler/fas/*`/`monitor/app_detect.rs`。

### 文件接触点

`daemon.lock`（单实例锁，WebUI 不读写）· `LiveTime.chr`（只读心跳，15s 写 `MM:SS`，差 >20s 判停止）· `active_config.chr`/`current_mode.chr`/`PowerAVG.chr`（只读）· `config/{rel}`（读写）· `rules.yaml`/`special_tuned.yaml`/`fas_whitelist.yaml`（只读，仅 Chiri 生成）· `logs/daemon.log`（只读，仅本次运行）· `logs/status.csv`(+`.1`) · `logs/watchdog.pid`（读+删）· `rhine.chr`（读写）+`rhine-back.chr`（快照）· `down.chr`（读写，`down`=停摆）。附：`devimp/`、`logd/`、`config/{soc}/`。

读取失败三态必须分开：正常空值 / 合法缺失（非 Chiri 无白名单、无 status.csv）/ 读取失败（界面明确报错，不得伪装成空值）。

### meta.yaml 写侧

- 严格层 `common::MetaYamlFile`+`deny_unknown_fields`；字段全可选（缺省沿用内嵌默认），出现即校验（loglevel 五级大小写不敏感、language∈zh/en、name/author trim 非空、开关键布尔、power_max_w 数字越界回退 12）。
- 可写字段：`loglevel` `language` `dev_record` `fas_enabled` `scenemode_enabled` `thread_bind` `power_avg` `notify` + 电池 `oplus_chg` `oplus_dual_cell` `voltage_double` `current_double` `voltage_divisor` `current_divisor`（旧键 `unit_divisor` 兜底）。
- 非法后果：`sync_meta_snapshot`（启动+每次热重载前）用内嵌默认整份覆盖并追加 `# 上一次修改存在非法字段…`，用户其它改动一并丢失。
- **新增字段同步点**：`MetaYamlFile`/`ExternalMetaOverrides` + `chiri::config::Meta` 与 `Config::load` 覆盖合并 + 四个 meta.yaml 模板 + WebUI `contract/meta.ts` + `tests/contract.test.ts`。**nofix 门控**（进程级 `NOFIX`，`read_nofix_flag` 必须早于自愈）。
- 双层解析：严格层仅 sync 用；容错层 chiri::config::Meta（`#[serde(default)]`）、scheduler::config::Meta（只 loglevel/language）。`fas_enabled`/`scenemode_enabled`/`thread_bind` 缺省 true。

### 存活判定与关闭

- daemon `flock(LOCK_EX|LOCK_NB)` 模块根 `daemon.lock`，拿不到即 `exit(0)`；fd 故意泄漏持锁到退出。`pidof` 只作弱信号。
- 关闭顺序：**先按 `logs/watchdog.pid` 杀看门狗 → 再杀 chiri**（缺 pidfile 只杀 bin 会被 3s 拉起）；杀完 flock 复核。
- 看门狗：`service.sh`/`action.sh` 前台 while 跑 daemon，任何退出都重启（退避 3→10→30→60s，活过 60s 归零）。触发：main/monitor_core panic、调度线程连续 panic >5、日志门限 `exit(0)`；监控子线程 panic 由 `spawn_guarded` 落盘后 `exit(1)`。硬挂无探测。

### 热重载

- `config_watcher` 监听生效配置父目录（inotify 不递归），`CLOSE_WRITE|MOVED_TO`，`utils::DirWatcher` 跨重载复用，命中后 100ms 静默+清积压。同目录 tmp→rename 原子替换受契约支持（WebUI 后缀 `.webui.tmp`，避开 daemon 的 `<name>.tmp`）。
- 链路：事件 → `sync_meta_snapshot` → `Config::load` → `update_level` → 语言变更 `load_language` → `apply_system_tweaks`（DOWN 期间跳过）+ 置 config_dirty。
- **多节点/周期性写入**一律走 `utils::write_nodes(items, what)`：逐节点失败 debug、全失败才 warn、成功复位；**告警键按操作切分**；不做 `Path::exists()` 预判。
- 外部命令（`Command::new`）一律 `Stdio::null()`，否则用法文本灌进 daemon.log。

### 模式 id 与命名

- 模式值域：`reduce` `default` `boost` `vector` `fas` + 特调（`akmode`/`playback`）+ `down`。`scenemode` 不产生模式值（独立息屏轴）。daemon 停止后 `current_mode.chr` 是陈旧值。
- **UI 家族**：clg / special / lab / down / stardust（=scenemode 家族位）/ fas / unknown；同步面 = `data/mode.ts` + 两语 locale + `tests/i18n.test.ts`。
- **二进制名 chiri（原 yumi）**：Cargo 包名/产物/`core/bin/chiri`/`DAEMON_PATH`/脚本 killall、pidof/WebUI `stopScheduler`/app_detect 黑名单/i18n。**刻意保留**：`yumi-ebpf`、`YUMI_SKIP_EBPF`、设备形态 `'yumi'`、README 上游引用。（Yumi 调度本体已删 2026-09-22，`src/scheduler/` 只剩 FAS 引擎 + policy 工具。）
- 档位段名与 `feature.yaml` 段名、`chiri/config.rs` 的 `Modes`/`get_mode` 同名（无 deny_unknown_fields，漏改静默忽略整段）；原 Yumi 侧同名结构已随调度本体删除。
- **feature.yaml 的 CLG 段只归 `chiri::config` 消费**：`smoothing_down / slow_down_scale / down_fast_mult` 是 Yumi 参数，已随 Yumi 调度本体删除并从 root/8745 feature.yaml 移除（2026-09-22），`util_smoothing` 归 chiri。判死键先确认消费方是否已随子系统删除，勿只对现存结构体。
- UI 原理上不可知：`is_special_tuned_available()` 还取决于嵌入 `tuned_profiles.yaml` 加载成功；`fas_available()` 要求白名单非空且至少一个 app 配置解析成功——白名单命中 ≠ 生效。生效态以 `current_mode.chr` 观测值为准。

### 日志与预算

- `daemon.log` ≤50MB + 3 备份；行格式 `[YYYY-MM-DD HH:MM:SS] [LEVEL] [module] msg`（本地时间），模块剥 crate 前缀。
- `status.csv` ≤8MB + `.1`，每秒一行 **22 列**（末列 `fps` 仅 FAS 激活有值）；新增列一律追加末尾；全精度写入、显示层 `toFixed(1)`。两者都不可整读。
- 启动：先判短会话（首行距启动 <30s → 清空不打包；解析失败按非短会话），再把上轮打包进 `logd/`（`pack.sh` 走外部 tar，**不得改**；导出 = logd→tar→**gzip**→删中间产物）。
- 预算：`logd/`、`devimp/` 各自 >128MB 删本目录最旧到 <96MB（最新永不删）。写路径记账 ≥128MB 即 `exit(0)`。
- **定版手段（离线第一件事）**：devimp 文件头三行 `# module=ChiRi Canary <ver> (versionCode N)` / `# soc=… board=… model=…` / `# android=… kernel=…` 一直存在；daemon.log 另有 `[Main] 模块版本:` 行（2026-09-22 新增）。**同名 tar 里可能混着不同版本/不同机型**，先定版再比数据。
- devimp/ 双文件（2026-09-22 拆分，原 devimp*<pkg>* 单文件）：`main_<pkg>_<MMDD-HHmmss>.log`（44 列 CSV v2026-09-22，tick/snap/event）+ `aff_<MMDD-HHmmss>.log`（文本帧：@A 动作帧含 `result=ok|e{errno}`、@S 每秒快照帧 top-N〔meta `devimp_top_n` 缺省 10 clamp 1..=64〕+前台/被管线程下钻；`\x01` 保留二进制帧位）。目录/归档 tar/记账键保留 devimp 名；锁 6 把（+AFF_TH_STATE）；t 行 pid=0=归属未知、uclamp 恒 -1。

### PowerAVG

- status.csv 行后顺序 `power_avg_update`；文件是输出**不读回**，重启从零且启动清空。
- 口径：参考 `(旧×10+新)/11`；平均 `(旧×次数+新)/(次数+1)`（次数两模式共享）。
- 样本：`charge_state=="discharging" && (!use_average || is_screen_on)`。跳过按原因变化打一次 `power-avg-skip`，好后清空——静默跳过必须留痕。

### 实验室（rhine）

- 三文件：`rhine-init.yaml`（嵌入，不落盘、WebUI 不读）/`rhine.chr`（可手改，内容即 key）/`rhine-back.chr`（原值快照，存在=上轮启用过）。影响项缺省=不变更；`global_mode` 运行时覆盖；三开关改写 meta.yaml（`replace_top_level_bool`+`rewrite_meta_toggles`，**绝不 serde 重排**）。
- `rhine::converge` **先还原后套用**；覆盖层在 `common.rs`（`LAB_GLOBAL_MODE`+`LAB_SPECIAL_TUNED_DISABLED`），只改兜底不动白名单/app_modes 优先级。
- **运行时锁定**：套用成功写 `/tmp/chiri-labs.lock`（退 `/dev`），存在期间不还原、以 rhine.chr 为意图来源 reassert（**绝不重写 rhine-back**）。重启 tmpfs 空 + service.sh 删 rhine.chr 才还原。
- **保留字 `off`** = 强制关闭（UI 走风险确认），必须在 YAML 解析前字面量比较（YAML 1.1 里 off 是布尔）。
- 篡改痕迹：标记首行模式名 + 其后 `# 代号`（代号而非文案，可能早于 `load_language`）；WebUI `labWarnings` 映射。
- WebUI 模式列表硬编码（`LAB_MODE_KEYS`+`lab.mode.*`+`LAB_TAKEOVER`+rhine-init 四处同步）；`LAB_ENABLEABLE` = vector/contingency/babel。**zh 用中文名+彩蛋、en 用英文名**（2026-09-17 起），勿按旧约「修正」。

### DOWN 停摆

- 判据 `down.chr`（模块根，可手改；`down`=停摆，空/注释/其它=正常），`src/down.rs` 维护进程级 `AtomicBool`+`down_watcher`（掩码含 DELETE；缺失补建模板，读不到按不停摆）。
- 行为：释放全部接管（CLG/特调/FAS/fast_lock/affinity/core_ctl/scenemode），`current_mode.chr` 写一次 `down` 不再覆盖；eBPF/status.csv/daemon.log/devimp 照常，app_detect 照常判前台。
- 实现在调度循环（governor 归它独占）。必须跳过：5s 模式文件自愈（只推进计时）、config_dirty 下发、2s 亲和/热保护块；事件在 `match msg` 前 `if halted { continue }`（**不能放 recv 之前**）。屏幕事件例外：只吸收 ScreenStateChange。devimp 开关同步在门控之外。
- 退出：删模式文件 + 模式回实时判定（`app_detect::last_determined_mode()` 优先，空则回退快照）+ **立即全量重建接管**（含 fas/特调）；`last_load_event` 复位；panic 自愈重建分支含 `vector → fast_lock.init()`。
- **2026-09-20 补漏**：`apply_system_tweaks`（cpuidle/IO/cpu_boost/内核 Sched）不在 governor 快照链，现 `CpuScheduler` 加 `[snapshot]`+`restore_system_tweaks()`，进入 DOWN 还原、退出补发；判定收口在唯一入口内 + 静态闸 `tweaks_gate()` 与还原互斥；down_watcher/on_startup 上移到首次 tweaks 之前；循环内 fas tick/1s 兜底/触摸/fast_lock.tick 与 panic 重建块显式 `!halted`。**教训**：凡「写 sysfs 但不属于某个 governor」的面，新增时必须自己接停摆语义（`04-hard-lessons.md`）。
- 可观测性：开机即停摆时状态机不走「进入」分支，与调度线程未启动同形 → 启动打 `down-boot-halted`（warn）+ 每 300s `down-heartbeat`（info）。

### eBPF 探针契约（2026-09-20 起）

- 逐核状态由**单个** `PerCpuArray<CoreState>`（map `CORE_STATE`）：`#[repr(C)]`，u64×3（last_time/idle/busy）+ u32×2（cur_tid/cur_tgid）= 32 字节无 padding。**两侧定义必须同批发布**（`yumi-ebpf/src/main.rs` 与 `src/monitor/cpu_monitor.rs`），布局不一致会错位。原五个独立 map 已删除，勿引用。
- Pod：用户态 `unsafe impl aya::Pod for CoreState`（aya 0.14 自带，非 bytemuck）；内核侧 aya-ebpf 0.2.1 `PerCpuArray<T>` 无 trait 约束。
- `THREAD_ACCT`（`Array<u32>`）= 线程记账开关：用户态在 `is_chiri_soc() && fas_available()` 置 1（同 `FAS_FG_UTIL_ENABLED`）；关闭时探针跳过 `THREAD_RUN_TIME`（32768 项）hash 累加——该 map 只被「TGID 主路径失败」降级路径消费。旧产物缺该 map 只告警、恒记账（= 原行为）。

### 未提交 WIP（用户，勿擅改）

① feature.yaml `Sched` 段（8550/8475/8998 默认开 `sched_migration_cost_ns=200000`、`sched_nr_migrate=27`），按 `SCHED_ALLOWED_PARAMS` 白名单写 `/proc/sys/kernel/*`；root 兜底 feature.yaml 无该段（默认关），与三 SoC 不一致。② 子进程名（`com.x:push`）**一律归一主包名**：前台名在 `app_detect::set_current_package`、规则键在 `deserialize_app_modes`（冲突保留后者+`rule-key-conflict`）；`determine_mode` 就是精确查表，**不要**加回退链。③ `module/system.prop` 的 `persist.sys.horae.enable=1→0`，提交前确认。

## 调度要点

- **动态限频**：CLG 与特调同构——写 schedutil、min 压硬件最低、只调 max；按核心组 `up/down_core_count|util_percent` 判定，util=0 不计升频、计入降频；抖动 wait_ms 防抖。
- **特调参数组**：`Config.tuned_profiles: HashMap<模式名, SpecialTunedConfig>`，`get_tuned_profile(mode)` 缺省回退 `akmode`；新增模式 = special_tuned.yaml 条目+同名参数组（无需改 .rs）；缺参数组打 `tuned-profile-missing`。`boost_affinity`=boost 类亲和（省电型必须 false）；`util_smoothing`=负载 EMA。
- **降频计时**：`down_target` 只在目标明显回升（>+hyst）时重置计时。
- **CLG 调频主旋钮（2026-09-23，口径修正）**：`target_perf = clamp(util × headroom, floor, ceil)`，**与 up_threshold 无关**（up 只管 headroom ramp 与升频速度）。**降功耗不封顶**：除特调外在用模式都必须保留随时到最高频的能力（`perf_ceil 1.0`），控功耗靠「高频门槛」（up_threshold / rate limit / smoothing / jump）与「headroom 过渡带宽」；实测实际频率紧贴决策上限（QQ big 实际/决策 = 92%）。触摸地板 = `perf_init + tiers×0.05`。
- **Alpha06-06 亮屏画像（2026-09-23 下午包）**：息屏 0.45W；launcher 1.30W（GPU 2%）vs QQ 4.92W / 酷安 4.24W / 微信 3.30W / bili 4.59W（GPU 18-24%、psi 30-40）→ 亮屏功耗差主要来自内容型 App 的 GPU+CPU 负载，不是「亮屏 UI 固有形态」。
- **FAS「频率不匹配」不是外部压制的证据**（2026-09-23 定位）：旧实现 `apply_freq_locked` 写完立刻读 `scaling_cur_freq`，内核调频异步未完成 → 读到的是**写入前的旧档**，被误判成压制；实测 55 次的实际值全部 = 旧档、近旁 snap 的 cur==max、帧率达标。已改为「目标稳定 ≥ VERIFY_SETTLE=200ms 后、在下一次写入前抽查」（频繁改频期间跳过；真实压制仍会抓到并重写）。
- **FAS migration_cost**（2026-09-23 起）：`FasRulesConfig.migration_cost_ns`（`module/config/normal/fas/*.yaml`，None=完全不动），接管期写 `/proc/sys/kernel/sched_migration_cost_ns`、deactivate 按快照恢复（读不到原值就不写）；sgame.yaml 已配 400000。
- **`scaling_governor` 写入恒 EPERM**（PHB110/A16，三 policy）：内核本就是 schedutil，意图已满足；但 OEM 换 governor 则切不回（环境约束，非本轮回归）。
- **白名单 `re:` 解析**：必须 `strip_prefix("re:")` 再按 `:` 切分；导出 yaml 只含精确条目 → UI 只能近似。
- **FAS 单实例+延迟退出**：activate 时 GovernorGuard 切 performance；失前台 `request_delayed_exit` 持策略 15s（夹 1..=600），到期 1s tick 退出并按 `pending_mode_after_fas` 重接管；ModeChange 的 fas→非fas 拦截在 mode_clone 更新前。息屏与 FAS 完全解耦。
- **帧指标口径**：eBPF 只有一个 uprobe（`Surface::queueBuffer`），`frame_delta_ns` = 相邻帧间隔；必须只投喂新产生的帧。
- **息屏轴**：scenemode 停迁移 + cpuset 全核 + 只压 CLG 上限；判定走屏幕**投票仲裁**（OFF 票**超过有效票数一半**即确认，2026-09-23 由固定 2 票改为多数票）；**已无节点退役、无恒亮屏兜底**——`verify_screen_state` 每轮全节点投票校正，票数是唯一判断标准。
- **后台降权**：`AffinityConfig.background_uclamp_max_pct`（默认 50）钳后台/受限组 `cpu.uclamp.max`；`affinity_blacklist.yaml` 含 SystemUI/桌面。`thread_bind` 是线程摆放总闸。
- **stat comm 口径（2026-09-23 修）**：`sample_one_tid` 以首 '(' 与末 ')' 定界 comm；旧 `text[1..close]` 混入「tid 尾部+` (`」致 KEY_THREAD_COMMS/黑名单精确匹配恒不命中，修复后已生效。
- **FG util 口径（2026-09-23 修）**：`compute_tgid_util` 基线存 adj(raw+pending)、util = adj 差分/墙钟；旧「raw 差分+当前 pending」多算 pending(t0)，util 系统性高估。
- **亲和/core_ctl 恢复写失败（2026-09-23）**：非 ESRCH 失败保留状态（home/计数/moved_group/自钉清单）待重试，成功或 ESRCH 才清；unpin_self 按成功 tid 清单恢复，勿用 self_pinned 总开关短路。
- **CLG 热压制语义（2026-09-23 修）**：只钳写频、**不回写 `current_perf`**（回写会让状态卡死在 cap，「持续高负载涨过豁免档」不成立）；生效区间 `(soft_perf_cap, free_above)`，`>= free_above` 不钳制。**豁免档必须严格大于 soft_perf_cap**——8550 曾 free 0.80 被 normalize 抬到 0.85 = 压制带为空、41℃ 软限档全程空转（cap=85 仍写 hw_max），已改 0.95（normalize 同日收紧为「过低或相等 → soft+0.10」）。热压制只作用于 CLG，tuned（playback/akmode）不参与。

## 电池读数（遥测）

- 默认标准节点 `/sys/class/power_supply/battery/{current_now,voltage_now}`（ABI µV/µA）；`oplus_chg` 开时优先 `bcc_parms`（0 基下标 6=电芯0电压、8=电流、11=电芯1电压），存在**且有效**才只认它，无效回退标准节点；私有节点 60s 复查存在性。
- `oplus_dual_cell` = 两节并联：电压平均、电流 ×2；下标 11 非正视为单电芯。`voltage_double`/`current_double` 仅标准节点路径，与 `oplus_chg` 互斥（Config::load 强制 `!oplus_chg && x`）。
- `voltage_divisor`/`current_divisor`：全链**无内置换算**，`原始值 ÷ 校准值` = V / 安培（`batt_current_ma` 列名是遗留）。**默认各 1000000**（µV/µA ABI）；OPlus 私有节点报 mV/mA → 都填 1000。**daemon 运行期不猜量级**。
- **安装期自动校准（`customize.sh [battery-detect]`）**：只改「不保留配置」分支 `$MODPATH`（modules_update 暂存）里当前机型 meta.yaml。读到 `bcc_parms` 有内容 → `oplus_chg=true` + divisor 都写 1000；无私有节点 → 读 `voltage_now`，n 位 → 除数 = 1 后跟 n-1 个 0（首位非 3/4 或非法不写）。双电芯判据先数逗号确认 ≥12 字段且第 12 个为正（**`cut -f 12` 字段不足时透传整行**，会误判）。热更新末尾「恢复备份」只在保留配置那条路执行。

## 8550 实测结论

- 整夜 5h default：亮屏均 1.35W；bili 1.27W；游戏 2.48W；息屏 0.07-0.09W。游戏段 65% 时间被 vendor 热限流（cap 70%）而 ChiRi 未参与。
- **main\_（原 devimp）列语义**：`cur_freq_khz` = CLG 动态上限，`max_freq_khz` = 硬件最高；`batt_current_ma` 实为安培；wakeups/migrations 累计。（历史样本 fps 列全空——那是 status.csv 的列。）
- 无平滑特调在抖动负载下上限均值反而高于 CLG；EMA + hr 1.0 才能把 little/big 压到 0.90-0.96。**调度层剩余空间个位数 %。**
- **Alpha06-04 / PHB110 / A16 实测（2026-09-23 凌晨）**：王者 FAS 120 档位 10.75 min = 4.32~4.34 W / fps 121.3（cpuT 62.4℃）；bili playback 2.78 W（cap 85 段 2.5~2.7 W）；batt 42.1℃ 触发 cap 85；daemon 重启会重置热保护状态。
- **迁移率基线（2026-09-22 结案）**：亮屏 UI（cloudmusic/launcher）迁移 2500~5000/s 是 surfaceflinger/system_server 主导的**固有形态**（40 列无天花板的老包同样如此），视频稳态才 ~300/s——勿再拿亮屏 UI 对比视频稳态判「迁移异常」。**over/under 列语义**：组内 util 过 `up_threshold` / 低于 `down_threshold` 的核数（big 簇 0.1/3.6 = 负载集中在 1 核、其余空转）。
- **2026-09-23 补测**：bili 播放态迁移 ≈4500/s（含弹幕/网页渲染，`@S` 帧里 `bili:web`+surfaceflinger 同时活跃）→ 旧的「视频稳态 ≈300/s」只适用于纯视频无 UI 动画；王者 FAS ≈9000/s。判定异常前先确认场景组成。
- **频率上限摆动与死区**（2026-09-23）：特调 `hysteresis` 是**绝对频率死区** `hw_max×hysteresis`，必须 ≥ 频表相邻档步长才不来回摆——playback 原 0.04 在 8550 上只有 81/112/127 MHz（步长 115~135 MHz）→ big 每秒改 scaling_max 5.6~6.0 次；已提到 0.06（配 down_hold_ms 150），**待实测复核**。

## WebUI

- Svelte 5 runes + TS strict + Vite + `@yunyoujun/ak-ui@1.0.0` + vitest；命令 `npm run dev/build/type-check/test`。
- 分层 `kernel`（ksu 桥+可注入 ShellRunner）→ `contract` → `data` → `views`/`components`；`src/dev/mock-shell.ts` 替身。
- hash 路由两级（`VIEW_IDS` 四项导航，二级 `#/lab`/`#/battery` 用 `navOwner()`）；自实现 runes i18n（插值 `{name}`）。
- **样式**：优先 ak-ui 官方原语（Panel→ak-card、ConfirmSheet→ak-dialog、LiveStatus→ak-status、Segmented→ak-segmented、ToggleField→ak-choice--switch、StateBox→ak-notice、进度→ak-progress/`ak-gauge`、字段→ak-form-stack+ak-field），适配集中 `app.css [ak-adapt]`。保留自研：`.btn*`、`.kv`、`.u-*`（`.u-note` 必须排在 `.u-mt-*` 前）、`.log*`/`.snapshot*`/`.terminal*`/`.nav*`/`.commit*`。
- **i18n 双语**：zh 是唯一语义源，改文案两边同步；`i18n.test.ts` 不校验语义。风格：无句尾句号、不用破折号、避免 AI 腔。信号色成套。
- **交互定值**：日志/快照限高 60vh 子滚动+自动跟随底部（滑离即退出，点「回到底部」才恢复）；状态码/日志页每秒自刷新+失败 toast 节流；`.u-stack` 必须 `grid-template-columns: minmax(0,1fr)`；关闭用 `ksu.exit`。
- 读取文件前必须 `[ -f ]` 守卫（`2>/dev/null` 盖不住重定向错误）。导出 history：`pack.sh` + `/sdcard/Download`（系统目录不要 mkdir），前端轮询产物；压缩用 gzip（xz 太慢）。
- **字体（2026-09-22）**：构建期 `scripts/fetch-fonts.mjs`（predev/prebuild 钩子）从 jsdelivr 拉 Poppins/Noto Sans SC 并按仓库用字子集化，二进制不入库；字表哈希戳 `.subset-charset`；JetBrains Mono 已入库。**ak-ui 的 dist 把 `font-family` 硬编码进 78 处组件规则、绕开 `--ak-font-*` token**（拉丁落 Roboto、中文落系统字体），故 `app.css [ak-adapt]` 用 `[data-ak-ui] :is(...)` 按「正文 / 等宽」两组收回字体族；`--ak-font-mono` 在 ChiRi Mono 后插 `'ChiRi Sans CJK'`（等宽组件里也有中文）。新用官方类若字体不对，先查这两组是否漏了它。

## 构建与仓库约定

- `cargo xtask build` 先跑 `webui/npm run build` 再拷 `webui/dist` 到模块 `webroot/`；硬约束 `base:'./'`+`type="module"`。CI Node 24。`module.prop` id = `chiri`。dist 由根 `build.rs` 嵌入（`restore_webroot` 启动补齐，缺则降级）。
- **.gitignore 已合并为根单文件（2026-09-22）**：webui/、module/ 的子 .gitignore 已删除；根内新增 WebUI 段（`webui/` 前缀）与 Magisk 段；根 `/package.json`、`/package-lock.json` 刻意忽略（npm init 残留）。后续新增忽略规则一律进根文件。
- `mdocs/` 只放项目原有文档；AI 产出放 `.codebuddy/docs/`（已忽略）；`.codebuddy/memory/` 跟踪。
- **devimp 日志包分析入口 = 命令 `/devimp-log-analysis`（2026-09-23 用户定，同日由 skill 转入）**：正文在 `.cursor/commands/devimp-log-analysis.md`（唯一副本，勿再建 skill 或镜像），聚合脚本在 `scripts/devimp-analyze.py`（git mv 自 skill 目录，用法 `python scripts\devimp-analyze.py <解压目录>`）。`.agents/skills/` 下已无项目 skill，旧「skill 位置/镜像」约定随之作废。
- 评估与准备 ≠ 批准开工。没说「开始改」就不建不改源码；改动前 `git status` 核对足迹，汇报给文件级清单。
- **只改任务范围内的东西，不「顺手修」**：未提交改动、被注释的代码可能是用户 WIP。检查报错若指向用户正在编辑的文件，只报告不动手。汇报区分「我改的」与「工作区里已有的」。
- **Yumi 权重归零（2026-09-20 用户声明）**：性能优化及同类工作中，`src/scheduler/` 与 Yumi 设备兼容**不再作为约束**，改动即使波及也可进行（通常只做类型适配，不主动改逻辑）。2026-09-22 Yumi 调度本体已删，`docs/agents/` 口径已同步，本条冲突消解。
- **注释口径（2026-09-23 用户定）**：注释只写「这是什么 + 注意点什么」，不写实测数据、日期溯源、「因为…导致…」的因果叙述；待验证/待办一律写**标准 `TODO: ` 前缀**（不放日期括注，避免 IDE 的 TODO→Problems 扩展正则不匹配），关键数字放进 TODO 文案里。
- 常驻技能 **humanizer-zh** 与 **token-efficient-coding**：中文去 AI 腔；Rust 文件头 `//! x.rs - 区块索引: [a] [b]`（ASCII 方括号）；先读头部再定位、单任务 Read ≤200 行、Edit 局部改禁整文件重写、工具调用并行、shell 输出过滤、回复精简给 diff。
