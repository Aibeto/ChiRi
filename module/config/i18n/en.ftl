# en.ftl: [main-monitor] [app-detect] [screen-detect] [monitors] [scheduler] [scheduler-config-watcher] [sysfs] [clg] [tuned] [touch] [fas] [fas-whitelist] [scheduler-settings] [fast-lock] [logger] [affinity] [corectl] [telemetry] [notify] [config-reload] [governor] [gpu]
# --- Main & Monitor ---
chiri-module-starting = chiri-module Unified Starting...
scheduler-module-started = Scheduler module started.
scheduler-module-start-failed = Failed to start scheduler module: { $error }
monitor-module-crashed = Monitor module crashed: { $error }
monitor-module-started = Monitor module started.
monitor-starting = Starting chiri-monitor module...
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
main-special-tuned-exported = [Main] exported { $count } internal special-tuned whitelist entries to special_tuned.yaml
main-log-archive-submitted = [Main] previous logs archived, packing in background to logd/{ $zip }
main-devimp-archive-submitted = [Main] previous devimp diagnostics archived, packing in background to logd/{ $zip }
main-nofix-skip = [Main] meta.yaml has nofix=true: skipping webui asset restore and meta.yaml self-heal
main-webui-restored = [Main] WebUI assets restored from embedded copy: { $count } file(s)
main-webui-not-embedded = [Main] binary has no embedded WebUI assets (webui/dist missing at build time), restore skipped
main-log-short-session-discarded = [Main] previous session lived less than 30s; its logs were discarded without packing
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
app-detect-special-unavailable = [AppDetect] Special tuning unavailable (tuned_profiles.yaml missing/corrupt), { $pkg } mapped { $mode } not applied, falling back to global mode
app-detect-special-fallback = [AppDetect] Special whitelist hit: { $pkg } uses fallback profile { $mode }
app-detect-special-global-rejected = [AppDetect] Global mode { $mode } is a special profile and does not apply to non-whitelisted app { $pkg }, falling back to default

