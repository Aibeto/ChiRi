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

use kobject_uevent::{ActionType, UEvent};
use log::{debug, info};
use netlink_sys::{Socket, SocketAddr, protocols::NETLINK_KOBJECT_UEVENT};
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::SyncSender;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::common::DaemonEvent;
use crate::fluent_args;
use crate::i18n::{t, t_with_args};

/// 更新共享屏幕状态；返回是否发生状态变化。
/// 变化时由调用方决定是否转发 `DaemonEvent::ScreenStateChange`：
/// uevent 线程直推（零轮询延迟），verify_screen_state 自愈路径的变化
/// 由 app_detect 主循环兜底转发（其循环本身每轮比对 arc）。
fn update_state_if_changed(state_arc: &Arc<Mutex<bool>>, new_state: bool, source: &str) -> bool {
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
        info!(
            "{}",
            t_with_args(
                "screen-state-change-detected",
                &fluent_args!("source" => source)
            )
        );
        *state_lock = new_state;
        let state_str = if new_state { "ON" } else { "OFF" };
        info!(
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

/// 屏幕状态检测源类别：不同机型暴露的屏幕状态节点不同（QCOM/通用内核走
/// backlight class；MTK 等仅以 leds class 暴露背光；老内核可读 fbdev blank），
/// 按可靠性优先级依次探测，找到第一个可用源即锁定缓存。
#[derive(Clone, Copy, PartialEq, Eq)]
enum ScreenSourceKind {
    /// backlight class（/sys/class/backlight）：bl_power/actual_brightness 三分支判定
    Backlight,
    /// leds class（/sys/class/leds/*backlight*，如 MTK lcd-backlight）：
    /// Android 息屏时背光亮度写 0，brightness > 0 即面板在发光
    Leds,
    /// fbdev blank（/sys/class/graphics/fb0/blank）：0 = unblank（亮），非 0 = 灭
    FbBlank,
}

impl ScreenSourceKind {
    fn as_str(self) -> &'static str {
        match self {
            ScreenSourceKind::Backlight => "backlight",
            ScreenSourceKind::Leds => "leds",
            ScreenSourceKind::FbBlank => "fb-blank",
        }
    }
}

/// 检测源缓存：`None` 表示尚未发现，每次校验重扫直到找到——
/// 避免开机早期 sysfs 尚未就绪时永久缓存 None 导致自愈失效。
static SCREEN_SOURCE: Mutex<Option<(PathBuf, ScreenSourceKind)>> = Mutex::new(None);
/// 缓存源连续不可读计数（verify_screen_state 维护，达限丢弃缓存重扫）
static SCREEN_READ_FAILS: AtomicU32 = AtomicU32::new(0);
/// 连续不可读丢弃阈值：verify 每轮调用（息屏期 1s / 亮屏期 1.5s+），8 次 ≈ 8~12s
const SCREEN_READ_FAIL_LIMIT: u32 = 8;
/// 诊断打点去重：检测源发现 / 无源告警各只打一条（verify 高频调用防刷屏）
static SOURCE_FOUND_LOGGED: AtomicBool = AtomicBool::new(false);
static NO_SOURCE_LOGGED: AtomicBool = AtomicBool::new(false);

/// 按可靠性优先级扫描可用的屏幕状态检测源：
/// 1. `/sys/class/backlight`（QCOM/通用内核）——具备状态节点（bl_power 或
///    actual_brightness）的设备；
/// 2. `/sys/class/leds` 下名字含 "backlight" 的节点（MTK 常见 lcd-backlight）——
///    具备 brightness 节点；这是「/sys/class/backlight 不存在导致屏幕状态
///    完全读不到」机型的主要修复路径；
/// 3. `/sys/class/graphics/fb0/blank`（fbdev 旧接口兜底）。
/// 全部缺失返回 None（调用方 info 告警一次，verify 静默跳过）。
fn find_screen_source() -> Option<(PathBuf, ScreenSourceKind)> {
    if let Ok(entries) = fs::read_dir("/sys/class/backlight") {
        for entry in entries.flatten() {
            let dev = entry.path();
            if dev.join("bl_power").exists() || dev.join("actual_brightness").exists() {
                return Some((dev, ScreenSourceKind::Backlight));
            }
        }
    }
    if let Ok(entries) = fs::read_dir("/sys/class/leds") {
        for entry in entries.flatten() {
            let name = entry.file_name();
            // 只认背光类 leds（lcd-backlight/backlight/pwm-backlight 等），
            // 跳过通知灯、按键灯、充电灯等无关节点
            if !name.to_string_lossy().to_lowercase().contains("backlight") {
                continue;
            }
            let dev = entry.path();
            if dev.join("brightness").exists() {
                return Some((dev, ScreenSourceKind::Leds));
            }
        }
    }
    let fb0 = PathBuf::from("/sys/class/graphics/fb0");
    if fb0.join("blank").exists() {
        return Some((fb0, ScreenSourceKind::FbBlank));
    }
    None
}

/// 取缓存的检测源；未缓存时扫描一次并缓存。
fn screen_source() -> Option<(PathBuf, ScreenSourceKind)> {
    let mut cache = SCREEN_SOURCE.lock().unwrap();
    if cache.is_none() {
        *cache = find_screen_source();
        if cache.is_some() {
            // 从「无源/源失效」恢复：重置告警去重与失败计数，重新打点新源
            NO_SOURCE_LOGGED.store(false, Ordering::Relaxed);
            SOURCE_FOUND_LOGGED.store(false, Ordering::Relaxed);
            SCREEN_READ_FAILS.store(0, Ordering::Relaxed);
        }
    }
    cache.clone()
}

/// 读取检测源的屏幕开关状态；None = 不可读（调用方静默跳过）。
fn read_screen_state(kind: ScreenSourceKind, dev: &Path) -> Option<bool> {
    match kind {
        ScreenSourceKind::Backlight => read_backlight_state(dev),
        ScreenSourceKind::Leds => crate::utils::read_i32_from_file(
            &dev.join("brightness").to_string_lossy(),
        )
        .ok()
        .map(|v| v > 0),
        ScreenSourceKind::FbBlank => crate::utils::read_i32_from_file(
            &dev.join("blank").to_string_lossy(),
        )
        .ok()
        .map(|v| v == 0),
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
/// - 全部不可读 → None（调用方静默跳过）。
fn read_backlight_state(dev: &Path) -> Option<bool> {
    let bl_power = dev.join("bl_power");
    let actual = dev.join("actual_brightness");
    let bl = crate::utils::read_i32_from_file(&bl_power.to_string_lossy()).ok();
    let act = crate::utils::read_i32_from_file(&actual.to_string_lossy()).ok();
    match (bl, act) {
        (Some(0), _) => Some(true),
        (Some(_), Some(a)) => Some(a > 0),
        (Some(_), None) => Some(false),
        (None, Some(a)) => Some(a > 0),
        (None, None) => None,
    }
}

/// 屏幕状态自愈校验：uevent 可能漏报（开机早期 sysfs 未就绪、长时间息屏后
/// 唤醒、netlink 缓冲溢出，或机型根本不广播屏幕类 uevent——现代内核已无
/// early_suspend/late_resume power uevent，leds/backlight 亮度变化多数驱动
/// 也不广播 KOBJ_CHANGE），导致 `state_arc` 与实际屏幕状态脱节。
/// 由 app_detect 主循环每轮调用一次，直接读检测源 sysfs 校正 arc；
/// 无可用源或读取失败时静默跳过，不干扰 uevent 主路径。
///
/// 诊断打点（各一次，防刷屏）：发现可用源时 info 打点源类别与路径——
/// 「屏幕状态读不到」时从 daemon.log 即可确认设备实际可用的检测源；
/// 三类源全部缺失时 info 告警一次。缓存源连续 8 次全不可读（~8s）时
/// 丢弃缓存重扫：背光设备可能在运行中被移除/更换（多屏/折叠态），
/// 死缓存会让自愈永久失效。
pub fn verify_screen_state(state_arc: &Arc<Mutex<bool>>) {
    if let Some((dev, kind)) = screen_source() {
        match read_screen_state(kind, &dev) {
            Some(state) => {
                SCREEN_READ_FAILS.store(0, Ordering::Relaxed);
                if !SOURCE_FOUND_LOGGED.swap(true, Ordering::Relaxed) {
                    info!(
                        "{}",
                        t_with_args(
                            "screen-detect-source-found",
                            &fluent_args!(
                                "kind" => kind.as_str(),
                                "path" => dev.display().to_string()
                            )
                        )
                    );
                }
                update_state_if_changed(state_arc, state, kind.as_str());
            }
            None => {
                let fails = SCREEN_READ_FAILS.fetch_add(1, Ordering::Relaxed) + 1;
                if fails >= SCREEN_READ_FAIL_LIMIT {
                    if let Ok(mut cache) = SCREEN_SOURCE.lock() {
                        *cache = None;
                    }
                    SCREEN_READ_FAILS.store(0, Ordering::Relaxed);
                }
            }
        }
    } else if !NO_SOURCE_LOGGED.swap(true, Ordering::Relaxed) {
        info!("{}", t("screen-detect-no-source"));
    }
}

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
                    } else if event.subsystem == "backlight" && event.action == ActionType::Change {
                        thread::sleep(Duration::from_millis(100));
                        // 与 verify 自愈同口径（read_backlight_state）：bl_power==0 → 亮；
                        // 非 0（含亮屏路径不清零的陈旧值）以 actual_brightness 为准，
                        // 不可读返回 None。此前行内旧口径把陈旧 bl_power 非 0 当权威
                        // 灭屏信号，亮屏期间每次背光 uevent 误报 OFF、~1s 后才被 verify
                        // 拉回，纠正竞态失败时调度器滞留息屏态。
                        let dev = std::path::PathBuf::from(format!("/sys{}", event.devpath.display()));
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
                        // 仅认名字含 backlight 的节点（跳过通知灯/按键灯/充电灯等
                        // 无关 leds 事件）。多数 led 驱动不广播亮度变化 uevent，
                        // verify 轮询兜底才是主路径，此处能收到即零延迟直推。
                        let dev = std::path::PathBuf::from(format!("/sys{}", event.devpath.display()));
                        let is_backlight_led = dev
                            .file_name()
                            .map(|n| n.to_string_lossy().to_lowercase().contains("backlight"))
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
