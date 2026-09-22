## [chiri] ChiRi 调度子系统

以下子系统仅在命中 `CHIRI_SOC_HINTS` 的 SoC 上生效（未命中不接管 CPU，原 Yumi 兜底已删）。全局统一 schedutil：CLG 与 akmode 均把内核调速器写为 schedutil（各自 `init_policies` 时写 governor、release 时恢复快照）。

### 特调（akmode）

白名单数据：

- 白名单数据在独立文件 `src/chiri/special_tuned.yaml`，经 `include_str!` 编译进二进制（common.rs 的 `parse_special_tuned` 解析、OnceLock 缓存），用户/WebUI 不可修改。

- 格式：每行 `匹配器:模式列表(逗号分隔):优先回退模式`。匹配器支持精确包名与 `re:` 前缀正则（忽略大小写用 `(?i)`，匹配器内不能含 ':'）；`special_tuned_entry(pkg)` 先精确匹配（文件顺序）、未命中再按正则条目匹配。每项含可用模式列表 `modes` 与优先回退模式 `fallback`（用户未显式配置则采用 `fallback`）。

- 当前条目：明日方舟国服 `com.hypergryph.arknights`、日服 `com.YoStarJP.Arknights`、正则兜底 `re:(?i)arknights`（覆盖台服/Mod 变体），模式均为 `akmode`（fallback 同）；**playback 组**（视频播放，2026-09-17 8550 日志分析新增）：`tv.danmaku.bili` + `re:(?i)(bilibili|danmaku\.bili)`，模式 `playback`。**daily 组已于 2026-09-17 晚上机实测后移除**（桌面/聊天/工具属高频短交互，特调收益低、侵入高——「不是所有程序都需要特调」；移除条目留档在 special_tuned.yaml 注释里，特调只保留长稳态场景）。新增特调模式 = 本表加条目 + `normal/tuned_profiles.yaml` 的 `tuned_profiles` 段加同名参数组，无需改 .rs。

- main.rs 启动时把精确条目导出到运行时文件 `special_tuned.yaml`（每行 `包名:模式列表(逗号分隔):优先回退模式`，正则条目无法按包名精确查找故不导出）并 info 打点。导出仅 `is_chiri_soc()` 下发生，非 ChiRi 机型不生成该文件。

生效范围与门控：

- 特调体系仅限 ChiRi：`determine_mode` 先判 `is_chiri_soc()`，非 ChiRi SoC 上特调映射一律回退全局模式。

- 只在 chiri 的 `Config` 挂载独立特调字段 `akmode`（缺省段：游戏特调兼未注册模式的回退）与 `tuned_profiles: HashMap<模式名, SpecialTunedConfig>`（**特调参数组**，2026-09-17 加：`normal/tuned_profiles.yaml` 的 `tuned_profiles:` 段，`get_tuned_profile(mode)` 按模式名取、缺省回退 `akmode` 段，**全部调用点唯一的取参入口**——不要再加第二个取参方法）；不要注册进 `get_mode`（只认 CLG 常规模式）。

- 白名单应用始终进特调：前台命中 `special_tuned_entry()` 就返回特调模式，不管 app_modes/global_mode 配了什么（`determine_mode` 开头直接判定）。rules.yaml 里给该应用配的普通模式只作为特调起始档（scheduler 侧 `get_ak_initial_tier` 识别）。

- 非白名单应用的模式优先级仍为用户 `app_modes` > `global_mode`；后端门控：非白名单包名映射到特调模式时 warn 并回退 `global_mode`（`app_detect.rs` 的 `determine_mode`）。

调度行为：

- 特调是完全独立调度：`src/chiri/tuned.rs` 的 `TunedGovernor`（akmode / playback / daily 共用）与 CLG 完全解耦。前台为白名单应用时由 `mod.rs` 的 scheduler_ipc 先 `cpu_governor.release()` 再 `ak_governor.init_policies()` 接管，退出前台反向释放。**全部特调模式共用这一套连续控制**（每 40ms 按组内最大占用 × headroom 直接算上限，升频立即执行），差异只在参数组；**`boost_affinity`**（参数字段，默认 true）= 是否走 boost 类亲和（cpuset 收窄 big+prime + core_ctl 保大核常在线）——游戏保响应 true，省电型特调（playback/daily）必须 false：保大核常在线与「贴负载降频」相反（空转漏电），且会把前台重线程钉到大核组被低上限压住。`is_boost_mode` 命中的特调再经 `tuned_boost_affinity()` 二次过滤。**`util_smoothing`**（参数字段，默认 1.0=不平滑）= 决策负载 EMA 系数：抖动负载下「升频立即执行」会被瞬时尖峰反复推高上限（降频又被 hold 拖住），上限均值反而高于带平滑的 CLG（2026-09-17 日志回放证实），playback/daily 用 0.35/0.5 滤尖峰，游戏保持 1.0。**降频计时重置语义（同日修复）**：down_target 只在目标**明显回升**（> down_target + hyst）时重置计时，下探/微调只更新目标——原「档位不等即重置」在抖动下让上限卡高位降不下来（回放中平滑 util 都降不动），连续控制退化成「只升不降」。

- 特调模式下息屏保持 akmode 接管（akmode 已统一 schedutil，息屏随负载自然降频省电）；非特调走 CLG doze → 超时 scenemode（见「息屏省电与屏幕状态」——2026-09 曾短暂暂停后**已恢复**，屏幕状态正常驱动息屏/亮屏切换；旧的「[已暂停] 屏幕状态不驱动调度」描述已删除）。

- **模式家族（2026-09-17 重申）**：CLG = reduce/default/boost（兜底档；2026-09-16 由 powersave/balance/performance/fast 改名，id 与 feature.yaml 段名/rules global_mode/current_mode.chr 同名，显示名走 i18n `mode.*`）；**stardust = scenemode（独立息屏轴，不产生模式值；2026-09-18 起在 UI 家族体系注册 `mode.family.stardust`——不管实际运行中看不看得到都占位，不并入 CLG）**；**down（停摆布尔）是独立家族 `mode.family.down`（kind `'down'`），不属于 stardust**；**rhine = vector/contingency/babel/frozen（仅实验室**，rhine.chr 经 global_mode 覆盖驱动）。CLG 档位语义：reduce 较 default 升频更保守（up 0.85）降频更激进（down 0.60、上限压 0.70）；default 为默认兼参考基线；boost 升频更激进（up 0.65）降频略消极（down 0.40、rate_limit 5）、headroom 1.40 给线程/核心更大余量。档位由 rules.yaml 生效模式决定（明日方舟 app_modes > global_mode）；特调期间固定应用；所有档位都能使用硬件最高档位。

- 档位差异仅在升降频策略参数和防抖等待（wait_ms，每档可不同）。核心组区间随命中 SoC 变化，统一在 `common::chiri_core_ranges()`（8550 little 0-2 / big 3-6 / prime 7；8475 0-3/4-6/7；8998 0-3/4-7 无 prime），akmode 与 CLG 触摸升频共用。每组独立 up_core_count/up_util_percent/down_core_count/down_util_percent：核心数为组内绝对个数，yaml 写整数，0 = 组内任一核心命中即触发，写大值如 64 = 关闭该方向判定；占用率写整数百分比，加载时转 0..1。

- 动态限频（schedutil + 负载驱动升降 max）：`TunedGovernor` 激活时写 schedutil、min 压到硬件最低、max 设为硬件最高。`on_load_update`（特调 40ms tick）用当前档位策略参数按核心组判定升降，升频优先：升频 = 任一组内达到 up_core_count 个核心 util > up_util_percent；降频 = 任一组内达到 down_core_count 个核心 util < down_util_percent（达到 = 组内核心数 >= core_count，即配置值就是绝对个数）。统计口径：util 恰为 0.0 的核心（离线与整窗空闲的在线核均为 0.0、不可区分）不计入升频 over、但计入降频 under——空闲即低负载，避免挂机/息屏时永不降频。升频前检查实际频率（scaling_cur_freq）是否已达当前设定的 max（schedutil 余量），达到才在频率表中升一档；降频直接把 max 降为当前实际频率对应档位（`read_cur_freq` 后 `partition_point` 找 <= 实际频率的最高档，实际不可读回退降一档，绝不高于当前 max），max 上下限均为硬件上下限。升降频带 wait_ms 防抖（升降后 `after_change_duration_ms` 内减半）。CLG 与 akmode 同构：min 压硬件最低、只调 max。

- 特调参数独立成文件（仅嵌入 normal/，非处理器绑定）：`module/config/normal/tuned_profiles.yaml` 定义缺省段 `akmode` + 按模式名分派的 `tuned_profiles` 段，经 `common::embedded_tuned_profiles_str()` 编译进二进制，不放在默认 `feature.yaml` 里。原处理器目录 `{soc}/akmode.yaml` 绑定已在 4cb4d97 重构中移除（磁盘已无该文件，勿再加回）。嵌入内容解析失败时 `set_special_tuned_available(false)` → 特调不可用、白名单应用回退 CLG（不再保留旧值用默认参数接管 CPU）。

