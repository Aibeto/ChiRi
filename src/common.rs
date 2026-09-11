//! common.rs: [events] [paths] [soc_detect] [core_ranges] [special_tuned] [fas_whitelist] [aff_blacklist] [embedded] [external_meta]

use crate::monitor::config::RulesConfig;
use include_dir::{Dir, include_dir};
use serde::Deserialize;
use std::collections::HashMap;
use std::env;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

// [events]
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

// [paths]
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
// [soc_detect]
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
pub(crate) fn matched_soc_hint() -> Option<&'static str> {
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
// [core_ranges]
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

// 功能总开关（fas_enabled / scenemode_enabled，meta.yaml 顶层字段，缺省 true）：
// Config::load（启动 + config_watcher 热重载）时同步到进程级原子标志，
// 高频路径（fas_available / scenemode 进入判定）只读原子量，不触碰磁盘与锁。
static FAS_ENABLED: AtomicBool = AtomicBool::new(true);
static SCENEMODE_ENABLED: AtomicBool = AtomicBool::new(true);

/// 设置 FAS 总开关（meta.yaml 的 fas_enabled，Config::load 时调用）。
pub fn set_fas_enabled(enabled: bool) {
    FAS_ENABLED.store(enabled, Ordering::Release);
}

/// 设置 scenemode 总开关（meta.yaml 的 scenemode_enabled，Config::load 时调用）。
pub fn set_scenemode_enabled(enabled: bool) {
    SCENEMODE_ENABLED.store(enabled, Ordering::Release);
}

/// FAS 总开关是否开启（meta.yaml 的 fas_enabled，缺省 true）。
pub fn fas_enabled() -> bool {
    FAS_ENABLED.load(Ordering::Acquire)
}

/// scenemode 总开关是否开启（meta.yaml 的 scenemode_enabled，缺省 true）。
pub fn scenemode_enabled() -> bool {
    SCENEMODE_ENABLED.load(Ordering::Acquire)
}

/// 返回当前应加载的配置文件路径：
/// - 命中 Chiri 目标 SoC 且存在处理器子目录 `config/{命中片段}/meta.yaml` 时，使用该文件
/// - 否则回退到默认 `config/meta.yaml`
///
/// 所有配置加载/热重载入口（main.rs 与两套调度器的 config_watcher）统一走这里，
/// 保证 8550 等目标机型使用处理器独立配置，其余机型不受影响。
pub fn get_config_path() -> PathBuf {
    matched_soc_config_dir()
        .map(|dir| dir.join("meta.yaml"))
        .unwrap_or_else(|| get_module_root().join("config").join("meta.yaml"))
}

// [special_tuned]
// 内部特调白名单（编译期嵌入，见 src/chiri/special_tuned.yaml）
/// 特调白名单条目（由 src/chiri/special_tuned.yaml 编译期嵌入并解析）。
/// 用户 / WebUI 均不可修改；磁盘上的 special_tuned.yaml 只是运行时导出快照。
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
const SPECIAL_TUNED_TEXT: &str = include_str!("chiri/special_tuned.yaml");

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
/// main.rs 导出 special_tuned.yaml 时只取 regex.is_none() 的精确条目。
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

// [fas_whitelist]
// FAS（帧感知调度）白名单与每应用配置（编译期嵌入）
// 白名单运行时导出到模块根 fas_whitelist.yaml 供 WebUI 只读展示；
// 每应用配置不导出。用户/WebUI 不可修改。

// FAS 白名单与每应用配置（normal/fas.yaml 与 normal/fas/<配置名>.yaml）随
// module/config 目录整体构建期嵌入（见 [embedded]），不在此逐文件硬编码。

