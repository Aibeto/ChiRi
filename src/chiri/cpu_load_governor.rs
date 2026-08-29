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

use crate::chiri::config::CpuLoadGovernorConfig;
use crate::utils::FastWriter;
use log::{debug, info, warn};
use std::fs;
use std::time::{Duration, Instant};

use crate::fluent_args;
use crate::i18n::{t, t_with_args};

// ════════════════════════════════════════════════════════════════
//  PolicyRestore — CLG 接管前的系统状态快照，release 时恢复
// ════════════════════════════════════════════════════════════════

struct PolicyRestore {
    /// cpufreq policy 编号
    policy_id: i32,
    /// 接管前的 scaling_governor；读取失败时为 None，恢复时跳过该字段，不写退化值
    governor: Option<String>,
    /// 接管前的 scaling_min_freq（kHz）；读取失败为 None，恢复时跳过
    min_freq: Option<u32>,
    /// 接管前的 scaling_max_freq（kHz）；读取失败为 None，恢复时跳过
    max_freq: Option<u32>,
    /// 该 policy 的硬件最大可用频率，恢复时先放宽上限到它
    hw_max: u32,
}

// ════════════════════════════════════════════════════════════════
//  ClusterState — 单 cluster 运行时状态
// ════════════════════════════════════════════════════════════════

struct ClusterState {
    /// cpufreq policy 编号
    policy_id: i32,
    /// 受该 policy 管理的 CPU id 列表（从 affected_cpus 解析）
    affected_cpus: Vec<usize>,
    /// 可用频率档位（kHz，含 boost 频率，升序去重）
    available_freqs: Vec<u32>,
    /// 频率 -> [0,1] 性能比例的缓存：(f - fmin) / (fmax - fmin)
    cached_ratios: Vec<f32>,
    /// 该 cluster 的 boost 最高频率（kHz），无 boost 文件则为 0
    boost_max: u32,
    /// scaling_max_freq 写通道缓存
    max_writer: FastWriter,
    /// scaling_min_freq 写通道缓存
    min_writer: FastWriter,
    /// 当前目标性能比 [0,1]，调频时换算成频率档位
    current_perf: f32,
    /// 当前已成功写入的频率（kHz）；0 表示尚未写入成功，下次 tick 自动重试
    current_freq: u32,
    /// 降频确认计数：连续满 down_rate_limit_ticks 才执行降频
    down_wait: u32,
    /// 升频确认计数：连续满 up_rate_limit_ticks 才执行升频
    up_wait: u32,
    /// 上一 tick 的原始 max_util，用于尖峰跳升检测
    last_util: f32,
}

impl ClusterState {
    /// 把目标性能比映射到最近的可用频率档位（基于 cached_ratios 二分查找最近点）
    fn find_nearest_freq(&self, target_ratio: f32) -> u32 {
        let idx = self.cached_ratios.partition_point(|&r| r < target_ratio);
        if idx == 0 {
            self.available_freqs[0]
        } else if idx >= self.available_freqs.len() {
            *self.available_freqs.last().unwrap()
        } else {
            let lo = idx - 1;
            let hi = idx;
            if (self.cached_ratios[hi] - target_ratio).abs()
                < (self.cached_ratios[lo] - target_ratio).abs()
            {
                self.available_freqs[hi]
            } else {
                self.available_freqs[lo]
            }
        }
    }

