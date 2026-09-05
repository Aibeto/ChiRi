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

use std::collections::{HashMap, VecDeque};
use std::mem::size_of;
use std::num::NonZeroU32;
use std::os::unix::io::{AsRawFd, RawFd};
use std::ptr;
use std::sync::mpsc::SyncSender;
use std::time::Duration;

use aya::Ebpf;
use aya::maps::RingBuf;
use aya::programs::UProbe;
use aya::programs::uprobe::{UProbeAttachLocation, UProbeAttachPoint, UProbeScope};
use log::{debug, info, warn};
use mio::{Events, Interest, Poll, Token, unix::SourceFd};
use tokio::sync::watch;

use crate::common::DaemonEvent;
use crate::fluent_args;
use crate::i18n::{t, t_with_args};

// ─── 常量 ────────────────────────────────────────────────

/// uprobe 符号名（短签名）
const SYMBOL_SHORT: &str = "_ZN7android7Surface11queueBufferEP19ANativeWindowBufferi";
/// uprobe 符号名（长签名，fallback）
const SYMBOL_LONG: &str =
    "_ZN7android7Surface11queueBufferEP19ANativeWindowBufferiPNS_24SurfaceQueueBufferOutputE";
const LIBGUI_PATH: &str = "/system/lib64/libgui.so";

/// RingBuf 输出的帧时间戳事件（与 yumi-ebpf 的 FrameTimestampEvent 内存布局一致）
#[repr(C)]
struct FrameTimestampEvent {
    pid: u32,
    ktime_ns: u64,
}

const MIN_FRAME_NS: u64 = 1_000_000;
const MAX_FRAME_NS: u64 = 200_000_000;
const FRAMETIME_WINDOW: usize = 144;

// ─── ProbeState：单个 PID 的帧统计 ─────────────────────

struct ProbeState {
    last_ktime_ns: Option<u64>,
    frametimes: VecDeque<Duration>,
}

impl ProbeState {
    fn new() -> Self {
        Self {
            last_ktime_ns: None,
            frametimes: VecDeque::with_capacity(FRAMETIME_WINDOW),
        }
    }

    fn ingest(&mut self, ktime_ns: u64) {
        if let Some(last_ns) = self.last_ktime_ns {
            let delta_ns = ktime_ns.saturating_sub(last_ns);
            if (MIN_FRAME_NS..=MAX_FRAME_NS).contains(&delta_ns) {
                if self.frametimes.len() >= FRAMETIME_WINDOW {
                    self.frametimes.pop_back();
                }
                self.frametimes.push_front(Duration::from_nanos(delta_ns));
            }
        }
        self.last_ktime_ns = Some(ktime_ns);
    }

    fn latest_frametime(&self) -> Option<Duration> {
        self.frametimes.front().copied()
    }
}

// ─── FpsManager：单 eBPF 实例，多 PID attach ─────────────

struct FpsManager {
    bpf: Ebpf,
    ring_fd: RawFd,
    /// 当前活跃 PID → UProbeLinkId
    links: HashMap<u32, aya::programs::uprobe::UProbeLinkId>,
    /// 当前活跃 PID → 帧统计
    states: HashMap<u32, ProbeState>,
    /// 当前关注的目标 PID（最近一次 attach 的 PID）
    current_pid: u32,
}

impl FpsManager {
    /// 加载 eBPF 程序（只执行一次），获取 RingBuf fd
    fn new() -> Result<Self, anyhow::Error> {
        #[cfg(debug_assertions)]
        let mut bpf = Ebpf::load(include_bytes!(concat!(
            env!("OUT_DIR"),
            "/ebpf_target/bpfel-unknown-none/debug/yumi-ebpf"
        )))?;
        #[cfg(not(debug_assertions))]
        let mut bpf = Ebpf::load(include_bytes!(concat!(
            env!("OUT_DIR"),
            "/ebpf_target/bpfel-unknown-none/release/yumi-ebpf"
        )))?;

        let program: &mut UProbe = bpf.program_mut("handle_frame").unwrap().try_into()?;
        program.load()?;

        let ring_fd = {
            let ring_map = bpf.map_mut("RING_BUF").expect("RING_BUF not found");
            let ring = RingBuf::try_from(ring_map).expect("RingBuf::try_from");
            ring.as_raw_fd()
        };

        Ok(Self {
            bpf,
            ring_fd,
            links: HashMap::new(),
            states: HashMap::new(),
            current_pid: 0,
        })
    }