/// 按配置名返回对应应用的嵌入 FAS 配置文本（构建期自动收录 normal/fas/*.yaml）。
/// 新增 FAS 游戏：fas.yaml 白名单加一行 + 新建 normal/fas/<配置名>.yaml，无需改 .rs。
pub fn embedded_fas_app_str(name: &str) -> Option<&'static str> {
    embedded_config_file(&format!("normal/fas/{name}.yaml"))
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
        // 构建期嵌入的 normal/fas.yaml（缺失时按空表处理）
        let text = embedded_config_file("normal/fas.yaml").unwrap_or_default();
        match serde_yaml::from_str::<FasWhitelistFile>(text) {
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

/// FAS 是否可用：总开关开启（rules.yaml fas_enabled）、白名单非空且至少一个应用配置解析成功。
pub fn fas_available() -> bool {
    if !fas_enabled() {
        return false;
    }
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

// 线程亲和黑名单（src/chiri/affinity_blacklist.yaml，编译期嵌入）
//
// 命中黑名单的进程：全部线程保持全核运行，AffinityManager 不做任何迁移与亲和。
// 数据编译进二进制（用户/WebUI 不可修改），但独立成文件便于维护与调整。

// [aff_blacklist]
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

const AFFINITY_BLACKLIST_TEXT: &str = include_str!("chiri/affinity_blacklist.yaml");

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

// 编译期嵌入的配置（整目录 include_dir!，防篡改）
//
// 原 config.yaml 已拆分为 **meta.yaml（用户可修改抬头）+ feature.yaml（不可修改
// 调优段）**，各 SoC 目录与默认 config/ 下各一份；二者仅存于二进制，磁盘不落盘
// （feature 无任何外部读取方；meta 的磁盘副本即 meta.yaml，由 sync_meta_snapshot
// 自愈）。module/config 目录**整体**在构建期嵌入：新增/删除 yaml（新 SoC、新 FAS
// 应用配置等）无需改动任何 .rs；必需文件缺失由 build.rs 断言，直接编译失败。
// 已存在文件的改动由 rustc 的 include_bytes! 依赖跟踪触发重编译；目录增删由
// build.rs 的 rerun-if-changed=module/config 触发。

// [embedded]
static CONFIG_DIR: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/module/config");

/// 按相对 module/config 的路径取嵌入文本（如 "8550/meta.yaml"），缺失返回 None。
/// 必需文件的存在性由 build.rs 编译期断言兜底，运行时不会静默空转。
pub(crate) fn embedded_config_file(rel: &str) -> Option<&'static str> {
    CONFIG_DIR.get_file(rel).and_then(|f| f.contents_utf8())
}

/// 嵌入的 meta.yaml（用户可修改字段的默认值）：按命中处理器取 {soc}/meta.yaml，
/// 未命中（默认 Yumi）取 meta.yaml
pub fn embedded_meta_str() -> &'static str {
    if let Some(soc) = matched_soc_hint() {
        if let Some(text) = embedded_config_file(&format!("{soc}/meta.yaml")) {
            return text;
        }
    }
    embedded_config_file("meta.yaml").unwrap_or_default()
}

/// 嵌入的 feature.yaml（不可修改调优段）：按命中处理器取 {soc}/feature.yaml，
/// 未命中取 feature.yaml。仅存于二进制，磁盘不落盘、不监听
pub fn embedded_feature_str() -> &'static str {
    if let Some(soc) = matched_soc_hint() {
        if let Some(text) = embedded_config_file(&format!("{soc}/feature.yaml")) {
            return text;
        }
    }
    embedded_config_file("feature.yaml").unwrap_or_default()
}

/// 嵌入的 akmode.yaml（config/normal/，嵌入后特调始终可用）
pub fn embedded_akmode_str() -> &'static str {
    embedded_config_file("normal/akmode.yaml").unwrap_or_default()
}

/// 嵌入的 rules.yaml（模块根，编译期打包进二进制）：与其他只读文件（akmode/scenemode/
/// fas 配置）同口径——运行时一律读嵌入内容，磁盘文件仅作对外展示副本（启动时复制出去，
/// 被篡改不影响调度行为）。
pub fn embedded_rules_str() -> &'static str {
    include_str!("../module/rules.yaml")
}

/// 解析嵌入的 rules.yaml（运行时唯一规则来源；磁盘 rules.yaml 仅是启动时复制出的
/// 展示副本，被篡改不影响调度行为）。解析失败回退 Default 并告警，绝不 panic。
pub fn embedded_rules() -> crate::monitor::config::RulesConfig {
    match serde_yaml::from_str(embedded_rules_str()) {
        Ok(r) => r,
        Err(e) => {
            log::warn!("[Rules] Embedded rules parse failed: {e}. Default.");
            crate::monitor::config::RulesConfig::default()
        }
    }
}

