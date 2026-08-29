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

use crate::chiri::config::{SpecialTunedConfig, SpecialTunedGroup};
use crate::utils::FastWriter;
use log::{debug, info, warn};
use std::fs;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
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

/// 单个 policy 的运行时状态：动态 max 在频率表中的位置。
struct ClusterState {
    policy_id: i32,
    /// 核心组名：little / big / prime
    core_name: String,
    /// 内核可用频率（kHz，升序去重），升降 max 在此表中逐档移动
    available_freqs: Vec<u32>,
    max_writer: FastWriter,
    /// 当前 max 在 available_freqs 中的下标
    cur_max_idx: usize,
    /// 当前设定的 max（kHz）
    current_max: u32,
}

/// 按 affected_cpus 的 CPU ID 判定核心组，8550 固定分布，直接写死：
/// 0-2 小核 little，3-6 大核 big，7 超大核 prime。
fn core_name_for(affected: &[usize]) -> Option<&'static str> {
    let first = affected.iter().copied().min()?;
    match first {
        0..=2 => Some("little"),
        3..=6 => Some("big"),
        7 => Some("prime"),
        _ => None,
    }
}

/// 明日方舟特调（akmode）控制器：独立于 CLG 的动态限频调度。
/// 档位由 rules.yaml 生效模式决定（不自动切换），档位差异仅在升降频策略参数，
/// 所有档位的 max 上限/下限均为硬件上下限。激活时统一内核调速器为 schedutil、
/// min 压到硬件最低、max 为硬件最高；之后用本档策略参数按负载升降 max
/// （scaling_max_freq 在内核频率表中逐档移动，可升到硬件最高、降到硬件最低）——
/// schedutil 在 [硬件最低, 动态max] 内自由调频。
pub struct AkmodeGovernor {
    cfg: SpecialTunedConfig,
    /// 特调激活共享标志：Monitor 层（cpu_monitor）据此切换采样间隔（特调 40ms / 其余 120ms）
    ak_active: Arc<AtomicBool>,
    clusters: Vec<ClusterState>,
    /// 各 policy 的 governor/min/max 快照，release 时恢复
    restore: Vec<PolicyRestore>,
    active: bool,
    /// 当前生效档位 1..=4（由 rules.yaml 模式决定，固定不切换）
    current_tier: u32,
    /// 待执行的升降方向（1=升，0=降），防抖等待中
    pending_dir: Option<u8>,
    /// 待执行方向第一次被检测到的时间
    pending_since: Option<Instant>,
    /// 升降频后防抖等待临时减半的截止时间：到点前 wait_ms 按一半执行
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
            pending_dir: None,
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
    /// 3. 快照 governor/min/max；写 schedutil、min 压到硬件最低；
    /// 4. 初始 max = 硬件最高（所有档位都能用硬件最高档位）。
    pub fn init_policies(&mut self, cfg: &SpecialTunedConfig, tier: u32) {
        self.release();
        self.cfg = cfg.clone();
        self.cfg.normalize();
        let tier = tier.clamp(1, TIER_COUNT as u32);
        self.current_tier = tier;
        self.pending_dir = None;
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

            // 初始 max = 硬件最高（所有档位都能使用硬件的最高档位）
            let cur_max_idx = freqs.len() - 1;
            let current_max = freqs[cur_max_idx];
            let _ = max_writer.write_value_force(current_max);

            self.clusters.push(ClusterState {
                policy_id: pid,
                core_name: name.to_string(),
                available_freqs: freqs,
                max_writer,
                cur_max_idx,
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
        self.pending_dir = None;
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

    /// 热重载：rules.yaml 模式变化（`tier` 重新换算）或 akmode.yaml 参数变化后，
    /// 更新档位与策略参数（max 动态状态保持不变），当前档位由调用方传入的 `tier` 决定。
    pub fn reload_config(&mut self, cfg: &SpecialTunedConfig, tier: u32) {
        self.cfg = cfg.clone();
        self.cfg.normalize();
        let tier = tier.clamp(1, TIER_COUNT as u32);
        self.current_tier = tier;
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

    /// 动态限频入口，每个 SystemLoadUpdate（特调 40ms）触发一次。
    /// 用当前档位的策略参数按核心组统计忙/闲核心数：
    ///   升频 = 任一组内超过 up_core_count 个核心占用率 > up_util_percent
    ///   降频 = 任一组内超过 down_core_count 个核心占用率 < down_util_percent
    /// 升频优先于降频；条件持续成立并等 wait_ms（升降频后临时减半）再执行：
    ///   升频：先检查实际频率（scaling_cur_freq）是否已达当前设定的 max，达到才在频率表中升一档
    ///   降频：直接在频率表中降一档
    pub fn on_load_update(&mut self, core_utils: &[f32]) {
        if !self.active {
            return;
        }

        // 档位配置克隆成局部值，避免与 &mut self 借用冲突
        let tc = self.cfg.tier(self.current_tier).clone();

        // 核心组按 CPU ID 区间硬编码（8550：0-2 小核 little、3-6 大核 big、7 超大核 prime）
        struct GroupStat<'a> {
            g: &'a SpecialTunedGroup,
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

        // 升频优先于降频
        let desired_dir = if up_hit {
            Some(1u8)
        } else if down_hit {
            Some(0u8)
        } else {
            None
        };

        let now = Instant::now();
        let fast_wait = self.fast_wait_until.map_or(false, |until| now < until);
        let wait = if fast_wait {
            tc.wait_ms / 2
        } else {
            tc.wait_ms
        };

        match desired_dir {
            Some(d) if self.pending_dir == Some(d) => {
                if let Some(since) = self.pending_since {
                    if now.duration_since(since).as_millis() as u64 >= wait {
                        if d == 1 {
                            self.raise_max();
                        } else {
                            self.lower_max();
                        }
                        // 升降频后启动临时加速窗口：此后 wait_ms 减半执行
                        self.fast_wait_until = Some(
                            Instant::now()
                                + Duration::from_millis(self.cfg.after_change_duration_ms),
                        );
                        self.pending_dir = None;
                        self.pending_since = None;
                    }
                }
            }
            Some(d) => {
                self.pending_dir = Some(d);
                self.pending_since = Some(now);
            }
            None => {
                self.pending_dir = None;
                self.pending_since = None;
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

    /// 升 max：逐 policy 检查实际频率是否已达当前设定的 max（schedutil 余量），
    /// 达到才在频率表中升一档（上限硬件最高）。
    fn raise_max(&mut self) {
        for c in &mut self.clusters {
            if c.cur_max_idx + 1 >= c.available_freqs.len() {
                continue; // 已到硬件最高
            }
            let cur_freq = Self::read_cur_freq(c.policy_id).unwrap_or(c.current_max);
            if cur_freq < c.current_max {
                // schedutil 未跑满当前 max，先让 schedutil 自然升，不手动抬 max
                debug!(
                    "{}",
                    t_with_args(
                        "akmode-max-skipped",
                        &fluent_args!(
                            "pid" => c.policy_id.to_string(),
                            "name" => c.core_name.clone(),
                            "mode" => crate::chiri::config::tier_to_mode(self.current_tier)
                                .to_string(),
                            "cur_khz" => (cur_freq / 1000).to_string(),
                            "max_khz" => (c.current_max / 1000).to_string()
                        )
                    )
                );
                continue;
            }
            c.cur_max_idx += 1;
            let max = c.available_freqs[c.cur_max_idx];
            c.current_max = max;
            if c.max_writer.write_value_force(max) {
                debug!(
                    "{}",
                    t_with_args(
                        "akmode-max-set",
                        &fluent_args!(
                            "pid" => c.policy_id.to_string(),
                            "name" => c.core_name.clone(),
                            "mode" => crate::chiri::config::tier_to_mode(self.current_tier)
                                .to_string(),
                            "max_khz" => (max / 1000).to_string()
                        )
                    )
                );
            }
        }
    }

    /// 降 max：各 policy 在频率表中直接降一档（下限硬件最低）。
    fn lower_max(&mut self) {
        for c in &mut self.clusters {
            if c.cur_max_idx == 0 {
                continue; // 已到硬件最低
            }
            c.cur_max_idx -= 1;
            let max = c.available_freqs[c.cur_max_idx];
            c.current_max = max;
            if c.max_writer.write_value_force(max) {
                debug!(
                    "{}",
                    t_with_args(
                        "akmode-max-set",
                        &fluent_args!(
                            "pid" => c.policy_id.to_string(),
                            "name" => c.core_name.clone(),
                            "mode" => crate::chiri::config::tier_to_mode(self.current_tier)
                                .to_string(),
                            "max_khz" => (max / 1000).to_string()
                        )
                    )
                );
            }
        }
    }

    /// 读 policy 的当前实际频率（scaling_cur_freq）
    fn read_cur_freq(policy_id: i32) -> Option<u32> {
        let path = format!(
            "/sys/devices/system/cpu/cpufreq/policy{}/scaling_cur_freq",
            policy_id
        );
        fs::read_to_string(&path)
            .ok()
            .and_then(|s| s.trim().parse::<u32>().ok())
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
