//! config.rs: [meta] [clg_config] [clg_normalize] [mode_io] [toggles] [ak_config] [thermal_config] [affinity_config] [corectl_config] [config_root] [config_impl]

use serde::Deserialize;

use crate::fluent_args;
use crate::i18n::t_with_args;

/// 全局元信息段（对应 meta.yaml 顶层字段）
// [meta]
#[derive(Debug, Deserialize, Default)]
pub struct Meta {
    /// 日志级别：DEBUG / INFO / WARN / ERROR，热重载时即时生效
    #[serde(default = "default_loglevel", alias = "Loglevel")]
    pub loglevel: String,

    /// 守护进程日志语言：en / zh，改动后自动加载对应 .ftl
    #[serde(default = "default_language", alias = "Language")]
    pub language: String,

    /// 开发记录开关：true 时向 devimp/devimp_<启动时间戳>.log 写入按核调度
    /// 诊断日志（tick/snap/place/aff/core/event），供离线分析改善调度。
    /// meta 段中允许外部修改的字段之一（WebUI 开关，热重载生效）。
    #[serde(default, alias = "DevRecord")]
    pub dev_record: bool,

    /// FAS 帧感知调度总开关：关闭后 fas_available() 恒为 false（determine_mode
    /// 不再产生 fas 模式、FAS 监测线程不启动、运行中实例立即注销）。
    /// meta 段允许外部修改的字段之一（手改 meta.yaml 后热重载生效）。
    #[serde(default = "crate::utils::default_true", alias = "FasEnabled")]
    pub fas_enabled: bool,

    /// 息屏场景模式总开关：关闭后息屏不进入 scenemode（已激活的下个 tick 退出
    /// 并恢复 affinity/core_ctl 快照 + doze 配置）。
    /// meta 段允许外部修改的字段之一（手改 meta.yaml 后热重载生效）。
    #[serde(default = "crate::utils::default_true", alias = "ScenemodeEnabled")]
    pub scenemode_enabled: bool,
}

// Meta 缺省值：config.yaml 省略该字段时回退到此处
fn default_loglevel() -> String {
    "INFO".to_string()
}
fn default_language() -> String {
    "en".to_string()
}

// [clg_config]
// CPU Load Governor 配置
/// CLG（CPU Load Governor）调频参数。
/// 所有性能比/阈值均为 0.0~1.0 的相对值，`perf_init/perf_floor/perf_ceil` 再换算成最近频率档位。
#[derive(Debug, Deserialize, Clone)]
pub struct CpuLoadGovernorConfig {
    /// CLG 总开关：false 时不接管 CPU，也不做任何频率写入
    #[serde(default = "crate::utils::default_true")]
    pub enabled: bool,
    /// 升频阈值：cluster 最大 util >= 此值时按全速升频（配合 headroom_factor 放大）
    #[serde(default = "d_clg_up_thresh")]
    pub up_threshold: f32,
    /// 降频阈值：util 跌破此值进入降频区间
    #[serde(default = "d_clg_down_thresh")]
    pub down_threshold: f32,
    /// 升频平滑系数：每 tick 目标性能只逼近该比例，越大响应越快，越小越省电
    #[serde(default = "d_clg_smooth_up")]
    pub smoothing_up: f32,
    /// 降频速率限制：必须连续满足 down_wait >= 该 tick 数才执行一次降频。
    /// 降频本身为“直接降频”一步到位（不做平滑渐变），该值仅作防抖。
    #[serde(default = "d_clg_down_rate")]
    pub down_rate_limit_ticks: u32,
    /// 升频速率限制：必须连续满足 up_wait >= 该 tick 数才执行一次升频
    #[serde(default = "d_clg_up_rate")]
    pub up_rate_limit_ticks: u32,
    /// 性能余量：util 达 up_threshold 后，目标性能放大到此系数（>=1，给突发留余量）
    #[serde(default = "d_clg_headroom")]
    pub headroom_factor: f32,
    /// headroom 在 up_threshold 附近的过渡带宽度：从 up_threshold - headroom_ramp
    /// 到 up_threshold 线性由 1.0 渐变至 headroom_factor，避免阶跃导致振荡
    #[serde(default = "d_clg_headroom_ramp")]
    pub headroom_ramp: f32,
    /// 性能下限：目标性能永不低于此值（锁频下限的百分比）
    #[serde(default = "d_clg_floor")]
    pub perf_floor: f32,
    /// 性能上限：目标性能永不超过此值（锁频上限的百分比）
    #[serde(default = "d_clg_ceil")]
    pub perf_ceil: f32,
    /// 接管瞬间的初始性能：init_policies 时先把频率锁到该档位，避免从 0 爬升
    #[serde(default = "d_clg_init")]
    pub perf_init: f32,
    /// 升频快速通道判定：target_perf 超过 current_perf 的幅度大于此值时直接快速升频
    #[serde(default = "d_clg_up_jump")]
    pub up_jump_threshold: f32,
    /// 低负载升频（负载未达 up_threshold 时）对 smoothing_up 的缩放系数
    #[serde(default = "d_clg_slow_up_scale")]
    pub slow_up_scale: f32,
    /// 极低负载阈值：util 低于此值时跳过降频防抖、立即直接降频
    #[serde(default = "d_clg_down_fast_thresh")]
    pub down_fast_threshold: f32,
    /// 尖峰抑制：单 tick util 跳升超过此值时，其增量按 spike_decay 比例衰减，
    /// 避免孤立瞬时尖峰（如单核 0↔100%）瞬间拉满 perf
    #[serde(default = "d_clg_spike_jump")]
    pub spike_jump_threshold: f32,
    /// 尖峰增量保留比例（0.0=完全抑制，1.0=不抑制）
    #[serde(default = "d_clg_spike_decay")]
    pub spike_decay: f32,
    /// 触摸升频总开关：true 时触摸屏幕将把大核频率提前抬高一档（大核区间随命中 SoC
    /// 变化，见 common::chiri_core_ranges），减少操作卡顿
    #[serde(default = "crate::utils::default_true")]
    pub touch_boost_enabled: bool,
    /// 触摸升频保持时长（ms）：触摸后窗口期内大核锁定在抬高档位，窗口结束回落到负载调度
    #[serde(default = "d_clg_touch_boost_ms")]
    pub touch_boost_ms: u64,
    /// 触摸升频抬高的频率档数：在可用频率表中向上移动的档数（1 档即一个频率步进）
    #[serde(default = "d_clg_touch_boost_tiers")]
    pub touch_boost_tiers: u32,
}