/// 嵌入的 scenemode.yaml（config/normal/）
pub fn embedded_scenemode_str() -> &'static str {
    embedded_config_file("normal/scenemode.yaml").unwrap_or_default()
}

/// 嵌入的语言包：zh → i18n/zh.ftl，其余语言一律回退 i18n/en.ftl
pub fn embedded_ftl_str(lang: &str) -> &'static str {
    let rel = if lang.eq_ignore_ascii_case("zh") {
        "i18n/zh.ftl"
    } else {
        "i18n/en.ftl"
    };
    embedded_config_file(rel).unwrap_or_default()
}

/// 磁盘 meta.yaml 的严格结构：七个字段全部必填、拒绝未知字段。
/// 任一缺失/多余/类型不符，或取值不在白名单内，整文件判非法——
/// 由 sync_meta_snapshot 用二进制内嵌默认值整体覆盖修正。
// [external_meta]
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MetaYamlFile {
    name: String,
    author: String,
    language: String,
    loglevel: String,
    dev_record: bool,
    fas_enabled: bool,
    scenemode_enabled: bool,
}

/// 磁盘 meta.yaml 校验通过后交给 Config::load 的覆盖值（全字段必有效）。
/// daemon 只消费其中 5 项（name/author 仅 WebUI 展示，直接读文件即可）。
#[derive(Debug, Clone)]
pub struct ExternalMetaOverrides {
    pub loglevel: String,
    pub language: String,
    pub dev_record: bool,
    pub fas_enabled: bool,
    pub scenemode_enabled: bool,
}

impl Default for ExternalMetaOverrides {
    fn default() -> Self {
        Self {
            loglevel: "INFO".to_string(),
            language: "en".to_string(),
            dev_record: false,
            fas_enabled: true,
            scenemode_enabled: true,
        }
    }
}

/// 读磁盘 meta.yaml（先经 sync_meta_snapshot 校验/纠正）。文件缺失或仍非法时返回
/// None，调用方沿用嵌入默认值——绝不 panic，也绝不让坏文件拖垮配置加载。
pub fn read_external_meta(path: &Path) -> Option<ExternalMetaOverrides> {
    let text = std::fs::read_to_string(path).ok()?;
    parse_disk_meta(&text)
}

/// 嵌入 meta.yaml 的默认覆盖值（编译期内容，构建期断言文件存在，正常必合法）
pub fn embedded_meta_defaults() -> ExternalMetaOverrides {
    parse_disk_meta(embedded_meta_str()).unwrap_or_default()
}

/// 日志等级白名单（大小写不敏感，与 logger 支持的档位一致）
const LOGLEVELS: [&str; 6] = ["OFF", "ERROR", "WARN", "INFO", "DEBUG", "TRACE"];

/// 去掉成对的单层引号（meta.yaml 里字符串值常带引号）
fn unquote(s: &str) -> &str {
    for q in ['"', '\''] {
        if s.len() >= 2 && s.starts_with(q) && s.ends_with(q) {
            return &s[1..s.len() - 1];
        }
    }
    s
}

/// 校验并归一化日志等级：必须整体等于某个档位（去引号后），含空白/注释/换行的脏值拒绝
fn sanitize_loglevel(raw: &str) -> Option<String> {
    let v = unquote(raw.trim());
    LOGLEVELS
        .iter()
        .find(|l| l.eq_ignore_ascii_case(v))
        .map(|l| l.to_string())
}

/// 校验并归一化日志语言：仅接受 en / zh（嵌入语言包只打包这两种）
fn sanitize_language(raw: &str) -> Option<String> {
    match unquote(raw.trim()).to_ascii_lowercase().as_str() {
        "zh" => Some("zh".to_string()),
        "en" => Some("en".to_string()),
        _ => None,
    }
}

