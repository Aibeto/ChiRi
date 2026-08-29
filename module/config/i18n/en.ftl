# --- Main & Monitor ---
yumi-module-starting = yumi-module Unified Starting...
scheduler-module-started = Scheduler module started.
scheduler-module-start-failed = Failed to start scheduler module: { $error }
monitor-module-crashed = Monitor module crashed: { $error }
monitor-module-started = Monitor module started.
monitor-starting = Starting yumi-monitor module...
monitor-initial-config-failed = [Main] Failed to read initial config: { $error }.
    Using default.
monitor-screen-watcher-failed = [Main] Screen state watcher thread crashed: { $error }
monitor-config-watcher-failed = [Main] Config watcher thread crashed: { $error }
monitor-fps-crashed = [Main] FPS Monitor crashed: { $error }
monitor-fps-tokio-failed = [Main] Failed to create Tokio runtime for FPS monitor
monitor-cpu-crashed = [Main] CPU Load Monitor crashed: { $error }
monitor-cpu-tokio-failed = [Main] Failed to create Tokio runtime for CPU monitor
monitor-rlimit-memlock-failed = [Main] Failed to raise RLIMIT_MEMLOCK. eBPF maps might fail to load.
main-chdir = [Main] Changed working directory to: { $dir }
main-module-root = [Main] Module root: { $path }
main-config-loaded = [Main] Config loaded: { $path } (loglevel={ $loglevel }, language={ $language })
main-chiri-scheduler-selected = [Main] Specific SoC detected, enabling Chiri scheduler
main-special-tuned-exported = [Main] Exported { $count } special-tuned whitelist entries to special_tuned.txt
monitor-thread-start-screen = [Main] Starting screen state watcher thread...
monitor-thread-start-config-watch = [Main] Starting config watcher thread...
monitor-thread-start-fps = [Main] Starting eBPF FPS monitor thread...
monitor-thread-start-cpu = [Main] Starting eBPF CPU load monitor thread...
monitor-thread-start-app-detect = [Main] Starting app detection loop...

# --- AppDetect ---
app-detect-config-watch = [AppDetect] Started watching config file: { $path }
app-detect-change-detected = [AppDetect] Change detected, debouncing (100ms)...
app-detect-reloading = [AppDetect] Debounce finished. Reloading config...
app-detect-load-failed = [AppDetect] Failed: { $error }. Using default.
app-detect-reload-success = [AppDetect] Config reloaded successfully.
app-detect-loop-started = [AppDetect] App detection loop started (3000ms poll).
app-detect-screen-changed = [AppDetect] Screen state changed: { $old } -> { $new }
app-detect-mode-change-pkg = [AppDetect] Mode change: { $old } -> { $new } ({ $pkg })
app-detect-ime-auto = [AppDetect] Auto-detected IME: { $pkg }
app-detect-ime-fallback = [AppDetect] Failed to auto-detect IME, using fallback list.
app-detect-debounce-start = [AppDetect] Debounce started: new app { $pkg } (pid={ $pid })
app-detect-debounce-confirmed = [AppDetect] Debounce confirmed: app { $pkg } (pid={ $pid }) is stable
app-detect-pkg-change = [AppDetect] Foreground app state: { $pkg } (pid={ $pid }, temp={ $temp }°C, force={ $force })
app-detect-no-app = [AppDetect] No valid foreground app detected (system process or unknown package)
app-detect-special-override = [AppDetect] Special profile applied: { $pkg } -> { $mode }
app-detect-special-rejected = [AppDetect] Non-whitelisted app { $pkg } mapped to special profile { $mode }, rejected, falling back to global mode
app-detect-special-unavailable = [AppDetect] Special tuning unavailable (akmode.yaml missing/corrupt), { $pkg } mapped { $mode } not applied, falling back to global mode
app-detect-special-fallback = [AppDetect] Special whitelist hit: { $pkg } uses fallback profile { $mode }
app-detect-special-global-rejected = [AppDetect] Global mode { $mode } is a special profile and does not apply to non-whitelisted app { $pkg }, falling back to balance