// CLG 各参数缺省值：config.yaml 省略字段时回退到此处（与 normalize 的兜底默认一致）
fn d_clg_up_thresh() -> f32 {
    0.80
}
fn d_clg_down_thresh() -> f32 {
    0.50
}
fn d_clg_smooth_up() -> f32 {
    0.60
}
fn d_clg_down_rate() -> u32 {
    3
}
fn d_clg_up_rate() -> u32 {
    2
}
fn d_clg_headroom() -> f32 {
    1.25
}
fn d_clg_headroom_ramp() -> f32 {
    0.15
}
fn d_clg_floor() -> f32 {
    0.15
}
fn d_clg_ceil() -> f32 {
    1.0
}
fn d_clg_init() -> f32 {
    0.50
}
fn d_clg_up_jump() -> f32 {
    0.35
}
fn d_clg_slow_up_scale() -> f32 {
    0.02
}
fn d_clg_down_fast_thresh() -> f32 {
    0.10
}
fn d_clg_spike_jump() -> f32 {
    0.35
}
fn d_clg_spike_decay() -> f32 {
    0.30
}
fn d_clg_touch_boost_ms() -> u64 {
    400
}
fn d_clg_touch_boost_tiers() -> u32 {
    1
}

impl Default for CpuLoadGovernorConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            up_threshold: d_clg_up_thresh(),
            down_threshold: d_clg_down_thresh(),
            smoothing_up: d_clg_smooth_up(),
            down_rate_limit_ticks: d_clg_down_rate(),
            up_rate_limit_ticks: d_clg_up_rate(),
            headroom_factor: d_clg_headroom(),
            headroom_ramp: d_clg_headroom_ramp(),
            perf_floor: d_clg_floor(),
            perf_ceil: d_clg_ceil(),
            perf_init: d_clg_init(),
            up_jump_threshold: d_clg_up_jump(),
            slow_up_scale: d_clg_slow_up_scale(),
            down_fast_threshold: d_clg_down_fast_thresh(),
            spike_jump_threshold: d_clg_spike_jump(),
            spike_decay: d_clg_spike_decay(),
            touch_boost_enabled: true,
            touch_boost_ms: d_clg_touch_boost_ms(),
            touch_boost_tiers: d_clg_touch_boost_tiers(),
        }
    }
}

