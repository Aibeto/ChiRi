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

use crate::fas_types::ClusterProfile;
use crate::utils::FastWriter;
use log::warn;
use std::fs;
use std::time::{Duration, Instant};

use crate::fluent_args;
use crate::i18n::t_with_args;

pub struct PolicyController {
    pub max_writer: FastWriter,
    pub min_writer: FastWriter,
    pub available_freqs: Vec<u32>,
    cached_ratios: Vec<f32>,
    pub current_freq: u32,
    pub policy_id: usize,
    pub cluster_profile: ClusterProfile,
    pub freq_hold_frames: u32,
    pub freq_min: f32,
    pub freq_max: f32,

    verify_freq: Option<u32>,
    verify_timer: Instant,
    /// 写频后校验间隔（来自 FasRulesConfig::verify_freq_interval_secs）
    verify_interval: Duration,

    pub ignore_write: bool,

    /// 接管前该 policy 的原始 governor（load_policies 快照）。
    /// FAS 接管期间把 governor 改写为 performance 以配合 min=max 锁频；
    /// 若退出时不恢复，后续 CLG/akmode 与系统调频都会在 performance 下
    /// 恒跑 max cap——功耗不降且低负载降频节能全部失效（泄漏到所有模式）
    orig_governor: Option<String>,
}

impl PolicyController {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        max_writer: FastWriter,
        min_writer: FastWriter,
        available_freqs: Vec<u32>,
        policy_id: usize,
        cluster_profile: ClusterProfile,
        current_freq: u32,
        orig_governor: Option<String>,
        verify_interval_secs: u32,
    ) -> Self {
        let freq_min = *available_freqs.first().unwrap_or(&0) as f32;
        let freq_max = *available_freqs.last().unwrap_or(&1) as f32;
        let range = (freq_max - freq_min).max(1.0);
        let cached_ratios: Vec<f32> = available_freqs
            .iter()
            .map(|&f| (f as f32 - freq_min) / range)
            .collect();
        Self {
            max_writer,
            min_writer,
            available_freqs,
            cached_ratios,
            current_freq,
            policy_id,
            cluster_profile,
            freq_hold_frames: 0,
            freq_min,
            freq_max,
            verify_freq: None,
            verify_timer: Instant::now(),
            verify_interval: Duration::from_secs(verify_interval_secs.max(1) as u64),
            ignore_write: false,
            orig_governor,
        }
    }

    pub fn find_nearest_freq(&self, target_ratio: f32) -> u32 {
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

    pub fn current_ratio(&self) -> f32 {
        (self.current_freq as f32 - self.freq_min) / (self.freq_max - self.freq_min).max(1.0)
    }

    /// 锁频写入 (min=max)，用于关键 cluster
    pub fn apply_freq_locked(&mut self, target_freq: u32) {
        if self.ignore_write {
            return;
        }
        if target_freq >= self.current_freq {
            self.max_writer.write_value_force(target_freq);
            self.min_writer.write_value_force(target_freq);
        } else {
            self.min_writer.write_value_force(target_freq);
            self.max_writer.write_value_force(target_freq);
        }
        self.current_freq = target_freq;
        self.freq_hold_frames = 2;
        self.do_verify_freq(target_freq);
    }

    fn do_verify_freq(&mut self, write_freq: u32) {
        // 校验间隔来自 verify_freq_interval_secs（默认 1s）：verify 只在
        // 写频事件后触发一次读数（非周期轮询），调小无长期开销，能更快
        // 发现内核频率覆写
        if self.verify_timer.elapsed() >= self.verify_interval {
            self.verify_timer = Instant::now();
            if let Some(expected) = self.verify_freq {
                if let Some(actual) = self.read_current_freq() {
                    // 只校验下沿：min=max 锁频下 cur_freq 高于目标值属于
                    // hw 量化/迁移中的瞬态（MTK 频表步进可达 +18%），性能
                    // 无损，且实测反复「紧急重写」从未修正过该读数——重写
                    // 只造成 write/umount 抖动。低于目标值才是锁频未生效
                    // （thermal cap / QoS 覆写），需要重写夺回
                    let min_ok = self
                        .available_freqs
                        .iter()
                        .take_while(|&&f| f <= expected)
                        .last()
                        .copied()
                        .unwrap_or(expected);
                    if actual < min_ok {
                        warn!(
                            "{}",
                            t_with_args(
                                "fas-freq-mismatch",
                                &fluent_args!(
                                    "pid" => self.policy_id.to_string(),
                                    "min" => min_ok.to_string(),
                                    "max" => expected.to_string(),
                                    "actual" => actual.to_string()
                                )
                            )
                        );
                        self.max_writer.re_unmount();
                        self.min_writer.re_unmount();
                        self.max_writer.write_value_force(write_freq);
                        self.min_writer.write_value_force(write_freq);
                    }
                }
            }
        }
        self.verify_freq = Some(write_freq);
    }

    fn read_current_freq(&self) -> Option<u32> {
        let path = format!(
            "/sys/devices/system/cpu/cpufreq/policy{}/scaling_cur_freq",
            self.policy_id
        );
        fs::read_to_string(&path).ok()?.trim().parse::<u32>().ok()
    }

    pub fn force_reapply(&mut self) {
        if self.ignore_write {
            return;
        }
        self.max_writer.re_unmount();
        self.min_writer.re_unmount();
        self.min_writer.write_value_force(self.current_freq);
        self.max_writer.write_value_force(self.current_freq);
    }

    pub fn reset(&mut self) {
        let min_f = self.available_freqs[0];
        let max_f = *self.available_freqs.last().unwrap();
        self.max_writer.write_value_force(max_f);
        self.min_writer.write_value_force(min_f);
        self.current_freq = max_f;
        self.verify_freq = None;
        // 恢复接管前的 governor：load_policies 期间写入的 performance
        // 只对 FAS 锁频模式有意义，泄漏到 CLG/akmode/系统调频会让功耗
        // 停留在「性能拉满」档（用户实测 FAS 用后功耗不降的直接原因）
        if let Some(gov) = self.orig_governor.clone() {
            let _ = crate::utils::try_write_file(
                &format!(
                    "/sys/devices/system/cpu/cpufreq/policy{}/scaling_governor",
                    self.policy_id
                ),
                &gov,
            );
        }
    }
}
