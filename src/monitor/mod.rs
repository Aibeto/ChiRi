//! mod.rs: [mods] [guard] [start] [init] [watchers] [ebpf] [detect]

use log::error;
use std::error::Error;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::SyncSender;
use std::sync::{Arc, Mutex};
use std::thread;

// [mods]
pub mod app_detect;
pub mod config;
pub mod cpu_monitor;
pub mod fps_monitor;
pub mod screen_detect;
pub mod telemetry;

use crate::common::DaemonEvent;
use crate::fluent_args;
use crate::i18n::{t, t_with_args};

// [guard]
/// 受护子线程入口：监控子线程 panic 时**不再被静默吞掉**——线程级死亡看门狗
/// 感知不到（它只监控进程存活），该监控会永久缺失（如 fps 死后 FAS 无帧、
/// CLG 超时释放退回原生调频）。此处落盘后 `exit(1)`，由看门狗 3s 后按进程级
/// 拉起全新 daemon（main 的启动初始化会重建全部监控）。
/// 仅 `Err`（panic）触发退出；正常返回（通道关闭等）行为不变。
/// 消息刻意不走 i18n：本函数运行在 panic 线程的收尾路径上（与 main.rs 的
/// panic 钩子同口径，纯英文格式便于检索，且避免碰 i18n 锁）。
fn spawn_guarded<F: FnOnce() + Send + 'static>(name: &'static str, f: F) -> std::io::Result<()> {
    thread::Builder::new()
        .name(name.to_string())
        .spawn(move || {
            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).is_err() {
                log::error!("[PANIC] monitor thread \"{name}\" died, exiting for watchdog restart");
                std::process::exit(1);
            }
        })?;
    Ok(())
}

/// CPU hotplug 事件标记：netlink uevent 收到 `cpu` 子系统事件（`cpuN/online` 变更）
/// 时置位，消费方（affinity 的在线核位图）见到即立即刷新，不必等 8s 周期。
/// **周期兜底保留**：机型不广播 cpu uevent 时，行为与改造前完全一致。
pub static CPU_HOTPLUG_DIRTY: AtomicBool = AtomicBool::new(false);