// [clg_normalize]
impl CpuLoadGovernorConfig {
    /// 校验并规范化配置：
    /// - 非有限值（NaN/±Inf，如 YAML 溢出值）回退默认，防止污染控制链
    /// - 阈值/系数限制在合理区间
    /// - floor/ceil/init 交叉约束，保证 f32::clamp 永不 panic
    pub fn normalize(&mut self) {
        if !self.up_threshold.is_finite() {
            self.up_threshold = d_clg_up_thresh();
        }
        if !self.down_threshold.is_finite() {
            self.down_threshold = d_clg_down_thresh();
        }
        if !self.smoothing_up.is_finite() {
            self.smoothing_up = d_clg_smooth_up();
        }
        if !self.headroom_factor.is_finite() {
            self.headroom_factor = d_clg_headroom();
        }
        if !self.headroom_ramp.is_finite() {
            self.headroom_ramp = d_clg_headroom_ramp();
        }
        if !self.perf_floor.is_finite() {
            self.perf_floor = d_clg_floor();
        }
        if !self.perf_ceil.is_finite() {
            self.perf_ceil = d_clg_ceil();
        }
        if !self.perf_init.is_finite() {
            self.perf_init = d_clg_init();
        }
        if !self.up_jump_threshold.is_finite() {
            self.up_jump_threshold = d_clg_up_jump();
        }
        if !self.slow_up_scale.is_finite() {
            self.slow_up_scale = d_clg_slow_up_scale();
        }
        if !self.down_fast_threshold.is_finite() {
            self.down_fast_threshold = d_clg_down_fast_thresh();
        }
        if !self.spike_jump_threshold.is_finite() {
            self.spike_jump_threshold = d_clg_spike_jump();
        }
        if !self.spike_decay.is_finite() {
            self.spike_decay = d_clg_spike_decay();
        }

        // 区间限制（语义约束）
        self.up_threshold = self.up_threshold.clamp(0.0, 1.0);
        self.down_threshold = self.down_threshold.clamp(0.0, 1.0);
        // 滞回语义：降频阈值不得高于升频阈值
        if self.down_threshold > self.up_threshold {
            self.down_threshold = self.up_threshold;
        }
        self.smoothing_up = self.smoothing_up.clamp(0.0, 1.0);
        self.slow_up_scale = self.slow_up_scale.clamp(0.0, 1.0);
        self.up_jump_threshold = self.up_jump_threshold.clamp(0.0, 1.0);
        self.down_fast_threshold = self.down_fast_threshold.clamp(0.0, 1.0);
        self.spike_jump_threshold = self.spike_jump_threshold.clamp(0.0, 1.0);
        self.spike_decay = self.spike_decay.clamp(0.0, 1.0);
        self.headroom_ramp = self.headroom_ramp.clamp(0.0, 1.0);
        // headroom 语义 >= 1（余量放大）
        self.headroom_factor = self.headroom_factor.clamp(1.0, 3.0);

        // 触摸升频参数限制：保持时长限制在 1s 内防误配，档数限制在 8 档内
        self.touch_boost_ms = self.touch_boost_ms.clamp(1, 1000);
        self.touch_boost_tiers = self.touch_boost_tiers.min(8);
        // 触摸升频关闭时（touch_boost_enabled=false）时长置 0，避免窗口逻辑误判
        if !self.touch_boost_enabled {
            self.touch_boost_ms = 0;
        }

        // 交叉约束（顺序保证 clamp 边界合法）
        if self.perf_floor > self.perf_ceil {
            self.perf_floor = self.perf_ceil;
        }
        self.perf_floor = self.perf_floor.clamp(0.0, 1.0);
        self.perf_ceil = self.perf_ceil.clamp(0.0, 1.0);
        if self.perf_floor > self.perf_ceil {
            self.perf_floor = self.perf_ceil;
        }
        self.perf_init = self.perf_init.clamp(self.perf_floor, self.perf_ceil);
    }
}

// [mode_io]
// 核心模式与杂项配置
/// 单一性能模式的配置集合（config.yaml 中 powersave / balance / performance / fast 之一）
#[derive(Debug, Deserialize, Default, Clone)]
pub struct Mode {
    /// 该模式下的 CLG 调频参数
    #[serde(default, alias = "CpuLoadGovernor")]
    pub cpu_load_governor: CpuLoadGovernorConfig,
}

/// IO 优化段（对应 config.yaml `IO_Settings`），值均为写入 /sys/block/*/queue 的字符串
#[derive(Debug, Deserialize, Clone)]
pub struct IOSettings {
    /// I/O 调度器名（如 none / mq-deadline / cfq），空字符串则跳过不写
    #[serde(default, rename = "Scheduler")]
    pub scheduler: String,
    /// 预读大小（kB），写入 queue/read_ahead_kb
    #[serde(default = "default_read_ahead_kb")]
    pub read_ahead_kb: String,
    /// 请求合并策略：0=关闭，1=仅简单合并，2=完全合并
    #[serde(default = "default_nomerges")]
    pub nomerges: String,
    /// IO 统计开关：0=关闭，1=开启（写入 queue/iostats）
    #[serde(default = "default_iostats")]
    pub iostats: String,
}

