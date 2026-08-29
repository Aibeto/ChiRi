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

use crate::monitor::config::RulesConfig;
use std::env;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

/// 守护进程全局事件总线
/// FAS 暂禁用：`FrameUpdate` 变体暂无生产者、`ModeChange.pid` 与 `SystemLoadUpdate.foreground_max_util`
/// 暂无消费者，均为恢复 FAS 时保留，故允许 dead_code。
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub enum DaemonEvent {
    /// 低频事件：前台应用切换或环境温度变化引起的模式改变
    ModeChange {
        package_name: String,
        pid: i32,
        mode: String,
        temperature: f64,
    },
    /// 高频事件：eBPF 捕获到的底层渲染帧数据
    FrameUpdate {
        frame_delta_ns: u64, // 纳秒级帧间隔
    },
    /// eBPF 全局系统负载更新 (每 X 毫秒触发一次)
    SystemLoadUpdate {
        /// 每个 CPU 核心的真实利用率 (0.0 ~ 1.0)，数组索引即 cpu_id
        core_utils: Vec<f32>,
        /// 如果当前有前台应用，这是该应用最吃 CPU 的那 1 个线程的利用率
        foreground_max_util: f32,
    },

    ConfigReload(RulesConfig),

    ScreenStateChange(bool),
}

/// 获取模块根目录的绝对路径
pub fn get_module_root() -> PathBuf {
    // 获取当前执行文件的绝对路径
    let exe_path = env::current_exe().unwrap_or_else(|_| PathBuf::from("/"));

    // 回溯两级目录:
    // core/bin/yumi -> core/bin -> core -> yumi
    exe_path
        .parent()
        .unwrap_or(&exe_path) // .../core/bin
        .parent()
        .unwrap_or(&exe_path) // .../core
        .parent()
        .unwrap_or(&exe_path) // .../yumi (Root)
        .to_path_buf()
}

/// 读取文件首行并去除空白（用于 SoC 型号探测）
fn read_first_line(path: &str) -> String {
    std::fs::read_to_string(path)
        .map(|s| s.lines().next().unwrap_or("").trim().to_string())
        .unwrap_or_default()
}

/// 触发 Chiri 专用调度的特定处理器型号片段列表。
/// 探测到任一命中即启用 Chiri 调度器；新增机型只需在此追加片段，不要绑定单一型号。
/// 例：SM8550（骁龙 8 Gen 2）含 "8550"，SM8450（骁龙 8 Gen 1）含 "8450"，
/// MSM8998（骁龙 835）含 "8998"。片段须能互相区分（8550 不含 8450，反之亦然）。
const CHIRI_SOC_HINTS: &[&str] = &["8550", "8450", "8998"];

/// 读取单个 Android 系统属性（getprop key），失败/为空返回空串
fn getprop(key: &str) -> String {
    std::process::Command::new("getprop")
        .arg(key)
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_default()
}

/// 型号片段是否命中任一特定处理器。
///
/// 从多个权威来源取硬件标识统一比较，避免某台机型只暴露其中一两个来源而漏检：
///   1. /sys/devices/soc0/machine   —— 高通直接暴露 SoC 型号（如 "SM8550"）
///   2. /sys/devices/soc0/plat_name —— 部分内核补充的平台名
///   3. getprop ro.soc.model        —— Android 12+ 厂商填写的 SoC 型号（如 "SM8550"）
///   4. getprop ro.board.platform / ro.product.board / ro.hardware —— 平台代号兜底
///   5. /proc/cpuinfo               —— 通用兜底（Hardware / model name 行）
///
/// 结果统一转小写后做子串匹配，兼容 "SM8550" / "sm8550" / "8550"。
fn soc_hint_matches(hints: &[&str]) -> bool {
    hints.iter().any(|h| hint_matches(h))
}