    /// 写入目标频率（锁频：scaling_max_freq 与 scaling_min_freq 都设为同一值）。
    /// 升频先拉 max 再拉 min，降频先降 min 再降 max，保证任意中间状态满足 min <= max。
    /// 锁频配合 schedutil governor：min=max 时 schedutil 无调频空间，频率由本控制器决定。
    fn write_freq(&mut self, freq: u32) {
        if freq == self.current_freq {
            return;
        }
        let old_freq = self.current_freq;
        let ok = if freq >= self.current_freq {
            // 升频：先拉高 max 再拉高 min
            let ok_max = self.max_writer.write_value_force(freq);
            let ok_min = self.min_writer.write_value_force(freq);
            ok_max && ok_min
        } else {
            // 降频：先降 min 再降 max
            let ok_min = self.min_writer.write_value_force(freq);
            let ok_max = self.max_writer.write_value_force(freq);
            ok_max && ok_min
        };
        // 仅在两端均写入成功时更新缓存，失败则下次 tick 自动重试
        if ok {
            self.current_freq = freq;
            debug!(
                "{}",
                t_with_args(
                    "clg-freq-set",
                    &fluent_args!(
                        "pid" => self.policy_id.to_string(),
                        "old_khz" => (old_freq / 1000).to_string(),
                        "new_khz" => (freq / 1000).to_string()
                    )
                )
            );
        } else {
            debug!(
                "{}",
                t_with_args(
                    "clg-freq-write-failed-cached",
                    &fluent_args!(
                        "pid" => self.policy_id.to_string(),
                        "target_khz" => (freq / 1000).to_string(),
                        "cached_khz" => (self.current_freq / 1000).to_string()
                    )
                )
            );
        }
    }

    /// 取该 cluster 受影响 CPU 中的最大利用率（利用率来源为 eBPF 各核心负载）
    fn max_util(&self, core_utils: &[f32]) -> f32 {
        self.affected_cpus
            .iter()
            .filter_map(|&cpu| core_utils.get(cpu))
            .copied()
            .fold(0.0_f32, f32::max)
    }

    /// 频率（kHz）→ [0,1] 性能比例，与 cached_ratios 同口径：(f - fmin)/(fmax - fmin)。
    /// 供“直接降频为当前实际频率”时同步 current_perf，避免状态与实际写入脱节。
    fn ratio_of_freq(&self, freq: u32) -> f32 {
        let fmin = *self.available_freqs.first().unwrap_or(&0) as f32;
        let fmax = *self.available_freqs.last().unwrap_or(&0) as f32;
        ((freq as f32) - fmin) / (fmax - fmin).max(1.0)
    }
}

// ════════════════════════════════════════════════════════════════
//  CpuLoadGovernor — 主控制器
// ════════════════════════════════════════════════════════════════

pub struct CpuLoadGovernor {
    /// 各 cluster 的运行时状态（init_policies 时构建）
    clusters: Vec<ClusterState>,
    /// CLG 接管前的系统状态，release 时恢复（首次 init 时捕获）
    restore: Vec<PolicyRestore>,
    /// 当前生效的 CLG 配置（normalize 后的副本）
    cfg: CpuLoadGovernorConfig,
    /// 是否处于接管状态（至少一个 cluster 初始化成功才为 true）
    active: bool,
    /// 调试日志周期计数（每 25 tick 输出一次 clg-tick-log 摘要）
    log_counter: u32,
    /// 触摸升频窗口截止时间：窗口内大核性能下限锁定到 touch_boost_floor
    touch_boost_until: Option<Instant>,
    /// 触摸升频窗口内大核性能下限（由窗口开始时大核频率 + touch_boost_tiers 档换算）
    touch_boost_floor: f32,
}

impl CpuLoadGovernor {
    /// 创建空的控制器（未激活、未接管任何 policy）。
    /// 触摸升频由事件驱动：scheduler_ipc 收到触摸事件后调用 `on_touch()`。
    pub fn new() -> Self {
        Self {
            clusters: Vec::new(),
            restore: Vec::new(),
            cfg: CpuLoadGovernorConfig::default(),
            active: false,
            log_counter: 0,
            touch_boost_until: None,
            touch_boost_floor: 0.0,
        }
    }

