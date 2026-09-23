//! screen_detect.rs: [update] [source] [select] [read] [verify] [uevent]

use kobject_uevent::{ActionType, UEvent};
use log::{debug, info, warn};
use netlink_sys::{Socket, SocketAddr, protocols::NETLINK_KOBJECT_UEVENT};
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::SyncSender;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::common::DaemonEvent;
use crate::fluent_args;
use crate::i18n::{t, t_with_args};

// [update]
/// 更新共享屏幕状态；返回是否发生状态变化。
/// 变化时由调用方决定是否转发 `DaemonEvent::ScreenStateChange`：
/// uevent 线程直推（零轮询延迟），verify_screen_state 自愈路径的变化
/// 由 app_detect 主循环兜底转发（其循环本身每轮比对 arc）。
///
/// 息屏仲裁（口径：**多数票**，无节点退役）：亮→息**翻转尝试**时全节点投票——
/// OFF 票超过有效票数的一半才确认；只凑到一票 OFF 且没有其它可读节点时也确认
/// （机型只暴露一个节点的情况）；一个有效读数都没有时不改判。读不到、不存在的
/// 节点不计票、也不否决。
/// 稳态（无翻转）快速返回不做扫描——全节点复核由 verify 的定时轮询承担，
/// 不随事件频率空跑全节点扫描。
/// 所有息屏事件生产者（verify 自愈、power/backlight/leds uevent）共用本函数，
/// 策略天然覆盖全部息屏事件。
/// 注意：投票扫描必须在拿 arc 锁之前——本函数后半段要写 arc，持锁会与调用方互等。
fn update_state_if_changed(state_arc: &Arc<Mutex<bool>>, new_state: bool, source: &str) -> bool {
    // 窥视当前状态（短锁）：稳态（无翻转）直接返回，不做全节点扫描
    let current = *state_arc.lock().unwrap();
    if new_state == current {
        if new_state {
            VETO_WARNED.store(false, Ordering::Relaxed);
        }
        return false;
    }
    if !new_state {
        // 亮→息翻转尝试：全节点投票，OFF 票超过半数才确认
        let votes = tally_screen_nodes();
        if !screen_off_confirmed(&votes) {
            if let Some(node) = &votes.on_node {
                if !VETO_WARNED.swap(true, Ordering::Relaxed) {
                    warn!(
                        "{}",
                        t_with_args(
                            "screen-off-vetoed",
                            &fluent_args!("source" => source, "node" => node)
                        )
                    );
                }
            } else {
                // 一个有效读数都没有：保持亮屏
                debug!(
                    "{}",
                    t_with_args("screen-off-unconfirmed", &fluent_args!("source" => source))
                );
            }
            return false;
        }
    } else {
        VETO_WARNED.store(false, Ordering::Relaxed);
    }
    let mut state_lock = state_arc.lock().unwrap();
    if *state_lock != new_state {
        debug!(
            "{}",
            t_with_args(
                "screen-state-detect-detail",
                &fluent_args!(
                    "source" => source,
                    "old" => state_lock.to_string(),
                    "new" => new_state.to_string()
                )
            )
        );
        debug!(
            "{}",
            t_with_args(
                "screen-state-change-detected",
                &fluent_args!("source" => source)
            )
        );
        *state_lock = new_state;
        let state_str = if new_state { "ON" } else { "OFF" };
        debug!(
            "{}",
            t_with_args(
                "screen-state-changed-value",
                &fluent_args!("state" => state_str)
            )
        );
        true
    } else {
        false
    }
}

