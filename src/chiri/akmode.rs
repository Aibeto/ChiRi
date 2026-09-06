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
use std::time::Instant;

use crate::fluent_args;
use crate::i18n::{t, t_with_args};

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

/// 单个 policy 的运行时状态：无档位连续 max 控制。
struct ClusterState {
    /// 核心组名：little / big / prime
    core_name: &'static str,
    /// 内核可用频率（kHz，升序去重），目标上限在该表中就近取档
    available_freqs: Vec<u32>,
    max_writer: FastWriter,
    /// 当前设定的 max（kHz）
    current_max: u32,
    /// 降频保持计时起点（目标首次低于当前上限的时刻）；None = 无待执行降频
    down_since: Option<Instant>,
    /// 降频保持期记录的目标档位：目标变化时重置计时（防止逼近移动目标）
    down_target: u32,
}

/// 按 affected_cpus 的 CPU ID 判定核心组，区间随命中 SoC 变化
/// （8550：0-2 little / 3-6 big / 7 prime；8450：0-3 / 4-6 / 7；8998：0-3 / 4-7 / 无 prime）。
/// 由 common::chiri_core_ranges() 统一提供区间，新增 SoC 只需在那里追加。
fn core_name_for(affected: &[usize]) -> Option<&'static str> {
    let first = affected.iter().copied().min()?;
    let r = crate::common::chiri_core_ranges();
    if r.little.contains(&first) {
        Some("little")
    } else if r.big.contains(&first) {
        Some("big")
    } else if r.prime.contains(&first) {
        Some("prime")
    } else {
        None
    }
}