- 特调接管失败冷却：`TunedGovernor::init_policies(mode, cfg)` 返回 `bool`（无可用 cluster 即 false）。scheduler_ipc 在三个特调接管入口（亮屏恢复/ModeChange/ConfigReload）检查返回值，失败时置 `tuned_cooldown_until = now + 300s` 并 warn 打点 `scheduler-tuned-cooldown`，5 分钟内该特调模式不再触发、改由 CLG 接管（冷却中特调模式名保持，息屏 doze/亮屏恢复均走 CLG 分支），冷却结束后经 ConfigReload 或下次 ModeChange 自然恢复重试。

WebUI 侧：

- 设备形态两态：`contract/sources.ts::deviceKind()` 依据 active_config 是否指向处理器子目录判定 `chiri` / `unknown`（非 ChiRi 与读不到一律 unknown，界面显示「无法判定」）。状态落在 `src/state.svelte.ts`。

- 特调/FAS 在 WebUI 为只读标注：`data/whitelists.ts` 解析白名单，仅 `chiri` 机型显示「特调 / FAS」标签（其余不显示）；不提供专属特调模式选项、不做特调清理/重扫修复。应用性能模式已禁止在 WebUI 指定（rules.yaml 只读），应用列表仅展示特调/FAS/现存 app_modes 标签，提供「重新扫描」按钮（扫描中禁用防并发）。

- 当前模式读取与自愈：`contract/sources.ts::readCurrentModeRaw()` 读 `current_mode.chr`（内容为模式名原样字节、无换行无空格）；守护进程启动时写一次、模式切换时写、并常态每 5 秒**无条件**重写一次（两套 scheduler_ipc 各一份实现），因此**不能用 mtime 判断模式变化**；守护进程停止后文件是陈旧值，界面须结合存活探测呈现。文件为空/缺失时界面显示「未知 · 尚未产生模式记录」，与「模式名不在已注册档位内」区分开。

### FAS 帧感知调度（单实例 + 延迟进出，ChiRi 专属）

- 白名单与模式判定：`module/config/normal/fas.yaml` 精确包名→配置名；`determine_mode` 优先级 = FAS 白名单 > 特调 > app_modes > global_mode；rules.yaml 中 "fas" 映射视为非法（`app-detect-fas-rejected` / `app-detect-fas-global-rejected` 告警回退）；`fas_available()` = 白名单非空且至少一个应用配置解析成功（fps_monitor/FAS_FG_UTIL 联动门控见「架构与事件流」）。

- **单实例 + 延迟进出（2026-09-17 重构，原多实例已废弃）**：`src/chiri/fas_manager.rs` 的 `FasManager` 同一时刻至多一个白名单应用被接管（`instance: Option<FasInstance>`）。白名单前台 → `activate`（FasController.load_policies 快照频率 + **GovernorGuard 把各 policy 的 `scaling_governor` 切 performance**，原值快照待恢复）；失去前台不立即退出 → `request_delayed_exit` 进入 **15s 延迟期**（`FasRulesConfig.deactivate_delay_secs`，normalize 夹 1..=600，`fas-example.yaml` 可配）：mode 保持 fas、FAS 仍持有接管（频率停在最后状态、governor=performance），期间切回白名单应用由 activate 无缝续期；到期由 1s 遥测块的 `tick()` 完成退出（reset_all_freqs + clear_game + governor 按快照恢复），并按延迟期记住的目标模式（`pending_mode_after_fas`，fas→X 的 ModeChange 被延迟时记录）重新接管——contingency/babel 走 apply 的 lab 分支，其余走 `apply_mode_takeover`。ModeChange 的 fas→非fas 分支因此**不再立即切换**（拦截在 mode_clone 更新之前，避免「文件写 default、FAS 还持着频率」的中间态）。C5 收尾 deactivate_all（DOWN/panic/进程收尾/fas_enabled=false 热重载）立即全退。事件路由：FrameUpdate/SystemLoadUpdate/温度只喂活跃实例。

- 同模式热切换：新增 `DaemonEvent::PackageSwitch { package_name, pid }`（app_detect 主循环在模式不变且 ChiRi 且前台包变化时发射；非 ChiRi 机型不发射）。fas→fas 切换序列 = deactivate_active → activate → fas_affinity_hook；另有 1s 遥测兜底（事件丢失/启动即 fas 自愈：非活跃且冷却外且白名单命中 → 三 governor release 后 activate；启动残留 fas 模式且前台非白名单且三 governor 均不活跃 → CLG default 自愈）。兜底块仅亮屏时执行——息屏时 FAS 必须保持释放，否则会把息屏前旧包名拉回 FAS、绕过 CLG doze。

- FAS 模式下 CLG/特调/vector 全部暂停（三 governor release 后接管）；激活失败（load_policies 后无可用 policy）→ 300s 冷却（`FAS_COOLDOWN`，镜像 TUNED_COOLDOWN）+ CLG default 回退；进入 fas 的回退分支也先释放三 governor（可能从特调/极速切入）。

- **前台 util 口径（2026-09-23 修正）**：TGID 主路径 `compute_tgid_util` 以 adj = raw + pending 差分算 util（与线程级降级路径 `compute_thread_level_util` 同口径，基线存 adj）；旧写法「raw 差分 + 当前 pending」把上一轮 pending(t0) 重复计入（恒等式 consumed(t)=raw(t)+pending(t)），前台 util 系统性高估、FAS 输入失真，勿改回。

- 移除的调度（FAS 活跃期间豁免）：ChiRi 热保护仅当 `mode=="fas" && fas_mgr.is_active()` 时跳过（fas 模式但实例未活跃——息屏已释放/冷却/初始化失败——照常生效，否则 CLG doze 期间失去热保护；FAS 活跃时温度由 FasManager 每 3s 独立读传感器喂引擎内部限温，endfield 配置 core_temp_threshold=0 即关闭；1s 遥测温度读数保留）；触摸升频不参与（fas 非 boost，`is_boost_mode` 不含 fas）；scenemode 进入判定按 `!fas_mgr.is_active()` 门控（FAS 活跃即不进入——息屏保持接管后 fas 模式天然屏蔽 scenemode，FAS 失效后正常进入）；config_dirty/ConfigReload 的 CLG 分支对 fas 模式守卫（FAS 配置编译期嵌入静态）。

- 息屏释放已完全移除（2026-09，原 C4 doze 更早已废弃删除）：原 `ScreenStateChange(false)` 在非特调分支统一 `ak/vector release` + （fas 活跃时）`fas_mgr.deactivate_active()` → 走 CLG doze → scenemode 全局接管。现 FAS 与屏幕状态完全解耦：**FAS 激活/去激活仅由前台包名驱动**，息屏保持接管（fas 活跃时跳过 doze）、boost 布局全时段生效、1s 兜底与 FrameUpdate 喂帧无 is_screen_on 门控、ModeChange 息屏 fas 特殊分支删除。仅 FAS 失效（初始化冷却/异常）后息屏才走 doze → scenemode：亮屏恢复分支的 fas arm 对失效场景激活 CLG default（唯一恢复路径），1s 兜底在冷却结束后重新激活 FAS（activate 复用保留实例 + apply_freqs，activate 前 release 全部 governor，且若 scenemode 激活先恢复全部在线核）。勿改回「息屏 enter_doze 写低频锁、亮屏 exit_doze 恢复」：锁屏前台无帧事件时 apply_freqs 恢复路径不触发（且被 freq_hold_frames 挡 2 拍），全簇锁死最低频表现为亮屏 0.2fps。

- 线程亲和预留接口：`fas_affinity_hook(affinity_mgr, corectl_mgr, active, fg_pid)` 在 chiri/mod.rs，FAS 激活/去激活时调用；当前职责 = FAS 激活期放开 top-app uclamp.max 为 100（快照还原；boost 进入按机型写入的 85 会钳制 EAS 对重线程的 capacity 视图、抑制 prime 放置，与 FAS 让 prime 承载负载相悖——8475 实测 prime 空转 45%；FAS 激活期 min=max 锁频绕过 schedutil，85 对调频无效仅剩放置负效应；去激活时若 boost 已退出则跳过还原，由 restore 链归位避免把 boost 值泄漏到 normal；8998 内核 4.4 走兜底探测永久跳过），后续 FAS 线程钉核扩展也在此实现，无需再改事件循环接线。**同一「放开 100」机制已与特调（akmode）共用**：方法改名 `set_boost_uclamp_override`，由 `apply_affinity_and_corectl` 按模式同步 uclamp 归属（特调 → `Some(true)` 本函数负责、fas → `None` 让出给本 hook、其余 boost → `Some(false)` 保证不残留 100）；8550 实测 arknights 特调下 big 饱和 47.6% 而 prime 仅 37%，正是 85 抑制 prime 放置的同一现象。

- FAS 白名单与配置（2026-09-20）：新增 `com.levelinfinite.sgameGlobal: sgame` → `module/config/normal/fas/sgame.yaml`（fps_gears [30,60,120]、target_fps [60,120]、perf_init 0.50、core_temp_threshold 45——MOBA 锁 60/120 省电；perf_ceil 均不覆盖、保持默认 1.0，遵守「不限制最高性能释放」硬性约束）。两游戏是 FAS 白名单仅有的成员。