/// 设备硬件标识全集（小写、多源拼接）：只探测一次并缓存，供各片段子串匹配复用。
static SOC_HINT_HAYSTACK: OnceLock<String> = OnceLock::new();
fn soc_hint_haystack() -> &'static str {
    SOC_HINT_HAYSTACK.get_or_init(|| {
        let haystacks: Vec<String> = vec![
            read_first_line("/sys/devices/soc0/machine"),
            read_first_line("/sys/devices/soc0/plat_name"),
            getprop("ro.soc.model"),
            getprop("ro.board.platform"),
            getprop("ro.product.board"),
            getprop("ro.hardware"),
            std::fs::read_to_string("/proc/cpuinfo").unwrap_or_default(),
        ];
        haystacks.join("\n").to_lowercase()
    })
}

/// 单个处理器片段是否命中设备硬件标识。
/// 片段至少 3 个字符，避免过短片段在高频硬件标识中误匹配。
fn hint_matches(hint: &str) -> bool {
    let hl = hint.to_lowercase();
    hl.len() >= 3 && soc_hint_haystack().contains(&hl)
}

/// 是否应启用 Chiri 专用调度器（检测到列表中的特定处理器时为 true）。
/// 结果只探测一次并缓存：determine_mode 等路径会反复调用，避免每次读 /proc 与 sysfs。
static CHIRI_SOC: OnceLock<bool> = OnceLock::new();
pub fn is_chiri_soc() -> bool {
    *CHIRI_SOC.get_or_init(|| soc_hint_matches(CHIRI_SOC_HINTS))
}

/// 返回第一个「既命中设备硬件标识、又存在处理器专属配置目录」的片段。
/// 顺序与 CHIRI_SOC_HINTS 一致：配置目录缺失的机型继续向后找，
/// 防止多 SoC 并存时设备误用其它机型的配置目录（如 8450 设备读到 8550 的 config.yaml）。
fn matched_soc_hint() -> Option<&'static str> {
    if !is_chiri_soc() {
        return None;
    }
    let config_dir = get_module_root().join("config");
    CHIRI_SOC_HINTS
        .iter()
        .copied()
        .find(|hint| hint_matches(hint) && config_dir.join(hint).join("config.yaml").exists())
}

/// 命中 Chiri 目标 SoC 时，返回其处理器专属配置目录 `config/{命中片段}/`（存在则返回）。
fn matched_soc_config_dir() -> Option<PathBuf> {
    matched_soc_hint().map(|hint| get_module_root().join("config").join(hint))
}

/// 处理器核心组区间（little/big/prime 的 CPU ID 区间，左闭右开）。
/// akmode 按组统计忙/闲核心数、CLG 触摸升频判定大核簇时使用；
/// 各 SoC 簇布局不同，按命中片段区分（未命中时回退 8550 布局兜底）。
#[derive(Debug, Clone)]
pub struct CoreGroupRanges {
    /// 小核组
    pub little: std::ops::Range<usize>,
    /// 大核组
    pub big: std::ops::Range<usize>,
    /// 超大核组：无超大核的 SoC 为空区间（start == end），统计时自动跳过
    pub prime: std::ops::Range<usize>,
}

/// 按命中的处理器片段返回核心组区间：
/// - 8550（骁龙 8 Gen 2）：little 0-2 / big 3-6 / prime 7
/// - 8450（骁龙 8 Gen 1）：little 0-3 / big 4-6 / prime 7
/// - 8998（骁龙 835）：little 0-3 / big 4-7 / 无 prime
/// 未命中（非 ChiRi）回退 8550 布局兜底（仅 Chiri 路径调用，正常不会发生）。
pub fn chiri_core_ranges() -> CoreGroupRanges {
    match matched_soc_hint() {
        Some("8450") => CoreGroupRanges {
            little: 0..4,
            big: 4..7,
            prime: 7..8,
        },
        Some("8998") => CoreGroupRanges {
            little: 0..4,
            big: 4..8,
            prime: 7..7,
        },
        _ => CoreGroupRanges {
            little: 0..3,
            big: 3..7,
            prime: 7..8,
        },
    }
}

/// 特调（akmode）可用性共享标志：chiri Config 合并 akmode.yaml 成功后置 true，
/// 文件缺失/损坏时置 false。monitor 层 determine_mode 据此决定白名单应用
/// 是进入特调还是回退 CLG（缺 akmode.yaml 的机型不做特调，按普通模式调度）。
static AKMODE_AVAILABLE: AtomicBool = AtomicBool::new(false);