// [start]
/// `ak_active` 为特调（akmode）激活共享标志：cpu_monitor 据此在常规与 40ms 采样间切换。
/// `sample_ms_normal` 为常规采样间隔（由 main.rs 按 SoC 传入：ChiRi 160ms / Yumi 200ms）。
/// `fas_active` 为 FAS 前台激活共享标志：fps_monitor 据此门控 eBPF 探针加载与
/// uprobe 挂载（FAS 未激活前线程零开销待机，反偷跑）。
pub fn start_monitor(
    tx: SyncSender<DaemonEvent>,
    ak_active: Arc<AtomicBool>,
    sample_ms_normal: u64,
    fas_active: Arc<AtomicBool>,
) -> Result<(), Box<dyn Error>> {
    log::debug!("{}", t("monitor-starting"));

    // [init]
    // 解除内核 eBPF Map 内存锁定限制
    unsafe {
        let rlim = libc::rlimit {
            rlim_cur: libc::RLIM_INFINITY,
            rlim_max: libc::RLIM_INFINITY,
        };
        if libc::setrlimit(libc::RLIMIT_MEMLOCK, &rlim) != 0 {
            log::warn!("{}", t("monitor-rlimit-memlock-failed"));
        }
    }

    // --- 初始化配置 ---
    // 嵌入 rules.yaml 为唯一规则来源（编译期打包，防篡改）；磁盘 rules.yaml 仅是
    // 启动时复制出的展示副本（main.rs::sync_rules_snapshot），不参与运行时读取
    let initial_config = crate::common::embedded_rules();

    let config_arc = Arc::new(Mutex::new(initial_config));
    let config_arc_clone_for_watcher = Arc::clone(&config_arc);

    // --- 初始化共享的屏幕状态 ---
    let screen_state_arc = Arc::new(Mutex::new(true));
    let screen_state_clone_for_watcher = Arc::clone(&screen_state_arc);
    let screen_state_clone_for_app_detect = Arc::clone(&screen_state_arc);

    // 初始化共享的强制刷新标志
    let force_refresh_arc = Arc::new(AtomicBool::new(false));
    let force_refresh_clone_for_watcher = Arc::clone(&force_refresh_arc);

    // [watchers]
    // 3. 启动屏幕状态监控线程
    log::debug!("{}", t("monitor-thread-start-screen"));
    // uevent 线程直推 ScreenStateChange 事件：亮屏感知零轮询延迟，
    // app_detect 的轮询转发退化为 verify 自愈兜底（双发由调度器消费端去重）
    let tx_screen = tx.clone();
    spawn_guarded("screen_watcher", move || {
        if let Err(e) =
            screen_detect::monitor_screen_state_uevent(screen_state_clone_for_watcher, tx_screen)
        {
            error!(
                "{}",
                t_with_args(
                    "monitor-screen-watcher-failed",
                    &fluent_args!("error" => e.to_string())
                )
            );
        }
    })?;

    // 4. 启动配置监控线程
    log::debug!("{}", t("monitor-thread-start-config-watch"));
    let tx_config = tx.clone();
    spawn_guarded("config_watcher", move || {
        if let Err(e) = app_detect::watch_config_file(
            config_arc_clone_for_watcher,
            force_refresh_clone_for_watcher,
            tx_config,
        ) {
            error!(
                "{}",
                t_with_args(
                    "monitor-config-watcher-failed",
                    &fluent_args!("error" => e.to_string())
                )
            );
        }
    })?;

    // 5. 前台 PID 统一广播源
    //    FPS/CPU 两个 eBPF 监控共享同一份前台 PID，替代各自 500ms 的重复轮询。
    //    fps_monitor 在下方启动块（FAS 已恢复，仅 ChiRi 且 FAS 可用时启动）
    //    clone 一个 Receiver 消费同一广播。
    let initial_pid = app_detect::get_current_pid();
    let (tx_pid, rx_pid_cpu) = tokio::sync::watch::channel(initial_pid as u32);
    spawn_guarded("pid_watcher", move || {
        let mut last_pid = initial_pid;
        loop {
            std::thread::sleep(std::time::Duration::from_millis(500));
            let current_pid = app_detect::get_current_pid();
            if current_pid != last_pid && current_pid > 0 {
                log::debug!(
                    "{}",
                    t_with_args(
                        "cpu-monitor-fg-pid-updated",
                        &fluent_args!(
                            "old" => last_pid.to_string(),
                            "new" => current_pid.to_string()
                        )
                    )
                );
                last_pid = current_pid;
                let _ = tx_pid.send(current_pid as u32);
            }
        }
    })?;

    // [ebpf]
    // 6. 启动 eBPF FPS 监控线程 (带有独立的 Tokio 运行时)
    //    FAS 帧事件源：FPS 帧监控仅服务于 FAS 调频，仅 ChiRi 且 FAS 配置可用时启动
    //    （FpsManager 空 uprobe attach + RingBuf 轮询对 Yumi 设备是纯开销），
    //    Yumi 设备零开销。PID 来源复用上方 pid_watcher 的共享广播（rx_pid_cpu clone）。
    //    反偷跑门控：FasManager 激活 FAS 前线程以 1s 周期空转等待
    //    fas_active 置位——tokio runtime / eBPF 加载 / uprobe 挂载全部推迟到
    //    首个 FAS 应用进前台时才发生；非 FAS 会话（桌面/普通应用）全程零开销。
    if crate::common::is_chiri_soc() && crate::common::fas_available() {
        log::debug!("{}", t("monitor-thread-start-fps"));
        let tx_fps = tx.clone();
        let rx_pid_fps = rx_pid_cpu.clone();
        spawn_guarded("fps_monitor_ebpf", move || {
            // 激活前待机：1s 轮询共享标志（纳秒级原子读），不建 runtime 不加载 eBPF
            while !fas_active.load(Ordering::Acquire) {
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
            if let Ok(rt) = tokio::runtime::Runtime::new() {
                rt.block_on(async {
                    if let Err(e) =
                        fps_monitor::start_fps_loop(tx_fps, rx_pid_fps, fas_active).await
                    {
                        error!(
                            "{}",
                            t_with_args(
                                "monitor-fps-crashed",
                                &fluent_args!("error" => e.to_string())
                            )
                        );
                    }
                });
            } else {
                error!("{}", t("monitor-fps-tokio-failed"));
            }
        })?;
    }

    // 7. 启动 eBPF CPU 负载监控线程
    log::debug!("{}", t("monitor-thread-start-cpu"));
    let tx_cpu = tx.clone();
    let ak_active_cpu = ak_active.clone();
    spawn_guarded("cpu_monitor_ebpf", move || {
        if let Ok(rt) = tokio::runtime::Runtime::new() {
            rt.block_on(async {
                if let Err(e) =
                    cpu_monitor::start_cpu_loop(tx_cpu, rx_pid_cpu, ak_active_cpu, sample_ms_normal)
                        .await
                {
                    error!(
                        "{}",
                        t_with_args(
                            "monitor-cpu-crashed",
                            &fluent_args!("error" => e.to_string())
                        )
                    );
                }
            });
        } else {
            error!("{}", t("monitor-cpu-tokio-failed"));
        }
    })?;

    // 7.5 ChiRi 专属遥测线程：1s 轮询 PSI / GPU busy% / 电池电流电压，
    //     写入进程级共享原子量（telemetry()）供 chiri 调度层消费与落盘。
    //     仅 ChiRi SoC 启动，Yumi 设备零开销。
    if crate::common::is_chiri_soc() {
        log::debug!("{}", t("monitor-thread-start-telemetry"));
        spawn_guarded("telemetry_monitor", telemetry::telemetry_loop)?;
    }

    // [detect]
    // 8. 启动应用检测主循环 (阻塞)
    log::debug!("{}", t("monitor-thread-start-app-detect"));
    app_detect::app_detection_loop(
        config_arc,
        screen_state_clone_for_app_detect,
        force_refresh_arc,
        tx,
    )?;

    Ok(())
}