// [source]
/// 屏幕状态检测源类别：不同机型暴露的屏幕状态节点不同（QCOM/通用内核走
/// backlight class；MTK 等仅以 leds class 暴露背光；老内核可读 fbdev blank；
/// 三星/海思部分机型只有 LCD class 的 lcd_power），按可靠性优先级依次探测，
/// 找到第一个可用源即锁定缓存。
#[derive(Clone, Copy, PartialEq, Eq)]
enum ScreenSourceKind {
    /// backlight class（/sys/class/backlight）：bl_power/actual_brightness/brightness 判定
    Backlight,
    /// leds class（/sys/class/leds/*backlight*，如 MTK lcd-backlight）：
    /// Android 息屏时背光亮度写 0，brightness > 0 即面板在发光
    Leds,
    /// fbdev blank（/sys/class/graphics/fb*/blank）：0 = unblank（亮），非 0 = 灭
    FbBlank,
    /// LCD class（/sys/class/lcd/*/lcd_power，三星 Exynos `panel/lcd_power` 等）：
    /// 同 FB_BLANK 口径，0 = UNBLANK（亮）、4 = POWERDOWN（灭）
    LcdPower,
    /// DRM connector（/sys/class/drm/*DSI*/*eDP* 等内屏）：`enabled` 字符串节点，
    /// "enabled" = 亮、"disabled" = 灭（内核 DPMS 口径）；外接 HDMI/DP 不计
    DrmEnabled,
    /// DRM connector 的 `dpms`（`enabled` 缺失时的兜底，老内核 DPMS 字符串节点）：
    /// "On" = 亮，"Off"/"Standby"/"Suspend" = 灭
    DrmDpms,
}

impl ScreenSourceKind {
    fn as_str(self) -> &'static str {
        match self {
            ScreenSourceKind::Backlight => "backlight",
            ScreenSourceKind::Leds => "leds",
            ScreenSourceKind::FbBlank => "fb-blank",
            ScreenSourceKind::LcdPower => "lcd-power",
            ScreenSourceKind::DrmEnabled => "drm-enabled",
            ScreenSourceKind::DrmDpms => "drm-dpms",
        }
    }
}

/// leds class 背光节点名关键字：厂商命名差异极大——MTK/海思 `lcd-backlight`、
/// 高通 `panel-backlight`、Awinic `aw22xxx-backlight`、展锐 `sprd-backlight`、
/// 部分机型 `wled-backlight`/`disp-backlight`，只认命中这些关键字的亮度节点。
const BACKLIGHT_LED_KEYWORDS: [&str; 5] = ["backlight", "lcd", "panel", "wled", "disp"];

/// 非面板 LED 关键字豁免：名字即便含上表关键字，命中这里也不是屏幕背光
/// （键盘背光 keyboard-backlight、按键灯 button-backlight、充电/通知灯、
/// 闪光灯 torch/flash、RGB 指示灯等）。这类灯随充电/通知亮灭，计进息屏
/// 仲裁会持续投出亮屏票（后果：永远进不了息屏）。
const NON_PANEL_LED_KEYWORDS: [&str; 14] = [
    "keyboard",
    "keypad",
    "button",
    "keys",
    "charging",
    "charge",
    "battery",
    "notification",
    "notify",
    "torch",
    "flash",
    "indicator",
    "breath",
    "rgb",
];

/// 判断 leds class 节点名是否屏幕背光：命中背光关键字且不是非面板 LED。
/// 枚举与 uevent 两条路径共用（口径一致，避免 uevent 收到通知灯/按键灯事件
/// 时按背光处理）。
fn is_backlight_led_name(name: &str) -> bool {
    let n = name.to_lowercase();
    if !BACKLIGHT_LED_KEYWORDS.iter().any(|k| n.contains(k)) {
        return false;
    }
    !NON_PANEL_LED_KEYWORDS.iter().any(|k| n.contains(k))
}

/// 诊断打点去重：检测源发现 / 无源告警各只打一条（verify 高频调用防刷屏）
static SOURCE_FOUND_LOGGED: AtomicBool = AtomicBool::new(false);
static NO_SOURCE_LOGGED: AtomicBool = AtomicBool::new(false);
/// 亮屏否决告警去重：一次被否决的息屏 episode 只 warn 一条，ON 读数到达后复位
/// （防真实息屏期间每轮 verify 刷屏）
static VETO_WARNED: AtomicBool = AtomicBool::new(false);