- 调度能力范围（相对 CLG 的扩展仅频率维度）：per-policy min=max 锁频（PolicyController，1.5s 回读校验防内核覆写）+ 容量加权分频 + util 软封顶 + PID/Jank 帧级响应；FAS 自身不做 cpuset/uclamp/core_ctl——线程摆放复用 boost 布局（fas 亮屏入 boost，见亲和章节），uclamp.max 放开经 fas_affinity_hook（特调同机制但经 `apply_affinity_and_corectl` 按模式同步，见上条）。

### CLG 调频语义（动态上限制）

- CLG 不再锁频 min=max，改为 schedutil 动态上限：接管时把 `scaling_min_freq` 一次性压到硬件最低，之后只写 `scaling_max_freq`（CLG 决定性能上限），schedutil 在 \[硬件最低, 上限] 内按瞬时负载自主调频，空闲间隙内核微秒级降频到地板，消除锁频方案"采样间隔（160ms）内频率下不来"的空转发热（8550 实测主要热源）。

- 上限语义下 `perf_floor/perf_ceil/perf_init` 约束的是上限（空闲实际频率由 schedutil 决定），`perf_floor` 允许被热保护击穿。

- **`util_smoothing`（2026-09-22 新增，CLG 决策负载 EMA）**：抖动负载下 max*util 每 tick 大幅摆动 → target_perf 翻摆、决策方向反复翻转、`scaling_max_freq` 高频改写（8550 QQ 实测大核 2048 tick 反转 458 次）。EMA 滤波（语义同 tuned 的 `util_smoothing`，1.0=关闭、缺省即关闭=逐位等价）后反转降约五成、写频降三成。default/reduce 配 0.5，boost 保持 1.0（响应优先）；α 0.35 可再降抖动但上限均值抬升更多。main* `max_util` 列为双语义：CLG 行写**平滑前**原始值（离线回放可自行试验系数）；tuned（akmode）行写平滑后的决策负载（2026-09-17 起语义变化）——离线回放按行来源/decision 区分，tuned 行不可再当原始 util 二次平滑。字段新增须同步：`config.rs` 字段+缺省+normalize+Default、`feature.yaml` 全部 SoC 文件（SoC 文件整份覆盖根，只改根不生效）、`config-example.yaml`。

- 全局省电向（2026-09-20，用户要求「除 sgameGlobal 外所有场景功耗减半」+「不能限制最高性能释放」硬性约束）：**perf_ceil 一律保持硬性原值（reduce 0.60 / default 1.0 / boost 1.0），禁止用压 ceiling 省电**。省电路径 = 提升升频门槛与降地板：default 档 up_threshold 0.72→0.78、perf_floor 0.10→0.05、perf_init 0.40→0.25、headroom_factor 1.20→1.05、down_fast_threshold 0.25；boost headroom 1.40→1.25（保留相对性能优势）；reduce perf_init 0.25→0.20、headroom 1.05。`module/config/feature.yaml`（根）是未命中 SoC 目录机型的 ChiRi 兜底配置与代码默认值来源。**sgameGlobal（FAS 配置）唯一豁免**：改档位参数不得连带改动其 FAS 语义。（2026-09-22 用户复盘后更新：日常档加了天花板 default `perf_ceil` 0.80 + `per_cluster` little 0.70 / prime 0.65，交互由 touch_boost 兜——「禁止压 ceiling」条款对 CLG 日常档已解除，FAS 豁免不变。）

- 多线程按核心组独立调度：每个 cpufreq policy 由一个独立的 `CoreGroupWorker` 线程管理，持有各自的 `ClusterState`（频率档位、`FastWriter`、`current_perf`、防抖计数器等），线程内自主完成决策 + 写频，核心组之间完全并行无锁。`CpuLoadGovernor`（`cpu_load_governor.rs`）是 Worker 线程管理器：`init_policies` 枚举系统 cpufreq policy、为每个 policy spawn Worker 线程（写 schedutil governor、min 压硬件最低、max 按 perf_init 设初始上限）；`release` 停止所有 Worker（Worker 退出前自行恢复系统原始状态）；`reload_config` 停旧 Worker + 用新配置 spawn 新 Worker（等价轻量 init，current_perf 重置到新 perf_init）。

- scheduler_ipc 通过 `on_load_update(&core_utils)` 将负载数据广播给所有 Worker（非阻塞 `try_send`，通道满则丢弃本 tick），Worker 线程内自主决策 + 写频，无需外部 flush。

- 升频 = 平滑抬高上限：ceiling 提升不强制频率跳变，无需读 `scaling_cur_freq` 做余量检查（旧 `clg-up-skipped` 机制已随锁频语义删除）。降频 = 一步到位降上限：防抖确认（`down_rate_limit_ticks`，极低负载命中 `down_fast_threshold` 免防抖）后直接写目标档（`ratio_of_freq` 同步 current_perf），降 ceiling 只收窄 schedutil 区间、不会把实际频率抬上去。`smoothing_down / slow_down_scale / down_fast_mult` 是 Yumi 参数，已随 Yumi 调度本体删除（2026-09-22），feature.yaml 里的同名键一并移除；README.en.md 里仍有其文档，属上游历史参考。

- 热切换配置重放 perf_init：`reload_config` 重置 current_perf 到新 perf_init 并立即写频，避免接管间隙 current_perf 掉到 0 后频率要从地板缓慢爬升数秒（息屏 doze/scenemode 切换同样经此热切换恢复，见「息屏省电与屏幕状态」）；模式变更 / ConfigReload 热重载同理。

### Thermal 热保护（ChiRi 专属）

- chiri `Config` 顶层 `Thermal` 段（`ThermalGuardConfig`，`from_file` 时 normalize）。电池温度为主参考、CPU 温度仅极端参考：电池温度节点 `/sys/class/power_supply/battery/temp` 的**原始刻度单位因内核/厂商而异，禁止硬编码除数**——`utils::detect_battery_temp_scale()` 启动时预识别一次，按「唯一落进 5~60°C 合理窗口」的解释定档（0.1°C / 毫摄氏度 / 直读 °C，多解时 tenths 优先），结果经 `OnceLock` 缓存，CLG 热保护（`utils::read_battery_temp_celsius`）与 FAS 温度护栏（`utils::battery_temp_divisor`，FasManager 每次刷新现取、勿再固化）共用同一结论。历史事故：CLG 曾硬编码 /1000 而 FAS 硬编码 /10 两处口径不一致，8550 实测恒读 0.4°C（真实约 20~50°C），电池软/硬限（41/45°C）永不触发、主参考彻底失效；探测不出（节点缺失/读数未就绪）时退化为仅 CPU 温度并打 `clg-thermal-no-battery` / `battery-temp-scale-unknown`。该节点反映整机持续发热且不随游戏瞬时负载抖动，阈值 41/45°C；CPU 温度阈值 90/95°C——soc_max 等合成温区在大型游戏高负载下常态 85°C+，阈值过低（旧值 75/85）会让 8475 实测 84% 时间处于压制、比官调更卡，软件压制只在接近内核 ~95°C 温控起控点时参与。

- 温度输入必须滤波（`mod.rs::TempFilter`，1s snap 块内对原始读数应用）：物理范围门（电池 -10..70 / CPU 5..110）+ 毛刺丢弃（相对上次输出跳变 >10/12°C 视为 soc_max 合成温区切换热点，实测 2s 内 91.5→57.2°C）+ 3 样本中值 + 斜率限制（电池 ±1 / CPU ±3°C/样本）。不滤波会让热保护 cap 以 ~8s 周期在 40/70/100 间 bang-bang 振荡，每次跳变把 current_perf 砸回低位再缓慢爬升，高负载游戏周期性掉帧。

- 压制解除带斜坡（`THERMAL_UNPRESS_STEP=0.15`）：压制加深立即生效，解除方向每采样周期（2s）最多恢复 0.15，8s 级渐进；立即全量解除会让温度马上反弹再触发深压，重现振荡。

- 压制带豁免档 `free_above`（默认 0.80，normalize 保证 >= soft_perf_cap）：Worker flush 中 `current_perf < free_above` 才 `min(cap)` 钳制（cap 允许击穿 perf_floor）。持续高负载经平滑抬升越过豁免档后不再回落钳制，任何温度下都能到达硬件最高频，过热兜底交给系统/内核温控；但压制意图保留：中低负载区间仍被压在 cap 以下，减少发热积累。

- scheduler_ipc 启动时探测一次传感器（CPU `find_cpu_temp_path()` 缺失打点 `clg-thermal-no-sensor` 静默降级），事件循环内每 2s（`THERMAL_CHECK_INTERVAL`）采样双温度，逐传感器三级判定（>= 硬限压 hard_cap（默认 0.40）；>= 软限压 soft_cap（默认 0.70）；回落到 软限-hysteresis 才解除；回滞带内压制中先退软限档防阶跃）后取两者较小值。cap 或豁免档变化时经 `cpu_governor.set_thermal_limits(cap, free_above)`（f32 bit pattern 存 `AtomicU32`，启动即同步一次豁免档）下发，debug 打点 `clg-thermal-cap`（含电池/CPU 温度，缺失显示 "-"）。