impl Default for IOSettings {
    fn default() -> Self {
        Self {
            scheduler: String::new(),
            read_ahead_kb: default_read_ahead_kb(),
            nomerges: default_nomerges(),
            iostats: default_iostats(),
        }
    }
}

// IO_Settings 缺省值
fn default_read_ahead_kb() -> String {
    "128".to_string()
}
fn default_nomerges() -> String {
    "2".to_string()
}
fn default_iostats() -> String {
    "0".to_string()
}

/// cpuidle 段（对应 config.yaml `CpuIdle`）
#[derive(Debug, Deserialize, Default)]
// [toggles]
#[serde(rename_all = "snake_case")]
pub struct CpuIdle {
    /// 要切换到的 cpuidle 调度器名（写入 /sys/devices/system/cpu/cpuidle/current_governor）
    pub current_governor: String,
}

/// 功能总开关段（对应 config.yaml `Function`）
#[derive(Debug, Deserialize, Default)]
pub struct FunctionToggles {
    /// 是否应用 cpuidle governor 切换
    #[serde(rename = "CpuIdleScalingGovernor")]
    pub cpu_idle_scaling_governor: bool,
    /// 是否应用 IO 优化（调度器/预读/合并/统计）
    #[serde(rename = "IOOptimization")]
    pub io_optimization: bool,
}

/// 明日方舟特调（akmode）配置，跟 CLG 完全没关系。
/// **无档位**：不做模式分档，按各核心组实时负载直接移动 scaling_max_freq 上限
/// （FAS 式连续控制，替代原四档 core-count 阈值——后者在「少数线程高占用」负载
/// （如明日方舟资源校验）下升频条件永远凑不齐、降频条件持续满足，max 单边下探）。
/// 机制：激活时统一内核调速器为 schedutil、min 压到硬件最低；之后每个负载 tick
/// 计算目标上限比例 = clamp(组内最大核心占用率 × headroom, perf_floor, 1.0)：
///   升：目标 > 当前上限 + hysteresis → 立即上调（schedutil 在新上限内自由取频）；
///   降：目标 < 当前上限 − hysteresis 且持续 down_hold_ms → 上限收到目标档位。
// [ak_config]
#[derive(Debug, Deserialize, Clone)]
pub struct SpecialTunedConfig {
    /// 负载放大系数：目标上限 = 组内最大核心占用率 × headroom，留出升频余量
    #[serde(default = "d_ak_headroom")]
    pub headroom: f32,
    /// 目标上限比例下限（0 = 空闲组上限可收到硬件最低频）
    #[serde(default = "d_ak_perf_floor")]
    pub perf_floor: f32,
    /// 变更死区（比例）：目标与当前上限差值不超过该值时不写，防写抖动
    #[serde(default = "d_ak_hysteresis")]
    pub hysteresis: f32,
    /// 降频保持（ms）：目标持续低于当前上限该时长才真正下调，防负载抖动来回改写
    #[serde(default = "d_ak_down_hold_ms")]
    pub down_hold_ms: u64,
}

// SpecialTunedConfig 缺省值：akmode.yaml 没写的字段回退到这里
fn d_ak_headroom() -> f32 {
    1.15
}
fn d_ak_perf_floor() -> f32 {
    0.0
}
fn d_ak_hysteresis() -> f32 {
    0.03
}
fn d_ak_down_hold_ms() -> u64 {
    100
}

impl Default for SpecialTunedConfig {
    fn default() -> Self {
        Self {
            headroom: d_ak_headroom(),
            perf_floor: d_ak_perf_floor(),
            hysteresis: d_ak_hysteresis(),
            down_hold_ms: d_ak_down_hold_ms(),
        }
    }
}

impl SpecialTunedConfig {
    /// 校验配置：参数钳制在合理范围，非有限值回退默认
    pub fn normalize(&mut self) {
        if !self.headroom.is_finite() {
            self.headroom = d_ak_headroom();
        }
        self.headroom = self.headroom.clamp(1.0, 2.0);
        if !self.perf_floor.is_finite() {
            self.perf_floor = d_ak_perf_floor();
        }
        self.perf_floor = self.perf_floor.clamp(0.0, 0.5);
        if !self.hysteresis.is_finite() {
            self.hysteresis = d_ak_hysteresis();
        }
        self.hysteresis = self.hysteresis.clamp(0.0, 0.2);
        self.down_hold_ms = self.down_hold_ms.min(5_000);
    }
}

// [thermal_config]
// 热保护配置

