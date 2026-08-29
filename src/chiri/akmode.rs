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

use crate::chiri::config::SpecialTunedConfig;
use crate::utils::FastWriter;
use log::{debug, info, warn};
use std::fs;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::fluent_args;
use crate::i18n::{t, t_with_args};

/// 固定 4 档，对应 powersave/balance/performance/fast，powersave 最低 fast 最高
const TIER_COUNT: usize = 4;

/// 单个 policy 的 governor/min/max 快照：akmode 接管时保存，release 时恢复。
struct PolicyRestore {
    policy_id: i32,
    /// 接管前的 scaling_governor；读取失败为 None，恢复时跳过
    governor: Option<String>,
    /// 接管前的 scaling_min_freq（kHz）；读取失败为 None，恢复时跳过
    min_freq: Option<u32>,
    /// 接管前的 scaling_max_freq（kHz）；读取失败为 None，恢复时跳过
    max_freq: Option<u32>,
}

/// 单个 policy 的运行时状态：四档 max 上限吸附值，切档时写 scaling_max_freq。
struct ClusterState {
    policy_id: i32,
    /// 核心组名：little / big / prime
    core_name: String,
    /// 可用频率（kHz，升序去重），用于吸附 max_freq
    available_freqs: Vec<u32>,
    /// 四档 max 吸附值（下标 0..3 对应档 1..4），None = 该档该组未配置 max_freq（不写）
    tier_max: [Option<u32>; TIER_COUNT],
    max_writer: FastWriter,
    /// 当前已成功写入的 max（kHz），0 表示尚未写入
    current_max: u32,
}

/// 在 available_frequencies 里找离 target 最近的档位
fn nearest_freq(available: &[u32], target: u32) -> u32 {
    available
        .iter()
        .copied()
        .min_by_key(|&f| (f as i64 - target as i64).abs())
        .unwrap_or(target)
}

/// 按 affected_cpus 的 CPU ID 判定核心组，8550 固定分布，直接写死：
/// 0-2 小核 little，3-6 大核 big，7 超大核 prime。
/// 同一 soc 布局固定，没必要动态探测。判不出来的返回 None，该 policy 不接管。
fn core_name_for(affected: &[usize]) -> Option<&'static str> {
    let first = affected.iter().copied().min()?;
    match first {
        0..=2 => Some("little"),
        3..=6 => Some("big"),
        7 => Some("prime"),
        _ => None,
    }
}

/// 吸附四档的 max 上限：取各档该核心组的 max_freq（>0 才写），吸附到真实频率表。
/// 未配置（0）的档位返回 None，切到该档时不写 max（保持现状）。
fn snap_tier_max(cfg: &SpecialTunedConfig, name: &str, freqs: &[u32]) -> [Option<u32>; TIER_COUNT] {
    let tiers = [
        &cfg.powersave,
        &cfg.balance,
        &cfg.performance,
        &cfg.fast,
    ];
    let mut arr = [None; TIER_COUNT];
    for (i, t) in tiers.iter().enumerate() {
        let f = match name {
            "little" => t.little.max_freq,
            "big" => t.big.max_freq,
            "prime" => t.prime.max_freq,
            _ => 0,
        };
        if f > 0 {
            arr[i] = Some(nearest_freq(freqs, f));
        }
    }
    arr
}

/// 明日方舟特调（akmode）控制器：schedutil 动态调频 + 档位限频。
/// 接管时统一内核调速器为 schedutil、把 min 压到硬件最低（解决设备高 min 卡频），
/// 并按负载在四档间升降，每档把各核心组的 scaling_max_freq 写到该档 max 上限：
/// schedutil 在 [硬件最低, 档位max] 内按负载动态调频（能升能降，且受档位约束）。
pub struct AkmodeGovernor {
    cfg: SpecialTunedConfig,
    /// 特调激活共享标志：Monitor 层（cpu_monitor）据此切换采样间隔（特调 40ms / 其余 120ms）。
    /// 接管时置 true、释放时置 false，由 init_policies / release 维护。
    ak_active: Arc<AtomicBool>,
    clusters: Vec<ClusterState>,
    /// 各 policy 的 governor/min/max 快照，release 时恢复
    restore: Vec<PolicyRestore>,
    active: bool,
    /// 当前档位 1..=4
    current_tier: u32,
    /// 待执行的升降档目标，防抖等待中
    pending_tier: Option<u32>,
    /// 待执行目标第一次被检测到的时间
    pending_since: Option<Instant>,
    /// 升降档后防抖等待临时减半的截止时间：到点前 wait_ms 按一半执行
    fast_wait_until: Option<Instant>,
    /// 调试日志计数，每 25 tick 打一次摘要
    log_counter: u32,
}

