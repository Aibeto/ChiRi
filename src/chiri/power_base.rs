//! power_base.rs: [types] [init] [tick] [release]
//!
//! PowerBase（Stardust 家族）：以**放电功耗**为指标的调频器，用来替换 CLG。
//!
//! 与 CLG 的根本区别：CLG 只看「利用率够不够」，PowerBase 看「功耗超没超目标」——
//!   - 功耗**低于** feature 里的 `target_power_w`：升频放宽（`up_headroom_below`）；
//!   - 功耗**达到/超过**目标：守住不再升频，除非确实压不住——满占用核心占比达到
//!     `overload_cores_pct` 且持续 `overload_hold_ms`；
//!   - 降频**恒激进**（`down_scale` 直接砍目标），与当前功耗无关；
//!   - 触摸窗口内允许短暂突破功率上限（`touch_break_ms`）。
//!
//! **只替换 CLG**：FAS / 场景特调 / DOWN 停摆 / 实验室的启停判据一律不受影响。
//! 结构照 `fast.rs`（独立接管、快照/释放、5s 重写防篡改），但不锁死频率，
//! 而是按上面的规则动态算出每个 policy 的目标频率。

use crate::chiri::config::PowerBaseConfig;
use crate::utils::FastWriter;
use log::{info, warn};
use std::fs;
use std::time::{Duration, Instant};

use crate::fluent_args;
use crate::i18n::{t, t_with_args};

/// 防篡改重写间隔（与 fast.rs 同口径）
const REWRITE_INTERVAL: Duration = Duration::from_secs(5);
/// 性能比死区：目标与当前的差小于它就认为「已经到位」，不写频率
/// （利用率反馈滞后时的抖动会被它挡掉，同时把 sysfs 写入从每 tick 降到只在真正变化时）
const PERF_DEADBAND: f32 = 0.05;
/// 单次降频的最大幅度（性能比）：`down_scale` 再激进也不越过它，
/// 避免一步砍半导致下一 tick util 反弹成振荡
const MAX_DOWN_STEP: f32 = 0.25;

// [types]
/// 接管前的系统状态快照，release 时恢复
struct PolicySnapshot {
    policy_id: i32,
    governor: Option<String>,
    min_freq: Option<u32>,
    max_freq: Option<u32>,
}

/// 单个 policy 的运行状态
struct BasePolicy {
    /// 该 policy 覆盖的 CPU（用于取利用率）
    cpus: Vec<usize>,
    hw_min: u32,
    hw_max: u32,
    /// 可用频率表（升序），目标频率就近取档
    freqs: Vec<u32>,
    /// 当前目标性能比（0..1）
    perf: f32,
    max_writer: FastWriter,
    min_writer: FastWriter,
}

impl BasePolicy {
    /// 性能比 → 频率档位（硬件最低 + 跨度 × 比例，就近取不低于它的档）。
    /// 挂在 policy 上而不是 PowerBase 上：tick 里要在 `&mut self.policies` 的循环内
    /// 调用它，取 &self 会与那个可变借用冲突。
    fn freq_for(&self, perf: f32) -> u32 {
        let want = self.hw_min as f32 + (self.hw_max - self.hw_min) as f32 * perf.clamp(0.0, 1.0);
        let want = want as u32;
        self.freqs
            .iter()
            .copied()
            .find(|f| *f >= want)
            .unwrap_or(self.hw_max)
    }
}

/// 功耗闸门状态
#[derive(Default)]
struct PowerGate {
    /// 过载（满占用核心占比达标）的起始时刻
    overload_since: Option<Instant>,
}

pub struct PowerBase {
    cfg: PowerBaseConfig,
    policies: Vec<BasePolicy>,
    snapshots: Vec<PolicySnapshot>,
    gate: PowerGate,
    active: bool,
    last_write: Instant,
}

impl Default for PowerBase {
    fn default() -> Self {
        Self::new()
    }
}