/// 热保护配置（config.yaml `Thermal` 段）。
///
/// 双温度源取较小值：
/// - 电池温度是主参考——手机壳体发热由电池主导，温升慢但持续，不像 CPU 瞬间飙高又回落
/// - CPU 温度仅在极端情况参与——大型游戏里 CPU 温度由内核 95°C 温控兜底，
///   软件层重复压制没意义，阈值设得很高（75/85°C）只防内核兜不住的极端场景
///
/// 豁免档 free_above：当前性能上限已高于豁免档时不钳制。
/// 意味着持续高负载可以冲到硬件最高频——不挡性能的路，只在中低负载区间积热时压一压。
///
/// 仅对 CLG 接管模式生效（powersave/balance/performance/doze/scenemode）。
/// fast/akmode 走自己的路径，不受影响。
#[derive(Debug, Deserialize, Clone)]
pub struct ThermalGuardConfig {
    /// false = 完全不采温、不压制
    #[serde(default = "crate::utils::default_true")]
    pub enabled: bool,
    /// 电池软限（°C）：默认 41°C，手机壳体开始发烫的临界点
    #[serde(default = "d_batt_soft_temp")]
    pub batt_soft_temp_c: f32,
    /// 电池硬限（°C）：默认 45°C，手握明显发烫
    #[serde(default = "d_batt_hard_temp")]
    pub batt_hard_temp_c: f32,
    /// CPU 软限（°C）：默认 75°C，一般游戏不会到这里（内核 95°C 才兜底）
    #[serde(default = "d_cpu_soft_temp")]
    pub cpu_soft_temp_c: f32,
    /// CPU 硬限（°C）：默认 85°C，极端情况才触发
    #[serde(default = "d_cpu_hard_temp")]
    pub cpu_hard_temp_c: f32,
    /// 软限触发后性能上限压到这个比例（0..1）
    #[serde(default = "d_thermal_soft_cap")]
    pub soft_perf_cap: f32,
    /// 硬限触发后性能上限压到这个比例，必须 <= soft_perf_cap
    #[serde(default = "d_thermal_hard_cap")]
    pub hard_perf_cap: f32,
    /// 豁免档（0..1）：当前性能比已超过此值时不钳制。默认 0.80，
    /// 意味着 sustained load 能冲到 80%+ 硬件频率，只在中低负载积热时压住
    #[serde(default = "d_thermal_free_above")]
    pub free_above: f32,
    /// 回滞（°C）：温度降到 软限 - hysteresis 以下才解除压制。
    /// 设太小会在阈值附近反复触发/解除，频率抖动
    #[serde(default = "d_thermal_hysteresis")]
    pub hysteresis_c: f32,
}

// Thermal 缺省值：config.yaml 省略该段时回退到此处
fn d_batt_soft_temp() -> f32 {
    41.0
}
fn d_batt_hard_temp() -> f32 {
    45.0
}
fn d_cpu_soft_temp() -> f32 {
    90.0
}
fn d_cpu_hard_temp() -> f32 {
    95.0
}
fn d_thermal_soft_cap() -> f32 {
    0.70
}
fn d_thermal_hard_cap() -> f32 {
    0.40
}
fn d_thermal_free_above() -> f32 {
    0.80
}
fn d_thermal_hysteresis() -> f32 {
    3.0
}

impl Default for ThermalGuardConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            batt_soft_temp_c: d_batt_soft_temp(),
            batt_hard_temp_c: d_batt_hard_temp(),
            cpu_soft_temp_c: d_cpu_soft_temp(),
            cpu_hard_temp_c: d_cpu_hard_temp(),
            soft_perf_cap: d_thermal_soft_cap(),
            hard_perf_cap: d_thermal_hard_cap(),
            free_above: d_thermal_free_above(),
            hysteresis_c: d_thermal_hysteresis(),
        }
    }
}