- 仅作用于 CLG 接管的模式，fast/akmode 不受影响。

### 触摸升频（事件驱动，ChiRi 专属）

- **2026-09-20 起恢复启用**（2026-09-18 曾按用户要求临时停用）：`cpu_load_governor.rs` 的 `TOUCH_BOOST_SUSPENDED = false`，`on_touch()` 正常工作（升频窗口 + Worker 唤醒 + 打点），各模式参数由 `feature.yaml` 的 `touch_boost_enabled/ms/tiers` 决定。`apply_disable_touch_boost`（屏蔽内核 cpu_boost）**照常生效**，所以触摸时既有 ChiRi 升频、内核升频仍被屏蔽。触摸检测线程照常运行（poll 开销可忽略，daemon.log 的 debug `touch-event-received` 仍会打，便于确认事件源活着）。长期关闭应删配置/删链路，不要留常量。

- `touch_detect.rs` 线程读 `/dev/input/event*`（`libc::poll` + 解析 64 位 `input_event` 24 字节，`BTN_TOUCH==1` 或 `ABS_MT_TRACKING_ID>=0` 判定触摸按下），触摸按下时经独立事件通道 `mpsc::sync_channel::<()>` 发送事件（非阻塞 `try_send`）。

- scheduler_ipc 每次醒来先 drain `touch_rx`，收到事件即 `on_touch()` 更新共享 `AtomicTouchState`（f32 性能比 floor 以 bit pattern 存入 `AtomicU32`，窗口时长与写入时刻毫秒数各存一个 `AtomicU32`），并广播空负载包唤醒全部 Worker（经 Worker 负载通道 `try_send(Vec::new())`，recv_timeout 即时返回、Worker 只 flush 不决策）。大核 Worker 在本次 flush 中读取共享状态、把性能上限抬到 `touch_boost_floor`，不等待下一个 160ms 负载决策 tick（触摸延迟 ≈ `EVENT_POLL_MS=100ms` + 唤醒 flush，不含 Worker tick 间隔）。

- 配置项 `touch_boost_enabled/ms/tiers` 每模式独立，`enabled=false` 即关闭（normalize 会把 ms 置 0）。

- 屏蔽系统触摸升频：`chiri/scheduler.rs::apply_disable_touch_boost` 写 0 到 `/sys/module/cpu_boost/parameters/` 的 `input_boost_enabled / sched_boost_on_input / input_boost_ms / boost_ms`（按存在性尝试，无节点静默跳过）；`start_scheduler_thread` 启动时即调用一次 `apply_system_tweaks()`。**DOWN 停摆期间这些调整一并不下、且要把已写的还原回系统**：它们的下发与还原统一由 `CpuScheduler` 负责——首次写节点前记原值快照（`[snapshot]`，重复下发不覆盖；IO `queue/scheduler` 当前值带方括号要剥壳），DOWN 进入时 `restore_system_tweaks()` 逐个写回、退出时补发；判定收口在 `apply_system_tweaks` 内部并用静态互斥闸 `tweaks_gate()` 与还原互斥（详见 `02-convention.md` 的 DOWN 小节）。新增一次性系统调整必须走 `snapshot_write` / `snapshot_nodes`，否则它在停摆期无从还原。

### 息屏省电与屏幕状态

- scenemode（息屏超时省电，2026-09 曾短暂暂停后恢复）：chiri `Config` 的 `scenemode`（Mode 段，默认 `enabled:true`、`perf_ceil` 极低封顶、`up_threshold=1.0` 不主动升频）与顶层 `scene_mode_delay_secs`（默认 300s=5 分钟）。`mod.rs` 息屏时记录 `screen_off_at`，`SystemLoadUpdate` 分支在息屏超时且**非特调、无任意 FAS 实例**（`fas_mgr.has_any_instance()`，活跃与后台保留实例都算存在——CLG 绝不能接管 FAS，保留实例 60s TTL reap 后自动放行）时一次性把 CLG 热切到 scenemode（scenemode 未启用则 release 回系统默认），亮屏自动恢复原模式。**进入 scenemode 时同步应用离线核**（`CoreCtl.scenemode_offline` 门控，见 core_ctl 章节：**小核+大核全开常驻低频**（频率上限由 scenemode CLG 配置压制）+ prime 下线 + **专用小核独占**给调度服务 + 抑制 boost），CPU 侧待机功耗大幅下降；**常驻簇（小核∪大核）持续顶满上限则饱和退回 powersave 并 300s 冷却**。

- 息屏 doze 天花板 0.30：`ScreenStateChange(false)` 生成的 doze 配置 `perf_ceil` 钳到 0.30（原 0.40），配合动态上限后后台突发（sync/JobScheduler）借力受限、空闲间隙照常降到地板频，压制"口袋发热"；5 分钟后 scenemode 进一步压到 0.12（2026-09-20 全局省电调整，原 0.15）。特调与 FAS（活跃时）息屏保持接管、跳过 doze（见 FAS 小节）。

- **FAS 息屏省电已完全移除（2026-09，删除而非暂停）**：FAS 与屏幕状态完全解耦——① `ScreenStateChange(false)` 不再释放 FAS（fas 活跃时息屏走"保持接管"分支，与特调同语义；仅 FAS 失效后走 doze）；② 亮屏恢复分支的 fas arm 对 **FAS 失效场景激活 CLG default**（300s 冷却期内 1s 兜底被 `fas_cooldown` 门控短路，此处是退出 doze/scenemode 低性能配置的唯一恢复路径；FAS 活跃时空操作——CLG 绝不能接管 FAS 已接管的 CPU）；③ ModeChange 息屏进入 fas 的特殊分支删除；④ fas 全时段 boost（`apply_affinity_and_corectl` 不再按 screen_on 回退 normal）；⑤ FAS 兜底激活与 FrameUpdate 喂帧的 is_screen_on 门控删除；⑥ **FAS 优先于 scenemode**：兜底激活 FAS 前若 `scene_mode_active` 先退出 scenemode（恢复全部在线核，守卫「FAS 实例存在 ⇒ 非 scenemode」不变量），scenemode 入口按 `has_any_instance()` 门控。`scheduler-fas-screen-release` i18n key 已随代码删除。

- cpuidle governor 默认启用 menu：8550/8475/8998 的 `feature.yaml` 将 `CpuIdleScalingGovernor` 置 true、`CpuIdle.current_governor` 置 "menu"（menu 按预期空闲时长选最深 C 状态，降低空闲功耗），内核无 menu 时写入失败静默跳过、无副作用。

- 屏幕状态自愈校验：uevent 可能漏报/误报（开机早期背光未就绪、长时间息屏后唤醒、netlink 缓冲溢出，或机型根本不广播屏幕类 uevent——现代内核已无 early_suspend/late_resume power uevent，leds/backlight 亮度变化多数驱动也不发 KOBJ_CHANGE），导致 `screen_state_arc` 锁死在错误状态。`screen_detect.rs::verify_screen_state` 由 app_detect 主循环每轮调用，读检测源 sysfs 校正 arc。**检测源按可靠性优先级自动扫描并缓存**：① `/sys/class/backlight`（bl_power/actual_brightness/brightness 三分支：`bl_power==0`→亮（FB 权威亮屏信号）；非 0 **不可信**——部分 DRM 面板驱动息屏写 FB_BLANK 后亮屏不清零，以 `actual_brightness>0` 为准；actual_brightness 也读不到（驱动未实现 get_brightness）再回退 `brightness>0`。勿改回「bl_power==0 即亮、非 0 即灭」的两分支）；② `/sys/class/leds` 的面板背光节点（`brightness>0` 即亮——**此前这类机型屏幕状态完全读不到的主因**，2026-09 修复）；③ `/sys/class/graphics/fb*/blank`（0=亮，fb0/fb1/… 全部计入）；④ `/sys/class/lcd/*/lcd_power`（LCD class，三星 Exynos `panel/lcd_power` 等机型唯一可用源，同 FB_BLANK 口径：0=亮、4=灭）；⑤ `/sys/class/drm` 内屏 connector（名字含 dsi/edp/lvds）的 `enabled`（"enabled"=亮 / "disabled"=灭，内核 DPMS 口径；外接 HDMI/DP 不计），该 connector 无 `enabled` 时退 `dpms`（"On"=亮 / "Off|Standby|Suspend"=灭，老内核）。leds 节点名匹配由「只含 backlight」放宽到关键字 `backlight/lcd/panel/wled/disp`（覆盖 `panel-backlight`、`aw22xxx-backlight`、`wled-backlight`、`sprd-backlight` 等厂商命名），同时排除非面板 LED（`keyboard/keypad/button/keys/charge/battery/notify/torch/flash/indicator/breath/rgb`）——键盘背光/按键灯/充电灯随充电视亮灭，计票会永久挡住息屏；枚举与 uevent 两条路径共用 `is_backlight_led_name`，口径一致。源发现/无源/读失败各 info/warn 打点（`screen-detect-source-found` / `screen-detect-no-source` / `screen-detect-read-failed`），「屏幕状态读不到」时从 daemon.log 即可确认设备实际可用的源；缓存源连续 8 次全不可读时退役并切换下一个候选（见下方两阶段判定）。