    /// CLG 当前是否已接管 CPU 频率
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// 接管全部 cpufreq policy：
    /// 1. 先 release 清掉上一次接管状态；
    /// 2. 逐个 policy 读取可用频率、记录原始状态快照；
    /// 3. 写入 schedutil governor（统一内核调速器），并按 perf_init 锁到初始频率；
    /// 4. 任一 cluster 初始化成功即标记 active。
    pub fn init_policies(&mut self, gov_cfg: &CpuLoadGovernorConfig) {
        self.release();
        self.cfg = gov_cfg.clone();
        self.normalize_cfg();

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

            // 合并 boost 频率（部分平台额外暴露的高频点），去重排序
            if !policy.boost_frequencies.is_empty() {
                freqs.extend(&policy.boost_frequencies);
                freqs.sort_unstable();
                freqs.dedup();
            }

            let affected = Self::read_affected_cpus(pid);
            if affected.is_empty() {
                continue;
            }

            let fmin = *freqs.first().unwrap() as f32;
            let fmax = *freqs.last().unwrap() as f32;
            let range = (fmax - fmin).max(1.0);
            let cached_ratios: Vec<f32> =
                freqs.iter().map(|&f| (f as f32 - fmin) / range).collect();

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
                        "clg-writer-invalid",
                        &fluent_args!(
                            "pid" => pid.to_string(),
                            "max_valid" => max_writer.is_valid().to_string(),
                            "min_valid" => min_writer.is_valid().to_string()
                        )
                    )
                );
                continue;
            }

            // 记录系统原始状态（每个将被接管的 policy 单独记录），release 时恢复。
            // 必须位于 governor 写入之前，确保 release 能还原所有被接管的 cluster。
            // 读取失败记录为 None：恢复时跳过对应字段，避免写退化值（如 0）。
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
            // 同 policy 只保留最新快照（覆盖上次恢复失败遗留的旧记录）
            self.restore.retain(|r| r.policy_id != pid);
            self.restore.push(PolicyRestore {
                policy_id: pid,
                governor,
                min_freq,
                max_freq,
                hw_max: *freqs.last().unwrap(),
            });

            let _ = crate::utils::try_write_file(&gov_path, "schedutil");

            let init_perf = self
                .cfg
                .perf_init
                .clamp(self.cfg.perf_floor, self.cfg.perf_ceil);
            let boost_max = policy.boost_frequencies.iter().copied().max().unwrap_or(0);
            let mut cluster = ClusterState {
                policy_id: pid,
                affected_cpus: affected.clone(),
                available_freqs: freqs,
                cached_ratios,
                boost_max,
                max_writer,
                min_writer,
                current_perf: init_perf,
                current_freq: 0,
                down_wait: 0,
                up_wait: 0,
                last_util: 0.0,
            };

            let init_freq = cluster.find_nearest_freq(init_perf);
            let init_ok = cluster.max_writer.write_value_force(init_freq)
                && cluster.min_writer.write_value_force(init_freq);
            // 仅两端均写入成功才缓存频率；失败保持 0，下次 tick write_freq 自动重试
            if init_ok {
                cluster.current_freq = init_freq;
            }

            info!(
                "{}",
                t_with_args(
                    "clg-init",
                    &fluent_args!(
                        "pid" => pid.to_string(),
                        "cpus" => format!("{:?}", affected),
                        "fmin" => (fmin / 1000.0).to_string(),
                        "fmax" => (fmax / 1000.0).to_string(),
                        "perf" => format!("{:.2}", init_perf),
                        "freq" => (init_freq / 1000).to_string()
                    )
                )
            );

            self.clusters.push(cluster);
        }

        self.active = !self.clusters.is_empty();
        if self.active {
            info!(
                "{}",
                t_with_args(
                    "clg-activated",
                    &fluent_args!("count" => self.clusters.len().to_string())
                )
            );
        } else {
            warn!("{}", t("clg-no-clusters"));
        }
    }

    /// 释放接管：把每个 policy 恢复到接管前快照（governor / min / max），随后清空状态。
    /// 恢复失败的条目保留在快照中，下次 release / init 时继续重试，避免静默漂移。
    pub fn release(&mut self) {
        if self.active {
            info!("{}", t("clg-deactivated"));
        }
        // 恢复系统原始状态，避免 release 后 CPU 悬停在 CLG 最后写入的值上。
        // 恢复失败的条目保留，下次 release/init 时重试，避免静默漂移。
        self.restore.retain(|r| !Self::restore_policy(r));
        self.clusters.clear();
        self.active = false;
        self.log_counter = 0;
        // 清空触摸升频状态，避免配置切换/释放后残留窗口继续抬大地核下限
        self.touch_boost_until = None;
        self.touch_boost_floor = 0.0;
    }

    /// 热切换配置：仅替换控制参数，不重建 cluster（用于同模式参数调整、
    /// doze/scenemode 切换与模式变更）。
    /// 新配置同样先过 normalize，防止越界参数导致后续 clamp panic。
    ///
    /// 切换后把各 cluster 的 current_perf 重置为新配置的 perf_init 并立即 flush 写频：
    /// 若只替换参数，current_perf 仍停在旧配置的档位——息屏 doze/scenemode 期间
    /// current_perf 已掉到 ~0，亮屏恢复原模式后频率要从地板缓慢爬升数秒，
    /// 表现为"亮屏了却还卡在 scenemode 低频"（perf_init 相当于 init_policies 的接管起点）。
    pub fn reload_config(&mut self, gov_cfg: &CpuLoadGovernorConfig) {
        self.cfg = gov_cfg.clone();
        self.normalize_cfg();
        // 重置到新配置的初始性能档并清空旧配置残留的防抖计数，随后立即写频
        let init_perf = self
            .cfg
            .perf_init
            .clamp(self.cfg.perf_floor, self.cfg.perf_ceil);
        for cluster in &mut self.clusters {
            cluster.current_perf = init_perf;
            cluster.up_wait = 0;
            cluster.down_wait = 0;
        }
        self.flush();
        debug!(
            "{}",
            t_with_args(
                "clg-config-reloaded",
                &fluent_args!(
                    "up" => format!("{:.2}", self.cfg.up_threshold),
                    "down" => format!("{:.2}", self.cfg.down_threshold),
                    "floor" => format!("{:.2}", self.cfg.perf_floor),
                    "ceil" => format!("{:.2}", self.cfg.perf_ceil)
                )
            )
        );
    }

    /// 校验并规范化配置：防止 perf_floor > perf_ceil / NaN 导致 f32::clamp panic
    fn normalize_cfg(&mut self) {
        let floor = self.cfg.perf_floor;
        let ceil = self.cfg.perf_ceil;
        if floor.is_finite() && ceil.is_finite() && floor > ceil {
            warn!(
                "{}",
                t_with_args(
                    "clg-perf-clamped",
                    &fluent_args!(
                        "floor" => format!("{:.2}", floor),
                        "ceil" => format!("{:.2}", ceil)
                    )
                )
            );
        }
        self.cfg.normalize();
    }

    /// 将单个 policy 恢复为接管前的原始状态。
    /// 返回是否全部写入成功：失败时返回 false，调用方保留快照以便重试。
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
        // 写序保证任意中间状态均满足 min <= max：
        // 1) 恢复 governor（读取失败为 None 时跳过，保持现状）；
        // 2) 上限先放宽到硬件最大值（恒 >= 当前下限）；
        // 3) 恢复下限；4) 恢复上限。各步失败均记录，由调用方决定重试。
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
                "clg-restore",
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

    /// 决策入口：每次 SystemLoadUpdate（CLG 常规 160ms）触发，只计算目标性能比，不写 sysfs。
    /// 对每个 cluster：
    /// 1. 取本簇最大 util，做尖峰抑制；
    /// 2. 按 headroom 过渡带计算目标性能比并 clamp 到 floor..ceil；
    /// 3. 根据与当前 perf 的高低差走升/降频分支：
    ///    - 升频带速率限制 + schedutil 余量检查（实际频率未达当前锁定值时忽略一次升频，
    ///      等硬件自然追平，实现按需升频）；高负载/大跳变全速升频
    ///    - 降频为“直接降频”：防抖确认后一步到位写目标档，目标不高于当前实际频率
    /// 写频由统一的 backset 入口 `flush()` 完成（触摸升频也经它立即写频）。
    pub fn on_load_update(&mut self, core_utils: &[f32]) {
        if !self.active {
            return;
        }

        for cluster in &mut self.clusters {
            let raw_util = cluster.max_util(core_utils);
            // 尖峰抑制：单 tick 跳升超过阈值时衰减其增量，
            // 孤立瞬时尖峰（如单核 0↔100%）不瞬间拉满 perf；
            // 持续负载下一 tick jump 归零即全量生效，不拖慢真实升频
            let util = if raw_util > cluster.last_util + self.cfg.spike_jump_threshold {
                cluster.last_util + (raw_util - cluster.last_util) * self.cfg.spike_decay
            } else {
                raw_util
            };
            cluster.last_util = raw_util;

            // headroom 在 up_threshold 附近线性过渡，避免阶跃导致的振荡
            let ramp_start = self.cfg.up_threshold - self.cfg.headroom_ramp;
            let headroom = if util >= self.cfg.up_threshold {
                self.cfg.headroom_factor
            } else if util > ramp_start {
                let t = ((util - ramp_start) / self.cfg.headroom_ramp.max(1e-6)).clamp(0.0, 1.0);
                1.0 + (self.cfg.headroom_factor - 1.0) * t
            } else {
                1.0
            };

            let target_perf = (util * headroom).clamp(self.cfg.perf_floor, self.cfg.perf_ceil);
            let old_perf = cluster.current_perf;

            if target_perf > old_perf {
                cluster.down_wait = 0;
                cluster.up_wait += 1;

                // 升频速率限制：必须连续 up_rate_limit_ticks 才执行
                if cluster.up_wait < self.cfg.up_rate_limit_ticks {
                    continue;
                }

                // schedutil 余量检查（按需升频）：实际频率未追平当前锁定频率时，
                // 忽略本次升频，等硬件/调度器自然追平后再升，避免提前抬过实际需求。
                // 触摸升频不受此限制（由 on_touch + flush 独立提升）。
                let cur_freq =
                    Self::read_cur_freq(cluster.policy_id).unwrap_or(cluster.current_freq);
                if cur_freq < cluster.current_freq {
                    debug!(
                        "{}",
                        t_with_args(
                            "clg-up-skipped",
                            &fluent_args!(
                                "pid" => cluster.policy_id.to_string(),
                                "cur_khz" => (cur_freq / 1000).to_string(),
                                "lock_khz" => (cluster.current_freq / 1000).to_string()
                            )
                        )
                    );
                } else {
                    let is_high_load = util >= self.cfg.up_threshold;
                    let is_significant_jump = target_perf > old_perf + self.cfg.up_jump_threshold;

                    if is_high_load || is_significant_jump {
                        cluster.current_perf += (target_perf - old_perf) * self.cfg.smoothing_up;
                    } else {
                        // 滞回带内升频：速率随 util 接近 up_threshold 线性提升——
                        // 低 util 端用 slow_up_scale 防抖，高 util 端逼近全速
                        let span = (self.cfg.up_threshold - self.cfg.down_threshold).max(1e-6);
                        let gap = ((util - self.cfg.down_threshold) / span).clamp(0.0, 1.0);
                        let speed = self.cfg.smoothing_up
                            * (self.cfg.slow_up_scale + (1.0 - self.cfg.slow_up_scale) * gap);
                        cluster.current_perf += (target_perf - old_perf) * speed;
                    }
                }
            } else {
                cluster.up_wait = 0;
                cluster.down_wait += 1;
                // 极低负载立即降频（跳过 down_wait 确认期），否则连续满 down_rate_limit_ticks
                if cluster.down_wait >= self.cfg.down_rate_limit_ticks
                    || util < self.cfg.down_fast_threshold
                {
                    // 直接降频为当前目标频率（敢于降频，能效优先）：
                    // 不做平滑渐变，一步到位写目标档；目标不高于当前实际频率，
                    // 避免降频写序（min 先降）中间态反而瞬时抬升
                    let mut target_freq = cluster.find_nearest_freq(target_perf);
                    if let Some(actual) = Self::read_cur_freq(cluster.policy_id) {
                        if actual < target_freq {
                            target_freq = actual;
                        }
                    }
                    cluster.current_perf = cluster.ratio_of_freq(target_freq);
                }
            }
        }

        self.log_counter += 1;
        if self.log_counter % 25 == 0 {
            for c in &self.clusters {
                debug!(
                    "{}",
                    t_with_args(
                        "clg-tick-log",
                        &fluent_args!(
                            "pid" => c.policy_id.to_string(),
                            "util" => format!("{:.0}", c.max_util(core_utils) * 100.0),
                            "perf" => format!("{:.2}", c.current_perf),
                            "freq" => (c.current_freq / 1000).to_string(),
                            "boost" => format!("{:.0}", c.boost_max as f32 / 1000.0)
                        )
                    )
                );
            }
        }
    }

    /// backset 统一写频入口：把各 cluster 当前目标性能比换算成频率档位并写回 sysfs。
    /// 由 scheduler_ipc 在每次 CLG 决策后调用，也在触摸事件到达时立即调用；
    /// 写频经 FastWriter 按值去重，频率未变化时不产生 sysfs 写入。
    pub fn flush(&mut self) {
        if !self.active {
            return;
        }

        // 触摸升频窗口过期清理
        if let Some(until) = self.touch_boost_until {
            if Instant::now() >= until {
                self.touch_boost_until = None;
                self.touch_boost_floor = 0.0;
            }
        }

        for cluster in &mut self.clusters {
            // 触摸升频窗口内：大核性能下限锁定到提升档位（受开关约束，防残留）
            if self.cfg.touch_boost_enabled
                && self.touch_boost_until.is_some()
                && Self::is_big_cluster(&cluster.affected_cpus)
            {
                cluster.current_perf = cluster.current_perf.max(self.touch_boost_floor);
            }
            cluster.current_perf = cluster
                .current_perf
                .clamp(self.cfg.perf_floor, self.cfg.perf_ceil);
            let target_freq = cluster.find_nearest_freq(cluster.current_perf);
            cluster.write_freq(target_freq);
        }
    }

    /// 触摸事件驱动入口：收到触摸按下事件时把大核频率下限抬高一档，
    /// 并立即由 scheduler_ipc 调用 flush() 写频（不等待下一个 160ms 决策 tick）。
    pub fn on_touch(&mut self) {
        if !self.active || !self.cfg.touch_boost_enabled {
            return;
        }
        self.touch_boost_floor = self.compute_touch_boost_floor();
        self.touch_boost_until =
            Some(Instant::now() + Duration::from_millis(self.cfg.touch_boost_ms));
        debug!(
            "{}",
            t_with_args(
                "clg-touch-boost",
                &fluent_args!(
                    "floor" => format!("{:.2}", self.touch_boost_floor),
                    "ms" => self.cfg.touch_boost_ms.to_string()
                )
            )
        );
    }

    /// 读取 policy 的 affected_cpus，解析成 CPU id 列表（用于 max_util 取负载）
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

    /// 读 policy 的当前实际频率（scaling_cur_freq，kHz）
    fn read_cur_freq(policy_id: i32) -> Option<u32> {
        let path = format!(
            "/sys/devices/system/cpu/cpufreq/policy{}/scaling_cur_freq",
            policy_id
        );
        fs::read_to_string(&path)
            .ok()
            .and_then(|s| s.trim().parse::<u32>().ok())
    }

    /// 判定 cluster 是否覆盖当前 SoC 的大核区间：触摸升频只作用于大核。
    /// 大核区间随命中 SoC 变化（8550：3-6；8450：4-6；8998：4-7），
    /// 由 common::chiri_core_ranges() 统一提供。
    fn is_big_cluster(affected: &[usize]) -> bool {
        let big = crate::common::chiri_core_ranges().big;
        affected.iter().any(|&c| big.contains(&c))
    }

    /// 计算触摸升频的大核性能下限：取各覆盖大核的 cluster 当前频率在可用频率表中
    /// 向上移动 touch_boost_tiers 档后的性能比，取最大值（一个窗口期内保持不变）
    fn compute_touch_boost_floor(&self) -> f32 {
        let mut floor = 0.0_f32;
        for c in &self.clusters {
            if !Self::is_big_cluster(&c.affected_cpus) {
                continue;
            }
            let idx = c
                .available_freqs
                .iter()
                .position(|&f| f == c.current_freq)
                .unwrap_or(0);
            let target_idx = (idx + self.cfg.touch_boost_tiers as usize)
                .min(c.available_freqs.len().saturating_sub(1));
            let r = c.cached_ratios[target_idx];
            if r > floor {
                floor = r;
            }
        }
        floor
    }
}