// [select]
/// 按可靠性优先级枚举全部候选屏幕状态节点（有序）：
/// 1. `/sys/class/backlight`（QCOM/通用内核）——具备状态节点（bl_power、
///    actual_brightness，或只有 brightness）的设备；
/// 2. `/sys/class/leds` 下的背光节点（MTK 常见 lcd-backlight，另有
///    panel-backlight / wled-backlight / aw22xxx-backlight / sprd-backlight 等）——
///    具备 brightness 节点；这是「/sys/class/backlight 不存在导致屏幕状态
///    完全读不到」机型的主要修复路径（非面板 LED：键盘背光、按键灯、充电/通知灯、
///    闪光灯、RGB 灯不计）；
/// 3. `/sys/class/graphics/fb*/blank`（fbdev 旧接口兜底，fb0/fb1/… 全部计入）；
/// 4. `/sys/class/lcd/*/lcd_power`（LCD class，三星 Exynos `panel/lcd_power`
///    等机型唯一可用的屏幕状态节点，FB_BLANK 口径）；
/// 5. `/sys/class/drm` 内屏 connector（名字含 dsi/edp/lvds）的 `enabled` 节点
///    （内核 DPMS 状态，字符串型）——backlight/leds 全缺机型的又一兜底路径；
///    同一 connector 若没有 `enabled` 则退 `dpms`（老内核）。
fn enumerate_screen_nodes() -> Vec<(PathBuf, ScreenSourceKind)> {
    let mut nodes = Vec::new();
    if let Ok(entries) = fs::read_dir("/sys/class/backlight") {
        for entry in entries.flatten() {
            let dev = entry.path();
            // brightness 兜底：部分驱动未实现 get_brightness（actual_brightness
            // 读不到）、也无 bl_power，此时只能看驱动里存的亮度值
            if dev.join("bl_power").exists()
                || dev.join("actual_brightness").exists()
                || dev.join("brightness").exists()
            {
                nodes.push((dev, ScreenSourceKind::Backlight));
            }
        }
    }
    if let Ok(entries) = fs::read_dir("/sys/class/leds") {
        for entry in entries.flatten() {
            // 只认面板背光 leds，跳过通知灯/按键灯/键盘背光/充电灯/闪光灯
            if !is_backlight_led_name(&entry.file_name().to_string_lossy()) {
                continue;
            }
            let dev = entry.path();
            if dev.join("brightness").exists() {
                nodes.push((dev, ScreenSourceKind::Leds));
            }
        }
    }
    if let Ok(entries) = fs::read_dir("/sys/class/graphics") {
        for entry in entries.flatten() {
            let dev = entry.path();
            if dev.join("blank").exists() {
                nodes.push((dev, ScreenSourceKind::FbBlank));
            }
        }
    }
    if let Ok(entries) = fs::read_dir("/sys/class/lcd") {
        for entry in entries.flatten() {
            let dev = entry.path();
            // 内核 LCD class 属性为 lcd_power（FB_BLANK_*）；`power` 是运行时
            // PM 目录（每个设备都有），故用 is_file 判定真正的属性文件
            if dev.join("lcd_power").exists() || dev.join("power").is_file() {
                nodes.push((dev, ScreenSourceKind::LcdPower));
            }
        }
    }
    if let Ok(entries) = fs::read_dir("/sys/class/drm") {
        for entry in entries.flatten() {
            let name = entry.file_name();
            // 只认内屏 connector（DSI/eDP/LVDS），跳过 HDMI/DP 外接显示——
            // 外接屏亮灭与本机面板息屏无关，误计票会干扰息屏仲裁
            let name = name.to_string_lossy().to_lowercase();
            if !(name.contains("dsi") || name.contains("edp") || name.contains("lvds")) {
                continue;
            }
            let dev = entry.path();
            if dev.join("enabled").exists() {
                nodes.push((dev, ScreenSourceKind::DrmEnabled));
            } else if dev.join("dpms").exists() {
                nodes.push((dev, ScreenSourceKind::DrmDpms));
            }
        }
    }
    nodes
}

// [read]
/// 息屏仲裁票数：只统计**有效读数**。读不到、不存在的节点不计票，也不否决。
struct ScreenVotes {
    off: usize,
    on: usize,
    /// 第一个报 ON 的节点：打点用，说明为什么没确认息屏
    on_node: Option<String>,
}