- **两阶段息屏判定 + 节点退役 + 恒亮屏兜底（2026-09）**：① 正常情况只读主检测节点（省开销，`SCREEN_SOURCE` 缓存）；② 主节点报**息屏**且 arc 为 ON（翻转尝试）时触发全节点投票（`tally_screen_nodes`：全部 backlight + 全部面板背光 leds + 全部 fb\*/blank + 全部 lcd_power + 内屏 DRM enabled/dpms，跳过已退役节点，各源读数统一走 `read_screen_state`；**读数不可得的节点不计票、也不否决**）——**两个有效节点报息屏即确认**（`OFF_QUORUM=2`；有效节点只有一个时按一票算，否则机型只暴露一个节点就永远进不了息屏；一个有效读数都没有时不改判）；有节点报亮屏却凑不齐息屏票才不确认（warn `screen-off-vetoed`，每 episode 一条）；节点退役/切换**延迟 15s**（`INCONSISTENCY_SWITCH_DELAY`，`INCONSISTENT_SINCE` 计时器）：单次不一致可能是瞬时毛刺，**持续不一致 15s 才退役主节点**切换下一个候选（切换时 warn `screen-detect-node-switched`，新主节点重新计时）；息屏票够即确认，同时清掉不一致计时与驳回计数；**稳态（无翻转）快速返回不扫描**。③ 节点缺失/持续不可读（8 次 ≈8~12s）→ 同样退役并直接切下一个节点。④ **全部候选退役（耗尽）**→ **error 级打点 `screen-detect-nodes-exhausted`**（比 warn 高一级，提示所有节点均不正确或矛盾）+ 进入**恒亮屏模式**（`ALWAYS_ON`）：不再读取任何节点、不再接受息屏事件，arc 强制校正为 ON（若原为 OFF 会经 app_detect 转发 ScreenStateChange(true) 恢复亮屏安全态），verify 直接返回零 IO。⑤ **稳态息屏定时复核**（`LAST_OFF_REVIEW` 计时器，每 `OFF_REVIEW_INTERVAL`=10s 一次）：翻转复核只盖「亮→息」瞬间，主节点失真（息屏不清零/读数卡死）会让 arc **永久滞留 OFF**——scenemode/prime 下线无法退出，亮屏使用中超大核异常离线；复核发现「息屏票凑不齐且至少有节点报亮屏」→ 退役主节点切换（同样走 15s 持续不一致门控）+ 强制改判亮屏；票型仍是息屏（例如 2 OFF + 1 ON）则只清计时器，不受单个亮屏节点扰动。所有息屏事件生产者（verify 自愈、power/backlight/leds uevent）共用 `update_state_if_changed`，策略天然全覆盖。**复核/否决扫描必须在拿 arc 锁之前**（耗尽路径 enter_always_on 经 update_state_if_changed 写 arc，持锁会死锁）。仲裁口径（2026-09-18 用户要求「两个节点报息屏即确认」）取代了旧的「任一节点亮屏即驳回」：单个失真节点（如 `bl_power` 陈旧非零）不再挡住息屏，代价是两票同时失真会误判息屏。代价权衡仍是刻意的：false-ON 只损失节电，false-OFF 会让息屏机制在亮屏期间误触发卡死设备；恒亮屏兜底（宁可不节电，不可误判息屏）不变。

- 屏幕事件双源直推：`monitor_screen_state_uevent` 收到 power（early_suspend/late_resume）、backlight KOBJ_CHANGE 或 leds 背光 KOBJ_CHANGE（仅认面板背光 leds，跳过通知灯/按键灯/键盘背光/充电灯）且状态确实变化时**直接 send `DaemonEvent::ScreenStateChange`**（纯推送），不再依赖 app_detect 轮询转发；app_detect 的 verify+轮询转发保留为 uevent 漏报时的自愈兜底。双源可能对同一次屏幕切换各发一次事件，两套调度器的 ScreenStateChange 分支开头都有 `screen_on == is_screen_on` 去重守卫（状态未变只打点）——新增屏幕事件生产点时必须维持该守卫。

- **驳回 episode 退役（2026-09-11 补丁）**：15s 连续不一致门控存在死锁——主节点自身失真（息屏仍报亮，如某 8550 面板 panel1-backlight）时，verify 每轮读到 ON 都会清 `INCONSISTENT_SINCE`，15s 永远攒不满 → 节点永不退役、arc 永久钉死 ON（实测整夜 0 条 screen,off、scenemode 全程未进入 → 大核带电+小核上限全开）。现增加 `VETO_EPISODES` 计数兜底：每 60s 最多记 1 次「息屏被驳回」episode（`VETO_EPISODE_MIN_GAP` 防同屏连发 uevent 重复计数），1800s 窗口（`VETO_EPISODE_WINDOW`）内累计 ≥3 次（`VETO_RETIRE_EPISODES`）即退役主节点；ON 读数**不**清零，仅全节点一致 OFF 或退役换节点时复位（防连锁误退役逐个耗尽候选）。

- **scenemode 长息屏兜底（2026-09-11 补丁）**：进入门槛 `standby_max >= SCENEMODE_SAT_UTIL(0.75)` 在后台常驻负载下会整夜拒绝进入（实测小核 util 长期 60-70%、峰值触 0.75）。现息屏时长 ≥4×`scene_mode_delay_secs` 时绕过负载门槛进入 scenemode——短息屏仍按原门槛防「进→10s 饱和退出→300s 冷却」拉锯；门槛的防拉锯语义只对短息屏成立。

### 极速模式（fast）

- vector 档用专属锁频器、不读 yaml、停用 CLG：`src/chiri/fast.rs` 的 `FastLock` 与 CLG 完全独立，vector 档下由 `mod.rs` 的 scheduler\*ipc 先 `cpu_governor.release()` 再 `fast_lock.init()` 接管（**六个入口都不能漏**：启动 / 亮屏恢复 / ModeChange / ConfigReload / DOWN 退出 / panic 自愈——vector 不注册 CLG 参数，漏掉就是频率零接管且无任何告警）。**frozen（待春归）复用同一条通路、共享这六个入口**，差别只在 `fast_lock.init(true)` 锁到**硬件最低频**（vector 是最高频）；额外地，frozen 会停掉一切额外开销：devimp 诊断日志与 status.csv 停写（闸门在 `logger.rs` 的 `diag_active()` 与 `status_log_snapshot()`，只保留 daemon.log 便于排错）——frozen 下 main\_ 与 aff\_（含 `@S` 快照）同停（同一 `diag_active()` 闸门），线程亲和/绑核与 core_ctl 由 rhine 的 `thread_bind: false` 交还系统（不再迁移线程）。

- **PowerBase（Stardust 家族，meta.yaml `powerbase_enabled`，默认关）只替换「谁来调频」**：开启后原本由 CLG 接管的档位（reduce/default/boost）改由 `src/chiri/power_base.rs` 以**放电功耗**为指标调频——功耗低于 feature 里的 `target_power_w` 时放宽升频；达到或超过时守住不升，除非「满占用核心占比 ≥ `overload_cores_pct` 且持续 `overload_hold_ms`」；降频恒激进（不看功耗）；触摸窗口内允许突破功率上限。**模式名与所有外部接口一律不变**（`current_mode.chr` 仍是 default/boost，rules / WebUI / 通知都不受影响）。接管点有两处：`apply_mode_takeover`（主路径，即时）与调度循环每轮的兜底纠正块（覆盖启动块 / ConfigReload / 亮屏恢复 / lab 重建四条旁路——它们直接 init CLG，不兜就会「CLG 与 PowerBase 抢写 scaling_max_freq」或「切换后没人接管」）。affinity 的 promote 阈值在开启时翻倍（积极性减半）。**已知 TODO**：触摸突破当前恒 false——CLG 的 `AtomicTouchState` 是它私有字段，需另备共享标志。**热保护对它无效是预期行为**（thermal 靠压 CLG 上限工作）。

- `FastLock::init()` 遍历 `get_cpu_policies()`、快照原始状态、写 schedutil governor、把 min=max 锁到 target（vector=含 boost 硬件最高频、frozen=硬件最低频）；`tick()` 每 5 秒重写一次 **target** 防止系统/厂商守护进程篡改（2026-09-22 修：原误写 hw_max，frozen 每 5 秒被拉回最高频=失效）；`release()` 恢复接管前的 governor/min/max。

- `mod.rs` 事件循环中 `fast_lock.tick()` 在每次 `recv_timeout` 唤醒时调用；模式切换/息屏 doze/亮屏恢复/看门狗超时/panic 收尾均正确 release fast_lock。