/// 明日方舟特调（akmode）控制器：独立于 CLG 的动态限频调度，**无档位**。
/// 每个负载 tick 按核心组实时负载直接计算 scaling_max_freq 目标上限：
///   target_ratio = clamp(组内最大核心占用率 × headroom, perf_floor, 1.0)
///   target_max   = 频率表中不低于 ratio × 硬件最高的最低档位
/// 升频立即执行（响应性优先），降频须目标偏离超过 hysteresis 且持续
/// down_hold_ms 才执行（防负载抖动来回改写）。
/// 替代原四档 core-count 阈值方案——后者在「少数线程高占用」负载（如明日方舟
/// 资源校验：一两个线程吃满单核、组内其余核心空闲）下升频条件凑不齐、降频
/// 条件持续满足，max 单边下探到最低频，校验速度严重劣化。
pub struct AkmodeGovernor {
    cfg: SpecialTunedConfig,
    /// 特调激活共享标志：Monitor 层（cpu_monitor）据此切换采样间隔（特调 40ms / 其余 120ms）
    ak_active: Arc<AtomicBool>,
    clusters: Vec<ClusterState>,
    /// 各 policy 的 governor/min/max 快照，release 时恢复
    restore: Vec<PolicyRestore>,
    active: bool,
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
            log_counter: 0,
        }
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    /// 接管全部 cpufreq policy：
    /// 1. 先 release 清掉上一次状态；
    /// 2. 逐个 policy 读可用频率与 affected_cpus，按 CPU ID 判定核心组；
    /// 3. 快照 governor/min/max；写 schedutil、min 压到硬件最低；
    /// 4. 初始 max = 硬件最高（接管瞬间多为场景切换，先给满上限，由负载控制自然回落）。
    ///
    /// 返回 true 表示成功接管，false 表示无可用 cluster（配置错误或硬件不支持）。
    pub fn init_policies(&mut self, cfg: &SpecialTunedConfig) -> bool {
        self.release();
        self.cfg = cfg.clone();
        self.cfg.normalize();

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

            // 初始 max = 硬件最高：冷启动给足余量，稳态后 1-2 个 tick 收到实际需求
            let current_max = *freqs.last().unwrap();
            let _ = max_writer.write_value_force(current_max);

            self.clusters.push(ClusterState {
                core_name: name,
                available_freqs: freqs,
                max_writer,
                current_max,
                down_since: None,
                down_target: 0,
            });
        }

        self.active = !self.clusters.is_empty();
        if self.active {
            info!("{}", t("akmode-init"));
            info!("{}", t("akmode-activated"));
            // 特调激活通知 Monitor 层切换到 40ms 快速采样
            self.ak_active.store(true, Ordering::Relaxed);
        } else {
            warn!("{}", t("akmode-no-clusters"));
        }
        self.active
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

    /// 热重载：akmode.yaml 参数变化后更新控制参数（max 动态状态保持不变）。
    pub fn reload_config(&mut self, cfg: &SpecialTunedConfig) {
        self.cfg = cfg.clone();
        self.cfg.normalize();
        debug!("{}", t("akmode-config-reloaded"));
    }

    /// 频率表中找「不低于 ratio × 硬件最高」的最低档位：
    /// 升频响应性优先，落点略高于理论值无妨；降频由 hysteresis + hold 防抖兜住。
    fn freq_for_ratio(freqs: &[u32], ratio: f32) -> u32 {
        let hw_max = *freqs.last().unwrap_or(&0);
        let want = (hw_max as f32 * ratio) as u32;
        freqs.iter().copied().find(|&f| f >= want).unwrap_or(hw_max)
    }

    /// 无档位动态限频入口，每个 SystemLoadUpdate（特调 40ms）触发一次。
    /// 每个核心组独立决策（decision 语义与 CLG 对齐）：
    ///   up        目标 > 当前 max + hysteresis，立即上调；
    ///   down_wait 目标 < 当前 max − hysteresis，保持计时中（down_hold_ms）；
    ///   down      保持期满，上限收到目标档位；
    ///   hold      目标在死区内（或保持期内目标回升，取消等待）。
    pub fn on_load_update(&mut self, core_utils: &[f32]) {
        if !self.active {
            return;
        }

        let ranges = crate::common::chiri_core_ranges();
        let now = Instant::now();
        let cfg = self.cfg.clone();

        // devimp tick 行数据（每核心组一行）：cluster / util / decision / cur_max / hw_max
        let mut devimp_rows: Vec<(&'static str, String, &str, u32, u32)> = Vec::new();

        for c in &mut self.clusters {
            let range: &std::ops::Range<usize> = if c.core_name == "little" {
                &ranges.little
            } else if c.core_name == "big" {
                &ranges.big
            } else {
                &ranges.prime
            };
            let group_util = range
                .clone()
                .filter_map(|cpu| core_utils.get(cpu).copied())
                .fold(0.0_f32, f32::max);

            let hw_max = *c.available_freqs.last().unwrap_or(&0);
            let target_ratio = (group_util * cfg.headroom).clamp(cfg.perf_floor, 1.0);
            let target_max = Self::freq_for_ratio(&c.available_freqs, target_ratio);
            let hyst_freq = (hw_max as f32 * cfg.hysteresis) as u32;

            let decision;
            if target_max > c.current_max + hyst_freq {
                // 升频：立即执行（响应性优先），并取消进行中的降频等待。
                // 写成功才前移状态：失败时下一 tick target 仍超死区 → up 重试
                //（与 CLG「写失败下次 tick 自动重试」语义对齐）
                if c.max_writer.write_value_force(target_max) {
                    c.current_max = target_max;
                }
                c.down_since = None;
                decision = "up";
            } else if target_max < c.current_max.saturating_sub(hyst_freq) {
                // 降频：目标须持续 down_hold_ms 才执行；目标档位变化时重置计时
                match c.down_since {
                    None => {
                        c.down_since = Some(now);
                        c.down_target = target_max;
                        decision = "down_wait";
                    }
                    Some(_) if c.down_target != target_max => {
                        c.down_since = Some(now);
                        c.down_target = target_max;
                        decision = "down_wait";
                    }
                    Some(since) => {
                        if now.duration_since(since).as_millis() as u64 >= cfg.down_hold_ms {
                            // 写成功才前移状态并结束等待：失败保留计时起点，
                            // 下一 tick（elapsed 仍满）立即重试
                            if c.max_writer.write_value_force(target_max) {
                                c.current_max = target_max;
                                c.down_since = None;
                            }
                            decision = "down";
                        } else {
                            decision = "down_wait";
                        }
                    }
                }
            } else {
                // 死区内：目标与当前上限一致，取消进行中的降频等待
                c.down_since = None;
                decision = "hold";
            }

            devimp_rows.push((
                c.core_name,
                format!("{:.2}", group_util),
                decision,
                c.current_max,
                hw_max,
            ));
        }

        // devimp tick 行（开发记录开启时才有 IO）：
        // cur_freq_khz 列写当前动态 max（kHz），max_freq_khz 列写硬件最高（kHz），
        // 与旧版一致；over/under 列无档位阈值语义，恒 0。
        if crate::logger::devimp_active() {
            for (name, util, decision, cur_max, hw_max) in &devimp_rows {
                crate::logger::devimp_tick(
                    name,
                    util,
                    0,
                    0,
                    "-",
                    "-",
                    &cur_max.to_string(),
                    &hw_max.to_string(),
                    decision,
                    0,
                    0,
                    "-",
                    false,
                );
            }
        }

        self.log_counter += 1;
        // debug 心跳（独立于 devimp，日志通道随时可用）：每 25 tick 汇总各簇
        // util/max，供 devimp 关闭时观测负载直拉行为
        if self.log_counter % 25 == 0 && log::log_enabled!(log::Level::Debug) {
            let summary = devimp_rows
                .iter()
                .map(|(name, util, _d, cur_max, _hw)| {
                    format!("{}={}MHz({})", name, cur_max / 1000, util)
                })
                .collect::<Vec<_>>()
                .join(" ");
            debug!(
                "{}",
                t_with_args("akmode-tick-log", &fluent_args!("state" => summary))
            );
        }
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
