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

use serde::Deserialize;

use crate::fluent_args;
use crate::i18n::t_with_args;

/// 全局元信息段（对应 config.yaml 顶层 `Meta`）
#[derive(Debug, Deserialize, Default)]
pub struct Meta {
    /// 日志级别：DEBUG / INFO / WARN / ERROR，热重载时即时生效
    #[serde(default = "default_loglevel", alias = "Loglevel")]
    pub loglevel: String,

    /// 守护进程日志语言：en / zh，改动后自动加载对应 .ftl
    #[serde(default = "default_language", alias = "Language")]
    pub language: String,
}

// Meta 缺省值：config.yaml 省略该字段时回退到此处
fn default_loglevel() -> String {
    "INFO".to_string()
}
fn default_language() -> String {
    "en".to_string()
}

// ════════════════════════════════════════════════════════════════
//  CPU Load Governor 配置
// ════════════════════════════════════════════════════════════════

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

// ════════════════════════════════════════════════════════════════
//  核心模式与杂项配置
// ════════════════════════════════════════════════════════════════

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

/// 单个核心组（little/big/prime）的升降频参数，组内独立配置。
/// 核心数 yaml 里直接写整数（如 2 = 组内达到 2 个核心命中阈值即触发），
/// 0 = 组内任一核心命中即触发，写大值如 64 = 关闭该方向判定。
#[derive(Debug, Deserialize, Clone)]
pub struct SpecialTunedGroup {
    /// 升频核心数：组内达到这个数量的核心占用率 > up_util_percent 才考虑升 max
    #[serde(default = "d_ak_up_core_count")]
    pub up_core_count: u32,
    /// 升频占用率阈值（%）
    #[serde(default = "d_ak_up_util_threshold")]
    pub up_util_percent: f32,
    /// 降频核心数：组内达到这个数量的核心占用率 < down_util_percent 才降 max
    #[serde(default = "d_ak_down_core_count")]
    pub down_core_count: u32,
    /// 降频占用率阈值（%）
    #[serde(default = "d_ak_down_util_threshold")]
    pub down_util_percent: f32,
}

/// 单个档位的配置：按核心组（little/big/prime）的升降频参数 + 本档防抖等待。
/// 核心组区间按命中 SoC 区分（common::chiri_core_ranges：8550 0-2/3-6/7、
/// 8450 0-3/4-6/7、8998 0-3/4-7 无 prime），与 affected_cpus 的 CPU ID 对照判定。
/// 档位由 rules.yaml 生效模式决定，特调期间固定应用、不自动切换档位。
#[derive(Debug, Deserialize, Clone)]
pub struct SpecialTunedTier {
    /// 升降频防抖等待（ms）：升降条件成立到真正升降 max 之间的等待
    #[serde(default = "d_ak_wait_ms")]
    pub wait_ms: u64,
    /// 小核组升降频参数
    #[serde(default)]
    pub little: SpecialTunedGroup,
    /// 大核组升降频参数
    #[serde(default)]
    pub big: SpecialTunedGroup,
    /// 超大核组升降频参数（无超大核的 SoC 该组不生效）
    #[serde(default)]
    pub prime: SpecialTunedGroup,
}