impl AkmodeGovernor {
    pub fn new(ak_active: Arc<AtomicBool>) -> Self {
        Self {
            cfg: SpecialTunedConfig::default(),
            ak_active,
            clusters: Vec::new(),
            restore: Vec::new(),
            active: false,
            current_tier: 1,
            pending_tier: None,
            pending_since: None,
            fast_wait_until: None,
            log_counter: 0,
        }
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    /// 接管全部 cpufreq policy：
    /// 1. 先 release 清掉上一次状态；
    /// 2. 逐个 policy 读可用频率与 affected_cpus，按 CPU ID 硬编码判定大小核；
    /// 3. 吸附四档 max 上限，快照 governor/min/max；
    /// 4. 写 schedutil、min 压到硬件最低、写起始档的 max 上限。
    /// initial_tier 由调用方从 rules.yaml 的生效模式换算（powersave=1..fast=4）。
    pub fn init_policies(&mut self, cfg: &SpecialTunedConfig, initial_tier: u32) {
        self.release();
        self.cfg = cfg.clone();
        self.cfg.normalize();
        let initial_tier = initial_tier.clamp(1, TIER_COUNT as u32);
        self.current_tier = initial_tier;
        self.pending_tier = None;
        self.pending_since = None;

        let policies = crate::chiri::get_cpu_policies();

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

            let affected = Self::read_affected_cpus(pid);
            if affected.is_empty() {
                continue;
            }

            let name = match core_name_for(&affected) {
                Some(n) => n,
                None => {
                    warn!(
                        "{}",
                        t_with_args(
                            "akmode-cluster-skipped",
                            &fluent_args!(
                                "pid" => pid.to_string(),
                                "reason" => "unknown-cpu-range".to_string()
                            )
                        )
                    );
                    continue;
                }
            };

            let tier_max = snap_tier_max(&self.cfg, name, &freqs);
            let max_writer = FastWriter::new(max_path.clone());
            if !max_writer.is_valid() {
                warn!(
                    "{}",
                    t_with_args(
                        "akmode-cluster-skipped",
                        &fluent_args!(
                            "pid" => pid.to_string(),
                            "reason" => "writer-invalid".to_string()
                        )
                    )
                );
                continue;
            }

            // 快照系统原始状态，release 时恢复（必须位于写入之前）
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
            self.restore.retain(|r| r.policy_id != pid);
            self.restore.push(PolicyRestore {
                policy_id: pid,
                governor,
                min_freq,
                max_freq,
            });

            // 统一 schedutil + min 压到硬件最低（避免设备出厂高 min 导致频率降不下去）
            let _ = crate::utils::try_write_file(&gov_path, "schedutil");
            let min_hw = freqs[0];
            let _ = crate::utils::try_write_file(&min_path, min_hw.to_string());

            // 写起始档的 max 上限
            let mut current_max = 0u32;
            if let Some(max) = tier_max[(initial_tier - 1) as usize] {
                current_max = max;
                let _ = max_writer.write_value_force(max);
            }

            self.clusters.push(ClusterState {
                policy_id: pid,
                core_name: name.to_string(),
                available_freqs: freqs,
                tier_max,
                max_writer,
                current_max,
            });
        }

        self.active = !self.clusters.is_empty();
        if self.active {
            info!(
                "{}",
                t_with_args(
                    "akmode-init",
                    &fluent_args!(
                        "mode" => crate::chiri::config::tier_to_mode(self.current_tier).to_string()
                    )
                )
            );
            info!("{}", t("akmode-activated"));
            // 特调激活通知 Monitor 层切换到 40ms 快速采样
            self.ak_active.store(true, Ordering::Relaxed);
        } else {
            warn!("{}", t("akmode-no-clusters"));
        }
    }

