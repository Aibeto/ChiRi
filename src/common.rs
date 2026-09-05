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

/*
 * Copyright (C) 2026 ChiRi
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
use serde::Deserialize;
use std::collections::HashMap;
use std::env;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

/// 守护进程全局事件总线
#[derive(Debug, Clone)]
pub enum DaemonEvent {
    /// 低频事件：前台应用切换或环境温度变化引起的模式改变
    ModeChange {
        package_name: String,
        pid: i32,
        mode: String,
        temperature: f64,
    },
    /// 同模式前台包切换（模式不变、应用变化，ChiRi 侧 FAS fas→fas 热切换消费）
    PackageSwitch {
        package_name: String,
        pid: i32,
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

    /// eBPF 扩展探针的周期统计（ChiRi 专属：仅 ChiRi SoC 上 cpu_monitor 加载
    /// 可选探针并发送；Yumi 设备不会产生该事件）。字段为发送周期（2s）内的增量。
    BpfStats {
        /// sched_wakeup 唤醒次数：调度唤醒链活跃度
        wakeups: u32,
        /// sched_migrate_task 线程迁移次数：亲和策略的实际迁移观测
        migrations: u32,
        /// cpufreq_transition 频率切换次数：调频活跃度（含热限频切换）
        freq_transitions: u32,
    },
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
/// 例：SM8550（骁龙 8 Gen 2）含 "8550"，SM8475（骁龙 8+ Gen 1）含 "8475"，
/// MSM8998（骁龙 835）含 "8998"。片段须能互相区分（8550 不含 8475，反之亦然）。
const CHIRI_SOC_HINTS: &[&str] = &["8550", "8475", "8998"];

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

/// 返回第一个命中设备硬件标识的处理器片段。
/// 顺序与 CHIRI_SOC_HINTS 一致。配置已编译进二进制（embedded_config_str），
/// 匹配只看硬件标识，不再依赖磁盘目录是否存在——磁盘快照缺失/被删不影响识别。
fn matched_soc_hint() -> Option<&'static str> {
    if !is_chiri_soc() {
        return None;
    }
    CHIRI_SOC_HINTS
        .iter()
        .copied()
        .find(|hint| hint_matches(hint))
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
/// - 8475（骁龙 8+ Gen 1）：little 0-3 / big 4-6 / prime 7
/// - 8998（骁龙 835）：little 0-3 / big 4-7 / 无 prime
/// 未命中（非 ChiRi）回退 8550 布局兜底（仅 Chiri 路径调用，正常不会发生）。
pub fn chiri_core_ranges() -> CoreGroupRanges {
    match matched_soc_hint() {
        Some("8475") => CoreGroupRanges {
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

/// 设置特调可用性：chiri Config::load 合并嵌入的 akmode.yaml 时调用。
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

// ════════════════════════════════════════════════════════════════
//  内部特调白名单（编译期嵌入，见 src/chiri/special_tuned.txt）
// ════════════════════════════════════════════════════════════════

/// 特调白名单条目（由 src/chiri/special_tuned.txt 编译期嵌入并解析）。
/// 用户 / WebUI 均不可修改；磁盘上的 special_tuned.txt 只是运行时导出快照。
pub struct SpecialTunedEntry {
    /// 匹配器原文：精确包名，或 "re:" 前缀的正则表达式
    pub package: String,
    /// 正则条目的预编译结果（精确条目为 None）
    pub regex: Option<regex::Regex>,
    /// 该应用可用的特调模式列表（须在 chiri Config::get_mode 注册）
    pub modes: Vec<String>,
    /// 优先回退模式：用户未显式配置该应用时默认采用（必须在 modes 内）
    pub fallback: String,
}

impl SpecialTunedEntry {
    /// 包名是否命中本条目：精确条目全等比较，正则条目 is_match
    fn matches(&self, pkg: &str) -> bool {
        match &self.regex {
            Some(re) => re.is_match(pkg),
            None => self.package == pkg,
        }
    }
}

/// 嵌入的白名单原文（include_str! 相对 src/common.rs）
const SPECIAL_TUNED_TEXT: &str = include_str!("chiri/special_tuned.txt");

/// 解析结果只算一次，之后全部走缓存
static SPECIAL_TUNED: OnceLock<Vec<SpecialTunedEntry>> = OnceLock::new();

/// 解析嵌入文本：跳过空行与 # 注释行，按 `匹配器:模式列表:回退模式` 切分。
/// "re:" 前缀条目预编译正则，编译失败的条目跳过并告警（不影响其余条目）。
fn parse_special_tuned(text: &str) -> Vec<SpecialTunedEntry> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.splitn(3, ':');
        let (Some(pkg), Some(modes), Some(fallback)) = (parts.next(), parts.next(), parts.next())
        else {
            log::warn!("special-tuned: malformed entry skipped: {}", line);
            continue;
        };
        let modes: Vec<String> = modes
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        let fallback = fallback.trim().to_string();
        if modes.is_empty() || fallback.is_empty() {
            log::warn!(
                "special-tuned: entry with empty modes/fallback skipped: {}",
                line
            );
            continue;
        }
        let (package, regex) = match pkg.strip_prefix("re:") {
            Some(pat) => match regex::Regex::new(pat) {
                Ok(re) => (pkg.to_string(), Some(re)),
                Err(e) => {
                    log::warn!("special-tuned: invalid regex '{}' skipped: {}", pat, e);
                    continue;
                }
            },
            None => (pkg.to_string(), None),
        };
        out.push(SpecialTunedEntry {
            package,
            regex,
            modes,
            fallback,
        });
    }
    out
}

/// 全部白名单条目（精确 + 正则，按文件顺序）。
/// main.rs 导出 special_tuned.txt 时只取 regex.is_none() 的精确条目。
pub fn special_tuned_entries() -> &'static [SpecialTunedEntry] {
    SPECIAL_TUNED.get_or_init(|| parse_special_tuned(SPECIAL_TUNED_TEXT))
}

/// 查询包名命中的白名单条目：先按精确包名匹配（文件顺序），未命中再按正则条目。
/// 调度优先级：rules.yaml 用户自定义 app_modes > 特调白名单回退模式 > global_mode。
pub fn special_tuned_entry(pkg: &str) -> Option<&'static SpecialTunedEntry> {
    let list = special_tuned_entries();
    list.iter()
        .find(|e| e.regex.is_none() && e.package == pkg)
        .or_else(|| list.iter().find(|e| e.matches(pkg)))
}

/// 查询包名是否命中特调白名单，命中返回优先回退模式
pub fn special_tuned_mode(pkg: &str) -> Option<String> {
    special_tuned_entry(pkg).map(|e| e.fallback.clone())
}

/// 判断模式名是否为特调模式（任一白名单条目的 modes 列表中出现）
pub fn is_special_mode(mode: &str) -> bool {
    special_tuned_entries()
        .iter()
        .any(|e| e.modes.iter().any(|m| m == mode))
}

/// 包名是否被允许使用指定特调模式（包名命中白名单且模式在该条目 modes 列表中）
pub fn is_special_mode_allowed(pkg: &str, mode: &str) -> bool {
    special_tuned_entry(pkg)
        .map(|e| e.modes.iter().any(|m| m == mode))
        .unwrap_or(false)
}

// ════════════════════════════════════════════════════════════════
//  FAS（帧感知调度）白名单与每应用配置（编译期嵌入）
//  白名单运行时导出到模块根 fas_whitelist.txt 供 WebUI 只读展示；
//  每应用配置不导出。用户/WebUI 不可修改。
// ════════════════════════════════════════════════════════════════

const FAS_WHITELIST_TEXT: &str = include_str!("../module/config/normal/fas.yaml");
const FAS_APP_ENDFIELD_TEXT: &str = include_str!("../module/config/normal/fas/endfield.yaml");

/// 按配置名返回对应应用的嵌入 FAS 配置文本。
/// 新增 FAS 游戏：module/config/normal/fas.yaml 白名单加一行 + 此处加 arm + 新建配置文件。
pub fn embedded_fas_app_str(name: &str) -> Option<&'static str> {
    match name {
        "endfield" => Some(FAS_APP_ENDFIELD_TEXT),
        _ => None,
    }
}

#[derive(Debug, Deserialize)]
struct FasWhitelistFile {
    #[serde(default)]
    fas: FasWhitelistSection,
}

#[derive(Debug, Default, Deserialize)]
struct FasWhitelistSection {
    #[serde(default)]
    apps: HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
struct FasAppFile {
    #[serde(default)]
    fas_rules: crate::fas_types::FasRulesConfig,
}

static FAS_WHITELIST: OnceLock<HashMap<String, String>> = OnceLock::new();
static FAS_APP_CONFIGS: OnceLock<HashMap<String, crate::fas_types::FasRulesConfig>> =
    OnceLock::new();

/// FAS 白名单（精确包名 → 配置名），编译期嵌入，解析失败回退空表。
pub fn fas_whitelist() -> &'static HashMap<String, String> {
    FAS_WHITELIST.get_or_init(|| {
        match serde_yaml::from_str::<FasWhitelistFile>(FAS_WHITELIST_TEXT) {
            Ok(f) => f.fas.apps,
            Err(e) => {
                log::warn!("fas-config-parse-failed: {e}");
                HashMap::new()
            }
        }
    })
}

/// 精确匹配 FAS 白名单。
pub fn fas_whitelist_entry(pkg: &str) -> Option<&'static String> {
    fas_whitelist().get(pkg)
}

/// 按配置名取该应用的 FAS 规则（首次调用时解析全部白名单应用，normalize 后缓存）。
pub fn fas_app_config(name: &str) -> Option<&'static crate::fas_types::FasRulesConfig> {
    FAS_APP_CONFIGS
        .get_or_init(|| {
            let mut map = HashMap::new();
            let mut names: Vec<&String> = fas_whitelist().values().collect();
            names.sort();
            names.dedup();
            for name in names {
                let Some(text) = embedded_fas_app_str(name) else {
                    log::warn!("fas-config-missing: {name}");
                    continue;
                };
                match serde_yaml::from_str::<FasAppFile>(text) {
                    Ok(mut f) => {
                        f.fas_rules.normalize();
                        f.fas_rules.migrate_legacy_margins();
                        map.insert(name.clone(), f.fas_rules);
                    }
                    Err(e) => log::warn!("fas-config-parse-failed ({name}): {e}"),
                }
            }
            map
        })
        .get(name)
}

/// FAS 是否可用：白名单非空且至少一个应用配置解析成功。
pub fn fas_available() -> bool {
    if fas_whitelist().is_empty() {
        return false;
    }
    // 触发惰性解析（fas_app_config 首次调用会解析全部白名单应用）
    if let Some(name) = fas_whitelist().values().next() {
        let _ = fas_app_config(name);
    }
    FAS_APP_CONFIGS.get().map_or(false, |m| !m.is_empty())
}

/// 模式名是否为 FAS。
pub fn is_fas_mode(mode: &str) -> bool {
    mode == "fas"
}

// ════════════════════════════════════════════════════════════════
//  线程亲和黑名单（src/chiri/affinity_blacklist.txt，编译期嵌入）
// ════════════════════════════════════════════════════════════════
//
// 命中黑名单的进程：全部线程保持全核运行，AffinityManager 不做任何迁移与亲和。
// 数据编译进二进制（用户/WebUI 不可修改），但独立成文件便于维护与调整。

/// 黑名单条目：精确进程名/包名，或 "re:" 前缀的正则（同特调白名单格式）
pub struct AffinityBlacklistEntry {
    pub pattern: String,
    pub regex: Option<regex::Regex>,
}

impl AffinityBlacklistEntry {
    fn matches(&self, cmdline: &str) -> bool {
        match &self.regex {
            Some(re) => re.is_match(cmdline),
            None => self.pattern == cmdline,
        }
    }
}

const AFFINITY_BLACKLIST_TEXT: &str = include_str!("chiri/affinity_blacklist.txt");

static AFFINITY_BLACKLIST: OnceLock<Vec<AffinityBlacklistEntry>> = OnceLock::new();

/// 解析黑名单文本：跳过空行与 # 注释行；"re:" 前缀预编译正则，
/// 编译失败跳过该条（不影响其余条目）。
fn parse_affinity_blacklist(text: &str) -> Vec<AffinityBlacklistEntry> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        match line.strip_prefix("re:") {
            Some(pat) => match regex::Regex::new(pat) {
                Ok(re) => out.push(AffinityBlacklistEntry {
                    pattern: line.to_string(),
                    regex: Some(re),
                }),
                Err(e) => log::warn!("affinity-blacklist: invalid regex '{}' skipped: {}", pat, e),
            },
            None => out.push(AffinityBlacklistEntry {
                pattern: line.to_string(),
                regex: None,
            }),
        }
    }
    out
}

/// 全部黑名单条目（精确 + 正则，按文件顺序），OnceLock 缓存
pub fn affinity_blacklist_entries() -> &'static [AffinityBlacklistEntry] {
    AFFINITY_BLACKLIST.get_or_init(|| parse_affinity_blacklist(AFFINITY_BLACKLIST_TEXT))
}

/// 进程 cmdline（或线程 comm）是否命中亲和黑名单。
/// 另含两条内置兜底规则（不经文件、不可关闭）：
/// - 空 cmdline = 内核线程（kworker 等）→ 黑名单；
/// - 以 '/' 开头 = native 二进制路径（init 拉起的系统/厂商服务）→ 黑名单。
pub fn is_affinity_blacklisted(cmdline: &str) -> bool {
    if cmdline.is_empty() || cmdline.starts_with('/') {
        return true;
    }
    affinity_blacklist_entries()
        .iter()
        .any(|e| e.matches(cmdline))
}

// ════════════════════════════════════════════════════════════════
//  编译期嵌入的配置（include_str!，防篡改）
// ════════════════════════════════════════════════════════════════
//
// 调优配置一律以二进制内嵌内容为准；磁盘上的同名 yaml 只是「自愈快照 +
// meta 覆盖入口」：daemon 启动/重载时会把嵌入内容还原到磁盘（快照自愈），
// 只有 meta.loglevel 允许被外部修改（WebUI 日志等级切换），其余内容固定。

/// 嵌入的 config.yaml：按命中的处理器取对应内容，非 ChiRi SoC 用默认配置
pub fn embedded_config_str() -> &'static str {
    match matched_soc_hint() {
        Some("8550") => include_str!("../module/config/8550/config.yaml"),
        Some("8475") => include_str!("../module/config/8475/config.yaml"),
        Some("8998") => include_str!("../module/config/8998/config.yaml"),
        _ => include_str!("../module/config/config.yaml"),
    }
}

/// 嵌入的 akmode.yaml（config/normal/，嵌入后特调始终可用）
pub fn embedded_akmode_str() -> &'static str {
    include_str!("../module/config/normal/akmode.yaml")
}

/// 嵌入的 scenemode.yaml（config/normal/）
pub fn embedded_scenemode_str() -> &'static str {
    include_str!("../module/config/normal/scenemode.yaml")
}

/// 嵌入的语言包：zh → zh.ftl，其余语言一律回退 en.ftl
pub fn embedded_ftl_str(lang: &str) -> &'static str {
    if lang.eq_ignore_ascii_case("zh") {
        include_str!("../module/config/i18n/zh.ftl")
    } else {
        include_str!("../module/config/i18n/en.ftl")
    }
}

/// 磁盘配置文件的 meta 覆盖结构：只反序列化 meta 段，其余字段全部忽略
/// （调优字段即使被篡改也不会被读入，从根本上防篡改）。
/// **允许外部修改的字段只有两个**：meta.loglevel（WebUI 日志等级切换）、
/// meta.dev_record（WebUI 开发记录开关，控制 devimp/ 诊断日志写入）；
/// language 等其余 meta 字段不在此列——配置内容一律以二进制内嵌值为准。
#[derive(Deserialize, Default)]
struct ExternalMetaSection {
    #[serde(default, alias = "Loglevel")]
    loglevel: String,
    /// 缺省 None = 磁盘未提供该字段（不覆盖嵌入值）
    #[serde(default, alias = "DevRecord")]
    dev_record: Option<bool>,
}

#[derive(Deserialize, Default)]
struct ExternalMetaFile {
    #[serde(default, alias = "Meta")]
    meta: ExternalMetaSection,
}

/// 磁盘 meta 覆盖值（read_external_meta 的返回结构，逐字段 Option 区分「未提供」）
#[derive(Default)]
pub struct ExternalMetaOverrides {
    pub loglevel: Option<String>,
    pub dev_record: Option<bool>,
}

/// 读磁盘配置文件的 meta 覆盖值（仅 loglevel + dev_record 两个允许外部修改的字段）。
/// 文件缺失或解析失败返回 None：文件损坏时回退嵌入默认值，绝不让坏文件拖垮配置加载。
pub fn read_external_meta(path: &Path) -> Option<ExternalMetaOverrides> {
    let text = std::fs::read_to_string(path).ok()?;
    let file = serde_yaml::from_str::<ExternalMetaFile>(&text).ok()?;
    Some(ExternalMetaOverrides {
        loglevel: Some(file.meta.loglevel).filter(|v| !v.is_empty()),
        dev_record: file.meta.dev_record,
    })
}

/// 把 yaml 文本中指定标量键的值替换为 value（保留缩进与注释，与
/// WebUI bridge.ts::setLogLevel 的行替换同口径）。全文件按行扫描，
/// 仅替换第一个形如 `<缩进>key: ...` 的行。`quoted` 控制值是否带双引号
/// （字符串字段带引号；布尔字段必须裸值——serde_yaml 把 "true" 解析为字符串而非 bool）。
fn replace_yaml_scalar_impl(content: &str, key: &str, value: &str, quoted: bool) -> String {
    let mut out = Vec::with_capacity(content.lines().count());
    let mut replaced = false;
    for line in content.lines() {
        let trimmed = line.trim_start();
        if !replaced
            && trimmed.starts_with(key)
            && trimmed[key.len()..].trim_start().starts_with(':')
        {
            let indent = &line[..line.len() - trimmed.len()];
            let val = if quoted {
                format!("\"{value}\"")
            } else {
                value.to_string()
            };
            out.push(format!("{indent}{key}: {val}"));
            replaced = true;
        } else {
            out.push(line.to_string());
        }
    }
    let mut s = out.join("\n");
    if content.ends_with('\n') {
        s.push('\n');
    }
    s
}

/// 替换字符串字段（带引号，如 loglevel）
fn replace_yaml_scalar(content: &str, key: &str, value: &str) -> String {
    replace_yaml_scalar_impl(content, key, value, true)
}

/// 替换布尔字段（裸值，如 dev_record）
fn replace_yaml_scalar_raw(content: &str, key: &str, value: &str) -> String {
    replace_yaml_scalar_impl(content, key, value, false)
}

/// 配置快照自愈：把「嵌入内容 + 磁盘 meta 覆盖」写回生效配置路径。
/// - 磁盘文件被篡改 → 调优参数被还原为嵌入值（meta 保留用户选择）
/// - 文件缺失 → 重建（首次安装 / 被误删）
/// - 内容已一致 → 跳过写入（返回 false，防止 config_watcher 事件循环）
///
/// main.rs 启动时与两套 config_watcher 热重载后调用。
/// 返回是否实际写入。
pub fn sync_config_snapshot(path: &Path) -> bool {
    // 嵌入内容为唯一基准；外部仅 loglevel / dev_record 两项覆盖（语言等其余内容固定）
    let mut content = embedded_config_str().to_string();
    let meta = read_external_meta(path);
    if let Some(m) = &meta {
        if let Some(v) = &m.loglevel {
            content = replace_yaml_scalar(&content, "loglevel", v);
        }
        if let Some(v) = m.dev_record {
            content =
                replace_yaml_scalar_raw(&content, "dev_record", if v { "true" } else { "false" });
        }
    }
    // 内容一致就跳过：watcher 重载后再次写入会再次触发 inotify，必须防环
    if std::fs::read_to_string(path).ok().as_deref() == Some(content.as_str()) {
        return false;
    }
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    // 原子写：先写同目录临时文件再 rename。直接 fs::write 截断覆盖存在窗口期——
    // config_watcher 的 inotify（CLOSE_WRITE）与 WebUI 读盘可能读到半截内容，
    // 导致 meta 解析失败回退默认（用户日志等级设置被静默丢弃）。
    // rename 触发的 MOVED_TO 会再次走 reload，但内容一致时防环判断直接跳过。
    // 临时文件名兜底：path 无文件名（根目录 / ".." 结尾等）时 file_name() 为 None，
    // 空串会让临时文件退化为通用的 ".tmp"，可能覆盖目录中同名文件——回退固定名。
    let file_name = path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "config".to_string());
    let tmp_path = path.with_file_name(format!("{}.tmp", file_name));
    if std::fs::write(&tmp_path, content.as_bytes()).is_ok()
        && std::fs::rename(&tmp_path, path).is_ok()
    {
        return true;
    }
    let _ = std::fs::remove_file(&tmp_path);
    crate::utils::try_write_file(path, content.as_bytes()).is_ok()
}