- 8550/8475/8998 的 `feature.yaml` 没有 `vector:` 段（该档由 `fast_lock` 硬锁最高频、不读 CLG 参数；共享 `config/feature.yaml` 里有 vector 段，是代码默认值的来源）；akmode 已改为**无档位连续控制**（见 `normal/tuned_profiles-example.yaml`），不受影响。

### CPU 亲和与线程迁移（Affinity，ChiRi 专属）

- `src/chiri/affinity.rs` 的 `AffinityManager`（消费 `SysPathExist` 已探测但此前无人使用的 cpuset/cpuctl 能力位）。

- boost 类模式（boost/vector/特调，判定口径统一在 `chiri/mod.rs::is_boost_mode`；fas 模式亮屏在 `apply_affinity_and_corectl` 调用点同样并入 boost——FAS 只管调频、线程摆放沿用 boost 布局，息屏 FAS 释放后回退 normal，与 akmode 息屏保持 boost 不同）下：`/dev/cpuset/top-app`、`foreground` 的 `cpus` 收窄到大核+超大核（区间用 `common::chiri_core_ranges()`，不硬编码）；`background/system-background/restricted` 压到小核。

- 可选写 `/dev/cpuctl/top-app/cpu.uclamp.min`（配置 `Affinity.top_app_uclamp_min_pct`，默认 0 关闭——uclamp.min 会让 schedutil 独立于 CLG 抬频，避免与动态上限语义打架）。

- 可选写 `/dev/cpuctl/top-app/cpu.uclamp.max`（配置 `Affinity.top_app_uclamp_max_pct`，任务级性能上限钳制、EAS 原生感知，比 scaling_max_freq 硬顶更细）。按机型配置：8550/8475 配 85，8998 内核 4.4 配 0 关闭。运行时内核版本识别兜底纠正：`kernel_version()` 解析 `/proc/sys/kernel/osrelease`，< 5.3（uclamp 主线引入版本）或节点缺失或写入回读无效（防厂商半成品 backport 静默忽略）任一不满足即置 Unsupported 永久跳过并 warn 打点 `affinity-uclamp-unavailable`，判定结果缓存避免重复探测。

- **后台降权（2026-09-18 加；配置 `Affinity.background_uclamp_max_pct`，默认 50，0 = 关闭）**：写 `background` 与 `restricted` 两组的 `cpu.uclamp.max`（**刻意不含 system-background**——系统后台含媒体/音频等服务，画中画、后台播放等可感知场景保守跳过），把后台任务的 util 需求钳低（50 = 512）——EAS 放置与 schedutil 频率随之回落、优先落小核，**不禁止使用大核**（空闲时仍会被 EAS 调度上去）。boost/normal 两态在 `apply` 汇合点持续写（「始终压低后台、给 UI/视频让路」），幂等；`release()`/关闭总闸时写回内核默认 `max`。目标形态：UI（top-app/foreground 收窄 + 线程钉核）与视频（playback 特调 / 前台进程天然在 foreground）优先，后台运算/内容计算只做软降权。与下一条后台 promote 的关系：promote 仍会把「忙」后台线程送 big（能效兜底），但其 uclamp 需求已被压住，占核权重有限——如需彻底禁止后台升核，把 promote 门槛收紧或加开关（未做）。

- 线程层（按核心粒度放置，`pin_foreground_threads=true` 启用，每 2s 再平衡一轮、前台 PID/boost 变化立即触发）——**开销控制优先，不做逐线程每轮读 stat**：
  - 前台（fg_pid 由 app_detect 提供）：每轮 1 次 `read_dir /proc/<pid>/task`，仅对**新增**线程读一次 stat 判定关键/建档；存量线程的 home 合法性仅用缓存的逐核 util 与在线位图判断（合法范围 = prime ∪ big 全部性能核，含溢出落点）。**主池/溢出两级选核**（`pick_core_pref`）：关键线程（tid==pid 或 comm 命中 RenderThread/GLThread/GameThread/UnityMain/UnityGfxDeviceW 白名单）主池 = prime、溢出 = big（超大核全被钉才回落大核，UnityMain 等白名单线程直接绑定超大核）；普通线程主池 = big、溢出 = prime（**大核钉满后自动启用超大核**，修复"多个重负载挤单一大核而超大核闲置"）。boost + 亮屏才钉，否则恢复全核；已消失线程下一轮即清理释放钉核计数。
  - **default（非 boost）轻量前台保护**：8550 实测日常 default 模式下后台压小核 + EAS 把前台任务也堆在小核，little p50=88~99% 排队而 prime p50≈0% 空转、大核半闲（后台 promote 只救后台组线程，前台关键线程无人救援）。little 高水位（`LITTLE_HIGH_WATER` 0.70，迟滞 <`KEY_BIND_RELEASE_WATER` 0.50 解除，boost/息屏强制解除）时把前台关键线程组绑定到 big∪prime（reason `normal_press`），不钉单核、不占钉核计数。**非关键前台线程按窗口 util 升核**：压力窗口内每轮限量采样（`FG_SCAN_WINDOW` 32，游标轮转，窗口外零采样零写入），连续两窗 util ≥ `FG_BUSY_UTIL_PCT`(30%) 绑到 big∪prime（reason `normal_busy`），跌到 `FG_BUSY_RELEASE_UTIL_PCT`(15%) 以下连续 `DEMOTE_STREAK` 轮才回落——绑定/释放水位构成滞回，避免 5~30% 的中等负载线程长期滞留性能核反而抬高功耗（勿把释放水位改回 `DEMOTE_UTIL_PCT` 的 5%）。组绑定状态用单一枚举 `GroupBind`（None/Key/Busy）表达，绑/放只改一个字段，勿改回 `group_pinned + busy_bound` 两个需手工同步的布尔量。两窗忙判定由 `busy_window_update` 供前台/后台共用，勿各写一份；boost 布局接管时无缝衔接（掩码一致）。

  - 后台动态亲和（**不把后台全压小核**——小核过载能效灾难）：忙线程 promote 到 big、回落 demote。候选 TID 从三个后台 cpuset 的 `tasks` 文件读取（**不做 /proc 全量枚举**），每 2 轮刷新、按游标分片每轮只深扫 64 个；窗口 util = ticks 差分，**两窗防抖**（上次采样忙且本次仍忙、期间采到低负载即清标记）即 promote，不依赖采样间隔。promote 先把 TID 移入 top-app（cpuset v1 按 TID 记账）再按当前核心占用选核钉定（`pick_core_pref`：big 有未钉核只看 big、钉满溢出 prime，与前台普通线程同口径——游戏场景 big 是关键线程主场，后台忙线程不挤占但也不排队）。已 promote 线程每 2 轮复查：util 连续 3 次 < 5% → demote；线程仍忙（≥5%）且所在核心 util > 70%（`CORE_OVERLOAD_UTIL`）→ 换低占用核（`bg_overload`，带 4s 迁移防抖）。过载重钉统一口径（前台 `home_overload` / 后台 `bg_overload` 共用）：**候选 = 全性能池（big∪prime）最低分核 + 双滞回**（分数差 ≥ `OVERLOAD_MARGIN` 且目标核 util 严格低于 home）+ 反跳回冷却（`RETURN_COOLDOWN` 16s 禁回刚迁离核）。旧「big 池内选核 + 目标核 util ≤ 70% 硬门槛」在整体高载时必然静止——8475 实测 FAS 下大核 86-90% 排队、prime 空转 45%、26 分钟 85 次 `overload_hold`；全员过载时把负载摊向低载核（含 prime）仍有收益，乒乓由防抖+冷却兜底。**后台迁移仅亮屏**。

  - 在线核位图每 4 轮读一次并缓存（核热插拔不频繁）；place/core 行已随 aff\_ 拆分移除（2026-09-22）。

  - **多应用快速切换**：每轮再平衡开头按 pid 归属立即清理旧前台线程（`pid>0 且 ≠ 当前 fg_pid` → 解钉恢复全核），不等 30s 失联——否则旧应用转后台后（Android 会短暂把它留在 top-app/foreground cpuset）其单核掩码与大核相交继续生效，8550 仅一颗 prime 会让新旧前台关键线程同核互踩，连续切换还会令 core_pinned 计数漂移累积。过滤器幂等、零文件 IO；后台 promote 线程（pid==0）不受影响。模式变化的切换经 ModeChange → force 立即重平衡；同模式切换（无事件）依赖 2s 周期块发现，钉核延迟 ≤2s（可接受，切换清理在同一轮完成，新前台钉核时 core_pinned 已准确）。

  - 选核算法：score = 逐核 util（最近 SystemLoadUpdate 快照，即核心当前占用）+ 钉核计数 × 0.4（`PINNED_WEIGHT`，权重过弱会让 util 快照滞后时第二个重线程挤入"看似空闲"的核造成同核轮转卡顿），取候选 ∩ 在线核最低分核（离线核 util 恒 0.0、与空闲不可区分，钉到「唯一允许核为离线」会让线程有效可运行集为空而冻结，必须排除）；home 核过载（核 util > 70%，`CORE_OVERLOAD_UTIL`，前后台共用）或 home 离线在 **8s** 重钉防抖（`REPIN_DEBOUNCE`，promote/demote 保持 4s `MIN_MIGRATE_INTERVAL` 不影响负载响应）窗口外重钉，选回同核不写避免空迁移。

  - 掩码用 `mem::zeroed::<cpu_set_t>()` 分配保证对齐（`vec![0u8]` 强转是未对齐指针的脆弱实践），`libc::CPU_SET` 置位并带容量边界防护。

  - 稳态（前台线程集不变、后台空闲）单轮 ≈ 1 read_dir + 0\~64 stat + 低频辅助文件读——相对「逐线程每轮全读 stat」约省 >50% 文件 I/O。