impl ThermalGuardConfig {
    /// 校验并规范化：非有限值回退默认；保证各传感器 soft < hard、hard_cap <= soft_cap、
    /// 豁免档 >= soft_perf_cap（否则压制完全失效），防止误配。
    pub fn normalize(&mut self) {
        // 非有限值（NaN/±Inf，如 YAML 溢出值）回退默认
        if !self.batt_soft_temp_c.is_finite() {
            self.batt_soft_temp_c = d_batt_soft_temp();
        }
        if !self.batt_hard_temp_c.is_finite() {
            self.batt_hard_temp_c = d_batt_hard_temp();
        }
        if !self.cpu_soft_temp_c.is_finite() {
            self.cpu_soft_temp_c = d_cpu_soft_temp();
        }
        if !self.cpu_hard_temp_c.is_finite() {
            self.cpu_hard_temp_c = d_cpu_hard_temp();
        }
        if !self.soft_perf_cap.is_finite() {
            self.soft_perf_cap = d_thermal_soft_cap();
        }
        if !self.hard_perf_cap.is_finite() {
            self.hard_perf_cap = d_thermal_hard_cap();
        }
        if !self.free_above.is_finite() {
            self.free_above = d_thermal_free_above();
        }
        if !self.hysteresis_c.is_finite() {
            self.hysteresis_c = d_thermal_hysteresis();
        }
        // 电池阈值（主参考）：软限限制在合理区间，硬限必须高于软限
        self.batt_soft_temp_c = self.batt_soft_temp_c.clamp(25.0, 60.0);
        if self.batt_hard_temp_c <= self.batt_soft_temp_c {
            self.batt_hard_temp_c = (self.batt_soft_temp_c + 4.0).min(65.0);
        }
        // CPU 阈值（极端参考）：软限限制在合理区间，硬限必须高于软限
        self.cpu_soft_temp_c = self.cpu_soft_temp_c.clamp(50.0, 95.0);
        if self.cpu_hard_temp_c <= self.cpu_soft_temp_c {
            self.cpu_hard_temp_c = (self.cpu_soft_temp_c + 10.0).min(100.0);
        }
        // 比例限制在 0..1，硬限压制不高于软限（温度更高反而压得更松是误配）
        self.soft_perf_cap = self.soft_perf_cap.clamp(0.0, 1.0);
        self.hard_perf_cap = self.hard_perf_cap.clamp(0.0, 1.0);
        if self.hard_perf_cap > self.soft_perf_cap {
            self.hard_perf_cap = self.soft_perf_cap;
        }
        // 豁免档低于软限压制值时压制会被完全绕过：豁免档至少抬到 soft_perf_cap
        self.free_above = self.free_above.clamp(0.0, 1.0);
        if self.free_above < self.soft_perf_cap {
            self.free_above = self.soft_perf_cap;
        }
        self.hysteresis_c = self.hysteresis_c.clamp(0.0, 20.0);
    }
}

// [affinity_config]
// CPU 亲和 / core_ctl 配置

/// CPU 亲和与线程迁移配置（config.yaml `Affinity` 段）。
/// boost 模式（performance/fast/特调）下由 AffinityManager 应用：
/// top-app/foreground cpuset 收窄到大核+超大核、后台分组压小核、
/// 可选 uclamp.min 抬前台利用率下限、可选前台线程 sched_setaffinity 迁移。
/// normal/doze 下 top-app 恢复系统布局，后台保持压小核。配置热重载即时生效。
#[derive(Debug, Deserialize, Clone)]
pub struct AffinityConfig {
    /// 总开关：false 时全量恢复系统布局，不做任何写入
    #[serde(default = "crate::utils::default_true")]
    pub enabled: bool,
    /// boost 模式下 top-app 的 cpu.uclamp.min 百分比（0 = 不启用）。
    /// uclamp.min 会让 schedutil 独立于 CLG 抬频，与动态上限语义叠加，默认关闭
    #[serde(default = "d_aff_uclamp_min")]
    pub top_app_uclamp_min_pct: u32,
    /// boost 模式下 top-app 的 cpu.uclamp.max 百分比（0 = 不启用）。
    /// 任务级钳制：只限 top-app 的 util 需求，schedutil 频率随之回落、EAS 能量计算
    /// 同步感知；空闲间隙微秒级降到地板，比 scaling_max_freq 硬顶更贴合 EAS。
    /// 按机型在 yaml 配置；运行时内核 < 5.3 / 节点缺失 / 写入回读无效时自动纠正关闭
    #[serde(default = "d_aff_uclamp_max")]
    pub top_app_uclamp_max_pct: u32,
    /// boost 模式下把前台进程全部线程迁移（sched_setaffinity）到大核+超大核；
    /// 退出 boost 恢复全核
    #[serde(default = "crate::utils::default_true")]
    pub pin_foreground_threads: bool,
}

fn d_aff_uclamp_min() -> u32 {
    0
}
fn d_aff_uclamp_max() -> u32 {
    0
}

impl Default for AffinityConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            top_app_uclamp_min_pct: d_aff_uclamp_min(),
            top_app_uclamp_max_pct: d_aff_uclamp_max(),
            pin_foreground_threads: true,
        }
    }
}

impl AffinityConfig {
    /// 校验：uclamp 百分比限制在 0..=100
    pub fn normalize(&mut self) {
        self.top_app_uclamp_min_pct = self.top_app_uclamp_min_pct.clamp(0, 100);
        self.top_app_uclamp_max_pct = self.top_app_uclamp_max_pct.clamp(0, 100);
    }
}

