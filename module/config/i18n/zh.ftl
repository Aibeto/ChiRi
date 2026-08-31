# --- Main & Monitor ---
yumi-module-starting = yumi-module 统一启动中...
scheduler-module-started = 调度器模块已启动
scheduler-module-start-failed = 启动调度器模块失败: { $error }
monitor-module-crashed = 监控模块崩溃: { $error }
monitor-module-started = 监控模块已启动
monitor-starting = 正在启动 yumi-monitor 模块...
monitor-initial-config-failed = [Main] 读取初始配置失败: { $error }. 正在使用默认值。
monitor-screen-watcher-failed = [Main] 屏幕状态监控线程崩溃: { $error }
monitor-config-watcher-failed = [Main] 配置监控线程崩溃: { $error }
monitor-fps-crashed = [Main] FPS 监控崩溃: { $error }
monitor-fps-tokio-failed = [Main] 无法为 FPS 监控创建 Tokio 运行时
monitor-cpu-crashed = [Main] CPU 负载监控崩溃: { $error }
monitor-cpu-tokio-failed = [Main] 无法为 CPU 监控创建 Tokio 运行时
monitor-rlimit-memlock-failed = [Main] 提升 RLIMIT_MEMLOCK 失败，eBPF Map 可能无法加载。
main-chdir = [Main] 切换工作目录到: { $dir }
main-module-root = [Main] 模块根目录: { $path }
main-config-loaded = [Main] 已读取配置: { $path } (loglevel={ $loglevel }, language={ $language })
main-chiri-scheduler-selected = [Main] 检测到特定处理器，已启用 Chiri 专用调度器
main-special-tuned-exported = [Main] 已导出 { $count } 个内部特调白名单条目到 special_tuned.txt
monitor-thread-start-screen = [Main] 启动屏幕状态监控线程...
monitor-thread-start-config-watch = [Main] 启动配置监控线程...
monitor-thread-start-fps = [Main] 启动 eBPF FPS 监控线程...
monitor-thread-start-cpu = [Main] 启动 eBPF CPU 负载监控线程...
monitor-thread-start-app-detect = [Main] 启动应用检测主循环...

# --- AppDetect ---
app-detect-config-watch = [AppDetect] 开始监控配置文件: { $path }
app-detect-change-detected = [AppDetect] 检测到变更，正在防抖 (100ms)...
app-detect-reloading = [AppDetect] 防抖结束。正在重载配置...
app-detect-load-failed = [AppDetect] 失败: { $error }。使用默认值
app-detect-reload-success = [AppDetect] 配置重载成功
app-detect-loop-started = [AppDetect] 应用检测循环已启动 (3000ms 轮询)
app-detect-screen-changed = [AppDetect] 屏幕状态变更: { $old } -> { $new }
app-detect-mode-change-pkg = [AppDetect] 模式变更: { $old } -> { $new } ({ $pkg })
app-detect-ime-auto = [AppDetect] 自动检测到输入法: { $pkg }
app-detect-ime-fallback = [AppDetect] 自动检测输入法失败，使用后备列表。
app-detect-debounce-start = [AppDetect] 防抖开始: 检测到新应用 { $pkg } (pid={ $pid })
app-detect-debounce-confirmed = [AppDetect] 防抖确认: 应用 { $pkg } (pid={ $pid }) 保持稳定
app-detect-pkg-change = [AppDetect] 前台应用状态变化: { $pkg } (pid={ $pid }, temp={ $temp }°C, force={ $force })
app-detect-no-app = [AppDetect] 未检测到有效前台应用 (可能为系统进程或未知包)
app-detect-special-override = [AppDetect] 特调模式应用: { $pkg } -> { $mode }
app-detect-special-rejected = [AppDetect] 非白名单应用 { $pkg } 映射到特调模式 { $mode } 已拒绝，回退全局模式
app-detect-special-unavailable = [AppDetect] 特调配置不可用（akmode.yaml 缺失/损坏），{ $pkg } 映射的 { $mode } 不生效，回退全局模式
app-detect-special-fallback = [AppDetect] 特调白名单命中: { $pkg } 使用优先回退模式 { $mode }
app-detect-special-global-rejected = [AppDetect] 全局模式 { $mode } 为特调模式，不适用于非白名单应用 { $pkg }，回退 balance

# --- ScreenDetect ---
screen-state-change-detected = [Screen] 通过 '{ $source }' 检测到状态变更
screen-state-changed-value = [Screen] 屏幕状态已变更: { $state }
screen-netlink-started = [Screen] 已启动 netlink-sys 套接字监听器
screen-state-detect-detail = [Screen] 状态判定: { $old } -> { $new } (来源: { $source })
screen-uevent-received = [Screen] 收到 uevent: subsystem={ $subsystem } devpath={ $devpath }
screen-uevent-power-action = [Screen] 电源动作: { $action }
screen-uevent-backlight = [Screen] 背光事件: { $dev } -> state={ $state }
screen-uevent-backlight-unreadable = [Screen] 背光状态不可读: { $dev }