/// 全节点投票：枚举全部候选节点，逐个用 [`read_screen_state`] 读，
/// 不可读的直接跳过——各源读取口径只在 read_screen_state 一处，避免两套逻辑漂移。
fn tally_screen_nodes() -> ScreenVotes {
    let mut votes = ScreenVotes {
        off: 0,
        on: 0,
        on_node: None,
    };
    for (dev, kind) in enumerate_screen_nodes() {
        match read_screen_state(kind, &dev) {
            Some(true) => {
                votes.on += 1;
                if votes.on_node.is_none() {
                    votes.on_node = Some(dev.display().to_string());
                }
            }
            Some(false) => votes.off += 1,
            None => debug!("screen-detect: node unreadable, skipped: {}", dev.display()),
        }
    }
    debug!("screen-detect: votes off={} on={}", votes.off, votes.on);
    votes
}

/// 是否确认息屏：OFF 票**超过有效票数的一半**（多数票）。只暴露一个可读节点的机型
/// （1 票）仍按那一票算；一个有效读数都没有时不确认——没有证据就不改判。
fn screen_off_confirmed(votes: &ScreenVotes) -> bool {
    let valid = votes.off + votes.on;
    valid > 0 && votes.off * 2 > valid
}

/// 读取检测源的屏幕开关状态；None = 不可读（调用方静默跳过）。
fn read_screen_state(kind: ScreenSourceKind, dev: &Path) -> Option<bool> {
    match kind {
        ScreenSourceKind::Backlight => read_backlight_state(dev),
        ScreenSourceKind::Leds => {
            crate::utils::read_i32_from_file(&dev.join("brightness").to_string_lossy())
                .ok()
                .map(|v| v > 0)
        }
        ScreenSourceKind::FbBlank => {
            crate::utils::read_i32_from_file(&dev.join("blank").to_string_lossy())
                .ok()
                .map(|v| v == 0)
        }
        ScreenSourceKind::LcdPower => {
            // LCD class：lcd_power（少数驱动写作 power）= FB_BLANK_*，
            // 0 = UNBLANK（亮），其余（4 = POWERDOWN 等）= 灭
            let p = dev.join("lcd_power");
            let p = if p.exists() { p } else { dev.join("power") };
            crate::utils::read_i32_from_file(&p.to_string_lossy())
                .ok()
                .map(|v| v == 0)
        }
        ScreenSourceKind::DrmEnabled => {
            let raw = fs::read_to_string(dev.join("enabled")).ok()?;
            match raw.trim() {
                "enabled" => Some(true),
                "disabled" => Some(false),
                _ => None,
            }
        }
        ScreenSourceKind::DrmDpms => {
            let raw = fs::read_to_string(dev.join("dpms")).ok()?;
            match raw.trim() {
                "On" => Some(true),
                "Off" | "Standby" | "Suspend" => Some(false),
                _ => None,
            }
        }
    }
}

/// 读取 backlight class 设备的屏幕开关状态。
///
/// 判定口径（实测案例：部分 DRM 面板驱动的 bl_power 在息屏路径写 FB_BLANK
/// 后，亮屏路径不清零——节点长期停留非 0，把它当权威"灭屏"信号会让
/// verify 自愈反向把状态钉死在 false：亮屏使用微信/QQ 全程被判息屏，
/// scenemode 下线大核 + UI 挤小核，卡到不可用）：
/// - `bl_power == 0` → 亮（FB 明确未 blank，权威亮屏信号）；
/// - `bl_power != 0` → 不可信（可能是上述陈旧值），以 `actual_brightness`
///   为准：Android 息屏时会把背光亮度写 0，>0 即面板在发光；
/// - `bl_power` 不可读 → 回退 `actual_brightness > 0`（原行为）；
/// - `actual_brightness` 也不可读（驱动未实现 get_brightness）→ 回退
///   `brightness > 0`（驱动里存的目标亮度，息屏时 Android 同样写 0）；
/// - 全部不可读 → None（调用方静默跳过）。
fn read_backlight_state(dev: &Path) -> Option<bool> {
    let bl_power = dev.join("bl_power");
    let actual = dev.join("actual_brightness");
    let target = dev.join("brightness");
    let bl = crate::utils::read_i32_from_file(&bl_power.to_string_lossy()).ok();
    let act = crate::utils::read_i32_from_file(&actual.to_string_lossy()).ok();
    let tgt = crate::utils::read_i32_from_file(&target.to_string_lossy()).ok();
    match (bl, act, tgt) {
        (Some(0), _, _) => Some(true),
        (Some(_), Some(a), _) => Some(a > 0),
        (Some(_), None, Some(b)) => Some(b > 0),
        (Some(_), None, None) => Some(false),
        (None, Some(a), _) => Some(a > 0),
        (None, None, Some(b)) => Some(b > 0),
        (None, None, None) => None,
    }
}

