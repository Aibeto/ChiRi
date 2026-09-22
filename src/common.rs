//! common.rs: [events] [proc_snap] [paths] [soc_detect] [core_ranges] [special_tuned] [fas_whitelist] [aff_blacklist] [embedded] [external_meta]

use crate::monitor::config::RulesConfig;
use include_dir::{Dir, include_dir};
use serde::Deserialize;
use std::collections::HashMap;
use std::env;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
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

    /// 装箱：`RulesConfig` 是本枚举里最大的变体（HashMap + 多个 Vec/String），
    /// 内联会让每次 `SystemLoadUpdate`（40ms 一次）发送都按整枚举尺寸搬内存
    ConfigReload(Box<RulesConfig>),

    ScreenStateChange(bool),

    /// eBPF 扩展探针的周期统计（ChiRi 专属：仅 ChiRi SoC 上 cpu_monitor 加载
    /// 可选探针并发送；非 ChiRi 机型不产生该事件）。字段为发送周期（2s）内的增量。
    BpfStats {
        /// sched_wakeup 唤醒次数：调度唤醒链活跃度
        wakeups: u32,
        /// sched_migrate_task 线程迁移次数：亲和策略的实际迁移观测
        migrations: u32,
        /// cpufreq_transition 频率切换次数：调频活跃度（含热限频切换）
        freq_transitions: u32,
    },
}

// [proc_snap]
/// 每秒进程快照条目（aff `@S` 帧供数，`cpu_monitor::snapshot_procs` 产出）。
/// 全系统 TGID 级，util 来自 eBPF `TGID_RUN_TIME` 的 1s 窗口差分（不扫 /proc）。
pub struct ProcSnap {
    /// 进程 TGID
    pub pid: u32,
    /// 进程名（cmdline 首段优先，退化 comm，见 cpu_monitor::proc_name）
    pub comm: String,
    /// 1s 窗口内总 CPU 时间占比**百分比**：多核并行可 >100（320.0 ≈ 3.2 核满载）
    pub util: f32,
}

