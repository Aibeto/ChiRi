//! fps_monitor.rs: [consts] [probe] [manager] [loop] [gate] [pid-switch] [poll]

use std::collections::{HashMap, VecDeque};
use std::mem::size_of;
use std::num::NonZeroU32;
use std::os::unix::io::{AsRawFd, RawFd};
use std::ptr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{SyncSender, TrySendError};
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

// [consts] 

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

// [probe] 
// ProbeState：单个 PID 的帧统计

struct ProbeState {
    last_ktime_ns: Option<u64>,
    frametimes: VecDeque<Duration>,
    /// 本次 poll 尚未投喂给调度层的新帧间隔（ns）。
    /// poll_frames 把 ring 里的每个事件 ingest 后，由 take_pending 全量取走；
    /// 与 frametimes 分离保证投喂语义是「新帧」而非「最新一条」
    pending: VecDeque<u64>,
}

impl ProbeState {
    fn new() -> Self {
        Self {
            last_ktime_ns: None,
            frametimes: VecDeque::with_capacity(FRAMETIME_WINDOW),
            pending: VecDeque::new(),
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
                self.pending.push_back(delta_ns);
            }
        }
        self.last_ktime_ns = Some(ktime_ns);
    }

    fn latest_frametime(&self) -> Option<Duration> {
        self.frametimes.front().copied()
    }
}

// [manager] 
// FpsManager：单 eBPF 实例，多 PID attach

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

    /// 切换到新 PID：detach 旧 PID + attach 新 PID。
    /// `new_pid == 0` 为「纯 detach」语义：只摘除探针并复位状态，用于 FAS
    /// 去激活时回到零开销待机（detach 后 has_active_probe()==false）。
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
            // 旧 PID 的帧状态一并清理（PID 不会复用到同一次 attach 生命周期内）
            self.states.remove(&self.current_pid);
        }

        // new_pid == 0：纯 detach（FAS 去激活待机），不 attach
        if new_pid == 0 {
            self.current_pid = 0;
            debug!("{}", t("fps-monitor-detached"));
            return Ok(());
        }

        // attach 新 PID
        let pid_i32 = new_pid as i32;
        let scope = NonZeroU32::new(new_pid).map(UProbeScope::OneProcess);
        let Some(scope) = scope else {
            // 防御：非法 PID（理论上不会到这——调用方已过滤 0），不 panic
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

    /// 取走全部待投喂的新帧间隔（ns）。每 PID 独立队列合并返回；
    /// 多 PID 挂载时顺序按队列拼接，不保证严格时间序（FAS 只消费活跃 PID 的样本）。
    fn take_pending(&mut self) -> Vec<u64> {
        let mut out = Vec::new();
        for state in self.states.values_mut() {
            out.extend(state.pending.drain(..));
        }
        out
    }

    /// 当前 PID 的最新帧间隔
    #[allow(dead_code)]
    fn latest_frametime(&self) -> Option<Duration> {
        self.states.get(&self.current_pid)?.latest_frametime()
    }

    fn has_active_probe(&self) -> bool {
        self.current_pid > 0
    }
}

// [loop] 
// 主入口

