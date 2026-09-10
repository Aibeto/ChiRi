//! screen_detect.rs: [update] [source] [select] [switch] [read] [verify] [uevent]

use kobject_uevent::{ActionType, UEvent};
use log::{debug, error, info, warn};
use netlink_sys::{Socket, SocketAddr, protocols::NETLINK_KOBJECT_UEVENT};
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::SyncSender;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::common::DaemonEvent;
use crate::fluent_args;
use crate::i18n::{t, t_with_args};

// [update] 
/// 更新共享屏幕状态；返回是否发生状态变化。
/// 变化时由调用方决定是否转发 `DaemonEvent::ScreenStateChange`：
/// uevent 线程直推（零轮询延迟），verify_screen_state 自愈路径的变化
/// 由 app_detect 主循环兜底转发（其循环本身每轮比对 arc）。
///
/// 两阶段息屏判定（2026-09）：亮→息**翻转尝试**时做全节点复核——任一节点
/// 读到亮屏即驳回息屏（warn 打点），并退役报 OFF 的主节点、切换下一个（见
/// retire_primary_and_switch）。稳态（无翻转）快速返回不做扫描——稳态息屏
/// 期的失真检测由 verify 的定时复核驱动，不随事件频率空跑全节点扫描。
/// 所有息屏事件生产者（verify 自愈、power/backlight/leds uevent）共用本
/// 函数，策略天然覆盖全部息屏事件。
/// 注意：复核扫描必须在拿 arc 锁之前——命中驳回且全部节点耗尽时会进入
/// 恒亮屏模式（enter_always_on 需要写 arc），持锁状态下会死锁。
fn update_state_if_changed(state_arc: &Arc<Mutex<bool>>, new_state: bool, source: &str) -> bool {
    // 恒亮屏模式：全部节点已耗尽，拒绝一切息屏事件（永久按亮屏处理）
    if !new_state && ALWAYS_ON.load(Ordering::Relaxed) {
        return false;
    }
    // 窥视当前状态（短锁）：稳态（无翻转）直接返回，不做全节点扫描
    let current = *state_arc.lock().unwrap();
    if new_state == current {
        if new_state {
            VETO_WARNED.store(false, Ordering::Relaxed);
        }
        return false;
    }
    if !new_state {
        // 亮→息翻转尝试：全节点复核，任一节点亮屏即驳回息屏（立即生效）
        if let Some(node) = screen_on_reading_from_any_node() {
            if !VETO_WARNED.swap(true, Ordering::Relaxed) {
                warn!(
                    "{}",
                    t_with_args(
                        "screen-off-vetoed",
                        &fluent_args!("source" => source, "node" => node)
                    )
                );
            }
            // 主节点报 OFF 却被其他节点驳回：读数矛盾。退役切换延迟 15s——
            // 单次不一致可能是瞬时毛刺，持续不一致才切换（见
            // INCONSISTENCY_SWITCH_DELAY）；期间息屏事件持续驳回
            if inconsistency_due_for_switch() {
                retire_primary_and_switch(state_arc);
            }
            return false;
        }
        // 全节点一致 OFF：接受息屏，一致性恢复，清不一致计时
        INCONSISTENT_SINCE.lock().unwrap().take();
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

/// 主检测源缓存：`None` 表示尚未发现，每次校验重扫直到找到——
/// 避免开机早期 sysfs 尚未就绪时永久缓存 None 导致自愈失效。
static SCREEN_SOURCE: Mutex<Option<(PathBuf, ScreenSourceKind)>> = Mutex::new(None);
/// 缓存源连续不可读计数（verify_screen_state 维护，达限退役重扫）
static SCREEN_READ_FAILS: AtomicU32 = AtomicU32::new(0);
/// 连续不可读退役阈值：verify 每轮调用（息屏期 1s / 亮屏期 1.5s+），8 次 ≈ 8~12s
const SCREEN_READ_FAIL_LIMIT: u32 = 8;
/// 诊断打点去重：检测源发现 / 无源告警各只打一条（verify 高频调用防刷屏）
static SOURCE_FOUND_LOGGED: AtomicBool = AtomicBool::new(false);
static NO_SOURCE_LOGGED: AtomicBool = AtomicBool::new(false);
/// 读失败告警去重：一次持续读失败只 warn 一条，读成功后复位（防每 8~12s
/// 重扫循环刷屏）；换源后仍失败也不重复——至少留有一条含路径的告警可查
static READ_FAIL_WARNED: AtomicBool = AtomicBool::new(false);
/// 亮屏否决告警去重：一次被否决的息屏 episode 只 warn 一条，ON 读数到达后复位
/// （防真实息屏期间每轮 verify 刷屏）
static VETO_WARNED: AtomicBool = AtomicBool::new(false);
/// 退役节点记录器：出现过「报 OFF 被其他节点驳回」或「持续不可读/不存在」的
/// 主节点登记于此，重扫选主时跳过；全部候选退役 = 节点耗尽 → 恒亮屏模式
static RETIRED_NODES: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
/// 恒亮屏模式：全部候选节点耗尽后置位——不再读取任何节点、不再接受息屏事件，
/// 屏幕状态永久按亮屏处理（fail-safe：宁可不节电，不可亮屏被误判息屏卡死设备）
static ALWAYS_ON: AtomicBool = AtomicBool::new(false);
/// 恒亮屏 error 打点去重（仅一次）
static ALWAYS_ON_LOGGED: AtomicBool = AtomicBool::new(false);
/// 稳态息屏期全节点复核间隔：arc 已 OFF 期间每该时长全节点扫描一次，
/// 检测主节点失真（息屏不清零/读数卡死会让 arc 永久滞留 OFF——scenemode/
/// prime 下线无法退出，亮屏使用中超大核异常离线）。息屏期 verify ~1s 一轮，
/// 10s 复核 = 每 ~10 轮多读几个小文件，开销可忽略。
const OFF_REVIEW_INTERVAL: Duration = Duration::from_secs(10);
/// 稳态息屏复核计时器（记录器）：上次全节点复核时刻；None = 尚未复核过
/// （进入息屏后首轮即复核一次）
static LAST_OFF_REVIEW: Mutex<Option<Instant>> = Mutex::new(None);
/// 不一致切换延迟：主节点息屏读数被其他节点亮屏驳回后，**持续不一致达到
/// 该时长才退役主节点并切换**——单次不一致可能是瞬时毛刺，立即换节点反而
/// 抖动；期间息屏事件照常驳回（亮屏优先，宁可不节电）
const INCONSISTENCY_SWITCH_DELAY: Duration = Duration::from_secs(15);
/// 不一致起始时刻（记录器）：None = 当前主节点无不一致；Some(t) = 自 t 起
/// 持续不一致。清零时机：主节点读到 ON（verify）/ 息屏转换被全节点一致
/// 接受 / 稳态复核全节点一致 OFF / 退役切换后（新主节点重新计时）
static INCONSISTENT_SINCE: Mutex<Option<Instant>> = Mutex::new(None);

/// fb0/blank 节点路径（fbdev 旧接口，FB_BLANK 权威灭屏信号；0 = unblank 亮）
const FB0_BLANK: &str = "/sys/class/graphics/fb0/blank";

// [select] 
/// 按可靠性优先级枚举全部候选屏幕状态节点（有序）：
/// 1. `/sys/class/backlight`（QCOM/通用内核）——具备状态节点（bl_power 或
///    actual_brightness）的设备；
/// 2. `/sys/class/leds` 下名字含 "backlight" 的节点（MTK 常见 lcd-backlight）——
///    具备 brightness 节点；这是「/sys/class/backlight 不存在导致屏幕状态
///    完全读不到」机型的主要修复路径；
/// 3. `/sys/class/graphics/fb0/blank`（fbdev 旧接口兜底）。
fn enumerate_screen_nodes() -> Vec<(PathBuf, ScreenSourceKind)> {
    let mut nodes = Vec::new();
    if let Ok(entries) = fs::read_dir("/sys/class/backlight") {
        for entry in entries.flatten() {
            let dev = entry.path();
            if dev.join("bl_power").exists() || dev.join("actual_brightness").exists() {
                nodes.push((dev, ScreenSourceKind::Backlight));
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
                nodes.push((dev, ScreenSourceKind::Leds));
            }
        }
    }
    let fb0 = PathBuf::from("/sys/class/graphics/fb0");
    if fb0.join("blank").exists() {
        nodes.push((fb0, ScreenSourceKind::FbBlank));
    }
    nodes
}

/// 选主检测节点：按可靠性优先级枚举全部候选，跳过已退役节点，取第一个可用者。
/// 全部候选已退役（或系统无任何候选）返回 None。
fn select_primary_node() -> Option<(PathBuf, ScreenSourceKind)> {
    let retired = RETIRED_NODES.lock().unwrap();
    enumerate_screen_nodes()
        .into_iter()
        .find(|(dev, _)| !retired.iter().any(|r| r == dev))
}

/// 取缓存的检测源；未缓存时重扫一次并缓存。
fn screen_source() -> Option<(PathBuf, ScreenSourceKind)> {
    let mut cache = SCREEN_SOURCE.lock().unwrap();
    if cache.is_none() {
        *cache = select_primary_node();
        if cache.is_some() {
            // 从「无源/源失效」恢复：重置告警去重与失败计数，重新打点新源
            NO_SOURCE_LOGGED.store(false, Ordering::Relaxed);
            SOURCE_FOUND_LOGGED.store(false, Ordering::Relaxed);
            SCREEN_READ_FAILS.store(0, Ordering::Relaxed);
        }
    }
    cache.clone()
}

/// 登记一次「主节点息屏读数被亮屏驳回」的不一致，返回是否已持续达到
/// 切换延迟（应退役主节点切换下一个）。首次不一致仅启动计时，不切换。
fn inconsistency_due_for_switch() -> bool {
    let mut since = INCONSISTENT_SINCE.lock().unwrap();
    match *since {
        None => {
            *since = Some(Instant::now());
            false
        }
        Some(t) => t.elapsed() >= INCONSISTENCY_SWITCH_DELAY,
    }
}

// [switch] 
/// 进入恒亮屏模式：全部候选节点耗尽（不正确或矛盾）——不再检测息屏，屏幕
/// 状态永久按亮屏处理。error 打点一次（比 warn 高一级，提示所有节点均不正确
/// 或矛盾）。若当前 arc 为 OFF，校正为 ON——app_detect 主循环会把该变化转发
/// 为 ScreenStateChange(true)，调度器恢复亮屏安全态。
/// 注意：调用方不得持有 arc 锁（本函数经 update_state_if_changed 写 arc）。
fn enter_always_on(state_arc: &Arc<Mutex<bool>>) {
    ALWAYS_ON.store(true, Ordering::Relaxed);
    INCONSISTENT_SINCE.lock().unwrap().take();
    if !ALWAYS_ON_LOGGED.swap(true, Ordering::Relaxed) {
        let count = RETIRED_NODES.lock().unwrap().len();
        error!(
            "{}",
            t_with_args(
                "screen-detect-nodes-exhausted",
                &fluent_args!("count" => count.to_string())
            )
        );
    }
    update_state_if_changed(state_arc, true, "always-on");
}

/// 退役当前主节点并切换到下一个未退役候选（息屏读数被驳回且不一致持续
/// 超过 15s = 主节点读数矛盾；节点持续不可读/不存在 = 节点失效——两种
/// 情况都「再切一个节点」）。全部候选退役 → 恒亮屏模式（error 一次，不再
/// 检测息屏）。注意：调用方不得持有 arc 锁（耗尽路径经 enter_always_on 写 arc）。
fn retire_primary_and_switch(state_arc: &Arc<Mutex<bool>>) {
    // 1) 当前主源登记退役（None = 本就无主源，仅尝试补选）
    let retired = SCREEN_SOURCE.lock().unwrap().take();
    let retired_str = retired
        .as_ref()
        .map(|(d, _)| d.display().to_string())
        .unwrap_or_else(|| "-".to_string());
    if let Some((dev, _)) = retired {
        let mut set = RETIRED_NODES.lock().unwrap();
        if !set.contains(&dev) {
            set.push(dev);
        }
    }
    // 2) 切换到下一个未退役候选
    match select_primary_node() {
        Some((next, kind)) => {
            *SCREEN_SOURCE.lock().unwrap() = Some((next.clone(), kind));
            SCREEN_READ_FAILS.store(0, Ordering::Relaxed);
            // 新主节点重新计时（15s 不一致窗口重新开始）
            INCONSISTENT_SINCE.lock().unwrap().take();
            warn!(
                "{}",
                t_with_args(
                    "screen-detect-node-switched",
                    &fluent_args!(
                        "retired" => retired_str,
                        "next" => next.display().to_string()
                    )
                )
            );
        }
        None => {
            // 无候选可选：曾有候选（全部退役）→ 恒亮屏；系统本就无任何候选
            // 节点（开机早期 sysfs 未就绪）→ 维持「无源」路径等待就绪
            if !enumerate_screen_nodes().is_empty() {
                enter_always_on(state_arc);
            }
        }
    }
}

// [read] 
/// 亮屏否决探测：扫描全部已知屏幕状态节点（fb0/blank、全部 backlight 节点、
/// 全部背光类 leds 节点），任一节点读到「亮」即返回 Some(节点描述)。
///
/// 判定口径与各源读取函数一致：fb blank==0（unblank）、bl_power==0（FB 权威
/// 亮屏信号）、actual_brightness>0、leds brightness>0。
///
/// 策略（2026-09 两阶段）：正常只读主节点（省开销）；主节点报 OFF 时才触发
/// 本全节点复核——息屏判定要求所有节点一致 OFF，任一节点亮屏即驳回息屏。
/// 代价权衡：false-ON 只损失节电（doze/scenemode 不进入），false-OFF 会让
/// doze/scenemode 在亮屏期间误触发（下线大核 + UI 挤小核，卡到不可用）。
fn screen_on_reading_from_any_node() -> Option<String> {
    // fb0/blank：0 = unblank（亮）
    if let Ok(v) = crate::utils::read_i32_from_file(FB0_BLANK) {
        if v == 0 {
            return Some(format!("{FB0_BLANK}=0(unblank)"));
        }
    }
    // backlight class：bl_power==0 即亮（权威）；bl_power 非 0 不可信，
    // 以 actual_brightness>0 为准（与 read_backlight_state 三分支同口径）
    if let Ok(entries) = fs::read_dir("/sys/class/backlight") {
        for entry in entries.flatten() {
            let dev = entry.path();
            let bl = crate::utils::read_i32_from_file(&dev.join("bl_power").to_string_lossy()).ok();
            let act =
                crate::utils::read_i32_from_file(&dev.join("actual_brightness").to_string_lossy())
                    .ok();
            let on = match (bl, act) {
                (Some(0), _) => true,
                (_, Some(a)) => a > 0,
                _ => false,
            };
            if on {
                return Some(dev.display().to_string());
            }
        }
    }
    // leds class 背光：brightness > 0 即亮
    if let Ok(entries) = fs::read_dir("/sys/class/leds") {
        for entry in entries.flatten() {
            let name = entry.file_name();
            // 只认背光类 leds，跳过通知灯、按键灯、充电灯等无关节点
            if !name.to_string_lossy().to_lowercase().contains("backlight") {
                continue;
            }
            let dev = entry.path();
            if let Ok(v) =
                crate::utils::read_i32_from_file(&dev.join("brightness").to_string_lossy())
            {
                if v > 0 {
                    return Some(dev.display().to_string());
                }
            }
        }
    }
    None
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

// [verify] 
/// 屏幕状态自愈校验：uevent 可能漏报（开机早期 sysfs 未就绪、长时间息屏后
/// 唤醒、netlink 缓冲溢出，或机型根本不广播屏幕类 uevent——现代内核已无
/// early_suspend/late_resume power uevent，leds/backlight 亮度变化多数驱动
/// 也不广播 KOBJ_CHANGE），导致 `state_arc` 与实际屏幕状态脱节。
/// 由 app_detect 主循环每轮调用一次，直接读检测源 sysfs 校正 arc；
/// 无可用源或读取失败时静默跳过，不干扰 uevent 主路径。
///
/// 诊断打点（防刷屏）：① 发现可用源且读成功时 info 打点源类别与路径——
/// 「屏幕状态读不到」时从 daemon.log 即可确认设备实际可用的检测源；
/// ② 三类源全部缺失时 info 告警一次；③ 源存在但首次读失败时 warn 打点
/// （含源路径——SELinux 拒读/权限/IO 错误等故障模式的唯一可见痕迹，
/// 持续失败不重复，读成功后复位）。
/// 主节点连续 8 次全不可读（~8s）→ 退役并切换下一个候选（节点缺失/不存在
/// 的兜底：直接再切一个节点）；全部候选退役 → 恒亮屏模式（error 一次）。
pub fn verify_screen_state(state_arc: &Arc<Mutex<bool>>) {
    // 恒亮屏模式：不再读取任何节点、不再检测息屏（arc 已被校正为 ON）
    if ALWAYS_ON.load(Ordering::Relaxed) {
        return;
    }
    if let Some((dev, kind)) = screen_source() {
        match read_screen_state(kind, &dev) {
            Some(state) => {
                SCREEN_READ_FAILS.store(0, Ordering::Relaxed);
                READ_FAIL_WARNED.store(false, Ordering::Relaxed);
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
                if state {
                    // 主节点读到 ON：息屏读数矛盾已消失，清不一致计时
                    INCONSISTENT_SINCE.lock().unwrap().take();
                } else {
                    // 稳态息屏定时复核：arc 已 OFF 期间每 OFF_REVIEW_INTERVAL 全节点
                    // 扫描一次，兜住「主节点失真把 arc 永久钉在 OFF」——否则
                    // scenemode/prime 下线无法退出，亮屏使用中超大核异常离线
                    // （翻转路径的全节点复核只覆盖「亮→息」瞬间，盖不住稳态）。
                    // 复核发现任一节点亮屏 → 主节点读数失真：切换需不一致持续
                    // 15s（INCONSISTENCY_SWITCH_DELAY），随后强制改判亮屏（经
                    // update_state_if_changed 写 arc，app_detect 每轮比对转发
                    // ScreenStateChange(true)，scenemode 退出恢复 prime）。
                    let due = {
                        let mut last = LAST_OFF_REVIEW.lock().unwrap();
                        match *last {
                            Some(t) if t.elapsed() < OFF_REVIEW_INTERVAL => false,
                            _ => {
                                *last = Some(Instant::now());
                                true
                            }
                        }
                    };
                    if due {
                        if let Some(node) = screen_on_reading_from_any_node() {
                            if !VETO_WARNED.swap(true, Ordering::Relaxed) {
                                warn!(
                                    "{}",
                                    t_with_args(
                                        "screen-off-vetoed",
                                        &fluent_args!(
                                            "source" => "steady-review",
                                            "node" => node
                                        )
                                    )
                                );
                            }
                            if inconsistency_due_for_switch() {
                                retire_primary_and_switch(state_arc);
                            }
                            update_state_if_changed(state_arc, true, "steady-review");
                        } else {
                            // 全节点一致 OFF：一致性恢复，清不一致计时
                            INCONSISTENT_SINCE.lock().unwrap().take();
                        }
                    }
                }
            }
            None => {
                let fails = SCREEN_READ_FAILS.fetch_add(1, Ordering::Relaxed) + 1;
                if fails == 1 && !READ_FAIL_WARNED.swap(true, Ordering::Relaxed) {
                    warn!(
                        "{}",
                        t_with_args(
                            "screen-detect-read-failed",
                            &fluent_args!(
                                "kind" => kind.as_str(),
                                "path" => dev.display().to_string()
                            )
                        )
                    );
                }
                if fails >= SCREEN_READ_FAIL_LIMIT {
                    // 节点持续不可读/不存在 → 退役并切换下一个候选；
                    // 全部候选退役 → 恒亮屏模式（enter_always_on 内 error 打点）
                    retire_primary_and_switch(state_arc);
                }
            }
        }
    } else if !NO_SOURCE_LOGGED.swap(true, Ordering::Relaxed) {
        info!("{}", t("screen-detect-no-source"));
    }
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
                        // 仅认名字含 backlight 的节点（跳过通知灯/按键灯/充电灯等
                        // 无关 leds 事件）。多数 led 驱动不广播亮度变化 uevent，
                        // verify 轮询兜底才是主路径，此处能收到即零延迟直推。
                        let dev =
                            std::path::PathBuf::from(format!("/sys{}", event.devpath.display()));
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