# --- ScreenDetect ---
screen-state-change-detected = [Screen] State change detected via '{ $source }'.
screen-state-changed-value = [Screen] Screen state changed: { $state }
screen-netlink-started = [Screen] Started netlink-sys socket listener.
screen-state-detect-detail = [Screen] State evaluate: { $old } -> { $new } (src: { $source })
screen-uevent-received = [Screen] uevent received: subsystem={ $subsystem } devpath={ $devpath }
screen-uevent-power-action = [Screen] power action: { $action }
screen-uevent-backlight = [Screen] backlight event: { $dev } -> state={ $state }
screen-uevent-backlight-unreadable = [Screen] backlight state unreadable: { $dev }
screen-detect-source-found = [Screen] Screen state source ready: { $kind } @ { $path }
screen-detect-no-source = [Screen] No usable screen state source (/sys/class/backlight, /sys/class/leds/*backlight* and /sys/class/graphics/fb0/blank all unavailable); state cannot be self-healed, uevent events only
screen-detect-read-failed = [Screen] Screen state source read failed ({ $kind } @ { $path }); node retired and next one selected after consecutive failures, no repeat until success
screen-detect-nodes-exhausted = [Screen] All { $count } screen state nodes exhausted (incorrect or contradictory), entering always-on mode: screen-off detection disabled, screen state permanently treated as ON (prefer losing power saving over mis-detected screen-off freezing the device)
screen-off-vetoed = [Screen] Not enough screen-off votes: node { $node } reports ON (source: { $source }); screen-off not confirmed; node retired after 15s of persistent inconsistency
screen-off-unconfirmed = [Screen] No readable screen-state node (source: { $source }); screen-off not confirmed
screen-detect-node-switched = [Screen] Detection node persistently inconsistent/failed, retired { $retired }, switched to { $next }
screen-uevent-leds = [Screen] leds backlight event: { $dev } -> state={ $state }
screen-uevent-leds-unreadable = [Screen] leds backlight state unreadable: { $dev }
scheduler-screen-on = [Scheduler] Screen ON trigger event
scheduler-screen-off = [Scheduler] Screen OFF trigger event

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
fps-monitor-passive = [FPS Monitor] FAS not active, probe standing by (no uprobe attached, zero overhead)
fps-monitor-detached = [FPS Monitor] FAS deactivated, probe detached (back to zero-overhead standby)
fps-monitor-symbol-short-miss = [FPS Monitor] short symbol attach failed, trying long symbol...
fps-monitor-attach-symbol = [FPS Monitor] attached with symbol: { $lib } (pid={ $pid })
fps-monitor-frame-summary = [FPS Monitor] frame summary | pid={ $pid } window={ $window } latest={ $latest_ms }ms avg={ $avg_ms }ms
fps-monitor-frames-dropped = [FPS Monitor] event channel congested, { $count } frame samples dropped (scheduler consuming too slowly)

# --- Scheduler ---
scheduler-ipc-started = [Scheduler] IPC Channel listener started.
scheduler-mode-change-request = [Scheduler] Mode change request: { $old } -> { $new } (Pkg: { $pkg }, Temp: { $temp })
scheduler-apply-failed = [Scheduler] Failed to apply settings: { $error }
scheduler-channel-closed = [Scheduler] Channel closed! Thread exiting.
scheduler-ipc-panic = [Scheduler] IPC thread panicked, releasing CPU control.
scheduler-ipc-restart = [Scheduler] Cleaned up and restarting scheduler loop (attempt { $count }).
scheduler-ipc-restart-giveup = [Scheduler] { $count } consecutive panics, giving up on scheduler loop restart.
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
scheduler-tuned-cooldown = [Scheduler] Special tuning takeover failed, entering { $secs }s cooldown; CLG takes over during cooldown
scheduler-scene-mode-enter = [Scheduler] Screen off past threshold, switching to scenemode extreme power-saving.
scheduler-scene-mode-exit-fas = [Scheduler] FAS re-activated, exiting scenemode early (all cores restored)
scheduler-scene-mode-exit-switch = [Scheduler] scenemode_enabled disabled, exiting scenemode and restoring screen-off power-saving config
scheduler-fas-switch-off = [Scheduler] fas_enabled disabled, deactivating all FAS instances and restoring scheduler takeover
scheduler-scene-mode-saturation = [Scheduler] scenemode perf ceiling saturated (little util { $util }%), falling back to reduce with 300s cooldown

# --- Scheduler: DOWN (halt) ---
scheduler-down-enter = [Scheduler] DOWN halt enabled: CLG/special-tuned/FAS/fast_lock/thread placement/core_ctl all released, collection and logs only
scheduler-down-exit = [Scheduler] DOWN halt lifted, scheduling resumes
down-enabled = [Down] down.chr says down, scheduling halted
down-disabled = [Down] down.chr cleared, scheduling back to normal
down-watch-error = [Down] down.chr watch failed: { $error }
down-boot-halted = [Down] Booting in halt mode: no takeover is enabled, telemetry and logs continue
down-heartbeat = [Down] Halt still in effect: scheduling stays released, telemetry and logs continue (clear down.chr to resume)

# --- Scheduler: Config Watcher ---
config-reloading = [Config] Config file change detected, reloading...
config-reloaded-success = [Config] Config reloaded successfully.
config-reload-fail = [Config] Config reload failed: { $error }
config-special-load-failed = [Config] Failed to read special-tuned config: { $path } ({ $error }) — special tuning unavailable, whitelisted apps fall back to CLG
config-special-parse-failed = [Config] Failed to parse special-tuned config: { $path } ({ $error }) — special tuning unavailable, whitelisted apps fall back to CLG
config-special-merged = [Config] Merged special-tuned config: { $path }

# --- Governor (performance take-over, FAS/contingency) ---
governor-switched = [Governor] P{ $pid } governor { $from } -> performance
governor-restored = [Governor] P{ $pid } governor performance -> { $to }
governor-switch-failed = [Governor] failed to switch governor on policy { $pid }
governor-restore-failed = [Governor] failed to restore governor { $governor } on policy { $pid }
governor-residue-cleanup = [Governor] leftover performance governor detected (abnormal exit?), restored to schedutil

# --- GPU frequency lock (contingency) ---
gpu-locked = [GPU] locked to max frequency { $khz } kHz ({ $nodes } nodes)
gpu-released = [GPU] original frequency limits restored
gpu-detect-miss = [GPU] no usable GPU frequency node found; contingency GPU lock skipped

# --- SysFS generic ---
sysfs-write-failed = [SysFS] failed to write { $path }: { $error }
config-scenemode-merged = [Config] Merged scenemode config: { $path }
config-scenemode-parse-failed = [Config] Failed to parse the scenemode config ({ $path }): { $error }; keeping the current scenemode
battery-status-unknown = [Battery] Unrecognized status node value ({ $raw }); treated as unknown. Power averaging only samples while discharging, so check that the node uses a standard value (Charging / Discharging / Full / Not charging)
power-avg-skip = [PowerAVG] this sample was not counted ({ $reason }); PowerAVG.chr keeps its previous value. Sampling needs the battery discharging, and average mode also needs the screen on
telemetry-raw-snapshot = [Telemetry] first-reading snapshot (for unit checking): private node raw voltage={ $v }, raw current={ $i }; divisors voltage/{ $vd }, current/{ $cd } -> V={ $v }/{ $vd }, A={ $i }/{ $cd }
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
clg-touch-boost = [CLG] Touch boost window open: big-core perf floor={ $floor } held { $ms }ms
clg-thermal-cap = [CLG] Thermal guard: battery={ $batt }°C / CPU={ $cpu }°C, perf ceiling capped to { $cap }% (>= { $free } exempt)
clg-thermal-no-sensor = [CLG] Thermal guard: no CPU temperature sensor, CPU reference disabled
clg-thermal-no-battery = [CLG] Thermal guard: battery temp node not found, CPU-only suppression
battery-temp-scale = [Thermal] battery temp scale pre-detected: { $unit } (divisor { $divisor }); shared by CLG thermal guard and FAS temperature guard
battery-temp-scale-unknown = [Thermal] battery temp scale pre-detection inconclusive (node missing or reading not ready); degrading to CPU-only this run
clg-min-write-failed = [CLG] P{ $pid } failed to write scaling_min_freq={ $khz }MHz, idle floor may stay high

# --- Special tuning (per-mode profiles: akmode / playback / daily ...) ---
tuned-init = [Tuned] { $mode } special tuning take over (tier-less load-following)
tuned-activated = [Tuned] { $mode } special tuning activated (schedutil + dynamic max, instant up / debounced down)
tuned-no-clusters = [Tuned] { $mode } special tuning: no valid clusters found, staying inactive
tuned-cluster-skipped = [Tuned] { $mode } P{ $pid } skipped (reason: { $reason })
tuned-deactivated = [Tuned] { $mode } special tuning deactivated
tuned-config-reloaded = [Tuned] { $mode } profile hot-reloaded
tuned-tick-log = [Tuned] { $mode } { $state }
tuned-watchdog-release = [Tuned] WATCHDOG: no load events for { $secs }s, eBPF source failed. Releasing special tuning and restoring original governor/min/max.
tuned-profile-missing = [Tuned] whitelist mode { $mode } has no profile; takeover falls back to the akmode section (game params) - check tuned_profiles.yaml

# --- Touch (touch boost) ---
touch-detect-started = [Touch] Touch detection thread started (reading /dev/input devices).
touch-detect-no-devices = [Touch] No readable input devices found, retrying in 3s.
touch-detect-poll-error = [Touch] poll on input devices failed, re-enumerating.
touch-detect-down = [Touch] Touch down detected (type={ $type } code={ $code })
touch-event-received = [Touch] Touch event received, boosting big cores and flushing immediately.
touch-boost-disable-node = [TouchBoost] Wrote { $path } = 0 (disabled system touch boost)
touch-boost-disable-applied = [TouchBoost] Android built-in touch boost (cpu_boost) disabled, now handled by ChiRi touch boost.
sched-tuning-applied = [Sched] kernel scheduler knobs applied ({ $count } items)
sched-tuning-key-rejected = [Sched] kernel scheduler knob rejected by whitelist: { $key }
system-tweaks-restore = [Tweaks] DOWN halt: one-shot system tweaks restored ({ $count } nodes)
system-tweaks-skipped-down = [Tweaks] one-shot system tweaks skipped during DOWN halt

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

# --- FAS (whitelist / scheduler integration) ---
main-fas-whitelist-exported = [Main] exported { $count } FAS whitelist entries to fas_whitelist.yaml
app-detect-fas-fallback = [AppDetect] foreground app hit FAS whitelist, entering FAS mode: { $pkg }
app-detect-pkg-normalized = [AppDetect] foreground name has a sub-process suffix, normalized to the base package: { $pkg } -> { $base }
rule-key-normalized = [Rules] rule key has a sub-process suffix, treated as its base package: { $key } -> { $base }
rule-key-conflict = [Rules] normalized rule key conflicts with an existing base-package rule; the latter wins: { $key } -> { $base }
app-detect-fas-rejected = [AppDetect] non-whitelisted app { $pkg } mapped to FAS mode { $mode }, rejected, falling back to global mode
app-detect-fas-global-rejected = [AppDetect] global mode { $mode } is a FAS mode and does not apply to non-whitelisted app { $pkg }, falling back to default
scheduler-fas-activate = [Scheduler] FAS instance activated: { $pkg } (pid={ $pid })
scheduler-fas-switch = [Scheduler] FAS instance hot-switched: { $old } -> { $new }
scheduler-fas-deactivate = [Scheduler] FAS instance deactivated (frequencies restored): { $pkg }
scheduler-fas-delayed-exit = [Scheduler] FAS delayed exit expired; governor restored, taking over as { $mode }
scheduler-fas-init-failed = [Scheduler] FAS instance init failed, falling back to CLG: { $pkg }
scheduler-fas-cooldown = [Scheduler] FAS init failed, entering { $secs }s cooldown; CLG takes over during cooldown

# --- Scheduler: Settings ---
apply-settings-for-mode = Applying settings for mode: { $mode }
settings-applied-success = Settings for mode '{ $mode }' applied successfully.
apply-cpu-idle-governor-start = CPU idle governor settings applied.
apply-io-settings-start = I/O settings applied.
main-config-watch-thread-create = Main config watcher thread created.

# --- Fast Lock ---
fast-activated = [Fast] activated, all cores locked to max frequency
fast-deactivated = [Fast] deactivated, system frequencies restored
fast-init = [Fast] policy { $pid } locked at { $max_khz } kHz
fast-rewrite = [Fast] policy { $pid } rewrite { $max_khz } kHz
fast-writer-invalid = [Fast] policy { $pid } writer invalid (max_valid: { $max_valid }, min_valid: { $min_valid }), skipping
fast-restore = [Fast] policy { $pid } restore governor={ $governor } min={ $min } max={ $max }
fast-watchdog-release = [Fast] load source timeout ({ $secs }s), releasing fast lock

# --- Logger ---
log-level-updated = Log level updated to: { $level }
logger-log-restart-for-archive = [Logger] { $dir } reached { $mb }MB, restarting scheduler to archive logs

# --- Rhine (Lab) ---
rhine-state-created = [Rhine] rhine.chr missing, created with default content (not enabled)
rhine-state-invalid = [Rhine] rhine.chr content invalid ({ $value }), reset to default (not enabled)
rhine-mode-enabled = [Rhine] Lab mode enabled: { $mode }
rhine-mode-disabled = [Rhine] Lab mode disabled, changes reverted from backup
rhine-restored = [Rhine] reverted { $origin } changes from rhine-back.chr
rhine-restore-invalid = [Rhine] rhine-back.chr invalid, reverted with built-in defaults and cleared lab overrides
rhine-restore-meta-failed = [Rhine] failed to revert meta.yaml; rhine-back.chr kept for the next attempt
rhine-lock-unavailable = [Rhine] neither /tmp nor /dev is writable; lab lock not created (the lab still works)
rhine-lock-refused = [Rhine] lab is locked: turning it off needs a device reboot, request ignored
rhine-lock-lost = [Rhine] lock file exists but the locked mode is unknown, lock cleared
rhine-force-off = [Rhine] rhine.chr says off: lab force-disabled and original values restored (reboot skipped - reboot soon to verify)
rhine-lock-note-no-backup = rhine-back.chr was missing (the original state of the last enable is unknown), rebuilt from built-in defaults
rhine-apply-failed = [Rhine] Failed to enable lab mode: { $mode } ({ $error })
rhine-watch-error = [Rhine] rhine.chr watch failed: { $error }

# --- Affinity (CPU affinity & thread migration) ---
affinity-boost-applied = [Affinity] boost layout applied: top-app/foreground → { $big }, background groups → { $little }
affinity-normal-restore = [Affinity] normal affinity layout restored (background kept on little cores)
affinity-pin-threads = [Affinity] foreground pid={ $pid } threads migrated: { $pinned }/{ $total }
affinity-pin-failed = [Affinity] no migratable threads for foreground pid={ $pid } (process may have exited)
affinity-threads-restored = [Affinity] restored full-core affinity for { $count } threads of pid={ $pid }
affinity-pin-core = [Affinity] thread { $tid } pinned to core { $core } ({ $reason })
affinity-blacklisted = [Affinity] blacklisted process skipped: pid={ $pid } { $name }
affinity-promoted = [Affinity] background thread { $tid } promoted to big core (util { $util }%)
affinity-demoted = [Affinity] background thread { $tid } demoted back to little group (util { $util }%)
affinity-write-failed = [Affinity] cpuset write failed: { $path }
affinity-uclamp-unavailable = [Affinity] top_app_uclamp_max_pct unavailable, auto-corrected (kernel { $version }, reason: { $reason }; uclamp requires kernel >= 5.3 with a writable node)
affinity-released = [Affinity] takeover released, system affinity config restored

# --- CoreCtl (core_ctl online control) ---
corectl-boost-on = [CoreCtl] boost: min_cpus raised to keep all { $count } clusters fully online
corectl-boost-off = [CoreCtl] core_ctl min_cpus snapshot restored
corectl-scenemode-on = [CoreCtl] scenemode core offline: { $count } cores taken offline (little+big kept at low freq, prime powered down, one little core reserved for scheduler)
corectl-scenemode-off = [CoreCtl] { $count } offlined cores restored online
corectl-restore-pending = [CoreCtl] { $count } cores failed to come back online, retrying every 2s
corectl-self-pinned = [CoreCtl] scheduler service pinned to dedicated little core cpu{ $core }
corectl-unavailable = [CoreCtl] no usable core_ctl node found, takeover skipped
corectl-write-failed = [CoreCtl] core_ctl write failed: { $path }

# --- Notify (ongoing status notification) ---
# The daemon builds the text (src/notify.rs) and posts/updates it via `cmd notification post`
notify-title-fallback = ChiRi scheduler
# values only, no field labels; joined by notify::SEPARATOR (" · ")
notify-line-mode = { $mode }
notify-line-family = { $family }
notify-line-submode = { $sub }
notify-line-temp = { $batt }/{ $cpu } °C
notify-line-power = { $watt } W
notify-mode-scenemode = Screen-off scene
notify-mode-unknown = Unknown
notify-post-failed = [Notify] all three candidate cmd notification command lines failed; the status notification cannot be posted (reported once)

# --- Telemetry ---
monitor-thread-start-telemetry = [Main] starting telemetry monitor thread (PSI/GPU/battery)...
telemetry-oplus-bcc = [Telemetry] OPlus private node bcc_parms enabled: battery current/voltage reads use real-time BCC data (bypassing the ~10s cache of standard power_supply nodes)
telemetry-oplus-bcc-missing = [Telemetry] meta enables the OPlus private node (oplus_chg) but bcc_parms does not exist; falling back to the standard power_supply nodes for this run
telemetry-bcc-unusable = [Telemetry] bcc_parms fields 6/8 (voltage/current) are missing or non-integer; real-time BCC power is unavailable, so telemetry fell back to the standard power_supply node (~10s cache; its current unit may not match the uA assumption, so verify the power column scale)
telemetry-battery-unavailable = [Telemetry] all battery current/voltage candidate nodes are unreadable (both OPlus BCC and the standard power_supply nodes failed); power/voltage columns will be written as - (reported once per outage)
telemetry-gpu-unavailable = [Telemetry] no GPU busy candidate node exists (non-Adreno/GED device); the GPU column stays - (reported once)
telemetry-probe-attached = [CPU Monitor] eBPF telemetry probe attached: { $name }
telemetry-probe-failed = [CPU Monitor] eBPF telemetry probe { $name } attach failed (tracepoint may be missing): { $error }
telemetry-map-missing = [CPU Monitor] map { $name } missing from eBPF binary (binary/daemon version skew), its counters stay 0
telemetry-summary = [Telemetry] PSI cpu={ $cpu }% io={ $io }% mem={ $mem }% | GPU={ $gpu }% | wakeups={ $wakeups } migrations={ $migrations } freq={ $freq } | battery { $power }W

# --- Config hot-reload sync ---
scheduler-config-dirty-reload = [Scheduler] config.yaml hot reload synced to scheduler (mode={ $mode })