// [verify]
/// 屏幕状态自愈校验：uevent 可能漏报（开机早期 sysfs 未就绪、长时间息屏后
/// 唤醒、netlink 缓冲溢出，或机型根本不广播屏幕类 uevent——现代内核已无
/// early_suspend/late_resume power uevent，leds/backlight 亮度变化多数驱动
/// 也不广播 KOBJ_CHANGE），导致 `state_arc` 与实际屏幕状态脱节。
/// 由 app_detect 主循环每轮调用一次：**全节点投票**，票数为唯一判断标准，
/// 结论与 arc 不一致时经 [`update_state_if_changed`] 校正。无有效读数或票数
/// 平手时不改判，不干扰 uevent 主路径。
///
/// 诊断打点（防刷屏）：① 首次发现可读节点时 info 打点源类别与路径——「屏幕
/// 状态读不到」时从 daemon.log 即可确认设备实际可用的检测源；② 全部节点缺失
/// 或全不可读时 info 告警一次（恢复后复位）。
pub fn verify_screen_state(state_arc: &Arc<Mutex<bool>>) {
    let nodes = enumerate_screen_nodes();
    if nodes.is_empty() {
        if !NO_SOURCE_LOGGED.swap(true, Ordering::Relaxed) {
            info!("{}", t("screen-detect-no-source"));
        }
        return;
    }
    // 全节点投票：票数为唯一判断标准
    let votes = tally_screen_nodes();
    let valid = votes.off + votes.on;
    if valid == 0 {
        // 全部不可读：没有证据就不改判（恢复后复位，便于下次告警）
        if !NO_SOURCE_LOGGED.swap(true, Ordering::Relaxed) {
            info!("{}", t("screen-detect-no-source"));
        }
        return;
    }
    NO_SOURCE_LOGGED.store(false, Ordering::Relaxed);
    if !SOURCE_FOUND_LOGGED.swap(true, Ordering::Relaxed) {
        let (dev, kind) = &nodes[0];
        info!(
            "{}",
            t_with_args(
                "screen-detect-source-found",
                &fluent_args!("kind" => kind.as_str(), "path" => dev.display().to_string())
            )
        );
    }
    let decided = if votes.off * 2 > valid {
        false
    } else if votes.on * 2 > valid {
        true
    } else {
        // 票数平手：保持现状
        return;
    };
    update_state_if_changed(state_arc, decided, "verify");
}