# --- ScreenDetect ---
screen-state-change-detected = [Screen] State change detected via '{ $source }'.
screen-state-changed-value = [Screen] Screen state changed: { $state }
screen-netlink-started = [Screen] Started netlink-sys socket listener.
screen-state-detect-detail = [Screen] State evaluate: { $old } -> { $new } (src: { $source })
screen-uevent-received = [Screen] uevent received: subsystem={ $subsystem } devpath={ $devpath }
screen-uevent-power-action = [Screen] power action: { $action }
screen-uevent-backlight = [Screen] backlight event: { $dev } -> state={ $state }
screen-uevent-backlight-unreadable = [Screen] backlight state unreadable: { $dev }

# --- Monitors ---
cpu-monitor-started = [CPU Monitor] eBPF System Load monitor started (Long-task blind spot fixed).
cpu-monitor-online-cpus-failed = [CPU Monitor] Failed to get online CPUs: { $error }
cpu-monitor-online-cpus = [CPU Monitor] Detected online CPU core IDs: { $cpus }
cpu-monitor-fg-pid-updated = [CPU Monitor] Foreground PID updated { $old } -> { $new }
cpu-monitor-baseline = [CPU Monitor] baseline init | online_cpus={ $cpus } max_cpu_id={ $max_cpu }
cpu-monitor-fg-baseline-reset = [CPU Monitor] foreground PID changed, util baseline reset: { $old } -> { $new }
cpu-monitor-util-fallback = [CPU Monitor] TGID map missing, falling back to thread-level (pid={ $pid }, raw_tgid={ $raw })
cpu-monitor-tick-log = [CPU Monitor] cores=[{ $cores }] fg_pid={ $pid } fg_max_util={ $util }% threads_tracked={ $threads } delta={ $delta }ms
cpu-monitor-channel-closed = [CPU Monitor] Channel closed, exiting loop.
fps-monitor-init = [FPS Monitor] Initializing eBPF FPS monitor...
fps-monitor-attached = [FPS Monitor] Attached uprobe to PID: { $pid }
fps-monitor-attach-failed = [FPS Monitor] Failed to attach any Uprobe symbols!
fps-monitor-attach-failed-initial = [FPS Monitor] Initial attach failed: { $error }
fps-monitor-init-no-pid = [FPS Monitor] No foreground PID yet, waiting...
fps-monitor-pid-filter-updated = [FPS Monitor] Target PID updated: { $old } -> { $new }
fps-monitor-pid-switching = [FPS Monitor] Switching target PID: { $pid }
fps-monitor-pid-switched = [FPS Monitor] Switched to target PID: { $pid }
fps-monitor-pid-switch-failed = [FPS Monitor] PID switch failed: { $error }
fps-monitor-started = [FPS Monitor] eBPF FPS monitor started (per-PID uprobe mode)
fps-monitor-symbol-short-miss = [FPS Monitor] short symbol attach failed, trying long symbol...
fps-monitor-attach-symbol = [FPS Monitor] attached with symbol: { $lib } (pid={ $pid })
fps-monitor-frame-summary = [FPS Monitor] frame summary | pid={ $pid } window={ $window } latest={ $latest_ms }ms avg={ $avg_ms }ms

# --- Scheduler ---
scheduler-ipc-started = [Scheduler] IPC Channel listener started.
scheduler-mode-change-request = [Scheduler] Mode change request: { $old } -> { $new } (Pkg: { $pkg }, Temp: { $temp })
scheduler-apply-failed = [Scheduler] Failed to apply settings: { $error }
scheduler-channel-closed = [Scheduler] Channel closed! Thread exiting.
scheduler-ipc-panic = [Scheduler] IPC thread panicked, releasing CPU control.
scheduler-doze-enable = [Scheduler] Screen OFF: Enabling Extreme Doze mode (Restricting CPU max performance).
scheduler-doze-special-keep = [Scheduler] Screen OFF: Special tuned mode keeps control, skipping CLG doze.
scheduler-doze-restore = [Scheduler] Screen ON: Restoring previous performance constraints.
scheduler-clg-init = [Scheduler] CPU Load Governor: initialized at startup (mode={ $mode })
scheduler-event-screen = [Scheduler] screen event received: on={ $on } (last={ $last })
scheduler-event-mode-change = [Scheduler] mode change event: pkg={ $pkg } { $old } -> { $new } (temp={ $temp })
scheduler-event-load = [Scheduler] load event: core_utils=[{ $cores }]
scheduler-event-frame = [Scheduler] frame event: delta={ $delta_ms }ms
scheduler-event-config-reload = [Scheduler] config reload event: mode={ $mode }, screen_on={ $screen_on }
scheduler-special-mode-active = [Scheduler] Special profile active: { $pkg } -> { $mode }
scheduler-scene-mode-enter = [Scheduler] Screen off past threshold, switching to scenemode extreme power-saving.