pub async fn start_fps_loop(
    tx: SyncSender<DaemonEvent>,
    rx_pid: watch::Receiver<u32>,
    fas_active: Arc<AtomicBool>,
) -> Result<(), anyhow::Error> {
    info!("{}", t("fps-monitor-init"));

    // 订阅 pid_watcher 的共享前台 PID 广播（原 500ms 自轮询已删除）：
    // FpsManager 由下方 fps_probe 线程独占，这里把变化值桥接给该线程做
    // switch_pid；watch 接收端克隆一份随闭包进入线程，供 FAS 激活门控
    // 补挂时读取当前前台 PID（桥接任务只转发变化值，激活瞬间的最新值
    // 需从 watch 直接借）。
    let (pid_tx, pid_rx) = std::sync::mpsc::channel::<u32>();
    let rx_pid_bridge = rx_pid.clone();
    tokio::spawn(async move {
        let mut rx = rx_pid_bridge;
        while rx.changed().await.is_ok() {
            let pid = *rx.borrow();
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

            // 反偷跑：FAS 未激活时**不挂任何 uprobe**（eBPF 程序已加载但无
            // attach 点即零执行）。此前 daemon 启动即对前台应用 attach——桌面/
            // 普通应用的每帧 queueBuffer 都要过一次探针，而 FrameUpdate 在
            // 调度侧被 is_active() 直接丢弃，纯白烧。首个 FAS 应用激活后由
            // PID 广播驱动 switch_pid 正常挂载。
            info!("{}", t("fps-monitor-passive"));

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
            // 事件通道拥塞丢弃的帧样本数（仅计数，EMA 平滑可容忍少量丢失）
            let mut dropped_frames: u64 = 0;

            // 注册 RingBuf fd（只注册一次，不会变）。fd 与 attach 无关（探针
            // attach/detach 前后 fd 不变），无条件注册最简单——未注册仅丢失
            // 事件驱动唤醒，poll 超时兜底仍可处理帧（代价是无谓的 500ms 轮询）。
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
                // [gate] 
                // FAS 激活门控（反偷跑核心）
                // fas_active=false：不做任何 PID 消费/帧投喂，仅 500ms 周期
                // 检查标志；若上一会话的 uprobe 仍挂着，先 detach 回到零开销
                // 待机。置位后补挂当前前台 PID——桥接任务只转发 PID **变化**，
                // FAS 应用在激活前已在前台（无后续变化事件）时必须从 watch
                // 直接借当前值，否则首个会话永远挂不上探针。
                let fas_on = fas_active.load(Ordering::Acquire);
                if !fas_on {
                    if manager.has_active_probe() {
                        // 纯 detach：摘除探针 + 复位状态（has_active_probe → false）
                        let _ = manager.switch_pid(0);
                    }
                    std::thread::sleep(Duration::from_millis(500));
                    continue;
                }
                if !manager.has_active_probe() {
                    let cur = *rx_pid.borrow();
                    if cur > 0 {
                        if let Err(e) = manager.switch_pid(cur) {
                            warn!(
                                "{}",
                                t_with_args(
                                    "fps-monitor-pid-switch-failed",
                                    &fluent_args!("error" => e.to_string())
                                )
                            );
                        }
                    }
                }

                // [pid-switch] 
                // PID 变化（tokio 订阅任务桥接的共享前台 PID 广播）
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

                // [poll] 
                // 轮询
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

                // 全量投喂本窗口新产生的帧间隔。此前每 100ms 只发送
                // latest_frametime() 一条：60fps 下 6 帧丢 5 帧，所有以
                // 「帧数」为单位的控制常数（upgrade/downgrade_confirm、
                // jank_cooldown、fast_decay 阈值、freq_hold_frames 等）的
                // 实际时间尺度被拉长 6 倍，PID/防抖/jank 响应全面钝化，
                // 表现为平均帧下降且 1% Low 崩塌；且无新帧时会把同一条
                // 陈旧 delta 反复投喂——静态 UI/暂停场景一条 heavy 帧被
                // 重复计入 ~10 次即可在 1s 内累积出假 loading（perf 被钳
                // 0.60~0.70），表现为纯 UI 界面卡顿。改为只投喂真实新帧。
                let new_deltas = manager.take_pending();
                for delta_ns in new_deltas {
                    frame_counter += 1;
                    if frame_counter % 60 == 0 {
                        let avg_ns = manager.states.get(&manager.current_pid)
                            .and_then(|s| s.frametimes.iter().map(|d| d.as_nanos()).sum::<u128>().checked_div(s.frametimes.len() as u128))
                            .unwrap_or(0);
                        debug!("{}", t_with_args("fps-monitor-frame-summary", &fluent_args!(
                            "pid" => manager.current_pid.to_string(),
                            "window" => manager.states.get(&manager.current_pid).map(|s| s.frametimes.len().to_string()).unwrap_or_default(),
                            "latest_ms" => format!("{:.2}", delta_ns as f64 / 1_000_000.0),
                            "avg_ms" => format!("{:.2}", avg_ns as f64 / 1_000_000.0)
                        )));
                    }

                    match tx_clone.try_send(DaemonEvent::FrameUpdate {
                        frame_delta_ns: delta_ns,
                    }) {
                        Ok(()) => {}
                        // 通道拥塞：丢弃本帧样本。绝不能阻塞发送——fps_probe 阻塞
                        // 会让 eBPF ring buffer 被新事件覆盖，丢失更多帧
                        Err(TrySendError::Full(_)) => {
                            dropped_frames = dropped_frames.saturating_add(1);
                            if dropped_frames % 300 == 1 {
                                warn!("{}", t_with_args("fps-monitor-frames-dropped", &fluent_args!(
                                    "count" => dropped_frames.to_string()
                                )));
                            }
                        }
                        Err(TrySendError::Disconnected(_)) => return,
                    }
                }
            }
        })?;

    info!("{}", t("fps-monitor-started"));
    std::future::pending::<()>().await;
    Ok(())
}