/// 明日方舟特调（akmode）配置，跟 CLG 完全没关系。
/// 档位由 rules.yaml 生效模式决定（powersave/balance/performance/fast），特调期间固定；
/// 档位差异仅在升降频策略参数，所有档位的 max 上限/下限均为硬件上下限。
/// 机制：激活时统一内核调速器为 schedutil、min 压到硬件最低、max 为硬件最高；
/// 之后用本档策略参数按负载升降 max（内核频率表逐档移动，可升到硬件最高、降到硬件最低）——
/// 通过动态限制 scaling_max_freq 让 schedutil 在 [硬件最低, 动态max] 内自由调频。
#[derive(Debug, Deserialize, Clone)]
pub struct SpecialTunedConfig {
    /// 升降频后防抖等待临时减半的持续时间（ms）：发生一次升降频后，后续 wait_ms
    /// 在此时长内按一半执行；超过此时长恢复原 wait_ms。
    #[serde(default = "d_ak_after_change_duration_ms")]
    pub after_change_duration_ms: u64,
    /// 档 1（最低频）：powersave
    #[serde(default)]
    pub powersave: SpecialTunedTier,
    /// 档 2：balance
    #[serde(default)]
    pub balance: SpecialTunedTier,
    /// 档 3：performance
    #[serde(default)]
    pub performance: SpecialTunedTier,
    /// 档 4（最高频）：fast
    #[serde(default)]
    pub fast: SpecialTunedTier,
}

// SpecialTunedConfig 缺省值：akmode.yaml 没写的字段回退到这里
fn d_ak_up_core_count() -> u32 {
    2
}
fn d_ak_up_util_threshold() -> f32 {
    80.0
}
fn d_ak_down_core_count() -> u32 {
    2
}
fn d_ak_down_util_threshold() -> f32 {
    60.0
}
fn d_ak_wait_ms() -> u64 {
    300
}
fn d_ak_after_change_duration_ms() -> u64 {
    3000
}

impl Default for SpecialTunedGroup {
    fn default() -> Self {
        Self {
            up_core_count: d_ak_up_core_count(),
            up_util_percent: d_ak_up_util_threshold(),
            down_core_count: d_ak_down_core_count(),
            down_util_percent: d_ak_down_util_threshold(),
        }
    }
}

impl Default for SpecialTunedTier {
    fn default() -> Self {
        Self {
            wait_ms: d_ak_wait_ms(),
            little: SpecialTunedGroup::default(),
            big: SpecialTunedGroup::default(),
            prime: SpecialTunedGroup::default(),
        }
    }
}

impl Default for SpecialTunedConfig {
    fn default() -> Self {
        Self {
            after_change_duration_ms: d_ak_after_change_duration_ms(),
            powersave: SpecialTunedTier::default(),
            balance: SpecialTunedTier::default(),
            performance: SpecialTunedTier::default(),
            fast: SpecialTunedTier::default(),
        }
    }
}

impl SpecialTunedGroup {
    /// 把 yaml 里写的百分比（>1 视为百分比）转成 0..1 比例并 clamp。
    /// 写 50 还是 0.5 都认，前者按 50% 处理。
    fn normalize_pct(v: &mut f32, dft: f32) {
        if !v.is_finite() {
            *v = dft;
        }
        if *v > 1.0 {
            *v /= 100.0;
        }
        *v = v.clamp(0.0, 1.0);
    }

    /// 校验单个核心组：核心数限制在合理范围，占用率阈值转 0..1。
    fn normalize(&mut self) {
        self.up_core_count = self.up_core_count.min(64);
        self.down_core_count = self.down_core_count.min(64);
        Self::normalize_pct(&mut self.up_util_percent, d_ak_up_util_threshold());
        Self::normalize_pct(&mut self.down_util_percent, d_ak_down_util_threshold());
    }
}

impl SpecialTunedTier {
    /// 校验单个档位：逐核心组 normalize
    fn normalize(&mut self) {
        self.little.normalize();
        self.big.normalize();
        self.prime.normalize();
    }
}

impl SpecialTunedConfig {
    /// 校验配置：逐档 normalize；升降频加速持续时间限制在合理范围（上限 60s 防误配）
    pub fn normalize(&mut self) {
        self.after_change_duration_ms = self.after_change_duration_ms.min(60_000);
        self.powersave.normalize();
        self.balance.normalize();
        self.performance.normalize();
        self.fast.normalize();
    }

