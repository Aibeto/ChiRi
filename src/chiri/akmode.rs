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
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

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

/// 单个 policy 的运行时状态：当前生效档位的 max 上限吸附值。
struct ClusterState {
    policy_id: i32,
    /// 核心组名：little / big / prime
    core_name: String,
    /// 可用频率（kHz，升序去重），用于吸附 max_freq
    available_freqs: Vec<u32>,
    /// 当前档位下本组的 max 吸附值（None = 该档该组未配置 max_freq，不写）
    current_max: Option<u32>,
    max_writer: FastWriter,
    /// 当前已成功写入的 max（kHz），0 表示尚未写入
    written_max: u32,
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

/// 吸附指定档位下该核心组的 max 上限到真实频率表。未配置（0）返回 None（不写 max）。
fn snap_max(cfg: &SpecialTunedConfig, tier: u32, name: &str, freqs: &[u32]) -> Option<u32> {
    let t = cfg.tier(tier);
    let f = match name {
        "little" => t.little.max_freq,
        "big" => t.big.max_freq,
        "prime" => t.prime.max_freq,
        _ => 0,
    };
    if f > 0 {
        Some(nearest_freq(freqs, f))
    } else {
        None
    }
}

/// 明日方舟特调（akmode）控制器：按 rules.yaml 生效模式固定应用对应档位的限频策略。
/// 档位由 rules.yaml 决定（powersave/balance/performance/fast），特调期间不自动切换；
/// 激活时统一内核调速器为 schedutil、min 压到硬件最低，把各核心组 scaling_max_freq
/// 写到该档 max_freq 上限——schedutil 在 [硬件最低, 档位max] 内按负载动态调频。
/// 用户改 rules.yaml 模式后经热重载（reload_config）更新档位。
pub struct AkmodeGovernor {
    cfg: SpecialTunedConfig,
    /// 特调激活共享标志：Monitor 层（cpu_monitor）据此切换采样间隔（特调 40ms / 其余 120ms）。
    /// 接管时置 true、释放时置 false，由 init_policies / release 维护。
    ak_active: Arc<AtomicBool>,
    clusters: Vec<ClusterState>,
    /// 各 policy 的 governor/min/max 快照，release 时恢复
    restore: Vec<PolicyRestore>,
    active: bool,
    /// 当前生效档位 1..=4（由 rules.yaml 模式决定，固定不自动切换）
    current_tier: u32,
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
        }
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    /// 接管全部 cpufreq policy：
    /// 1. 先 release 清掉上一次状态；
    /// 2. 逐个 policy 读可用频率与 affected_cpus，按 CPU ID 硬编码判定大小核；
    /// 3. 快照 governor/min/max；写 schedutil、min 压到硬件最低；
    /// 4. 按 `tier`（rules.yaml 生效模式换算的档位）写各核心组 max 上限。
    pub fn init_policies(&mut self, cfg: &SpecialTunedConfig, tier: u32) {
        self.release();
        self.cfg = cfg.clone();
        self.cfg.normalize();
        let tier = tier.clamp(1, TIER_COUNT as u32);
        self.current_tier = tier;

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

            let current_max = snap_max(&self.cfg, tier, name, &freqs);
            let mut max_writer = FastWriter::new(max_path.clone());
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

            // 写当前档位的 max 上限
            let mut written_max = 0u32;
            if let Some(max) = current_max {
                written_max = max;
                let _ = max_writer.write_value_force(max);
            }

            self.clusters.push(ClusterState {
                policy_id: pid,
                core_name: name.to_string(),
                available_freqs: freqs,
                current_max,
                max_writer,
                written_max,
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

    /// 热重载：rules.yaml 模式变化（`tier` 重新换算）或 akmode.yaml 参数变化后，
    /// 重新吸附并重写当前档位的 max 上限，档位由调用方传入的 `tier` 决定。
    pub fn reload_config(&mut self, cfg: &SpecialTunedConfig, tier: u32) {
        self.cfg = cfg.clone();
        self.cfg.normalize();
        let tier = tier.clamp(1, TIER_COUNT as u32);
        self.current_tier = tier;
        for cluster in self.clusters.iter_mut() {
            cluster.current_max = snap_max(
                &self.cfg,
                tier,
                &cluster.core_name,
                &cluster.available_freqs,
            );
            if let Some(max) = cluster.current_max {
                if max != cluster.written_max {
                    if cluster.max_writer.write_value_force(max) {
                        cluster.written_max = max;
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
        debug!(
            "{}",
            t_with_args(
                "akmode-config-reloaded",
                &fluent_args!(
                    "mode" => crate::chiri::config::tier_to_mode(tier).to_string()
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