# --- Scheduler: Config Watcher ---
config-reloading = [Config] Config file change detected, reloading...
config-reloaded-success = [Config] Config reloaded successfully.
config-reload-fail = [Config] Config reload failed: { $error }
config-special-load-failed = [Config] Failed to read special-tuned config: { $path } ({ $error }) — special tuning unavailable, whitelisted apps fall back to CLG
config-special-parse-failed = [Config] Failed to parse special-tuned config: { $path } ({ $error }) — special tuning unavailable, whitelisted apps fall back to CLG
config-special-merged = [Config] Merged special-tuned config: { $path }
config-watch-error = [Config] Failed to watch config directory: { $error }
config-apply-mode-failed = [Config] Failed to apply reloaded mode settings: { $error }
config-apply-tweaks-failed = [Config] Failed to apply reloaded system tweaks: { $error }

# --- SysFS (shared FastWriter) ---
sysfs-open-failed = [SysFS] Failed to open { $path }: { $error }
sysfs-umount2-failed = [SysFS] umount2({ $path }) failed: { $error }
sysfs-write-freq-failed = [SysFS] Write freq { $freq } failed: { $error }

# --- CLG ---
clg-init = [CLG] P{ $pid } init | cores={ $cpus } | freqs={ $fmin }-{ $fmax } MHz | P={ $perf } -> { $freq } kHz
clg-activated = [CLG] CPU Load Governor activated, taking over { $count } cluster(s)
clg-no-clusters = [CLG] CPU Load Governor: no valid clusters found, staying inactive
clg-deactivated = [CLG] CPU Load Governor deactivated
clg-config-reloaded = [CLG] config hot-reloaded | up={ $up } down={ $down } floor={ $floor } ceil={ $ceil }
clg-perf-clamped = [CLG] config perf_floor > perf_ceil ({ $floor } > { $ceil }), clamped perf_floor to perf_ceil
clg-restore = [CLG] P{ $pid } restored | governor={ $governor } min={ $min } kHz max={ $max } kHz
clg-tick-log = [CLG] P{ $pid } util={ $util }% perf={ $perf } freq={ $freq }kHz boost={ $boost }kHz
clg-writer-invalid = [CLG] P{ $pid } sysfs writer invalid (max_valid: { $max_valid }, min_valid: { $min_valid }), skipping.
clg-freq-set = [CLG] P{ $pid } freq change: { $old_khz }MHz -> { $new_khz }MHz
clg-freq-write-failed-cached = [CLG] P{ $pid } freq write failed, keeping cached { $cached_khz }MHz (target { $target_khz }MHz)
clg-watchdog-release = [CLG] WATCHDOG: no load events for { $secs }s, eBPF source failed. Releasing CPU control to system defaults.
clg-up-skipped = [CLG] P{ $pid } actual={ $cur_khz }MHz below locked { $lock_khz }MHz, skipping this raise (schedutil headroom, on-demand up)
clg-touch-boost = [CLG] Touch boost window open: big-core perf floor={ $floor } held { $ms }ms

# --- AKMode (Arknights special tuning) ---
akmode-init = [AKMode] Arknights special tuning take over | tier={ $mode }
akmode-activated = [AKMode] Arknights special tuning activated (schedutil + fixed tier max limit)
akmode-no-clusters = [AKMode] Arknights special tuning: no valid clusters found, staying inactive
akmode-cluster-skipped = [AKMode] P{ $pid } skipped (reason: { $reason })
akmode-deactivated = [AKMode] Arknights special tuning deactivated
akmode-config-reloaded = [AKMode] special config hot-reloaded | tier={ $mode }
akmode-tick-log = [AKMode] tier={ $mode } up={ $up } down={ $down } busy/idle: L={ $l_over }/{ $l_under } B={ $b_over }/{ $b_under } P={ $p_over }/{ $p_under }
akmode-max-set = [AKMode] P{ $pid } ({ $name }) tier={ $mode } max={ $max_khz }MHz
akmode-max-skipped = [AKMode] P{ $pid } ({ $name }) tier={ $mode } actual={ $cur_khz }MHz below set max={ $max_khz }MHz, skipping max raise (schedutil headroom)
akmode-watchdog-release = [AKMode] WATCHDOG: no load events for { $secs }s, eBPF source failed. Releasing Arknights special tuning and restoring original governor/min/max.