// [uevent]
pub fn monitor_screen_state_uevent(
    state_arc: Arc<Mutex<bool>>,
    tx: SyncSender<DaemonEvent>,
) -> Result<(), Box<dyn Error>> {
    let mut socket = Socket::new(NETLINK_KOBJECT_UEVENT)?;
    let sa = SocketAddr::new(process::id(), 1);
    socket.bind(&sa)?;
    let _ = socket.set_rx_buf_sz(2 * 1024 * 1024);
    info!("{}", t("screen-netlink-started"));

    loop {
        match socket.recv_from_full() {
            Ok((buf, _)) => {
                if let Ok(event) = UEvent::from_netlink_packet(&buf) {
                    debug!(
                        "{}",
                        t_with_args(
                            "screen-uevent-received",
                            &fluent_args!(
                                "subsystem" => event.subsystem.as_str(),
                                "devpath" => event.devpath.to_string_lossy().to_string()
                            )
                        )
                    );
                    if event.subsystem == "power" {
                        if let Some(action) = event.env.get("POWER_ACTION") {
                            debug!(
                                "{}",
                                t_with_args(
                                    "screen-uevent-power-action",
                                    &fluent_args!("action" => action.as_str())
                                )
                            );
                            let new_state = if action == "early_suspend" {
                                Some(false)
                            } else if action == "late_resume" {
                                Some(true)
                            } else {
                                None
                            };
                            if let Some(state) = new_state {
                                // 状态变化直接推送 ScreenStateChange：消费者零轮询延迟
                                if update_state_if_changed(&state_arc, state, "power") {
                                    let _ = tx.send(DaemonEvent::ScreenStateChange(state));
                                }
                            }
                        }
                    } else if event.subsystem == "cpu" {
                        // CPU hotplug（cpuN/online 变更）：只置脏标记，下一轮 affinity
                        // 立即刷新在线核位图（≤2s）。这里不做任何读/写，事件风暴也只是一次原子写
                        crate::monitor::CPU_HOTPLUG_DIRTY.store(true, Ordering::Relaxed);
                    } else if event.subsystem == "backlight" && event.action == ActionType::Change {
                        thread::sleep(Duration::from_millis(100));
                        // 与 verify 自愈同口径（read_backlight_state）：bl_power==0 → 亮；
                        // 非 0（含亮屏路径不清零的陈旧值）以 actual_brightness 为准，
                        // 不可读返回 None。此前行内旧口径把陈旧 bl_power 非 0 当权威
                        // 灭屏信号，亮屏期间每次背光 uevent 误报 OFF、~1s 后才被 verify
                        // 拉回，纠正竞态失败时调度器滞留息屏态。
                        let dev =
                            std::path::PathBuf::from(format!("/sys{}", event.devpath.display()));
                        let new_state = read_screen_state(ScreenSourceKind::Backlight, &dev);

                        if let Some(state) = new_state {
                            debug!(
                                "{}",
                                t_with_args(
                                    "screen-uevent-backlight",
                                    &fluent_args!(
                                        "dev" => dev.display().to_string(),
                                        "state" => state.to_string()
                                    )
                                )
                            );
                            // 同 power 分支：变化直推事件，消除轮询延迟
                            if update_state_if_changed(&state_arc, state, "backlight") {
                                let _ = tx.send(DaemonEvent::ScreenStateChange(state));
                            }
                        } else {
                            debug!(
                                "{}",
                                t_with_args(
                                    "screen-uevent-backlight-unreadable",
                                    &fluent_args!(
                                        "dev" => dev.display().to_string()
                                    )
                                )
                            );
                        }
                    } else if event.subsystem == "leds" && event.action == ActionType::Change {
                        // leds class 背光（MTK lcd-backlight 等）亮度变化 uevent：
                        // 仅认面板背光节点（跳过通知灯/按键灯/键盘背光/充电灯等
                        // 无关 leds 事件），口径与枚举一致（is_backlight_led_name）。
                        // 多数 led 驱动不广播亮度变化 uevent，verify 轮询兜底才是
                        // 主路径，此处能收到即零延迟直推。
                        let dev =
                            std::path::PathBuf::from(format!("/sys{}", event.devpath.display()));
                        let is_backlight_led = dev
                            .file_name()
                            .map(|n| is_backlight_led_name(&n.to_string_lossy()))
                            .unwrap_or(false);
                        if is_backlight_led {
                            thread::sleep(Duration::from_millis(100));
                            let new_state = read_screen_state(ScreenSourceKind::Leds, &dev);
                            if let Some(state) = new_state {
                                debug!(
                                    "{}",
                                    t_with_args(
                                        "screen-uevent-leds",
                                        &fluent_args!(
                                            "dev" => dev.display().to_string(),
                                            "state" => state.to_string()
                                        )
                                    )
                                );
                                if update_state_if_changed(&state_arc, state, "leds") {
                                    let _ = tx.send(DaemonEvent::ScreenStateChange(state));
                                }
                            } else {
                                debug!(
                                    "{}",
                                    t_with_args(
                                        "screen-uevent-leds-unreadable",
                                        &fluent_args!(
                                            "dev" => dev.display().to_string()
                                        )
                                    )
                                );
                            }
                        }
                    }
                }
            }
            Err(_) => thread::sleep(Duration::from_secs(1)),
        }
    }
}