// [paths]
/// 获取模块根目录的绝对路径
pub fn get_module_root() -> PathBuf {
    // 获取当前执行文件的绝对路径
    let exe_path = env::current_exe().unwrap_or_else(|_| PathBuf::from("/"));

    // 回溯两级目录:
    // core/bin/chiri -> core/bin -> core -> <模块根>
    exe_path
        .parent()
        .unwrap_or(&exe_path) // .../core/bin
        .parent()
        .unwrap_or(&exe_path) // .../core
        .parent()
        .unwrap_or(&exe_path) // .../chiri (Root)
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

/// 读取单个 Android 系统属性（getprop key），失败/为空返回空串。
/// 跨分区属性（ro.product.model 等在 /product、/vendor 的 build.prop 里）
/// 只能走 getprop 拿合并视图——直接读 /system/build.prop 会读空（devimp 头
/// 的 model 曾因此显示 `-`）。
pub(crate) fn getprop(key: &str) -> String {
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
///
/// 结果同样只探测一次并缓存：硬件标识在进程生命周期内不可变，而本函数被
/// `chiri_core_ranges()` 等 20+ 处调用（亲和/热检查等周期块每轮都会走），
/// 每次重算要逐片段 `to_lowercase()` 分配字符串、再在数 KB 的硬件标识全集上
/// 做子串匹配——纯属重复劳动。
static MATCHED_SOC_HINT: OnceLock<Option<&'static str>> = OnceLock::new();
pub(crate) fn matched_soc_hint() -> Option<&'static str> {
    *MATCHED_SOC_HINT.get_or_init(|| {
        if !is_chiri_soc() {
            return None;
        }
        CHIRI_SOC_HINTS
            .iter()
            .copied()
            .find(|hint| hint_matches(hint))
    })
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

/// 特调可用性共享标志：chiri Config 合并 tuned_profiles.yaml 成功后置 true，
/// 文件缺失/损坏时置 false。monitor 层 determine_mode 据此决定白名单应用
/// 是进入特调还是回退 CLG（缺 tuned_profiles.yaml 的机型不做特调，按普通模式调度）。
static SPECIAL_TUNED_AVAILABLE: AtomicBool = AtomicBool::new(false);

/// 特调是否可用（tuned_profiles.yaml 已成功加载）。
pub fn is_special_tuned_available() -> bool {
    SPECIAL_TUNED_AVAILABLE.load(Ordering::Acquire)
}

/// 设置特调可用性：chiri Config::load 合并嵌入的 tuned_profiles.yaml 时调用。
pub fn set_special_tuned_available(available: bool) {
    SPECIAL_TUNED_AVAILABLE.store(available, Ordering::Release);
}

// 功能总开关（fas_enabled / scenemode_enabled，meta.yaml 顶层字段，缺省 true）：
// Config::load（启动 + config_watcher 热重载）时同步到进程级原子标志，
// 高频路径（fas_available / scenemode 进入判定）只读原子量，不触碰磁盘与锁。
static FAS_ENABLED: AtomicBool = AtomicBool::new(true);
static SCENEMODE_ENABLED: AtomicBool = AtomicBool::new(true);
/// PowerBase 总开关（meta.yaml `powerbase_enabled`，**缺省 false**：默认由 CLG 接管）
static POWERBASE_ENABLED: AtomicBool = AtomicBool::new(false);

/// 设置 PowerBase 总开关（meta.yaml 的 powerbase_enabled，Config::load 时调用）。
pub fn set_powerbase_enabled(enabled: bool) {
    POWERBASE_ENABLED.store(enabled, Ordering::Release);
}

/// PowerBase 是否开启（meta.yaml 的 powerbase_enabled，缺省 false）。
/// 高频路径（determine_mode 的模式替换、affinity 的 promote 阈值）只读这个原子量，
/// 不在 tick 内读磁盘与锁。
pub fn powerbase_enabled() -> bool {
    POWERBASE_ENABLED.load(Ordering::Acquire)
}

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

// 实验室（rhine）运行时覆盖层：实验室模式启用时，把两项「没有持久化载体」的影响收在这里。
// global_mode 在 rules.yaml 里是编译期嵌入，磁盘副本不生效，写文件没用；special_tuned
// 白名单同样是 include_str! 嵌入，没有任何外部开关。fas_enabled / scenemode_enabled 不走
// 这里——它们有 meta.yaml 载体，实验室直接改写文件，WebUI 的开关才能同步显示为关。
// 读取方（determine_mode / 特调判定）都在高频路径上，只读原子量，运行时不碰磁盘。
static LAB_GLOBAL_MODE: Mutex<Option<String>> = Mutex::new(None);
static LAB_SPECIAL_TUNED_DISABLED: AtomicBool = AtomicBool::new(false);

/// 实验室覆盖的全局模式（None = 无覆盖，按 rules.yaml 的 global_mode）
pub fn lab_global_mode() -> Option<String> {
    LAB_GLOBAL_MODE.lock().ok().and_then(|g| g.clone())
}

/// 设置或清除实验室的全局模式覆盖
pub fn set_lab_global_mode(mode: Option<String>) {
    if let Ok(mut g) = LAB_GLOBAL_MODE.lock() {
        *g = mode;
    }
}

/// 实验室是否关闭了所有场景特调
pub fn lab_special_tuned_disabled() -> bool {
    LAB_SPECIAL_TUNED_DISABLED.load(Ordering::Acquire)
}

/// 设置实验室的场景特调总闸（true = 所有特调判定失效）
pub fn set_lab_special_tuned_disabled(disabled: bool) {
    LAB_SPECIAL_TUNED_DISABLED.store(disabled, Ordering::Release);
}

/// 返回当前应加载的配置文件路径：
/// - 命中 Chiri 目标 SoC 且存在处理器子目录 `config/{命中片段}/meta.yaml` 时，使用该文件
/// - 否则回退到默认 `config/meta.yaml`
///
/// 所有配置加载/热重载入口（main.rs 与 chiri 的 config_watcher）统一走这里，
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
        // "re:" 前缀本身含冒号：先摘掉前缀，再对剩余部分切 `模式列表:回退模式`。
        // 2026-09-18 修：此前直接对整行 splitn(3, ':')，前缀被当成匹配器、正则体被当成
        // 模式名——条目的包名成了字面量 "re"（永远不命中，B 站/方舟的台服、Mod 变体实际
        // 从未被接管），模式列表里还混进 `(?i)arknights` 这类假模式名，触发
        // 「白名单模式没有对应参数组」告警。正则体按 yaml 约定不含冒号。
        let (is_regex, body) = match line.strip_prefix("re:") {
            Some(rest) => (true, rest),
            None => (false, line),
        };
        let mut parts = body.splitn(3, ':');
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
        let (package, regex) = if is_regex {
            match regex::Regex::new(pkg) {
                Ok(re) => (format!("re:{pkg}"), Some(re)),
                Err(e) => {
                    log::warn!("special-tuned: invalid regex '{}' skipped: {}", pkg, e);
                    continue;
                }
            }
        } else {
            (pkg.to_string(), None)
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
/// 实验室（rhine）关闭全部场景特调期间恒返回 None（`special_tuned_mode` 与
/// `is_special_mode_allowed` 都由本函数派生，一并失效）。
pub fn special_tuned_entry(pkg: &str) -> Option<&'static SpecialTunedEntry> {
    if lab_special_tuned_disabled() {
        return None;
    }
    let list = special_tuned_entries();
    list.iter()
        .find(|e| e.regex.is_none() && e.package == pkg)
        .or_else(|| list.iter().find(|e| e.matches(pkg)))
}

/// 查询包名是否命中特调白名单，命中返回优先回退模式
pub fn special_tuned_mode(pkg: &str) -> Option<String> {
    special_tuned_entry(pkg).map(|e| e.fallback.clone())
}

/// 判断模式名是否为特调模式（任一白名单条目的 modes 列表中出现）。
/// 实验室关闭全部场景特调期间恒 false——chiri 主循环里所有 `is_special_mode(current_mode)`
/// 分支随之走普通模式路径，正在跑的特调模式会被当作普通模式收尾。
pub fn is_special_mode(mode: &str) -> bool {
    if lab_special_tuned_disabled() {
        return false;
    }
    special_tuned_entries()
        .iter()
        .any(|e| e.modes.iter().any(|m| m == mode))
}

/// 白名单里注册的全部特调模式名（精确 + 正则条目的 modes 并集，去重）。
/// 用途：Config 合并参数组时校验「注册了模式但没有参数组」的错配——那种情况
/// 会静默回退 akmode 段（游戏参数：headroom 1.15 + boost 亲和），在省电场景
/// 是反效果；拼写错/漏配必须在日志里暴露。
pub fn special_tuned_mode_names() -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for e in special_tuned_entries() {
        for m in &e.modes {
            if !out.iter().any(|x| x == m) {
                out.push(m.clone());
            }
        }
    }
    out
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

/// FAS 是否可用：总开关开启（meta.yaml fas_enabled）、白名单非空且至少一个应用配置解析成功。
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
// ***-example.yaml 参考文件不放本目录**（会随之嵌入二进制）：一律放 mdocs/。

// [embedded]
static CONFIG_DIR: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/module/config");

/// 按相对 module/config 的路径取嵌入文本（如 "8550/meta.yaml"），缺失返回 None。
/// 必需文件的存在性由 build.rs 编译期断言兜底，运行时不会静默空转。
pub(crate) fn embedded_config_file(rel: &str) -> Option<&'static str> {
    CONFIG_DIR.get_file(rel).and_then(|f| f.contents_utf8())
}

/// 嵌入的 meta.yaml（用户可修改字段的默认值）：按命中处理器取 {soc}/meta.yaml，
/// 未命中取 meta.yaml
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

/// 嵌入的 tuned_profiles.yaml（config/normal/）：特调参数组——缺省段 `akmode`
/// （游戏特调兼未注册模式的回退）+ `tuned_profiles` 段按模式名分派
pub fn embedded_tuned_profiles_str() -> &'static str {
    embedded_config_file("normal/tuned_profiles.yaml").unwrap_or_default()
}