- 黑名单（全部编译嵌入、不外透）：`src/chiri/affinity_blacklist.yaml` 经 include_str! 打包（common.rs 的 `parse_affinity_blacklist` 解析、OnceLock 缓存），格式：每行一条精确进程名/包名或 `re:` 前缀正则（同特调白名单语法，含 com.example 注释示例），文件内含**系统进程默认名单**（system_server/surfaceflinger/logd/lmkd/zygote 等，勿删）；内置兜底：空 cmdline（内核线程）与 `/` 开头（native 二进制路径）一律黑名单。命中进程全部线程保持全核、不做任何迁移，`is_affinity_blacklisted` 另在逐线程层面兜底（防应用进程内的系统服务线程被误迁）。

- normal/doze 布局：top-app/foreground/uclamp 恢复快照，后台保持压小核 + bg uclamp 降权持续（两态都写）；boost 退出/开关关闭/调度线程收尾 `release()` 全量还原（已钉线程恢复全核，经 `demote_tid_group` 写回进程当前 cpuset 组）。同模式 App 切换不发 ModeChange 事件，靠 scheduler_ipc 2s 周期刷新兜底重迁移（manager 内部按 KIND/PID/boost 去重）。

- **恢复写失败保留状态（2026-09-23）**：`unpin_core`/`cleanup_thread`/`demote` 的内核写失败且非 ESRCH 时**保留状态**（home/钉核计数/moved_group/条目）待 departed、gone、stale 等清理路径重试，成功或 ESRCH（线程消亡、无从重试）才清；改回「写失败仍清状态」会让内核掩码/组归属与状态表永久分叉（掩码仍单核而状态记全核，再无重试路径）。stat comm 解析口径同日修正：`sample_one_tid` 以首 '(' 与末 ')' 定界 comm（旧 `text[1..close]` 把「tid 尾部+` (`」混进 comm，`KEY_THREAD_COMMS` 与 `affinity_blacklist` 的精确匹配恒不命中；修复后关键线程白名单/黑名单逐线程兜底恢复生效）。

- 配置段 `Affinity`（enabled/top_app_uclamp_min_pct/top_app_uclamp_max_pct/pin_foreground_threads/background_uclamp_max_pct），三份 SoC yaml 已带（背景降权默认 50；root feature.yaml 无 Affinity 段、走 serde 默认）。

### core_ctl 核心在线接管（ChiRi 专属）

- `src/chiri/core_ctl.rs` 的 `CoreCtlManager`，**三态状态机**（None/Boost/Scenemode，`set_power_state(boost, scenemode)` 统一入口，内部去重可被 2s 周期安全调用；切换时先退出旧状态恢复快照再进入新状态）。调度线程收尾 `release()` 按当前状态恢复。**NONE 与 BOOST 稳态都重试 `offlined` 残留核**（每 2s）——只重试 NONE 的话，scenemode 退出恢复失败后用户随即进入 boost（fas/performance 全时段 boost），prime 会整场游戏保持离线（超大核异常离线实测根因之一，2026-09 修复）。

- **Boost**：把各 cluster 的 core_ctl `min_cpus` 抬到全组常在线（防厂商热插拔与 ChiRi 调频打架），退出恢复快照；只动 min_cpus 不动 max_cpus/busy 阈值。

- **Scenemode 离线核（[已暂停] 息屏深度省电）**：`CoreCtl.scenemode_offline` 门控（8550/8475 true，8998 内核 4.4 默认 false）。进入 scenemode 时先解除 boost（min_cpus 抬着会让厂商 core_ctl 重新拉起被下线的核——两者互斥由 `apply_affinity_and_corectl` 保证），下线目标由 `scenemode_targets()` 计算：**小核 + 大核全开常驻**（频率上限由 scenemode CLG 配置统一压制），**仅 prime 整簇下线**消除空转漏电流；逐核写 online=0 回读验证，失败跳过（warn），已在 offlined 中的核防重复登记。**独占一颗小核给调度服务**（编号最大的 little——三步实现：① `affinity::exclude_core_from_cpusets` 把该核从全部业务 cpuset 组（top-app/foreground/background/system-background/restricted）的 cpus 移除，其他进程/新进程（继承组掩码）均不可调度到该核；② 自身全部线程移入 cpuset 根组（根组含全部在线核，sched_setaffinity 才不会被原组掩码二次过滤）；③ 全线程自钉到该核）；设备无 /dev/cpuset 时降级为仅自钉。维持期每 2s 纠偏：重新下线被外部拉起的核 + `exclude_core_from_cpusets` 重写被框架 CpusetManager 加回保留核的组（快照只记首次原始值，防框架中间值覆盖）。**退出恢复 `restore_online` 失败的核必须保留在 offlined 中由 STATE_NONE 分支每 2s 重试**——此前失败即 clear、状态机回 NONE 再无重试路径，写回被内核拒绝的核永久离线；全部恢复后才释放独占（cpuset 快照还原 + 自身线程移回原组 + 解除自钉），重新下线前先把残留核拉回在线（否则 online=0 被跳过记录、核永远失去恢复登记）。

- **scenemode 饱和退出**：常驻簇（小核∪大核）max_util 持续 10s ≥ 70%（`SCENEMODE_SAT_UTIL/SECS`，util 是忙时占比与频率无关，饱和即真饱和）→ 视为后台负载压不死常驻核：一次性退回 reduce 的 CLG 配置 + 立即恢复全部在线核 + 释放独占小核，并进入 **300s 冷却**（`SCENEMODE_COOLDOWN`，期间 scenemode 入口被门控不得重进，防反复拉锯）；冷却结束后息屏条件仍满足则自然重进。

- **自钉/解钉按成功清单（2026-09-23）**：`pin_self_dedicated` 记录实际钉住的自身 tid 清单（`self_pinned_tids`，只记成功），`unpin_self` 按清单逐个恢复全核（非 ESRCH 失败留清单下次重试）；勿用 `self_pinned` 总开关短路恢复——部分钉定失败时 `self_pinned=false` 会让 unpin 整体跳过，已钉住的线程永久滞留单核掩码。

- 为什么选核排除离线核而不"按需唤醒"：唤醒大核要拉电压轨/重建 L2，为后台线程点亮大核净亏能；直接写 online 会与厂商热插拔守护进程打架（对方再下线，ping-pong）。需要更多在线核时的正确姿势是抬 core_ctl min_cpus（Boost 态）。scenemode 是唯一反向使用 online 写入的场景（目标恰恰是让 prime 睡死，小核+大核常驻保住待命响应）。

- cluster 发现：遍历 `get_cpu_policies()` → related_cpus 首个 CPU 的 `/sys/devices/system/cpu/cpuN/core_ctl`（每 policy 一份，天然去重），惰性枚举一次；无节点打点 `corectl-unavailable` 后保持空表（scenemode 直接 sysfs 离线**不依赖** core_ctl 节点，仅受独立配置门控）。

- 配置段 `CoreCtl`（enabled/scenemode_offline）。

### 遥测数据源（Telemetry，ChiRi 专属）

- `src/monitor/telemetry.rs` 的 `telemetry_loop` 线程（1s 轮询，仅 `is_chiri_soc()` 时由 monitor/mod.rs 启动）读 PSI（`/proc/pressure/{cpu,io,memory}` 的 some avg10）、GPU busy%（`/sys/class/kgsl/kgsl-3d0/gpu_busy_percentage` → MTK `/sys/kernel/ged/hal/gpu_loading`，缺失为 None）、电池电流/电压。