    /// 切换到新 PID：detach 旧 PID + attach 新 PID
    fn switch_pid(&mut self, new_pid: u32) -> Result<(), anyhow::Error> {
        if new_pid == self.current_pid {
            return Ok(());
        }

        // detach 旧 PID
        if self.current_pid > 0 {
            if let Some(link_id) = self.links.remove(&self.current_pid) {
                let program: &mut UProbe =
                    self.bpf.program_mut("handle_frame").unwrap().try_into()?;
                let _ = program.detach(link_id);
            }
        }

        // attach 新 PID
        let pid_i32 = new_pid as i32;
        // 防御：PID 为 0（进程已退出/尚未检测到前台应用）时跳过 attach，
        // 避免 NonZeroU32::new(0).expect() panic 导致帧监控线程静默死亡。
        let Some(scope) = NonZeroU32::new(new_pid).map(UProbeScope::OneProcess) else {
            warn!(
                "{}",
                t_with_args(
                    "fps-monitor-pid-switch-failed",
                    &fluent_args!("error" => format!("invalid pid {new_pid}"))
                )
            );
            return Ok(());
        };

        let program: &mut UProbe = self.bpf.program_mut("handle_frame").unwrap().try_into()?;
        let link = program
            .attach(
                UProbeAttachPoint::from(UProbeAttachLocation::from(SYMBOL_SHORT)),
                LIBGUI_PATH,
                scope,
            )
            .or_else(|_| {
                debug!("{}", t("fps-monitor-symbol-short-miss"));
                program.attach(
                    UProbeAttachPoint::from(UProbeAttachLocation::from(SYMBOL_LONG)),
                    LIBGUI_PATH,
                    scope,
                )
            })?;

        self.links.insert(new_pid, link);
        self.states.entry(new_pid).or_insert_with(ProbeState::new);
        self.current_pid = new_pid;

        debug!(
            "{}",
            t_with_args(
                "fps-monitor-attach-symbol",
                &fluent_args!(
                    "pid" => pid_i32.to_string(),
                    "lib" => LIBGUI_PATH
                )
            )
        );

        info!(
            "{}",
            t_with_args(
                "fps-monitor-attached",
                &fluent_args!("pid" => pid_i32.to_string())
            )
        );
        Ok(())
    }

    /// 从共享 RingBuf 读取帧事件，按 PID 分派
    fn poll_frames(&mut self) {
        let ring_map = self.bpf.map_mut("RING_BUF").expect("RING_BUF not found");
        let mut ring = RingBuf::try_from(ring_map).expect("RingBuf::try_from failed");

        while let Some(data) = ring.next() {
            if data.len() < size_of::<FrameTimestampEvent>() {
                continue;
            }
            let event = unsafe { ptr::read_unaligned(data.as_ptr().cast::<FrameTimestampEvent>()) };

            if let Some(state) = self.states.get_mut(&event.pid) {
                state.ingest(event.ktime_ns);
            }
        }
    }

    /// 当前 PID 的最新帧间隔
    fn latest_frametime(&self) -> Option<Duration> {
        self.states.get(&self.current_pid)?.latest_frametime()
    }

    fn has_active_probe(&self) -> bool {
        self.current_pid > 0
    }
}

// ─── 主入口 ──────────────────────────────────────────────