/// 嵌入的 rhine-init.yaml（实验室模式定义）：只给守护进程读，不落盘也不对外暴露
/// （xtask 的 BIN_ONLY 会在打包时移除）。WebUI 的模式列表与文案是硬编码的。
pub fn embedded_rhine_init_str() -> &'static str {
    embedded_config_file("rhine-init.yaml").unwrap_or_default()
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

/// 磁盘 meta.yaml 的严格结构：**字段全部可选**（缺省 = 沿用二进制内嵌默认；见
/// ExternalMetaOverrides 的「None = 不变更」语义）、
/// 拒绝未知字段。**后加字段一律设计成可选**（serde default）：老文件缺行仍然合法，
/// 不会因为一次字段扩充就把用户全部设置判非法覆盖掉。
/// 出现即校验：类型不符 / 取值不在白名单 / 未知键 → 整文件判非法，由
/// sync_meta_snapshot 用二进制内嵌默认值整体覆盖修正（**缺字段不算非法**）。
/// **新增字段时四处必须同步**：本结构 + [`ExternalMetaOverrides`]、chiri::config::Meta
/// 与 Config::load 的合并、四个 meta.yaml 模板、WebUI 的 META_FIELDS/WRITABLE_FIELDS
/// （与布尔校验列表）——漏改一处会让用户写下的新键被判「未知字段」而整体重置。
// [external_meta]
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MetaYamlFile {
    name: Option<String>,
    author: Option<String>,
    language: Option<String>,
    loglevel: Option<String>,
    dev_record: Option<bool>,
    /// aff `@S` 每秒快照帧的 top-N 进程数（手改字段，WebUI 无开关），详见
    /// ExternalMetaOverrides 同名字段
    devimp_top_n: Option<usize>,
    fas_enabled: Option<bool>,
    scenemode_enabled: Option<bool>,
    /// 线程摆放总开关（affinity + core_ctl），详见 ExternalMetaOverrides
    thread_bind: Option<bool>,
    /// PowerBase 总开关（meta.yaml `powerbase_enabled`，缺省 false）。**必须与本结构
    /// 同步注册**：deny_unknown_fields 下 WebUI 直写的键若不在这里，整个 meta.yaml
    /// 会被判非法并整体重置（2026-09-22 死循环事故的根因——开关写了被抹、再写再抹）。
    powerbase_enabled: Option<bool>,
    /// 功耗口径开关（PowerAVG.chr），详见 ExternalMetaOverrides（缺省 false）
    power_avg: Option<bool>,
    /// 常驻状态通知开关，详见 ExternalMetaOverrides（缺省 true）
    notify: Option<bool>,
    /// 电池读数：OPlus 私有节点 / 双电芯 / 倍电压 / 倍电流 / 单位校准，缺省见结构注释
    oplus_chg: Option<bool>,
    oplus_dual_cell: Option<bool>,
    voltage_double: Option<bool>,
    current_double: Option<bool>,
    voltage_divisor: Option<f32>,
    current_divisor: Option<f32>,
    /// 旧键（曾把电压/电流校准合成一个）：仍接收，等价于只设电压校准
    unit_divisor: Option<f32>,
    /// 「不改」开关（模板不写）：详见 ExternalMetaOverrides
    nofix: Option<bool>,
    /// 耗电读数满量程 W（缺省 12）：详见 ExternalMetaOverrides
    power_max_w: Option<f32>,
}