- 电池读数来源（2026-09-18 重构，用户口径）：**默认走 Android 标准节点**（`/sys/class/power_supply/battery/{current_now,voltage_now}`，ABI = µV/µA）；meta `oplus_chg`（默认 false）打开后**优先读 OPlus 私有节点** `/sys/class/oplus_chg/battery/bcc_parms`（逗号分隔，**0 基下标**：下标 6 = 电芯0电压、8 = 电流、11 = 电芯1电压，即第 7/9/12 个字段；字段按 mV/mA 解读、**原样存不做归一**，两路单位差异全交给校准除数），**节点存在且读到有效值时才只认它**（2026-09-18 用户口径两段：① 同机型的标准通用节点不可信，OPlus 上约 10s 才刷新、单位也不保证——所以私有节点有效时不做回退；② 「存在」本身不等于「有效」，字段缺失/解析失败/全零都算无效，**无效时回退标准节点**，此刻它是唯一还能给数的来源，回退好过整段读数变空，无效原因由 `telemetry-bcc-unusable` warn 打点）：只有节点本身不存在（非 OPlus 机型）或读到无效值时，才走标准节点。**旧的量级启发式（mV/V、mA/A 自动识别）与物理范围门（2–6V / ±30A）已删除**——单位不匹配交给「单位校准」，代码不再猜。OPlus 内核的标准 power_supply 节点约每 10s 才刷新，1s 精度采样必须绕开（私有节点存在的理由）。
- 双电芯与单位校准（同一批 meta 字段）：`oplus_dual_cell`（默认 false，私有节点路径）= **两节并联（2P）口径**：电压取两节**平均**（下标 6 与 11）、电流 **×2**（下标 8 是单节支路电流，整包 = 两节之和；2026-09-18 用户补充）；下标 11 非正视为单电芯布局，电压退回下标 6 且电流不翻倍——**不取电压和**（那是 2S 串压口径，标准节点只报 ~4V 的机器上功率会翻倍）；判据仍是私有节点算出的 P 与「标准节点 V × I」同量级（±20%）；`voltage_double` / `current_double`（默认 false，**仅标准节点路径**）分别把电压 / 电流 ×2（双电芯机型上标准节点常只报单节/单芯值）——**与 `oplus_chg` 互斥**：`chiri::config::Config::load` 推给遥测层时强制 `!oplus_chg && x`，WebUI 打开私有节点时在同一笔写入里清掉这两项并置灰（`state.setBatteryFields` 内联合并，避免「私有开 + 倍压还开着」的中间态被热重载读到）。`voltage_divisor` / `current_divisor`（**各默认 1000000**，须 > 0）：**全链没有内置换算**（2026-09-18 用户要求「去除所有内置校准」），`节点原始值 ÷ 校准值` 直接就是输出——电压得 **V**、电流得**安培**（`batt_current_ma()` 的名字与 status.csv 的 `batt_current_ma` 列名都是历史遗留，值就是安培；该列 3 位小数），`batt_power_w` = 电流值 × 电压 = W。校准值 = 「原始单位 → 输出单位」的除数：**缺省 1000000 正是标准 Android ABI 口径（节点报 µV/µA）；OPlus 私有节点 bcc_parms 报 mV/mA → 实际会算出 1000，由安装脚本装机时按节点电压位数自动写入**（读取层不做任何归一——原先私有节点的 ×1000 与标准节点的 ÷1000 两步都已删除，两张基准靠这两个值区分）——`Telemetry::batt_voltage_v` / `batt_current_ma` 两个访问器执行，`batt_power_w` 仍是 |mA| × V（W = V×A 保持自洽）。**分开的理由**：节点的电压与电流未必同时错单位，共用一个值会让功率按平方变化、也说不清是谁的锅。旧键 `unit_divisor`（曾把两者合一）仍接收，等价于只设电压校准（`MetaYamlFile` 保留该字段 + `parse_disk_meta` 取 `.or(...)`，避免老文件被 `deny_unknown_fields` 判非法整份重置）。六个字段都是「出现即校验、缺省沿用内嵌默认」，经 `telemetry::set_battery_options` 落到进程级原子量（1s 遥测线程与各消费点读它，不每轮解析 YAML）。

- **安装期自动校准（2026-09-20，`customize.sh` 的 `[battery-detect]`）**：包内两个 divisor 默认 **1000000**（标准 Android ABI 的 µV/µA 口径）；安装器按机器把两个值改写成实际口径，否则电压/电流差三个数量级。安装器在「不保留配置」分支里只改 `$MODPATH`（= modules_update 暂存）下**当前机型**那一份 meta.yaml——完整安装由安装器落地、热更新由 `cp -r` 覆盖到 live 目录，**两条路同源**，之后才按分支决定「报安装完成退出」还是「走热更新的复制 + 删暂存 + abort」。**两条路走同一个入口 `apply_divisor_from_raw`**：按**节点电压原始值的位数**推算除数并写入两个 divisor（原始值 n 位 → 除数 = 1 后跟 n-1 个 0，4382→1000、4382000→1000000；电压与电流同值——同一节点两路单位一致），只是原始值来源不同——读到 `bcc_parms` 有内容 → `oplus_chg` 置 true 且取**下标 6（电芯0电压）**；读不到（或读到空）→ 取 `/sys/class/power_supply/battery/voltage_now`。**不做任何硬编码**：私有节点通常报 mV/mA 会算出 1000，万一某机型报 µV/µA 也能自动算成 1000000；缺省 1000000 已是 ABI 标准，这条启发式只在节点单位不是 ABI 标准时才改变它。**首位不是 3/4、读数非数字、字段不足（<7 项）或节点不可用就一律不写**，宁可留默认值让用户在 WebUI 电池读数页改（量级填错会让功率整条曲线失真）。这与上面「daemon 不做量级启发式」不冲突：**daemon 运行期仍然不猜，只有安装器在装机那一刻按节点量级写一次**。另修正两处：① 双电芯判据原来直接 `cut -d',' -f 12` 判非空——**字段不足 12 个时 cut 会把整行原样输出**（无分隔符的行默认透传），单电芯机型被误判为双电芯；现先数逗号确认 ≥12 个字段、且第 12 个字段为正数（与 `read_oplus_bcc` 同口径）；② 热更新末尾「恢复备份」只在**保留配置**那条路执行：选了覆盖却把旧 `config/meta.yaml` / `rules.yaml` 盖回去，会把刚写进去的校准倍数顶掉（回退机型的生效配置就是根上那份 `config/meta.yaml`；ChiRi 机型的生效配置是 `config/<soc>/meta.yaml`，本就不在这条备份链里，行为本就不一致）。

- 结果写入进程级共享原子量 `telemetry()`（f32 bit pattern 存 `AtomicU32`，与热保护/触摸状态同口径），不占事件通道容量。

- eBPF 扩展探针（`yumi-ebpf/src/main.rs` 的 `handle_sched_wakeup`/`handle_sched_migrate_task`/`handle_cpufreq_transition`，PerCpuArray 计数）由 `cpu_monitor.rs` 仅在 ChiRi 上可选挂载（内核缺 tracepoint 时 warn 一次跳过，不影响主探针），每 2s 读累计值取增量发 `DaemonEvent::BpfStats`（非 ChiRi 机型不发送）。

- chiri scheduler_ipc 以 `TELEMETRY_LOG_INTERVAL=1s` 消费：写 `logs/status.csv`（logger.rs `status_log_snapshot`，1s 一行，功耗精度 1s）+ 20s 一条 debug 摘要 `telemetry-summary`。BpfStats 不刷新 CLG 看门狗心跳（探针失效不影响负载源判定）。

- BCC 失效可见性（2026-09-11）：`bcc_parms` 下标 6/8（电压/电流）缺失或非整数时此前 `?` 静默回退标准节点，「BCC 从未生效、功耗列一直来自 10s 缓存节点」完全不可见（8550 实测 batt_current_ma 99% 在 ±5、74% 为 0）。现 warn 一次 `telemetry-bcc-unusable`（去重）后回退，便于从 daemon.log 确认功耗列口径。

- Sched 内核参数微调（2026-09-20，借鉴 LittleYouran CTS 的 Scheduler 段）：feature.yaml 新增 `Sched` 段（enabled + params map），`scheduler.rs::apply_sched_params` 按 `SCHED_ALLOWED_PARAMS` 白名单写 `/proc/sys/kernel/<key>`（越界键 warn 跳过、空值跳过，随 apply_system_tweaks 热重载生效）。8550/8475/8998 默认开启 `sched_migration_cost_ns: 200000` / `sched_nr_migrate: 27`（骁龙 855 八核经验值：迁移更及时、单轮迁移量收敛）。i18n：`sched-tuning-applied` / `sched-tuning-key-rejected`。DOWN 停摆期间随其余一次性调整一并不下发（进入时按快照还原已写的节点），退出停摆补发。

- **子进程名一律预处理为主包名（2026-09-20，借鉴 CTS 的 AppModeConfig::resolve，用户口径）**：`com.xx:push` 本质上是 `com.xx`（子进程与所属包在调度语义上就是同一个应用；厂商框架把 cmdline 首段写成 `pkg:proc` 同理），所以**归一到主包名再放进整个调度计算，下游走原本的算法与流程**，不做进程级粒度区分。归一在两侧各做一次、口径一致：① 前台名在**唯一入口** `app_detect::set_current_package` 归一（此后 `get_current_package()`、ModeChange/PackageSwitch 事件、main\_ 分组、通知、亲和拿到的都是主包名）；② 规则键在 `deserialize_app_modes` 归一（冲突时保留后者并打 `rule-key-conflict`，不静默丢规则）。因此 `determine_mode` 里就是原本的 `app_modes.get(pkg)` 精确查表，**没有回退链**——回退链会让「规则键粒度」与「运行期粒度」两套语义并存。i18n：`app-detect-pkg-normalized`（debug）/ `rule-key-normalized` / `rule-key-conflict`。