/// core_ctl（厂商核心在线控制器）接管配置（config.yaml `CoreCtl` 段）。
/// boost 模式下把各 cluster 的 min_cpus 抬到全组常在线，防厂商热插拔与
/// ChiRi 调频打架；退出 boost 恢复快照。仅动 min_cpus。
// [corectl_config]
#[derive(Debug, Deserialize, Clone)]
pub struct CoreCtlConfig {
    /// 总开关：false 时不写任何 core_ctl 节点
    #[serde(default = "crate::utils::default_true")]
    pub enabled: bool,
    /// scenemode 离线核（息屏深度省电）：进入 scenemode 后仅 prime（超大核）
    /// 整簇断电消除空转漏电流，小核 + 大核全开常驻（频率上限由 scenemode CLG
    /// 配置压制）；编号最大的小核独占给调度服务（从业务 cpuset 组移除 + 自身
    /// 线程移入根组 + 自钉）。亮屏/退出 scenemode 按快照恢复。逐核回读验证，
    /// 内核拒绝的核自动跳过。
    /// 按机型配置：8550/8475 开，8998（4.4 老内核热插拔质量未知）默认关。
    /// 注意：与 boost 互斥——scenemode 下 boost 被抑制，防止厂商 core_ctl
    /// 按 min_cpus 把下线的核又拉回来。
    #[serde(default = "crate::utils::default_true")]
    pub scenemode_offline: bool,
}

impl Default for CoreCtlConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            scenemode_offline: true,
        }
    }
}

/// 顶层配置（feature.yaml 调优段 + meta.yaml 抬头）
// [config_root]
#[derive(Debug, Deserialize, Default)]
pub struct Config {
    /// 全局元信息：日志级别 / 语言
    #[serde(default, alias = "Meta")]
    pub meta: Meta,
    /// 功能开关
    #[serde(default)]
    pub function: FunctionToggles,
    /// IO 优化参数
    #[serde(default, rename = "IO_Settings")]
    pub io_settings: IOSettings,
    /// cpuidle 参数
    #[serde(default, rename = "CpuIdle")]
    pub cpu_idle: CpuIdle,

    // 按场景划分的性能模式：键名即 mode，get_mode 按名检索
    #[serde(default)]
    pub powersave: Mode,
    #[serde(default)]
    pub balance: Mode,
    #[serde(default)]
    pub performance: Mode,
    #[serde(default)]
    pub fast: Mode,
    /// 息屏场景模式（scenemode）：屏幕熄灭超过 `scene_mode_delay_secs` 秒后切换到的
    /// 低功耗配置（压低频率上限、禁止主动升频），亮屏后恢复原模式。
    /// 未定义时回退 CLG 默认参数（兜底，通常 8550 config.yaml 会显式配置）。
    #[serde(default)]
    pub scenemode: Mode,
    /// 息屏进入 scenemode 的延迟（秒）：默认 300s（5 分钟），YAML 可覆盖
    #[serde(default = "default_scene_mode_delay_secs")]
    pub scene_mode_delay_secs: u64,

    /// 热保护配置：按 CPU 温度动态压低 CLG 性能上限（采样在 scheduler_ipc 线程）。
    /// 省略该段时用代码默认值（enabled=true，软限 60°C/硬限 70°C）。
    #[serde(default, rename = "Thermal")]
    pub thermal: ThermalGuardConfig,

    /// 明日方舟特调（akmode）独立调频配置：来自嵌入的 config/normal/akmode.yaml（编译期
    /// 打包，非处理器绑定；原 {soc}/akmode.yaml 方案已随 4cb4d97 重构移除），与 CLG 完全解耦。
    /// 前台为白名单应用时由 AkmodeGovernor 接管，参数不再走 CLG。
    #[serde(default)]
    pub akmode: SpecialTunedConfig,

    /// CPU 亲和与线程迁移控制（cpuset / cpuctl uclamp / sched_setaffinity）
    #[serde(default, rename = "Affinity")]
    pub affinity: AffinityConfig,

    /// core_ctl 核心在线接管（boost 模式保持大核常在线）
    #[serde(default, rename = "CoreCtl")]
    pub core_ctl: CoreCtlConfig,
}

/// scenemode 延迟缺省值：5 分钟
fn default_scene_mode_delay_secs() -> u64 {
    300
}