/// 磁盘 meta.yaml 交给 Config::load 的覆盖值：**None = 文件里没写这个键 → 沿用内嵌默认**
/// （与 rhine 影响项同一「缺省 = 不变更」语义；字段全部可选，老文件/精简文件都合法）。
/// daemon 只消费其中 7 项（name/author 仅 WebUI 展示，直接读文件即可）。
#[derive(Debug, Clone, Default)]
pub struct ExternalMetaOverrides {
    pub loglevel: Option<String>,
    pub language: Option<String>,
    pub dev_record: Option<bool>,
    /// aff `@S` 每秒快照帧的 top-N 进程数（meta.yaml `devimp_top_n`，缺省 10）：
    /// 每秒按 util 降序落盘前 N 个进程；前台树与被管进程不受 N 截断、恒定落盘。
    /// 消费点：chiri Config::load 合并后由 `Meta::normalize` 钳到 1..=64
    /// （0/超限不合法会被 clamp，不判整个文件非法）。
    pub devimp_top_n: Option<usize>,
    pub fas_enabled: Option<bool>,
    pub scenemode_enabled: Option<bool>,
    /// 线程摆放总闸（meta.yaml `thread_bind`）：**实验室 frozen 专用机制**——用户侧
    /// 开关已移除，仅 frozen 模式写 false 交还线程亲和/绑核与 core_ctl。
    /// 与机型内嵌 feature.yaml 的两个子开关取「与」，见 chiri/config.rs::Config::load。
    pub thread_bind: Option<bool>,
    /// PowerBase 总开关（meta.yaml `powerbase_enabled`，缺省 false）：开启后原本由
    /// CLG 接管的亮屏日常场合改由 PowerBase 接管（以放电功耗为指标）。
    /// 消费点：chiri Config::load 合并后 set_powerbase_enabled 同步原子量。
    pub powerbase_enabled: Option<bool>,
    /// 功耗口径开关（PowerAVG.chr）：false（默认）= 参考值（旧值先乘 10 再与新值
    /// 按 10:1 加权递推，偏历史，含息屏样本）；true = 累计平均值（等权全史，
    /// **仅亮屏**放电样本）。两者都只在电池放电时取样。daemon 只在 ChiRi 的 1s 状态采样里
    /// 消费（非 ChiRi 无效）；写侧走「单次读-改-写」顶层行替换，见 WebUI contract/meta.ts。
    pub power_avg: Option<bool>,
    /// 常驻状态通知开关（meta.yaml `notify`，默认 true）：daemon 每 5s 把调度状态
    /// （前台包名 / 模式 / 家族 / 子模式 / 温度 / 功耗）写到系统通知栏的常驻通知，
    /// 见 src/notify.rs。false = 不投递，并**撤销已投递的通知**（daemon 自己还在跑，
    /// 有能力清理）。同样只在 ChiRi 的 1s 循环里消费（非 ChiRi 无效）。
    pub notify: Option<bool>,
    /// OPlus 私有电压/电流节点（`oplus_chg`，默认 false）：true = 优先读
    /// `/sys/class/oplus_chg/battery/bcc_parms`（下标 6 电芯电压0、8 电流、11 电芯电压1，
    /// 毫单位 mV/mA，随采样刷新），读不到才回退标准 power_supply 节点。仅 OPlus 机型有意义。
    pub oplus_chg: Option<bool>,
    /// OPlus 双电芯（`oplus_dual_cell`，默认 false）：私有节点按两节并联读——电压取
    /// 两节平均（下标 6 与 11）、电流 ×2（下标 8 为单节支路）。仅在 `oplus_chg` 打开时生效。
    pub oplus_dual_cell: Option<bool>,
    /// 倍电压（`voltage_double`，默认 false）：标准节点路径电压 ×2（双电芯机型上标准
    /// 节点只报单节值）。**与 `oplus_chg` 互斥**：私有开关打开时被强制关闭。
    pub voltage_double: Option<bool>,
    /// 倍电流（`current_double`，默认 false）：标准节点路径电流 ×2，互斥关系同上。
    pub current_double: Option<bool>,
    /// 电压校准除数（`voltage_divisor`，默认 1000000，须 > 0）：`节点原始值 ÷ 该值 = V`，
    /// 读取层不做换算。缺省 1000000 = 标准 Android ABI 的 µV 口径；OPlus 私有节点报 mV，
    /// 安装脚本检测到该节点时会写入 1000（见 customize.sh 的 [battery-detect]）。
    pub voltage_divisor: Option<f32>,
    /// 电流校准除数（`current_divisor`，默认 1000000，须 > 0）：`节点原始值 ÷ 该值 = 安培`。
    /// 口径同电压（标准节点 µA → 1000000；OPlus 私有节点 mA → 1000，安装脚本自动写入）。
    /// 与电压分开：节点的两个量未必同时错单位，分开才能单独校正（W = |A| × V 保持自洽）。
    pub current_divisor: Option<f32>,
    /// 「不改」开关（meta.yaml 字段 `nofix`，默认 false，且默认不写进配置）：
    /// true = 启动时跳过所有「二进制内容对外部文件的覆盖类操作」——webui 资产还原
    /// （webui_asset::restore_webroot）与 meta.yaml 快照自愈（sync_meta_snapshot）。
    /// 用户自担文件被篡改的风险；rhine 实验（用户主动开启）与 WebUI 写入不受影响。
    pub nofix: Option<bool>,
    /// 耗电读数满量程 W（meta.yaml `power_max_w`，缺省 12）：只影响 WebUI 状态页
    /// 仪表盘的进度换算，不参与任何调度决策。
    pub power_max_w: Option<f32>,
}

