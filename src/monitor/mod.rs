/*
 * Copyright (C) 2026 yuki
 *
 * This program is free software: you can redistribute it and/or modify
 * it under the terms of the GNU General Public License as published by
 * the Free Software Foundation, either version 3 of the License, or
 * (at your option) any later version.
 *
 * This program is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 * GNU General Public License for more details.
 *
 * You should have received a copy of the GNU General Public License
 * along with this program.  If not, see <https://www.gnu.org/licenses/>.
 */

use log::error;
use std::error::Error;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::SyncSender;
use std::sync::{Arc, Mutex};
use std::thread;

pub mod app_detect;
pub mod config;
pub mod cpu_monitor;
pub mod fps_monitor;
pub mod screen_detect;
pub mod telemetry;

use crate::common::DaemonEvent;
use crate::fluent_args;
use crate::i18n::{t, t_with_args};

// 启动函数
/// `ak_active` 为特调（akmode）激活共享标志：cpu_monitor 据此在常规与 40ms 采样间切换。
/// `sample_ms_normal` 为常规采样间隔（由 main.rs 按 SoC 传入：ChiRi 160ms / Yumi 200ms）。
pub fn start_monitor(
    tx: SyncSender<DaemonEvent>,
    ak_active: Arc<AtomicBool>,
    sample_ms_normal: u64,
) -> Result<(), Box<dyn Error>> {
    log::debug!("{}", t("monitor-starting"));

    // ===== 解除内核 eBPF Map 内存锁定限制 =====
    unsafe {
        let rlim = libc::rlimit {
            rlim_cur: libc::RLIM_INFINITY,
            rlim_max: libc::RLIM_INFINITY,
        };
        if libc::setrlimit(libc::RLIMIT_MEMLOCK, &rlim) != 0 {
            log::warn!("{}", t("monitor-rlimit-memlock-failed"));
        }
    }

    // --- 初始化共享配置 ---
    let rules_path = config::get_rules_path();

    // --- 初始化配置 ---
    let initial_config = crate::utils::read_config(&rules_path).unwrap_or_else(|e| {
        log::warn!(
            "{}",
            t_with_args(
                "monitor-initial-config-failed",
                &fluent_args!("error" => e.to_string())
            )
        );
        app_detect::get_default_rules()
    });

    let config_arc = Arc::new(Mutex::new(initial_config));
    let config_arc_clone_for_watcher = Arc::clone(&config_arc);

    // --- 初始化共享的屏幕状态 ---
    let screen_state_arc = Arc::new(Mutex::new(true));
    let screen_state_clone_for_watcher = Arc::clone(&screen_state_arc);
    let screen_state_clone_for_app_detect = Arc::clone(&screen_state_arc);

    // 初始化共享的强制刷新标志
    let force_refresh_arc = Arc::new(AtomicBool::new(false));
    let force_refresh_clone_for_watcher = Arc::clone(&force_refresh_arc);

    // 3. 启动屏幕状态监控线程
    log::debug!("{}", t("monitor-thread-start-screen"));
    // uevent 线程直推 ScreenStateChange 事件：亮屏感知零轮询延迟，
    // app_detect 的轮询转发退化为 verify 自愈兜底（双发由调度器消费端去重）
    let tx_screen = tx.clone();
    thread::Builder::new()
        .name("screen_watcher".to_string())
        .spawn(move || {
            if let Err(e) = screen_detect::monitor_screen_state_uevent(
                screen_state_clone_for_watcher,
                tx_screen,
            ) {
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
    thread::Builder::new()
        .name("config_watcher".to_string())
        .spawn(move || {
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
    //    （FAS 暂禁用：恢复 FAS 时在此为 fps_monitor clone 一个 Receiver）
    let initial_pid = app_detect::get_current_pid();
    let (tx_pid, rx_pid_cpu) = tokio::sync::watch::channel(initial_pid as u32);
    thread::Builder::new()
        .name("pid_watcher".to_string())
        .spawn(move || {
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

    // 6. 启动 eBPF FPS 监控线程 (带有独立的 Tokio 运行时)
    // ==== FAS 暂禁用：FPS 帧监控仅服务于 FAS 调频，随 FAS 一并关闭，
    //      避免空跑 uprobe attach + RingBuf 轮询 + 无效 FrameUpdate 事件。
    //      恢复 FAS 时取消下行注释，并把 rx_pid_cpu.clone() 一并传入。 ====
    // log::debug!("{}", t("monitor-thread-start-fps"));
    // let tx_fps = tx.clone();
    // thread::Builder::new()
    //     .name("fps_monitor_ebpf".to_string())
    //     .spawn(move || {
    //         if let Ok(rt) = tokio::runtime::Runtime::new() {
    //             rt.block_on(async {
    //                 if let Err(e) = fps_monitor::start_fps_loop(tx_fps).await {
    //                     error!("{}", t_with_args("monitor-fps-crashed", &fluent_args!("error" => e.to_string())));
    //                 }
    //             });
    //         } else {
    //             error!("{}", t("monitor-fps-tokio-failed"));
    //         }
    //     })?;

    // 7. 启动 eBPF CPU 负载监控线程
    log::debug!("{}", t("monitor-thread-start-cpu"));
    let tx_cpu = tx.clone();
    let ak_active_cpu = ak_active.clone();
    thread::Builder::new()
        .name("cpu_monitor_ebpf".to_string())
        .spawn(move || {
            if let Ok(rt) = tokio::runtime::Runtime::new() {
                rt.block_on(async {
                    if let Err(e) = cpu_monitor::start_cpu_loop(
                        tx_cpu,
                        rx_pid_cpu,
                        ak_active_cpu,
                        sample_ms_normal,
                    )
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
        thread::Builder::new()
            .name("telemetry_monitor".to_string())
            .spawn(telemetry::telemetry_loop)?;
    }

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