pub async fn start_fps_loop(
    tx: SyncSender<DaemonEvent>,
    mut rx_pid: watch::Receiver<u32>,
) -> Result<(), anyhow::Error> {
    info!("{}", t("fps-monitor-init"));

    // 初始 pid：共享 watch 通道的当前值（pid_watcher 创建通道时已写入）
    let initial_pid = *rx_pid.borrow();

    // 订阅 pid_watcher 的共享前台 PID 广播（原 500ms 自轮询已删除）：
    // FpsManager 由下方 fps_probe 线程独占，这里仅把变化值桥接给该线程做 switch_pid。
    let (pid_tx, pid_rx) = std::sync::mpsc::channel::<u32>();
    tokio::spawn(async move {
        while rx_pid.changed().await.is_ok() {
            let pid = *rx_pid.borrow();
            if pid > 0 {
                let _ = pid_tx.send(pid);
            }
        }
    });

    let tx_clone = tx.clone();
    std::thread::Builder::new()
        .name("fps_probe".into())
        .spawn(move || {
            let mut manager = match FpsManager::new() {
                Ok(m) => m,
                Err(e) => {
                    warn!(
                        "{}",
                        t_with_args(
                            "fps-monitor-attach-failed-initial",
                            &fluent_args!("error" => e.to_string())
                        )
                    );
                    return;
                }
            };

            // 初始 attach（共享 watch 通道当前值，pid_watcher 创建通道时已写入）
            if initial_pid > 0 {
                if let Err(e) = manager.switch_pid(initial_pid) {
                    warn!(
                        "{}",
                        t_with_args(
                            "fps-monitor-attach-failed-initial",
                            &fluent_args!("error" => e.to_string())
                        )
                    );
                }
            } else {
                info!("{}", t("fps-monitor-init-no-pid"));
            }

            // mio 轮询（只创建一次；创建失败则本线程无法工作，告警退出交由看门狗自愈）
            let mut poll = match Poll::new() {
                Ok(p) => p,
                Err(e) => {
                    warn!(
                        "{}",
                        t_with_args(
                            "fps-monitor-attach-failed-initial",
                            &fluent_args!("error" => e.to_string())
                        )
                    );
                    return;
                }
            };
            let mut events = Events::with_capacity(64);
            let token = Token(0);
            // 帧事件统计（周期性输出 debug 摘要）
            let mut frame_counter: u32 = 0;

            // 注册 RingBuf fd（只注册一次，不会变）。fd 与 attach 无关（探针 attach 前后
            // fd 不变），必须无条件注册：daemon 启动早于首次前台检测，initial_pid 恒为 0、
            // attach 稍后才发生——若按 has_active_probe() 门控，此路径下永远不注册，
            // 帧处理将退化为 100ms 超时轮询、事件驱动唤醒失效。
            let fd = manager.ring_fd;
            let mut source = SourceFd(&fd);
            if let Err(e) =
                poll.registry()
                    .register(&mut source, token, Interest::READABLE)
            {
                // 注册失败仅丢失事件驱动唤醒，poll 超时兜底仍可处理帧
                warn!(
                    "{}",
                    t_with_args(
                        "fps-monitor-attach-failed-initial",
                        &fluent_args!("error" => e.to_string())
                    )
                );
            }

            loop {
                // ── PID 变化（tokio 订阅任务桥接的共享前台 PID 广播）──
                while let Ok(new_pid) = pid_rx.try_recv() {
                    // 无需重新注册 Poll——RingBuf fd 不变
                    if let Err(e) = manager.switch_pid(new_pid) {
                        warn!(
                            "{}",
                            t_with_args(
                                "fps-monitor-pid-switch-failed",
                                &fluent_args!("error" => e.to_string())
                            )
                        );
                    }
                }

                // ── 轮询 ──
                let timeout = if manager.has_active_probe() {
                    Some(Duration::from_millis(100))
                } else {
                    Some(Duration::from_millis(500))
                };

                // mio poll error 只意味着被信号打断，sleep 后重试即可
                if poll.poll(&mut events, timeout).is_err() {
                    std::thread::sleep(Duration::from_millis(10));
                    continue;
                }

                manager.poll_frames();

                if let Some(delta) = manager.latest_frametime() {
                    frame_counter += 1;
                    if frame_counter % 60 == 0 {
                        let avg_ns = manager.states.get(&manager.current_pid)
                            .and_then(|s| s.frametimes.iter().map(|d| d.as_nanos()).sum::<u128>().checked_div(s.frametimes.len() as u128))
                            .unwrap_or(0);
                        debug!("{}", t_with_args("fps-monitor-frame-summary", &fluent_args!(
                            "pid" => manager.current_pid.to_string(),
                            "window" => manager.states.get(&manager.current_pid).map(|s| s.frametimes.len().to_string()).unwrap_or_default(),
                            "latest_ms" => format!("{:.2}", delta.as_secs_f64() * 1000.0),
                            "avg_ms" => format!("{:.2}", avg_ns as f64 / 1_000_000.0)
                        )));
                    }

                    if tx_clone
                        .send(DaemonEvent::FrameUpdate {
                            frame_delta_ns: delta.as_nanos() as u64,
                        })
                        .is_err()
                    {
                        return;
                    }
                }
            }
        })?;

    info!("{}", t("fps-monitor-started"));
    std::future::pending::<()>().await;
    Ok(())
}