/// 读磁盘 meta.yaml（先经 sync_meta_snapshot 校验/纠正）。文件缺失或仍非法时返回
/// None，调用方沿用嵌入默认值——绝不 panic，也绝不让坏文件拖垮配置加载。
pub fn read_external_meta(path: &Path) -> Option<ExternalMetaOverrides> {
    let text = std::fs::read_to_string(path).ok()?;
    parse_disk_meta(&text)
}

/// 提前读「不改」开关（meta.yaml 可选字段 `nofix`）：必须在 sync_meta_snapshot 之前
/// 调用——晚了文件可能已被内嵌默认覆盖（该覆盖本身就是要跳过的操作之一）。
/// 文件缺失/非法时返回 false（照常自愈）。
pub fn read_nofix_flag(path: &Path) -> bool {
    read_external_meta(path)
        .and_then(|m| m.nofix)
        .unwrap_or(false)
}

// 「不改」进程级标志：main 启动期判定后置位，此后所有覆盖类操作入口（webui 资产
// 还原、meta/rules 快照自愈，含热重载路径）只读这个原子量——高频路径不许读磁盘，
// 与 FAS_ENABLED 同范式。
static NOFIX: AtomicBool = AtomicBool::new(false);

/// 记录启动期判定的 nofix 状态（在 read_nofix_flag 之后、任何覆盖类操作之前调用）
pub fn set_nofix(active: bool) {
    NOFIX.store(active, Ordering::SeqCst);
}