/// 特调（akmode）是否可用（akmode.yaml 已成功加载）。
pub fn is_akmode_available() -> bool {
    AKMODE_AVAILABLE.load(Ordering::Acquire)
}

/// 设置特调可用性：chiri Config::from_file 合并 akmode.yaml 时调用。
pub fn set_akmode_available(available: bool) {
    AKMODE_AVAILABLE.store(available, Ordering::Release);
}

/// 返回当前应加载的配置文件路径：
/// - 命中 Chiri 目标 SoC 且存在处理器子目录 `config/{命中片段}/config.yaml` 时，使用该文件
/// - 否则回退到默认 `config/config.yaml`
///
/// 所有配置加载/热重载入口（main.rs 与两套调度器的 config_watcher）统一走这里，
/// 保证 8550 等目标机型使用处理器独立配置，其余机型不受影响。
pub fn get_config_path() -> PathBuf {
    matched_soc_config_dir()
        .map(|dir| dir.join("config.yaml"))
        .unwrap_or_else(|| get_module_root().join("config").join("config.yaml"))
}

/// 内部特调白名单条目：一个应用可对应多个特调模式。
/// 编译进守护进程二进制，不随 rules.yaml 下发，用户 / WebUI 均不可修改。
pub struct SpecialTunedEntry {
    /// 应用包名
    pub package: &'static str,
    /// 该应用可用的特调模式列表（WebUI 动作单据此提供专属选项）
    pub modes: &'static [&'static str],
    /// 优先回退模式：用户未显式配置该应用时默认采用的模式（必须存在于 modes 中）
    pub fallback: &'static str,
}

/// 内部特调白名单。
/// 调度优先级：rules.yaml 用户自定义 app_modes > 特调白名单的优先回退模式 > global_mode。
/// 特调模式名不可用于白名单之外的包名——monitor 侧做门控（determine_mode），
/// 非白名单包名映射到特调模式时回退 global_mode 并告警。
/// 新增特调只需追加条目，模式名须在 chiri 的 Config::get_mode 中注册，
/// 参数写在处理器目录 `module/config/{命中SoC}/akmode.yaml`（特调段，与白名单条目一致）。
/// WebUI 通过守护进程启动时导出的 special_tuned.txt 展示“特调”标签与专属选项。
pub const SPECIAL_TUNED_MODES: &[SpecialTunedEntry] = &[SpecialTunedEntry {
    package: "com.hypergryph.arknights", // 明日方舟
    modes: &["akmode"],
    fallback: "akmode",
}];

/// 查询包名是否命中特调白名单，命中返回优先回退模式
pub fn special_tuned_mode(pkg: &str) -> Option<&'static str> {
    SPECIAL_TUNED_MODES
        .iter()
        .find(|e| e.package == pkg)
        .map(|e| e.fallback)
}

/// 判断模式名是否为特调模式（任一白名单条目的 modes 列表中出现）
pub fn is_special_mode(mode: &str) -> bool {
    SPECIAL_TUNED_MODES.iter().any(|e| e.modes.contains(&mode))
}

/// 包名是否被允许使用指定特调模式（包名在白名单且模式在该条目 modes 列表中）
pub fn is_special_mode_allowed(pkg: &str, mode: &str) -> bool {
    SPECIAL_TUNED_MODES
        .iter()
        .find(|e| e.package == pkg)
        .map(|e| e.modes.contains(&mode))
        .unwrap_or(false)
}

/// 处理器专属特调配置文件路径：与处理器主配置同目录 `config/{命中片段}/akmode.yaml`
/// （特调与处理器绑定，各目标 SoC 子目录自带一份；8450 与 8998 参数相同、各自一份）。
/// 命中 SoC 时必返处理器目录下的路径，文件缺失/损坏由 merge_akmode 置特调不可用、
/// 白名单应用回退 CLG（不落到其它目录的共享文件）。非 Chiri（不会调用）兜底根 config 路径。
pub fn get_akmode_path() -> PathBuf {
    matched_soc_config_dir()
        .map(|dir| dir.join("akmode.yaml"))
        .unwrap_or_else(|| get_module_root().join("config").join("akmode.yaml"))
}