// [config_impl]
impl Config {
    /// 加载生效配置：feature 段以嵌入 feature.yaml 为基准（磁盘不落盘，防篡改），
    /// meta 以嵌入 meta.yaml 为默认值（common::embedded_meta_defaults），再被磁盘
    /// meta.yaml 覆盖（sync_meta_snapshot 已先行校验/纠正；缺失或非法时沿用嵌入默认）。
    /// `path` 为生效 meta.yaml 路径（common::get_config_path()）。
    /// 加载后合并嵌入的 akmode/scenemode 段，并把功能总开关同步到进程级原子标志
    /// （fas_available / scenemode 判定读取）。
    pub fn load(path: &str) -> anyhow::Result<Self> {
        let mut config: Config = serde_yaml::from_str(crate::common::embedded_feature_str())?;
        let d = crate::common::embedded_meta_defaults();
        config.meta.loglevel = d.loglevel;
        config.meta.language = d.language;
        config.meta.dev_record = d.dev_record;
        config.meta.fas_enabled = d.fas_enabled;
        config.meta.scenemode_enabled = d.scenemode_enabled;
        if let Some(m) = crate::common::read_external_meta(std::path::Path::new(path)) {
            config.meta.loglevel = m.loglevel;
            config.meta.language = m.language;
            config.meta.dev_record = m.dev_record;
            config.meta.fas_enabled = m.fas_enabled;
            config.meta.scenemode_enabled = m.scenemode_enabled;
        }
        // 功能总开关同步到进程级原子标志（覆盖启动 + config_watcher 热重载两条路径）
        crate::common::set_fas_enabled(config.meta.fas_enabled);
        crate::common::set_scenemode_enabled(config.meta.scenemode_enabled);
        config.merge_akmode();
        config.merge_scenemode();
        config.thermal.normalize();
        config.affinity.normalize();
        Ok(config)
    }

    /// 合并嵌入的特调配置（akmode.yaml，编译期打包进二进制）。
    /// 嵌入内容随版本发布、始终存在；仅当嵌入 YAML 意外损坏时置特调不可用。
    fn merge_akmode(&mut self) {
        match serde_yaml::from_str::<Config>(crate::common::embedded_akmode_str()) {
            Ok(special) => {
                self.akmode = special.akmode;
                self.akmode.normalize();
                crate::common::set_akmode_available(true);
                log::debug!(
                    "{}",
                    t_with_args(
                        "config-special-merged",
                        &fluent_args!("path" => "<embedded>".to_string())
                    )
                );
            }
            Err(e) => {
                log::warn!(
                    "{}",
                    t_with_args(
                        "config-special-parse-failed",
                        &fluent_args!("path" => "<embedded>".to_string(), "error" => e.to_string())
                    )
                );
                crate::common::set_akmode_available(false);
            }
        }
    }

    /// 合并嵌入的 scenemode 配置。只反序列化 scenemode 段（先解析成 Value 再提取）：
    /// 段缺失时保持 config.yaml 已配置的值，而不是用默认值覆盖。
    fn merge_scenemode(&mut self) {
        let scene_value = match serde_yaml::from_str::<serde_yaml::Value>(
            crate::common::embedded_scenemode_str(),
        ) {
            Ok(value) => match value.get("scenemode").cloned() {
                Some(v) => v,
                None => return, // 嵌入文件无 scenemode 段：保持当前已配置的 scenemode
            },
            Err(e) => {
                log::warn!(
                    "{}",
                    t_with_args(
                        "config-scenemode-parse-failed",
                        &fluent_args!("path" => "<embedded>".to_string(), "error" => e.to_string())
                    )
                );
                return;
            }
        };
        match serde_yaml::from_value::<Mode>(scene_value) {
            Ok(scene_config) => {
                self.scenemode = scene_config;
                log::debug!(
                    "{}",
                    t_with_args(
                        "config-scenemode-merged",
                        &fluent_args!("path" => "<embedded>".to_string())
                    )
                );
            }
            Err(e) => {
                log::warn!(
                    "{}",
                    t_with_args(
                        "config-scenemode-parse-failed",
                        &fluent_args!("path" => "<embedded>".to_string(), "error" => e.to_string())
                    )
                );
            }
        }
    }

    /// 取明日方舟特调（akmode）配置段（已合并 akmode.yaml）
    pub fn get_akmode(&self) -> &SpecialTunedConfig {
        &self.akmode
    }

    /// 按模式名取对应 CLG 配置段；未知模式（含特调模式）返回 None。
    /// 特调模式（akmode）不走 CLG，由 AkmodeGovernor 独立接管。
    pub fn get_mode(&self, mode_name: &str) -> Option<&Mode> {
        match mode_name {
            "powersave" => Some(&self.powersave),
            "balance" => Some(&self.balance),
            "performance" => Some(&self.performance),
            "fast" => Some(&self.fast),
            _ => None,
        }
    }
}