/// 是否跳过「二进制内容对外部文件的覆盖类操作」（webui 资产还原 / meta、rules 快照自愈）
pub fn nofix_active() -> bool {
    NOFIX.load(Ordering::SeqCst)
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
    // 出现即校验（缺省跳过）：name/author 非空
    if f.name.as_deref().is_some_and(|v| v.trim().is_empty())
        || f.author.as_deref().is_some_and(|v| v.trim().is_empty())
    {
        return None;
    }
    // 满量程：非有限/非正/超 200 视为写错，回退内嵌默认 12 —— 不判整个文件非法：
    // 单个数笔误不该连带丢掉用户其它设置（WebUI 写入侧另有 1..=200 校验）
    let power_max_w = f.power_max_w.map(|v| {
        if v.is_finite() && v > 0.0 && v <= 200.0 {
            v
        } else {
            crate::utils::default_power_max_w()
        }
    });
    // 单位校准：非有限/非正视为写错，回退内嵌默认（与 power_max_w 同口径，
    // 单个数笔误不判整个文件非法；WebUI 写入侧另有 > 0 校验）。
    // 旧键 `unit_divisor`（曾把电压/电流合一个）仍接收，作为电压校准的兜底值。
    let sane = |v: f32| {
        if v.is_finite() && v > 0.0 {
            v
        } else {
            crate::utils::DEFAULT_UNIT_DIVISOR
        }
    };
    let voltage_divisor = f.voltage_divisor.or(f.unit_divisor).map(sane);
    let current_divisor = f.current_divisor.map(sane);
    Some(ExternalMetaOverrides {
        loglevel: match f.loglevel.as_deref() {
            Some(v) => Some(sanitize_loglevel(v)?),
            None => None,
        },
        language: match f.language.as_deref() {
            Some(v) => Some(sanitize_language(v)?),
            None => None,
        },
        dev_record: f.dev_record,
        devimp_top_n: f.devimp_top_n,
        fas_enabled: f.fas_enabled,
        scenemode_enabled: f.scenemode_enabled,
        thread_bind: f.thread_bind,
        powerbase_enabled: f.powerbase_enabled,
        power_avg: f.power_avg,
        notify: f.notify,
        oplus_chg: f.oplus_chg,
        oplus_dual_cell: f.oplus_dual_cell,
        voltage_double: f.voltage_double,
        current_double: f.current_double,
        voltage_divisor,
        current_divisor,
        nofix: f.nofix,
        power_max_w,
    })
}