# --- Touch (touch boost) ---
touch-detect-started = [Touch] Touch detection thread started (reading /dev/input devices).
touch-detect-no-devices = [Touch] No readable input devices found, retrying in 3s.
touch-detect-poll-error = [Touch] poll on input devices failed, re-enumerating.
touch-detect-down = [Touch] Touch down detected (type={ $type } code={ $code })
touch-event-received = [Touch] Touch event received, boosting big cores and flushing immediately.
touch-boost-disable-node = [TouchBoost] Wrote { $path } = 0 (disabled system touch boost)
touch-boost-disable-applied = [TouchBoost] Android built-in touch boost (cpu_boost) disabled, now handled by ChiRi touch boost.

# --- FAS ---
fas-freq-mismatch = [FAS] P{ $pid }: freq mismatch! expected { $min }-{ $max }, actual { $actual } -> emergency reapply
fas-auto-capacity = [FAS] auto capacity weight:
fas-auto-capacity-core = [FAS]   P{ $pid }: cap={ $cap } -> w={ $weight }
fas-policy-init = [FAS] P{ $pid } { $min }-{ $max } MHz | w={ $weight }
fas-init-summary = [FAS] init | { $fps }fps margin:{ $margin } clusters:{ $clusters } P:{ $perf } profiles:{ $profiles }
fas-app-switch = [FAS] app switch ({ $ms }ms) | P -> { $perf }
fas-loading-start = [FAS] entering loading state ({ $frames } frames, { $ms }ms) | P { $old_perf } -> { $new_perf }
fas-loading-exit = [FAS] exit loading state | P -> { $perf }
fas-gear-switch = [FAS] gear switch { $old } -> { $new }fps | P -> { $perf }
fas-low-perf-upgrade = [FAS] low-load steady frame upgrade | P={ $perf } avg={ $avg } stddev={ $stddev } -> { $fps }fps
fas-downgrade-boost = [FAS] downgrade boost | avg:{ $avg } | P { $old } -> { $new } (inc={ $inc })
fas-boost-expired = [FAS] boost expired, fast-tracking downgrade (confirm={ $confirm })
fas-floor-rescue = [FAS] floor-rescue | stuck { $frames } frames at P={ $old }, avg:{ $avg } -> P:{ $new }
fas-tick-log = [FAS] { $target }fps avg:{ $avg } | { $ms }ms ema:{ $ema } | err:{ $err_ema }/{ $err_inst } | { $act } | P:{ $perf } fg_util:{ $util }{ $cd }{ $damp }{ $temp }{ $offset }
fas-set-game = [FAS] set_game | pkg={ $pkg } | gears={ $gears } | target={ $target }fps
fas-no-profile = [FAS] no per-app profile for '{ $pkg }', using global gears { $gears }
fas-ignore-write = [FAS] P{ $pid } ignore_write = { $ignore }
fas-pid-reloaded = [FAS] PID coefficients hot-reloaded: Kp={ $kp } Ki={ $ki } Kd={ $kd }
fas-rules-reloaded = [FAS] rules hot-reloaded (margin={ $margin }, floor={ $floor }, ceil={ $ceil }, profiles={ $profiles })
fas-policy-writer-invalid = [FAS] P{ $pid } policy writer invalid (max_valid: { $max_valid }, min_valid: { $min_valid }), skipping.

# --- Scheduler: Settings ---
apply-settings-for-mode = Applying settings for mode: { $mode }
settings-applied-success = Settings for mode '{ $mode }' applied successfully.
apply-cpu-idle-governor-start = CPU idle governor settings applied.
apply-io-settings-start = I/O settings applied.
main-config-watch-thread-create = Main config watcher thread created.

# --- Logger ---
log-level-updated = Log level updated to: { $level }