    /// 取档位的配置（内部档位 1..4：powersave=1 balance=2 performance=3 fast=4）
    pub fn tier(&self, tier: u32) -> &SpecialTunedTier {
        match tier {
            1 => &self.powersave,
            2 => &self.balance,
            3 => &self.performance,
            _ => &self.fast,
        }
    }
}

/// 模式名 → 特调档位（1..4）。未知模式或特调自身回退 balance（档 2）。
/// 特调的档位就是全局那套模式档位，起始档从 rules.yaml 的生效模式识别。
pub fn mode_to_tier(mode: &str) -> u32 {
    match mode {
        "powersave" => 1,
        "balance" => 2,
        "performance" => 3,
        "fast" => 4,
        _ => 2,
    }
}

/// 特调档位（1..4）→ 模式名，日志展示用
pub fn tier_to_mode(tier: u32) -> &'static str {
    match tier {
        1 => "powersave",
        2 => "balance",
        3 => "performance",
        _ => "fast",
    }
}

/// 顶层配置（config.yaml 全量）
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
    /// 极致省电配置（不择手段压功耗提续航），亮屏后恢复原模式。
    /// 未定义时回退 CLG 默认参数（兜底，通常 8550 config.yaml 会显式配置）。
    #[serde(default)]
    pub scenemode: Mode,
    /// 息屏进入 scenemode 的延迟（秒）：默认 300s（5 分钟），YAML 可覆盖
    #[serde(default = "default_scene_mode_delay_secs")]
    pub scene_mode_delay_secs: u64,

    /// 明日方舟特调（akmode）独立调频配置：来自处理器目录 akmode.yaml，与 CLG 完全解耦。
    /// 前台为白名单应用时由 AkmodeGovernor 接管，参数不再走 CLG。
    #[serde(default)]
    pub akmode: SpecialTunedConfig,
}

/// scenemode 延迟缺省值：5 分钟
fn default_scene_mode_delay_secs() -> u64 {
    300
}

impl Config {
    /// 从 YAML 文件加载配置；读取或反序列化失败时返回 Err。
    /// 加载后合并处理器专属特调文件（common::get_akmode_path()，与主配置同目录）。
    /// 热重载路径同样经由本函数，因此特调参数修改后触发热重载即可生效。
    pub fn from_file(path: &str) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let mut config: Config = serde_yaml::from_str(&content)?;
        config.merge_akmode();
        Ok(config)
    }

    /// 合并独立特调配置文件（akmode.yaml）中的特调段：
    /// - 读取失败（文件缺失等）→ warn 并置特调不可用（白名单应用回退 CLG）
    /// - 解析失败（内容损坏等）→ warn 并置特调不可用（回退 CLG）
    /// - 成功 → debug 打点
    ///
    /// 特调可用性经 common::AKMODE_AVAILABLE 共享给 monitor 层：缺 akmode.yaml 的
    /// 机型（如未随处理器发布特调文件时）不启用特调，避免用默认参数接管 CPU。
    fn merge_akmode(&mut self) {
        let path = crate::common::get_akmode_path();
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => {
                log::warn!(
                    "{}",
                    t_with_args(
                        "config-special-load-failed",
                        &fluent_args!("path" => path.to_string_lossy().to_string(), "error" => e.to_string())
                    )
                );
                crate::common::set_akmode_available(false);
                return;
            }
        };
        match serde_yaml::from_str::<Config>(&content) {
            Ok(special) => {
                self.akmode = special.akmode;
                self.akmode.normalize();
                crate::common::set_akmode_available(true);
                log::debug!(
                    "{}",
                    t_with_args(
                        "config-special-merged",
                        &fluent_args!("path" => path.to_string_lossy().to_string())
                    )
                );
            }
            Err(e) => {
                log::warn!(
                    "{}",
                    t_with_args(
                        "config-special-parse-failed",
                        &fluent_args!("path" => path.to_string_lossy().to_string(), "error" => e.to_string())
                    )
                );
                crate::common::set_akmode_available(false);
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