/// 整体校验磁盘 meta.yaml：结构严格（字段齐全、无未知键、类型正确）+ 取值白名单。
/// 任一字段异常返回 None——调用方以二进制内嵌默认值覆盖修正，用户乱改不会生效。
fn parse_disk_meta(text: &str) -> Option<ExternalMetaOverrides> {
    let f: MetaYamlFile = serde_yaml::from_str(text).ok()?;
    if f.name.trim().is_empty() || f.author.trim().is_empty() {
        return None;
    }
    Some(ExternalMetaOverrides {
        loglevel: sanitize_loglevel(&f.loglevel)?,
        language: sanitize_language(&f.language)?,
        dev_record: f.dev_record,
        fas_enabled: f.fas_enabled,
        scenemode_enabled: f.scenemode_enabled,
    })
}

/// meta.yaml 快照自愈：main.rs 启动时与两套 config_watcher **触发热重载前**调用。
/// - 文件合法 → 跳过写入（返回 false，防 config_watcher 事件成环）
/// - 文件缺失/不可读 → 嵌入原文原子重建（首次安装 / 被误删；不留警告注释）
/// - 任一字段非法 → 嵌入原文整体覆盖 + 文件末尾追加警告注释 + warn 日志
///   （完全丢失重建与「修改出错被纠正」是两回事，前者不留言）
pub fn sync_meta_snapshot(meta_path: &Path) -> bool {
    let embedded = embedded_meta_str();
    let corrected = match std::fs::read_to_string(meta_path) {
        Ok(text) => {
            if parse_disk_meta(&text).is_some() {
                return false;
            }
            log::warn!(
                "[Meta] meta.yaml fields invalid, reset to embedded defaults: {}",
                meta_path.display()
            );
            format!(
                "{embedded}\n# 上一次修改存在非法字段，已恢复为默认值；详情见 logs/daemon.log\n"
            )
        }
        Err(_) => {
            log::info!(
                "[Meta] meta.yaml missing, rebuilt from embedded defaults: {}",
                meta_path.display()
            );
            embedded.to_string()
        }
    };
    if let Some(parent) = meta_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    write_file_no_panic(meta_path, corrected.as_bytes())
}

/// rules.yaml 快照复制：把编译期嵌入的 rules.yaml 向外复制到模块根（与 config.yaml
/// 快照自愈同口径——嵌入内容为唯一基准，磁盘副本仅供展示/备份，运行时一律读嵌入值，
/// 被篡改不影响调度行为）。内容一致时跳过写入。
///
/// **防 panic 约定**：对外写文件失败绝不 panic——同口径原子写失败后重写一次
/// （rename 失败时回退 try_write_file 兜底），重写仍无效则记 warn 并直接跳过，
/// 不影响守护进程启动与运行。
pub fn sync_rules_snapshot(path: &Path) -> bool {
    let content = embedded_rules_str();
    // 内容一致就跳过：防止 config_watcher/inotify 事件循环
    if std::fs::read_to_string(path).ok().as_deref() == Some(content) {
        return false;
    }
    if write_file_no_panic(path, content.as_bytes()) {
        return true;
    }
    // 第一次写入失败（目录缺失/权限/IO 抖动）：补建父目录后重写一次
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if write_file_no_panic(path, content.as_bytes()) {
        log::warn!("[Rules] Snapshot rewrite recovered: {}", path.display());
        return true;
    }
    // 重写仍无效：跳过（磁盘副本保持原样），绝不 panic
    log::warn!(
        "[Rules] Snapshot write failed, skip (embedded content stays authoritative): {}",
        path.display()
    );
    false
}

/// 无 panic 的原子文件写：tmp + rename 失败时回退 try_write_file，全程不 panic。
/// 供快照复制使用（调用方自行决定失败后的跳过策略）。
fn write_file_no_panic(path: &Path, bytes: &[u8]) -> bool {
    let file_name = path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "rules".to_string());
    let tmp_path = path.with_file_name(format!("{}.tmp", file_name));
    let atomic_ok =
        std::fs::write(&tmp_path, bytes).is_ok() && std::fs::rename(&tmp_path, path).is_ok();
    if !atomic_ok {
        let _ = std::fs::remove_file(&tmp_path);
    }
    atomic_ok || crate::utils::try_write_file(path, bytes).is_ok()
}