# --- Monitors ---
cpu-monitor-started = [CPU Monitor] eBPF 系统负载监控已启动 (修复长任务盲区)。
cpu-monitor-online-cpus-failed = [CPU Monitor] 获取在线 CPU 失败: { $error }
cpu-monitor-online-cpus = [CPU Monitor] 检测到在线 CPU 核心 ID: { $cpus }
cpu-monitor-fg-pid-updated = [CPU Monitor] 前台 PID 已更新 { $old } -> { $new }
cpu-monitor-baseline = [CPU Monitor] 基线初始化 | 在线核心={ $cpus } 最大核心ID={ $max_cpu }
cpu-monitor-fg-baseline-reset = [CPU Monitor] 前台 PID 变化，重置利用率基线: { $old } -> { $new }
cpu-monitor-util-fallback = [CPU Monitor] TGID map 无数据，降级到线程级计算 (pid={ $pid }, raw_tgid={ $raw })
cpu-monitor-tick-log = [CPU Monitor] 核心=[{ $cores }] 前台pid={ $pid } 前台最大利用率={ $util }% 跟踪线程数={ $threads } 耗时={ $delta }ms
cpu-monitor-channel-closed = [CPU Monitor] 通道已关闭，退出循环。
fps-monitor-init = [FPS Monitor] 正在初始化 eBPF FPS 监控...
fps-monitor-attached = [FPS Monitor] 已挂载 uprobe 到 PID: { $pid }
fps-monitor-attach-failed = [FPS Monitor] 未能挂载任何 Uprobe 符号！
fps-monitor-attach-failed-initial = [FPS Monitor] 初始挂载失败: { $error }
fps-monitor-init-no-pid = [FPS Monitor] 前台 PID 未知，等待前台应用启动...
fps-monitor-pid-filter-updated = [FPS Monitor] 目标 PID 已更新: { $old } -> { $new }
fps-monitor-pid-switching = [FPS Monitor] 正在切换目标 PID: { $pid }
fps-monitor-pid-switched = [FPS Monitor] 已切换到目标 PID: { $pid }
fps-monitor-pid-switch-failed = [FPS Monitor] PID 切换失败: { $error }
fps-monitor-started = [FPS Monitor] eBPF FPS 监控启动成功（per-PID uprobe 模式）
fps-monitor-symbol-short-miss = [FPS Monitor] 短签名符号 attach 失败，尝试长签名符号...
fps-monitor-attach-symbol = [FPS Monitor] 使用符号 attach: { $lib } (pid={ $pid })
fps-monitor-frame-summary = [FPS Monitor] 帧摘要 | pid={ $pid } 窗口={ $window } 最新={ $latest_ms }ms 平均={ $avg_ms }ms

# --- Scheduler ---
scheduler-ipc-started = [Scheduler] IPC 通道监听器已启动
scheduler-mode-change-request = [Scheduler] 模式变更请求: { $old } -> { $new } (包名: { $pkg }, 温度: { $temp })
scheduler-apply-failed = [Scheduler] 应用设置失败: { $error }
scheduler-channel-closed = [Scheduler] 通道已关闭！线程退出
scheduler-ipc-panic = [Scheduler] IPC 线程发生 panic，正在释放 CPU 控制权。
scheduler-doze-enable = [Scheduler] 息屏: 启用深度睡眠模式 (限制 CPU 最高性能)。
scheduler-doze-special-keep = [Scheduler] 息屏: 特调模式保持接管，不切换 CLG doze。
scheduler-doze-restore = [Scheduler] 亮屏: 恢复之前的性能限制。
scheduler-clg-init = [Scheduler] CPU 负载调频器: 在启动时初始化 (模式={ $mode })
scheduler-event-screen = [Scheduler] 收到屏幕状态事件: on={ $on } (此前 last={ $last })
scheduler-event-mode-change = [Scheduler] 收到模式切换事件: 包={ $pkg } { $old } -> { $new } (温度={ $temp })
scheduler-event-load = [Scheduler] 收到负载事件: 核心利用率=[{ $cores }]
scheduler-event-frame = [Scheduler] 收到帧事件: 帧间隔={ $delta_ms }ms
scheduler-event-config-reload = [Scheduler] 收到配置重载事件: 当前模式={ $mode }, 亮屏={ $screen_on }
scheduler-special-mode-active = [Scheduler] 特调模式激活: { $pkg } -> { $mode }
scheduler-scene-mode-enter = [Scheduler] 息屏已超过阈值，切换到 scenemode 省电模式