    /// 释放接管：恢复各 policy 的 governor/min/max，清空状态
    pub fn release(&mut self) {
        if self.active {
            info!("{}", t("akmode-deactivated"));
        }
        self.active = false;
        // 恢复原 governor/min/max（快照读取失败的字段跳过）
        self.restore.retain(|r| !Self::restore_policy(r));
        self.clusters.clear();
        // 特调退出通知 Monitor 层恢复常规采样
        self.ak_active.store(false, Ordering::Relaxed);
        self.pending_tier = None;
        self.pending_since = None;
        self.fast_wait_until = None;
        self.log_counter = 0;
    }

    /// 恢复单个 policy 的原 governor/min/max，返回是否全部写成功（失败保留快照下次重试）。
    /// 写序：先恢复 max 再恢复 min（min 不高于恢复后的 max），避免中间态 min > max。
    fn restore_policy(r: &PolicyRestore) -> bool {
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

    /// 热切换配置：重新吸附四档 max 并重写当前档，当前档位不变
    pub fn reload_config(&mut self, cfg: &SpecialTunedConfig) {
        self.cfg = cfg.clone();
        self.cfg.normalize();
        for cluster in self.clusters.iter_mut() {
            cluster.tier_max = snap_tier_max(&self.cfg, &cluster.core_name, &cluster.available_freqs);
        }
        self.current_tier = self.current_tier.clamp(1, TIER_COUNT as u32);
        // 用新配置重写当前档的 max 上限
        for cluster in self.clusters.iter_mut() {
            if let Some(max) = cluster.tier_max[(self.current_tier - 1) as usize] {
                if max != cluster.current_max {
                    if cluster.max_writer.write_value_force(max) {
                        cluster.current_max = max;
                    }
                }
            }
        }
        debug!(
            "{}",
            t_with_args(
                "akmode-config-reloaded",
                &fluent_args!("wait" => self.cfg.tier(self.current_tier).wait_ms.to_string())
            )
        );
    }

    /// 档位判定入口，每个 SystemLoadUpdate（常规 120ms / 特调 40ms）触发一次。
    /// 按核心组（little/big/prime）分别统计忙/闲核心数，每组用本组独立条件判定：
    ///   升档 = 任一组内超过 up_core_count 个核心占用率 > up_util_percent
    ///   降档 = 任一组内超过 down_core_count 个核心占用率 < down_util_percent
    /// 升档优先于降档，条件成立后等 wait_ms 防抖再切档（切档时写新档位的 max 上限）。
    pub fn on_load_update(&mut self, core_utils: &[f32]) {
        if !self.active {
            return;
        }

        // 档位配置克隆成局部值：`self.cfg.tier(...)` 若直接借用 self.cfg，其共享借用会
        // 随下方 GroupStat 一直存活，之后调 self.apply_tier()（&mut self）会触发借用冲突
        // （E0502）。克隆后 tc 只借局部变量，不占 self 的借用。
        let tc = self.cfg.tier(self.current_tier).clone();

        // 核心组按 CPU ID 区间硬编码（8550：0-2 小核 little、3-6 大核 big、7 超大核 prime，
        // 同 SoC 布局固定不动态探测）。每组独立配置升降档条件。
        struct GroupStat<'a> {
            g: &'a crate::chiri::config::SpecialTunedGroup,
            range: std::ops::Range<usize>,
            over: usize,
            under: usize,
        }

        let mut stats = [
            GroupStat {
                g: &tc.little,
                range: 0..3,
                over: 0,
                under: 0,
            },
            GroupStat {
                g: &tc.big,
                range: 3..7,
                over: 0,
                under: 0,
            },
            GroupStat {
                g: &tc.prime,
                range: 7..8,
                over: 0,
                under: 0,
            },
        ];

        let mut up_hit = false;
        let mut down_hit = false;
        for s in &mut stats {
            for cpu in s.range.clone() {
                // core_utils 按真实 CPU ID 索引，离线核心固定为 0.0，不参与统计
                if let Some(&u) = core_utils.get(cpu) {
                    if u <= 0.0 {
                        continue;
                    }
                    if u > s.g.up_util_percent {
                        s.over += 1;
                    }
                    if u < s.g.down_util_percent {
                        s.under += 1;
                    }
                }
            }
            if s.over as u32 > s.g.up_core_count {
                up_hit = true;
            }
            if s.under as u32 > s.g.down_core_count {
                down_hit = true;
            }
        }

        // 升档优先于降档，频率贴着需求走。
        // 升降档条件和等待都看当前档：升档用本档各组 up_*，降档用本档各组 down_*。
        let mut desired = self.current_tier as i32;
        if up_hit {
            desired += 1;
        } else if down_hit {
            desired -= 1;
        }
        let desired = desired.clamp(1, TIER_COUNT as i32) as u32;

        if desired == self.current_tier {
            self.pending_tier = None;
            self.pending_since = None;
        } else {
            let now = Instant::now();
            // 升降档后的临时加速：刚切过档（fast_wait_until 未过期）时 wait_ms 减半执行，
            // 让连续跳档更跟手；超过 after_change_duration_ms 恢复原 wait_ms。
            let fast_wait = self.fast_wait_until.map_or(false, |until| now < until);
            let wait = if fast_wait { tc.wait_ms / 2 } else { tc.wait_ms };
            match self.pending_tier {
                Some(t) if t == desired => {
                    if let Some(since) = self.pending_since {
                        if now.duration_since(since).as_millis() as u64 >= wait {
                            self.apply_tier(desired);
                            self.pending_tier = None;
                            self.pending_since = None;
                        }
                    }
                }
                _ => {
                    self.pending_tier = Some(desired);
                    self.pending_since = Some(now);
                }
            }
        }

        self.log_counter += 1;
        if self.log_counter % 25 == 0 {
            let mode = crate::chiri::config::tier_to_mode(self.current_tier);
            let (l_over, l_under) = (stats[0].over, stats[0].under);
            let (b_over, b_under) = (stats[1].over, stats[1].under);
            let (p_over, p_under) = (stats[2].over, stats[2].under);
            debug!(
                "{}",
                t_with_args(
                    "akmode-tick-log",
                    &fluent_args!(
                        "mode" => mode.to_string(),
                        "up" => up_hit.to_string(),
                        "down" => down_hit.to_string(),
                        "l_over" => l_over.to_string(),
                        "l_under" => l_under.to_string(),
                        "b_over" => b_over.to_string(),
                        "b_under" => b_under.to_string(),
                        "p_over" => p_over.to_string(),
                        "p_under" => p_under.to_string()
                    )
                )
            );
        }
    }

