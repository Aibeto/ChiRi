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
use std::sync::atomic::{AtomicU32, Ordering};
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

/// 背光设备路径缓存：`None` 表示尚未发现，每次校验重扫直到找到——
/// 避免开机早期 /sys/class/backlight 尚未就绪时永久缓存 None 导致自愈失效。
static BACKLIGHT_CACHE: Mutex<Option<PathBuf>> = Mutex::new(None);
/// 缓存背光节点连续不可读计数（verify_screen_state 维护，达限丢弃缓存重扫）
static BACKLIGHT_READ_FAILS: AtomicU32 = AtomicU32::new(0);
/// 连续不可读丢弃阈值：verify 每轮调用（息屏期 1s / 亮屏期 1.5s+），8 次 ≈ 8~12s
const BACKLIGHT_READ_FAIL_LIMIT: u32 = 8;

/// 扫描 /sys/class/backlight，返回首个具备状态节点（bl_power 或 actual_brightness）的背光设备路径
fn backlight_dev_path() -> Option<PathBuf> {
    let mut cache = BACKLIGHT_CACHE.lock().unwrap();
    if cache.is_none() {
        *cache = fs::read_dir("/sys/class/backlight")
            .ok()?
            .flatten()
            .find_map(|entry| {
                let dev = entry.path();
                if dev.join("bl_power").exists() || dev.join("actual_brightness").exists() {
                    Some(dev)
                } else {
                    None
                }
            });
    }
    cache.clone()
}

/// 读取背光设备的屏幕开关状态。
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

/// 屏幕状态自愈校验：uevent 可能漏报（开机早期背光未就绪、长时间息屏后唤醒、
/// netlink 缓冲溢出等），导致 `state_arc` 与实际屏幕状态脱节——亮屏时仍为 false，
/// scenemode 计时器在亮屏期间被误触发、且后续亮屏因无 ScreenStateChange(true) 无法退出。
/// 由 app_detect 主循环每轮调用一次，直接读 backlight sysfs 校正 arc；
/// 无背光节点或读取失败时静默跳过，不干扰 uevent 主路径。
/// 缓存节点连续 8 次全不可读（~8s，app_detect 息屏轮询周期 1s）时丢弃缓存
/// 重扫 /sys/class/backlight：背光设备可能在运行中被移除/更换（多屏/折叠态），
/// 死缓存会让自愈永久失效。
pub fn verify_screen_state(state_arc: &Arc<Mutex<bool>>) {
    if let Some(dev) = backlight_dev_path() {
        match read_backlight_state(&dev) {
            Some(state) => {
                BACKLIGHT_READ_FAILS.store(0, Ordering::Relaxed);
                update_state_if_changed(state_arc, state, "verify");
            }
            None => {
                let fails = BACKLIGHT_READ_FAILS.fetch_add(1, Ordering::Relaxed) + 1;
                if fails >= BACKLIGHT_READ_FAIL_LIMIT {
                    if let Ok(mut cache) = BACKLIGHT_CACHE.lock() {
                        *cache = None;
                    }
                    BACKLIGHT_READ_FAILS.store(0, Ordering::Relaxed);
                }
            }
        }
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
                                // 状态变化直接推送 ScreenStateChange：亮屏事件不再等
                                // app_detect 息屏轮询（1s）转发，scenemode 下感知延迟
                                // 从最坏 ~1.1s+ 降到 ~100ms（含背光稳定 sleep）
                                if update_state_if_changed(&state_arc, state, "power") {
                                    let _ = tx.send(DaemonEvent::ScreenStateChange(state));
                                }
                            }
                        }
                    } else if event.subsystem == "backlight" && event.action == ActionType::Change {
                        thread::sleep(Duration::from_millis(100));
                        let dev = event.devpath.display();
                        let bl_power = format!("/sys{}/bl_power", dev);
                        let actual = format!("/sys{}/actual_brightness", dev);

                        let new_state = crate::utils::read_i32_from_file(&bl_power)
                            .map(|v| v == 0)
                            .or_else(|_| crate::utils::read_i32_from_file(&actual).map(|v| v > 0))
                            .ok();

                        if let Some(state) = new_state {
                            debug!(
                                "{}",
                                t_with_args(
                                    "screen-uevent-backlight",
                                    &fluent_args!(
                                        "dev" => dev.to_string(),
                                        "state" => state.to_string()
                                    )
                                )
                            );
                            // 同 power 分支：变化直推事件，消除 app_detect 轮询延迟
                            if update_state_if_changed(&state_arc, state, "backlight") {
                                let _ = tx.send(DaemonEvent::ScreenStateChange(state));
                            }
                        } else {
                            debug!(
                                "{}",
                                t_with_args(
                                    "screen-uevent-backlight-unreadable",
                                    &fluent_args!(
                                        "dev" => dev.to_string()
                                    )
                                )
                            );
                        }
                    }
                }
            }
            Err(_) => thread::sleep(Duration::from_secs(1)),
        }
    }
}