# --- Scheduler: Config Watcher ---
config-reloading = [Config] 检测到配置文件变更，正在重载...
config-reloaded-success = [Config] 配置重载成功
config-reload-fail = [Config] 配置重载失败: { $error }
config-special-load-failed = [Config] 特调配置文件读取失败: { $path } ({ $error }) — 特调不可用，白名单应用回退 CLG
config-special-parse-failed = [Config] 特调配置文件解析失败: { $path } ({ $error }) — 特调不可用，白名单应用回退 CLG
config-special-merged = [Config] 已合并特调配置文件: { $path }
config-watch-error = [Config] 监控配置目录失败: { $error }
config-apply-mode-failed = [Config] 应用重载的模式设置失败: { $error }
config-apply-tweaks-failed = [Config] 应用重载的系统微调失败: { $error }

# --- SysFS (共享 FastWriter) ---
sysfs-open-failed = [SysFS] 打开 { $path } 失败: { $error }
sysfs-umount2-failed = [SysFS] umount2({ $path }) 失败: { $error }
sysfs-write-freq-failed = [SysFS] 写入频率 { $freq } 失败: { $error }

# --- CLG ---
clg-init = [CLG] P{ $pid } 初始化 | 核心={ $cpus } | 频率={ $fmin }-{ $fmax } MHz | P={ $perf } -> { $freq } kHz
clg-activated = [CLG] CPU 负载调频器已激活，共接管 { $count } 个集群
clg-no-clusters = [CLG] CPU 负载调频器: 未找到有效集群，保持未激活状态
clg-deactivated = [CLG] CPU 负载调频器已停用
clg-config-reloaded = [CLG] 配置已热重载 | 升频={ $up } 降频={ $down } 地板={ $floor } 天花板={ $ceil }
clg-perf-clamped = [CLG] 配置 perf_floor > perf_ceil ({ $floor } > { $ceil })，已将 perf_floor 限制为 perf_ceil
clg-restore = [CLG] P{ $pid } 已恢复 | governor={ $governor } min={ $min } kHz max={ $max } kHz
clg-tick-log = [CLG] P{ $pid } 利用率={ $util }% perf={ $perf } 频率={ $freq }kHz boost={ $boost }kHz
clg-writer-invalid = [CLG] P{ $pid } sysfs 写入器无效 (max_valid: { $max_valid }, min_valid: { $min_valid })，已跳过。
clg-freq-set = [CLG] P{ $pid } 频率调整: { $old_khz }MHz -> { $new_khz }MHz
clg-freq-write-failed-cached = [CLG] P{ $pid } 频率写入失败，保持缓存值 { $cached_khz }MHz (目标 { $target_khz }MHz)
clg-watchdog-release = [CLG] 看门狗: 已 { $secs } 秒未收到负载事件，eBPF 负载源疑似失效，已释放 CPU 控制权恢复系统默认调频
clg-up-skipped = [CLG] P{ $pid } 实际频率={ $cur_khz }MHz 未达当前锁定 { $lock_khz }MHz，忽略本次升频（schedutil 余量，按需升频）
clg-touch-boost = [CLG] 触摸升频窗口开启：大核性能下限={ $floor } 保持 { $ms }ms

# --- AKMode（明日方舟特调） ---
akmode-init = [AKMode] 明日方舟特调接管 | 档位={ $mode }
akmode-activated = [AKMode] 明日方舟特调已激活（schedutil + 档位限频，不自动切档）
akmode-no-clusters = [AKMode] 明日方舟特调: 未找到有效集群，保持未激活状态
akmode-cluster-skipped = [AKMode] P{ $pid } 跳过接管 (原因: { $reason })
akmode-deactivated = [AKMode] 明日方舟特调已停用
akmode-config-reloaded = [AKMode] 特调配置已热重载 | 档位={ $mode }
akmode-tick-log = [AKMode] 档位={ $mode } 升频={ $up } 降频={ $down } 忙/闲: 小核={ $l_over }/{ $l_under } 大核={ $b_over }/{ $b_under } 超大核={ $p_over }/{ $p_under }
akmode-max-set = [AKMode] P{ $pid } ({ $name }) 档位={ $mode } max={ $max_khz }MHz
akmode-max-skipped = [AKMode] P{ $pid } ({ $name }) 档位={ $mode } 实际频率={ $cur_khz }MHz 未达设定 max={ $max_khz }MHz，跳过升频（schedutil 余量）
akmode-watchdog-release = [AKMode] 看门狗: 已 { $secs } 秒未收到负载事件，eBPF 负载源疑似失效，已释放明日方舟特调控制权并恢复原 governor/min/max

