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

/// 极速模式（fast）专属锁频器：与 CLG 完全独立，不读 yaml 调频参数。
/// 接管时把所有 cluster 的 scaling_min_freq / scaling_max_freq 都锁到硬件最高频
/// （min=max=hw_max，schedutil 无调频空间），release 时恢复系统原始状态。
/// 每 5 秒重写一次频率，防止系统/厂商守护进程篡改。

use crate::utils::FastWriter;
use log::{debug, info, warn};
use std::fs;

use crate::fluent_args;
use crate::i18n::{t, t_with_args};

/// 每 5 秒重写一次频率，防止外部篡改
const REWRITE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);

/// 接管前的系统状态快照，release 时恢复
struct PolicySnapshot {
    policy_id: i32,
    governor: Option<String>,
    min_freq: Option<u32>,
    max_freq: Option<u32>,
    hw_max: u32,
}

/// 单个 policy 的锁频状态
struct LockedPolicy {
    policy_id: i32,
    hw_max: u32,
    max_writer: FastWriter,
    min_writer: FastWriter,
    /// 已成功写入的频率（kHz），与 hw_max 一致；0 表示尚未写入成功
    current_freq: u32,
}

/// 极速模式锁频器
pub struct FastLock {
    policies: Vec<LockedPolicy>,
    snapshots: Vec<PolicySnapshot>,
    active: bool,
    last_write: std::time::Instant,
}

impl FastLock {
    pub fn new() -> Self {
        Self {
            policies: Vec::new(),
            snapshots: Vec::new(),
            active: false,
            last_write: std::time::Instant::now(),
        }
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    /// 接管全部 cpufreq policy：读取可用频率、快照原始状态、写 schedutil + 锁 hw_max。
    pub fn init(&mut self) {
        self.release();

        let clusters = crate::chiri::get_cpu_policies();

        for policy in &clusters {
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

            // 合并 boost 频率
            if !policy.boost_frequencies.is_empty() {
                freqs.extend(&policy.boost_frequencies);
                freqs.sort_unstable();
                freqs.dedup();
            }

            let hw_max = *freqs.last().unwrap();

            // 快照原始状态（release 时恢复）
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
                hw_max,
            });

            // 写 schedutil governor（CLG 与 fast 统一）
            let _ = crate::utils::try_write_file(&gov_path, "schedutil");

            let max_writer = FastWriter::new(format!(
                "/sys/devices/system/cpu/cpufreq/policy{}/scaling_max_freq",
                pid
            ));
            let min_writer = FastWriter::new(format!(
                "/sys/devices/system/cpu/cpufreq/policy{}/scaling_min_freq",
                pid
            ));
            if !max_writer.is_valid() || !min_writer.is_valid() {
                warn!(
                    "{}",
                    t_with_args(
                        "fast-writer-invalid",
                        &fluent_args!(
                            "pid" => pid.to_string(),
                            "max_valid" => max_writer.is_valid().to_string(),
                            "min_valid" => min_writer.is_valid().to_string()
                        )
                    )
                );
                continue;
            }

            let mut locked = LockedPolicy {
                policy_id: pid,
                hw_max,
                max_writer,
                min_writer,
                current_freq: 0,
            };
            // 首次锁频：max 先拉高再拉 min（升频写序，保证 min <= max）
            let ok = locked.max_writer.write_value_force(hw_max)
                && locked.min_writer.write_value_force(hw_max);
            if ok {
                locked.current_freq = hw_max;
            }

            info!(
                "{}",
                t_with_args(
                    "fast-init",
                    &fluent_args!(
                        "pid" => pid.to_string(),
                        "max_khz" => (hw_max / 1000).to_string()
                    )
                )
            );
            self.policies.push(locked);
        }

        self.active = !self.policies.is_empty();
        self.last_write = std::time::Instant::now();
        if self.active {
            info!("{}", t("fast-activated"));
        }
    }

    /// 释放接管：恢复系统原始 governor / min / max。
    pub fn release(&mut self) {
        if self.active {
            info!("{}", t("fast-deactivated"));
        }
        self.snapshots.retain(|r| !Self::restore_policy(r));
        self.policies.clear();
        self.active = false;
    }

    /// 每 5 秒重写一次 hw_max，防止系统/厂商守护进程篡改频率。
    /// 由 scheduler_ipc 在事件循环中调用。
    pub fn tick(&mut self) {
        if !self.active {
            return;
        }
        if self.last_write.elapsed() < REWRITE_INTERVAL {
            return;
        }
        self.last_write = std::time::Instant::now();

        for p in &mut self.policies {
            // 直接写 hw_max，不走 current_freq 去重——tick 本身就是"重写"
            let ok = p.max_writer.write_value_force(p.hw_max)
                && p.min_writer.write_value_force(p.hw_max);
            if ok {
                p.current_freq = p.hw_max;
            }
            debug!(
                "{}",
                t_with_args(
                    "fast-rewrite",
                    &fluent_args!(
                        "pid" => p.policy_id.to_string(),
                        "max_khz" => (p.hw_max / 1000).to_string()
                    )
                )
            );
        }
    }

    /// 将单个 policy 恢复为接管前的原始状态。
    /// 返回是否全部写入成功；失败时返回 false，调用方保留快照以便重试。
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
        // 恢复写序保证任意中间状态满足 min <= max：
        // 1) governor；2) max 先放宽到 hw_max；3) min；4) max
        if let Some(gov) = &r.governor {
            if crate::utils::write_to_file(&gov_path, gov.as_bytes()).is_err() {
                all_ok = false;
            }
        }
        if crate::utils::write_to_file(&max_path, r.hw_max.to_string()).is_err() {
            all_ok = false;
        }
        if let Some(min) = r.min_freq {
            if crate::utils::write_to_file(&min_path, min.to_string()).is_err() {
                all_ok = false;
            }
        }
        if let Some(max) = r.max_freq {
            if crate::utils::write_to_file(&max_path, max.to_string()).is_err() {
                all_ok = false;
            }
        }

        debug!(
            "{}",
            t_with_args(
                "fast-restore",
                &fluent_args!(
                    "pid" => r.policy_id.to_string(),
                    "governor" => r.governor.clone().unwrap_or_else(|| "<unread>".to_string()),
                    "min" => r.min_freq.map(|v| v.to_string()).unwrap_or_else(|| "<unread>".to_string()),
                    "max" => r.max_freq.map(|v| v.to_string()).unwrap_or_else(|| "<unread>".to_string())
                )
            )
        );
        all_ok
    }
}