/// meta.yaml 快照自愈：main.rs 启动时与 chiri config_watcher **触发热重载前**调用。
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

/// rules.yaml 快照复制：把编译期嵌入的 rules.yaml 向外复制到模块根（与 meta.yaml
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
pub(crate) fn write_file_no_panic(path: &Path, bytes: &[u8]) -> bool {
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

/// meta.yaml 顶层行替换：只匹配缩进为 0 的 `键: 值` 行，保留键名大小写、分隔空白与
/// 行内注释。字段不存在返回 None，调用方据此放弃写入，绝不退化成整文件重排。
///
/// 为什么不直接 serde 反序列化再序列化整份：整文件重排会吃掉用户写的注释，而且
/// `MetaYamlFile` 带 deny_unknown_fields，漏掉一个新字段就会让下一次
/// sync_meta_snapshot 判非法、用内嵌默认把整个文件覆盖掉。
/// 口径与 WebUI `contract/meta.ts::replaceTopLevelField` 一致，两边不要各写一套。
pub(crate) fn replace_top_level_bool(content: &str, field: &str, value: bool) -> Option<String> {
    let want = field.to_ascii_lowercase();
    let mut lines: Vec<String> = content.split('\n').map(str::to_string).collect();
    for line in lines.iter_mut() {
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with('-') {
            continue;
        }
        // 只认顶层：有缩进的行（嵌套段）跳过
        if line.len() != trimmed.len() {
            continue;
        }
        let Some(colon) = trimmed.find(':') else {
            continue;
        };
        let key = trimmed[..colon].trim();
        if key.is_empty() || key.contains(char::is_whitespace) || key.contains('#') {
            continue;
        }
        if key.to_ascii_lowercase() != want {
            continue;
        }
        let rest = &trimmed[colon + 1..];
        let sep: String = rest.chars().take_while(|c| c.is_whitespace()).collect();
        let tail = &rest[sep.len()..];
        let comment = match tail.find(" #") {
            Some(i) => &tail[i..],
            None => "",
        };
        *line = format!("{key}:{sep}{value}{comment}");
        return Some(lines.join("\n"));
    }
    None
}

/// 一次写盘改掉 meta.yaml 的总开关（None = 该项不动；thread_bind 仅实验室使用）。
///
/// 返回 false 表示文件没有被改到期望状态（字段缺失 / 读写失败），调用方据此放弃
/// 本次实验室套用。多个开关必须一次写完：分两次写会触发两轮 config_watcher 热重载，
/// 中间态（只关了一半）会真的被 apply_system_tweaks 执行一遍。
pub(crate) fn rewrite_meta_toggles(
    path: &Path,
    fas: Option<bool>,
    scenemode: Option<bool>,
    thread_bind: Option<bool>,
) -> bool {
    if fas.is_none() && scenemode.is_none() && thread_bind.is_none() {
        return true;
    }
    let Ok(original) = std::fs::read_to_string(path) else {
        log::warn!("[Rhine] meta.yaml unreadable: {}", path.display());
        return false;
    };
    let mut text = original.clone();
    for (field, value) in [
        ("fas_enabled", fas),
        ("scenemode_enabled", scenemode),
        ("thread_bind", thread_bind),
    ] {
        let Some(v) = value else { continue };
        match replace_top_level_bool(&text, field, v) {
            Some(next) => text = next,
            None => {
                log::warn!("[Rhine] meta.yaml has no `{field}` field, give up writing");
                return false;
            }
        }
    }
    // 值本来就对：不写盘，省掉一次无谓的热重载
    if text == original {
        return true;
    }
    if !write_file_no_panic(path, text.as_bytes()) {
        log::warn!("[Rhine] meta.yaml write failed: {}", path.display());
        return false;
    }
    true
}