# --- Touch（触摸升频） ---
touch-detect-started = [Touch] 触摸检测线程已启动（读取 /dev/input 输入设备）
touch-detect-no-devices = [Touch] 未找到可读的输入设备，3 秒后重试
touch-detect-poll-error = [Touch] poll 输入设备失败，重新枚举设备
touch-detect-down = [Touch] 检测到触摸按下 (type={ $type } code={ $code })
touch-event-received = [Touch] 收到触摸事件，触发大核升频并立即写频
touch-boost-disable-node = [TouchBoost] 已写 { $path } = 0（屏蔽系统触摸升频）
touch-boost-disable-applied = [TouchBoost] 已屏蔽 Android 自带触摸升频（cpu_boost），改由 ChiRi 触摸升频接管

# --- FAS ---
fas-freq-mismatch = [FAS] P{ $pid }: 频率不匹配！预期 { $min }-{ $max }，实际 { $actual } -> 正在紧急重写
fas-auto-capacity = [FAS] 自动计算算力权重:
fas-auto-capacity-core = [FAS]   P{ $pid }: 算力={ $cap } -> 权重={ $weight }
fas-policy-init = [FAS] P{ $pid } { $min }-{ $max } MHz | 权重={ $weight }
fas-init-summary = [FAS] 初始化 | { $fps }fps 冗余:{ $margin } 集群:{ $clusters } P:{ $perf } 配置数:{ $profiles }
fas-app-switch = [FAS] 应用切换 ({ $ms }ms) | P -> { $perf }
fas-loading-start = [FAS] 进入加载状态 ({ $frames } 帧, { $ms }ms) | P { $old_perf } -> { $new_perf }
fas-loading-exit = [FAS] 退出加载状态 | P -> { $perf }
fas-gear-switch = [FAS] 档位切换 { $old } -> { $new }fps | P -> { $perf }
fas-low-perf-upgrade = [FAS] 低负载稳帧升档 | P={ $perf } 平均帧={ $avg } 标准差={ $stddev } -> { $fps }fps
fas-downgrade-boost = [FAS] 降档加速 | 平均帧:{ $avg } | P { $old } -> { $new } (增量={ $inc })
fas-boost-expired = [FAS] 加速期满，开启降档快车道 (确认帧={ $confirm })
fas-floor-rescue = [FAS] 触底救援 | 卡在地板 { $frames }帧 P={ $old }, 平均帧:{ $avg } -> P:{ $new }
fas-tick-log = [FAS] { $target }fps 平均:{ $avg } | { $ms }ms ema:{ $ema } | 误差:{ $err_ema }/{ $err_inst } | { $act } | P:{ $perf } 前台利用率:{ $util }{ $cd }{ $damp }{ $temp }{ $offset }
fas-set-game = [FAS] 设置游戏 | 包名={ $pkg } | 档位={ $gears } | 目标={ $target }fps
fas-no-profile = [FAS] 未找到 '{ $pkg }' 的专属配置，使用全局档位 { $gears }
fas-ignore-write = [FAS] P{ $pid } 忽略写入 = { $ignore }
fas-pid-reloaded = [FAS] PID 系数热重载: Kp={ $kp } Ki={ $ki } Kd={ $kd }
fas-rules-reloaded = [FAS] 规则已热重载 (冗余={ $margin }, 地板={ $floor }, 天花板={ $ceil }, 配置数={ $profiles })
fas-policy-writer-invalid = [FAS] P{ $pid } 策略写入器无效 (max_valid: { $max_valid }, min_valid: { $min_valid })，已跳过。

# --- Scheduler: Settings ---
apply-settings-for-mode = 正在应用模式: { $mode }
settings-applied-success = 模式 '{ $mode }' 的设置已成功应用
apply-cpu-idle-governor-start = CPU 空闲调速器设置已完成
apply-io-settings-start = I/O 设置已完成
main-config-watch-thread-create = 主配置监控线程已创建

# --- Fast Lock ---
fast-activated = [Fast] 极速模式已激活，所有核心锁定最高频
fast-deactivated = [Fast] 极速模式已解除，系统频率恢复
fast-init = [Fast] policy { $pid } 锁频 { $max_khz } kHz
fast-rewrite = [Fast] policy { $pid } 重写 { $max_khz } kHz
fast-writer-invalid = [Fast] policy { $pid } 写入器无效 (max_valid: { $max_valid }, min_valid: { $min_valid })，已跳过
fast-restore = [Fast] policy { $pid } 恢复 governor={ $governor } min={ $min } max={ $max }
fast-watchdog-release = [Fast] 负载源超时 ({ $secs }s)，释放极速锁频

# --- Logger ---
log-level-updated = 日志级别已更新为: { $level }