    /// 切档：写各核心组新档位的 max 上限，并启动临时加速窗口
    fn apply_tier(&mut self, tier: u32) {
        let old = self.current_tier;
        self.current_tier = tier;
        // 写新档位的 max 上限：schedutil 在 [硬件最低, 档位max] 内按负载动态调频
        for cluster in &mut self.clusters {
            if let Some(max) = cluster.tier_max[(tier - 1) as usize] {
                if max != cluster.current_max {
                    if cluster.max_writer.write_value_force(max) {
                        cluster.current_max = max;
                        debug!(
                            "{}",
                            t_with_args(
                                "akmode-max-set",
                                &fluent_args!(
                                    "pid" => cluster.policy_id.to_string(),
                                    "name" => cluster.core_name.clone(),
                                    "mode" => crate::chiri::config::tier_to_mode(tier).to_string(),
                                    "max_khz" => (max / 1000).to_string()
                                )
                            )
                        );
                    }
                }
            }
        }
        // 切档后启动临时加速窗口：此后 after_change_duration_ms 内防抖等待减半执行
        self.fast_wait_until = Some(
            Instant::now() + Duration::from_millis(self.cfg.after_change_duration_ms),
        );
        info!(
            "{}",
            t_with_args(
                "akmode-tier-change",
                &fluent_args!(
                    "old" => crate::chiri::config::tier_to_mode(old).to_string(),
                    "new" => crate::chiri::config::tier_to_mode(tier).to_string()
                )
            )
        );
    }

    /// 读 policy 的 affected_cpus
    fn read_affected_cpus(policy_id: i32) -> Vec<usize> {
        let path = format!(
            "/sys/devices/system/cpu/cpufreq/policy{}/affected_cpus",
            policy_id
        );
        fs::read_to_string(&path)
            .unwrap_or_default()
            .split_whitespace()
            .filter_map(|s| s.parse::<usize>().ok())
            .collect()
    }
}