impl PowerBase {
    pub fn new() -> Self {
        Self {
            cfg: PowerBaseConfig::default(),
            policies: Vec::new(),
            snapshots: Vec::new(),
            gate: PowerGate::default(),
            active: false,
            last_write: Instant::now(),
        }
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    // [init]
    /// 接管全部 cpufreq policy：读可用频率、快照原状态、写 schedutil，
    /// 并**立刻把 min=max=硬件最高频**（内部 `perf` 同步为 1.0）。
    ///
    /// **为什么接管就必须写一次**：内部 `perf` 与硬件实际状态必须一致，否则首个降频
    /// tick 会按「升频写序」先写 max（低于接管前的 min）→ 内核以 min>max 拒绝 →
    /// 写入失败 → `perf` 不前移 → 下个 tick 重试同样失败，**永久卡住**（表现为
    /// PowerBase 完全不生效且无任何报错）。取硬件最高频起手：接管瞬间系统往往有负载，
    /// 先给足余量避免掉帧，紧随的 tick 会按功耗规则迅速压下来（降频本就激进）。
    pub fn init(&mut self, cfg: &PowerBaseConfig) {
        self.release();
        self.cfg = cfg.clone();
        self.cfg.normalize();

        let policies = crate::chiri::get_cpu_policies();
        let ranges = crate::common::chiri_core_ranges();

        for policy in &policies {
            let pid = policy.id;
            let gov_path = format!(
                "/sys/devices/system/cpu/cpufreq/policy{}/scaling_governor",
                pid
            );
            let min_path = format!(
                "/sys/devices/system/cpu/cpufreq/policy{}/scaling_min_freq",
                pid
            );
            let max_path = format!(
                "/sys/devices/system/cpu/cpufreq/policy{}/scaling_max_freq",
                pid
            );
            let freq_path = format!(
                "/sys/devices/system/cpu/cpufreq/policy{}/scaling_available_frequencies",
                pid
            );

            let mut freqs: Vec<u32> = fs::read_to_string(&freq_path)
                .unwrap_or_default()
                .split_whitespace()
                .filter_map(|s| s.parse().ok())
                .collect();
            if freqs.is_empty() {
                continue;
            }
            freqs.sort_unstable();
            freqs.dedup();
            let hw_min = *freqs.first().unwrap();
            let hw_max = *freqs.last().unwrap();

            let governor = fs::read_to_string(&gov_path)
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            let min_freq = fs::read_to_string(&min_path)
                .ok()
                .and_then(|s| s.trim().parse::<u32>().ok());
            let max_freq = fs::read_to_string(&max_path)
                .ok()
                .and_then(|s| s.trim().parse::<u32>().ok());
            self.snapshots.retain(|r| r.policy_id != pid);
            self.snapshots.push(PolicySnapshot {
                policy_id: pid,
                governor,
                min_freq,
                max_freq,
            });

            let _ = crate::utils::try_write_file(&gov_path, "schedutil");

            let mut max_writer = FastWriter::new(max_path.clone());
            let mut min_writer = FastWriter::new(min_path.clone());
            if !max_writer.is_valid() || !min_writer.is_valid() {
                warn!(
                    "{}",
                    t_with_args(
                        "powerbase-writer-invalid",
                        &fluent_args!("pid" => pid.to_string())
                    )
                );
                continue;
            }

            // 该 policy 覆盖的 CPU：policy 名就是首个 CPU id，落到哪个区间就属于哪簇
            let id = pid.max(0) as usize;
            let cpus: Vec<usize> = if ranges.little.contains(&id) {
                ranges.little.clone().collect()
            } else if ranges.big.contains(&id) {
                ranges.big.clone().collect()
            } else {
                ranges.prime.clone().collect()
            };

            // 起手锁硬件最高频（写序：先 max 后 min，保证不出现 min > max）。
            // 写成功才把 perf 记为 1.0；失败保持 0.0，下一 tick 会按「升频写序」重试，
            // 不会落进上面注释里那个「降频却先写 max」的死局。
            let mut perf = 0.0_f32;
            if max_writer.write_value_force(hw_max) && min_writer.write_value_force(hw_max) {
                perf = 1.0;
            }

            self.policies.push(BasePolicy {
                cpus,
                hw_min,
                hw_max,
                freqs,
                perf,
                max_writer,
                min_writer,
            });
        }

        self.active = !self.policies.is_empty();
        self.last_write = Instant::now();
        if self.active {
            info!("{}", t("powerbase-activated"));
        }
    }

    // [tick]
    /// 每个负载 tick 调用一次。
    ///
    /// - `core_utils`：各 CPU 利用率（0..1）
    /// - `power_w`：当前功耗（W）；**None 表示未在放电或没有读数** —— 此时不做功耗限制
    /// - `touch_active`：触摸窗口内，允许短暂突破功率上限
    ///
    /// 返回距下次防篡改重写的剩余时间（非激活返回 None）。
    pub fn on_load_update(
        &mut self,
        core_utils: &[f32],
        power_w: Option<f32>,
        touch_active: bool,
    ) -> Option<Duration> {
        if !self.active {
            return None;
        }
        let now = Instant::now();

        // ── 功耗闸门 ──
        // 放电才有意义：充电时功耗口径与电池功率不是一回事，不该压性能。
        let capped = power_w.is_some_and(|p| p >= self.cfg.target_power_w) && !touch_active;
        let overload_ok = if capped {
            self.overload_reached(core_utils, now)
        } else {
            false
        };
        if !capped {
            self.gate.overload_since = None;
        }

        for p in &mut self.policies {
            let util = p
                .cpus
                .iter()
                .filter_map(|c| core_utils.get(*c).copied())
                .fold(0.0_f32, f32::max);

            let mut target = util;
            if !capped {
                // 功耗低于目标（或不在放电）：按宽松度放大
                target *= self.cfg.up_headroom_below;
            } else if !overload_ok {
                // 功耗已达标且没到过载门槛：只允许保持或下降
                target = target.min(p.perf);
            }

            // 降频恒激进：目标低于当前时再砍一刀（不看功耗），但**限制单次降幅**——
            // 一步砍半会在下一个 tick 因频率骤降把 util 顶回去（利用率反馈滞后），
            // 形成「砍半 → util 反弹 → 升回 → 再砍」的振荡，平均功耗反而更高。
            if target < p.perf {
                let stepped = target * self.cfg.down_scale;
                target = stepped.max(p.perf - MAX_DOWN_STEP);
            }
            target = target.clamp(0.0, 1.0);

            // 死区：目标与当前差不到 PERF_DEADBAND 就不写（抑制抖动、省 sysfs 写入）
            if (target - p.perf).abs() > PERF_DEADBAND {
                let want = p.freq_for(target);
                // 写序：升频先抬 max 再抬 min，降频先压 min 再压 max（避免 min > max）
                let ok = if want > p.freq_for(p.perf) {
                    p.max_writer.write_value_force(want) && p.min_writer.write_value_force(want)
                } else {
                    p.min_writer.write_value_force(want) && p.max_writer.write_value_force(want)
                };
                if ok {
                    p.perf = target;
                }
            }
        }

        // ── 防篡改重写 ──
        let elapsed = self.last_write.elapsed();
        if elapsed < REWRITE_INTERVAL {
            return Some(REWRITE_INTERVAL - elapsed);
        }
        self.last_write = now;
        for p in &mut self.policies {
            let want = p.freq_for(p.perf);
            let _ = p.max_writer.write_value_force(want);
            let _ = p.min_writer.write_value_force(want);
        }
        Some(REWRITE_INTERVAL)
    }

    /// 过载判定：满占用（利用率 100%）核心占比达到门槛，且持续足够久
    fn overload_reached(&mut self, core_utils: &[f32], now: Instant) -> bool {
        let total = core_utils.len().max(1);
        let full = core_utils.iter().filter(|u| **u >= 0.995).count();
        let pct = full as f32 * 100.0 / total as f32;
        if pct < self.cfg.overload_cores_pct {
            self.gate.overload_since = None;
            return false;
        }
        let since = *self.gate.overload_since.get_or_insert(now);
        now.duration_since(since).as_millis() as u64 >= self.cfg.overload_hold_ms
    }

    // [release]
    /// 释放接管：恢复各 policy 的 governor / min / max
    pub fn release(&mut self) {
        if self.active {
            info!("{}", t("powerbase-deactivated"));
        }
        self.snapshots.retain(|r| !Self::restore_policy(r));
        self.policies.clear();
        self.gate.overload_since = None;
        self.active = false;
    }

    fn restore_policy(r: &PolicySnapshot) -> bool {
        let gov_path = format!(
            "/sys/devices/system/cpu/cpufreq/policy{}/scaling_governor",
            r.policy_id
        );
        let min_path = format!(
            "/sys/devices/system/cpu/cpufreq/policy{}/scaling_min_freq",
            r.policy_id
        );
        let max_path = format!(
            "/sys/devices/system/cpu/cpufreq/policy{}/scaling_max_freq",
            r.policy_id
        );
        let mut all_ok = true;
        if let Some(max) = r.max_freq {
            if crate::utils::write_to_file(&max_path, max.to_string()).is_err() {
                all_ok = false;
            }
        }
        if let Some(min) = r.min_freq {
            if crate::utils::write_to_file(&min_path, min.to_string()).is_err() {
                all_ok = false;
            }
        }
        if let Some(gov) = &r.governor {
            if crate::utils::write_to_file(&gov_path, gov.as_bytes()).is_err() {
                all_ok = false;
            }
        }
        all_ok
    }
}
