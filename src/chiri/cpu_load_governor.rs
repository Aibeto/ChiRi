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
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::{self, JoinHandle};
use std::time::Duration;

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
    /// 当前目标性能比 [0,1]，调频时换算成频率档位
    current_perf: f32,
    /// 当前已写入的性能上限（scaling_max_freq，kHz）；0 表示尚未写入成功，下次 tick 自动重试。
    /// 注意：这是上限而非实际频率，实际频率由 schedutil 在 [硬件最低, 上限] 内自主决定
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

    /// 写 scaling_max_freq（性能上限）。
    /// min 已在 init 时压到硬件最低，之后只调 max。
    /// schedutil 在 [min, max] 里自己看着办——空闲时降到地板频，忙碌时贴近 max。
    /// 这比锁频(min=max)好的地方：不用等 160ms tick 才降频，内核微秒级就降下来了，
    /// 空转发热大幅减少。写 max 也不用管写序问题，因为 min 恒 <= max。
    fn write_freq(&mut self, freq: u32) {
        if freq == self.current_freq {
            return;
        }
        let old_freq = self.current_freq;
        let ok = self.max_writer.write_value_force(freq);
        // 写入成功才更新缓存，失败则下次 tick 自动重试
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
    /// 降频时用来把 current_perf 同步到实际写入的档位，防止状态与写入脱节。
    fn ratio_of_freq(&self, freq: u32) -> f32 {
        let fmin = *self.available_freqs.first().unwrap_or(&0) as f32;
        let fmax = *self.available_freqs.last().unwrap_or(&0) as f32;
        ((freq as f32) - fmin) / (fmax - fmin).max(1.0)
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
}

// ════════════════════════════════════════════════════════════════
//  AtomicTouchState — 跨线程共享的触摸升频状态
// ════════════════════════════════════════════════════════════════

/// 跨线程共享的触摸升频状态，Worker 通过 `Arc<AtomicTouchState>` 读取当前窗口。
/// f32 以 bit pattern 存入 AtomicU32（合法的原子操作，所有位组合都是合法 f32）。
/// 使用 generation 计数器保证 set/get 一致性：Worker 读取时若 generation 不匹配
/// 则视为写入中、返回 0.0（无升频），下次 tick 重试。
struct AtomicTouchState {
    /// 触摸升频地板性能比（f32 的 bit pattern），0 表示无窗口
    floor_bits: AtomicU32,
    /// 窗口截止时间（相对 Instant::now() 的毫秒偏移量，u32 可表示 ~49 天）
    /// 比 epoch 秒+纳秒方案更紧凑且不易溢出
    duration_ms: AtomicU32,
    /// 写入时刻的毫秒级 epoch 时间戳（u32 截断，~49 天一轮回，足够触摸窗口判断）
    set_epoch_ms: AtomicU32,
}

impl AtomicTouchState {
    fn new() -> Self {
        Self {
            floor_bits: AtomicU32::new(0.0_f32.to_bits()),
            duration_ms: AtomicU32::new(0),
            set_epoch_ms: AtomicU32::new(0),
        }
    }

    /// 设置触摸升频窗口：floor 为大核性能下限，duration 为窗口持续时长。
    /// 由 scheduler_ipc 线程调用。
    fn set(&self, floor: f32, duration: Duration) {
        // 写入顺序：先 duration/floor，最后 set_epoch_ms（作为"就绪标志"）。
        // Worker 读取时先读 set_epoch_ms，再读 duration/floor：
        // 若 set_epoch_ms 在 Worker 读取期间被更新，Worker 可能读到旧 floor/duration
        // 但会因 set_epoch_ms 变化而在下次 tick 重新读取，不会永久卡在错误状态。
        // 最坏情况：一次 tick 的触摸升频用错 floor（下一 tick 立即修正），无害。
        let epoch_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u32;
        self.duration_ms
            .store(duration.as_millis() as u32, Ordering::Relaxed);
        self.floor_bits.store(floor.to_bits(), Ordering::Relaxed);
        // Release 保证前两行对读取侧可见
        self.set_epoch_ms.store(epoch_ms, Ordering::Release);
    }

    /// Worker 调用：窗口未过期时返回地板性能比，已过期或无窗口返回 0.0。
    /// 读取顺序与 set 写入顺序相反，保证一致性。
    fn get_floor(&self) -> f32 {
        // Acquire 保证读到 set_epoch_ms 时，对应的 floor/duration 已可见
        let set_ms = self.set_epoch_ms.load(Ordering::Acquire);
        if set_ms == 0 {
            return 0.0;
        }
        let duration = self.duration_ms.load(Ordering::Relaxed);
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u32;
        // u32 环形减法：即使 now_ms 溢出回绕，差值仍然正确（~49 天周期内）
        let elapsed = now_ms.wrapping_sub(set_ms);
        if elapsed < duration {
            f32::from_bits(self.floor_bits.load(Ordering::Relaxed))
        } else {
            0.0
        }
    }
}

// ════════════════════════════════════════════════════════════════
//  CoreGroupWorker — 每个核心组独立线程的调度 Worker
// ════════════════════════════════════════════════════════════════

/// 单核心组的独立调度 Worker：在专属线程内持有 ClusterState，
/// 接收负载数据自主做升降频决策 + 写频，与其他核心组完全并行。
struct CoreGroupWorker {
    cluster: ClusterState,
    cfg: CpuLoadGovernorConfig,
    restore: PolicyRestore,
    core_ranges: crate::common::CoreGroupRanges,
    load_rx: mpsc::Receiver<Vec<f32>>,
    stop: Arc<AtomicBool>,
    touch: Arc<AtomicTouchState>,
    /// 热保护性能上限（f32 bit pattern 存 AtomicU32，1.0 = 无压制）。
    /// scheduler_ipc 线程定期采样电池/CPU 温度后写入，Worker 每次 flush 读取并 clamp
    thermal_cap: Arc<AtomicU32>,
    /// 压制豁免档（f32 bit pattern，0..1）：current_perf >= 该值时不钳制，
    /// 保证任何温度下持续高负载都能到达硬件最高频
    thermal_free_above: Arc<AtomicU32>,
    /// 上次决策摘要（devimp tick 行用，on_load_update 填写）
    dev_decision: &'static str,
    dev_over: u32,
    dev_under: u32,
    dev_tgt_perf: f32,
    dev_raw_util: f32,
    dev_prev_perf: f32,
}

impl CoreGroupWorker {
    /// 在新线程中运行 Worker 事件循环：接收负载数据、做决策、写频。
    /// 线程退出（stop 或 channel 断开）前恢复系统原始状态。
    ///
    /// 灵敏性说明：性能响应全部走**推送事件**，与下方超时无关——
    /// - 负载决策：cpu_monitor 每 160ms（特调 40ms）send 负载包，recv_timeout
    ///   立即返回 → on_load_update + flush，零延迟；
    /// - 触摸升频：scheduler_ipc 广播空负载包，同样立即打断阻塞 → flush。
    /// 超时分支只承担两件非性能任务：清理过期触摸窗口、重写当前频率防篡改
    /// （厂商守护进程的篡改也是秒级动作，1s 粒度足够）。空闲（无负载事件）
    /// 时 Worker 从每秒 ~6 次空转降到 1 次。
    fn run(mut self) {
        let tick_interval = Duration::from_secs(1);
        let mut log_counter: u32 = 0;

        loop {
            if self.stop.load(Ordering::Acquire) {
                break;
            }

            match self.load_rx.recv_timeout(tick_interval) {
                Ok(core_utils) => {
                    // 空负载包 = 触摸唤醒信号（on_touch 广播）：只 flush 立即应用
                    // 触摸升频，跳过决策——避免用空数据把 util 算成 0 扰动 up/down_wait
                    if !core_utils.is_empty() {
                        self.on_load_update(&core_utils);
                    }
                    self.flush(&core_utils, &mut log_counter);
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    // tick 到期：仅做 flush（清理过期触摸窗口、重写当前频率）
                    // 此处无新的 core_utils，传空切片（max_util 返回 0，不触发决策变更）
                    let empty: [f32; 0] = [];
                    self.flush(&empty, &mut log_counter);
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }

        // 线程退出时恢复系统原始状态
        ClusterState::restore_policy(&self.restore);
    }

    /// 决策入口：每次 SystemLoadUpdate 触发，只计算目标性能比，不写 sysfs。
    /// 同时记录 devimp tick 行所需摘要（over/under 计数、目标性能、决策标签）。
    fn on_load_update(&mut self, core_utils: &[f32]) {
        let raw_util = self.cluster.max_util(core_utils);
        self.dev_raw_util = raw_util;
        self.dev_prev_perf = self.cluster.current_perf;

        // devimp：组内超升频阈值 / 低于降频阈值的核心数（0.0 核不计入 over）
        let mut over = 0u32;
        let mut under = 0u32;
        for &cpu in &self.cluster.affected_cpus {
            if let Some(&u) = core_utils.get(cpu) {
                if u > self.cfg.up_threshold {
                    over += 1;
                }
                if u < self.cfg.down_threshold {
                    under += 1;
                }
            }
        }
        self.dev_over = over;
        self.dev_under = under;

        // 尖峰抑制：单 tick 跳升超过阈值时衰减其增量
        let util = if raw_util > self.cluster.last_util + self.cfg.spike_jump_threshold {
            self.cluster.last_util + (raw_util - self.cluster.last_util) * self.cfg.spike_decay
        } else {
            raw_util
        };
        self.cluster.last_util = raw_util;

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
        self.dev_tgt_perf = target_perf;
        let old_perf = self.cluster.current_perf;

        if target_perf > old_perf {
            self.cluster.down_wait = 0;
            self.cluster.up_wait += 1;

            // 升频速率限制：必须连续 up_rate_limit_ticks 才执行
            if self.cluster.up_wait < self.cfg.up_rate_limit_ticks {
                self.dev_decision = "up_wait";
                return;
            }
            self.dev_decision = "up";

            // 动态上限语义：抬高 ceiling 不强制频率跳变（schedutil 按实际需求在
            // [硬件最低, ceiling] 内取频），无需读取实际频率做余量检查，直接平滑抬升
            let is_high_load = util >= self.cfg.up_threshold;
            let is_significant_jump = target_perf > old_perf + self.cfg.up_jump_threshold;

            if is_high_load || is_significant_jump {
                self.cluster.current_perf += (target_perf - old_perf) * self.cfg.smoothing_up;
            } else {
                // 滞回带内升频：速率随 util 接近 up_threshold 线性提升
                let span = (self.cfg.up_threshold - self.cfg.down_threshold).max(1e-6);
                let gap = ((util - self.cfg.down_threshold) / span).clamp(0.0, 1.0);
                let speed = self.cfg.smoothing_up
                    * (self.cfg.slow_up_scale + (1.0 - self.cfg.slow_up_scale) * gap);
                self.cluster.current_perf += (target_perf - old_perf) * speed;
            }
        } else {
            self.cluster.up_wait = 0;
            self.cluster.down_wait += 1;
            // 极低负载立即降频（跳过 down_wait 确认期），否则连续满 down_rate_limit_ticks
            if self.cluster.down_wait >= self.cfg.down_rate_limit_ticks
                || util < self.cfg.down_fast_threshold
            {
                self.dev_decision = "down";
                // 直接降上限（能效优先）：不做平滑渐变，一步到位写目标档。
                // 降 ceiling 只收窄 schedutil 可用区间，不会把实际频率抬上去，
                // 无需读取 scaling_cur_freq 做钳制
                let target_freq = self.cluster.find_nearest_freq(target_perf);
                self.cluster.current_perf = self.cluster.ratio_of_freq(target_freq);
            } else {
                self.dev_decision = "down_wait";
            }
        }
    }

    /// 把 current_perf 转成实际频率并写 sysfs。每 tick 调一次。
    /// 执行顺序：触摸升频检查 → 热保护 clamp → 性能区间 clamp → 写频。
    /// 热保护在最后一步 clamp：低于豁免档才压，高于豁免档不管——
    /// 持续高负载会平滑涨到豁免档以上，这时温度再高也不挡路（内核兜底）。
    fn flush(&mut self, core_utils: &[f32], log_counter: &mut u32) {
        // 触摸升频：Worker 自主检查共享的 AtomicTouchState
        let mut touch_active = false;
        if self.cfg.touch_boost_enabled {
            let floor = self.touch.get_floor();
            if floor > 0.0 && Self::is_big_cluster(&self.cluster.affected_cpus, &self.core_ranges) {
                self.cluster.current_perf = self.cluster.current_perf.max(floor);
                touch_active = true;
            }
        }
        self.cluster.current_perf = self
            .cluster
            .current_perf
            .clamp(self.cfg.perf_floor, self.cfg.perf_ceil);
        // 热压制（带豁免带）：cap 由 scheduler_ipc 按电池/CPU 温度写入，允许击穿
        // perf_floor（发热优先于保底性能）；>= 豁免档不钳制，保留到达最高频的能力
        let cap = f32::from_bits(self.thermal_cap.load(Ordering::Relaxed));
        let free_above = f32::from_bits(self.thermal_free_above.load(Ordering::Relaxed));
        if self.cluster.current_perf < free_above {
            self.cluster.current_perf = self.cluster.current_perf.min(cap);
        }
        let target_freq = self.cluster.find_nearest_freq(self.cluster.current_perf);
        self.cluster.write_freq(target_freq);

        // devimp tick 行：仅决策 tick（core_utils 非空）且开发记录开启时写
        if !core_utils.is_empty() && crate::logger::devimp_active() {
            let ranges = &self.core_ranges;
            let name = if self
                .cluster
                .affected_cpus
                .iter()
                .any(|c| ranges.prime.contains(c))
            {
                "prime"
            } else if self
                .cluster
                .affected_cpus
                .iter()
                .any(|c| ranges.big.contains(c))
            {
                "big"
            } else {
                "little"
            };
            crate::logger::devimp_tick(
                name,
                &format!("{:.2}", self.dev_raw_util),
                self.dev_over,
                self.dev_under,
                &format!("{:.2}", self.dev_prev_perf),
                &format!("{:.2}", self.dev_tgt_perf),
                &self.cluster.current_freq.to_string(), // cur_freq_khz 列：原始 kHz
                &self.restore.hw_max.to_string(),       // max_freq_khz 列：原始 kHz
                self.dev_decision,
                self.cluster.up_wait,
                self.cluster.down_wait,
                &format!("{:.0}", cap * 100.0),
                touch_active,
            );
        }

        // 日志摘要：每 25 tick 输出一次本 cluster 的 util/perf/freq。
        // format! 在宏外求值，用 log_enabled! 门控省掉 INFO 级别下的分配。
        *log_counter += 1;
        if *log_counter % 25 == 0 && log::log_enabled!(log::Level::Debug) {
            debug!(
                "{}",
                t_with_args(
                    "clg-tick-log",
                    &fluent_args!(
                        "pid" => self.cluster.policy_id.to_string(),
                        "util" => format!("{:.0}", self.cluster.max_util(core_utils) * 100.0),
                        "perf" => format!("{:.2}", self.cluster.current_perf),
                        "freq" => (self.cluster.current_freq / 1000).to_string(),
                        "boost" => format!("{:.0}", self.cluster.boost_max as f32 / 1000.0)
                    )
                )
            );
        }
    }

    /// 判定 cluster 是否覆盖当前 SoC 的大核区间：触摸升频只作用于大核。
    fn is_big_cluster(affected: &[usize], core_ranges: &crate::common::CoreGroupRanges) -> bool {
        let big = &core_ranges.big;
        affected.iter().any(|&c| big.contains(&c))
    }

    /// 创建 Worker 并在新线程中启动，返回（PolicyRestore, JoinHandle）。
    fn spawn(
        policy_id: i32,
        cfg: CpuLoadGovernorConfig,
        boost_frequencies: &[u32],
        core_ranges: crate::common::CoreGroupRanges,
        touch: Arc<AtomicTouchState>,
        thermal_cap: Arc<AtomicU32>,
        thermal_free_above: Arc<AtomicU32>,
        stop: Arc<AtomicBool>,
    ) -> Option<(PolicyRestore, (JoinHandle<()>, mpsc::SyncSender<Vec<f32>>))> {
        let pid = policy_id;
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
            return None;
        }
        freqs.sort_unstable();
        freqs.dedup();

        // 合并 boost 频率（部分平台额外暴露的高频点），去重排序
        if !boost_frequencies.is_empty() {
            freqs.extend(boost_frequencies);
            freqs.sort_unstable();
            freqs.dedup();
        }

        let affected = Self::read_affected_cpus(pid);
        if affected.is_empty() {
            return None;
        }

        let fmin = *freqs.first().unwrap() as f32;
        let fmax = *freqs.last().unwrap() as f32;
        let range = (fmax - fmin).max(1.0);
        let cached_ratios: Vec<f32> = freqs.iter().map(|&f| (f as f32 - fmin) / range).collect();

        let max_writer = FastWriter::new(format!(
            "/sys/devices/system/cpu/cpufreq/policy{}/scaling_max_freq",
            pid
        ));
        let mut min_writer = FastWriter::new(format!(
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
            return None;
        }

        // 记录系统原始状态（每个将被接管的 policy 单独记录），release 时恢复。
        // 必须位于 governor 写入之前，保证 release 能还原所有被接管的 cluster。
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
        let restore = PolicyRestore {
            policy_id: pid,
            governor,
            min_freq,
            max_freq,
            hw_max: *freqs.last().unwrap(),
        };

        // 写入 schedutil governor
        let _ = crate::utils::try_write_file(&gov_path, "schedutil");

        // 动态上限接管：min 一次性压到硬件最低并保持（空闲时 schedutil 可降到地板频率），
        // 之后仅由 write_freq 调整 max。先降 min 再定 max，保证中间态 min <= max。
        let hw_min = *freqs.first().unwrap();
        if !min_writer.write_value_force(hw_min) {
            warn!(
                "{}",
                t_with_args(
                    "clg-min-write-failed",
                    &fluent_args!("pid" => pid.to_string(), "khz" => (hw_min / 1000).to_string())
                )
            );
        }

        let init_perf = cfg.perf_init.clamp(cfg.perf_floor, cfg.perf_ceil);
        let boost_max = boost_frequencies.iter().copied().max().unwrap_or(0);
        let mut cluster = ClusterState {
            policy_id: pid,
            affected_cpus: affected.clone(),
            available_freqs: freqs,
            cached_ratios,
            boost_max,
            max_writer,
            current_perf: init_perf,
            current_freq: 0,
            down_wait: 0,
            up_wait: 0,
            last_util: 0.0,
        };

        let init_freq = cluster.find_nearest_freq(init_perf);
        // 初始接管只写 max=perf_init 档（min 已在上面压到硬件最低）
        let init_ok = cluster.max_writer.write_value_force(init_freq);
        if init_ok {
            cluster.current_freq = init_freq;
        }

        let (load_tx, load_rx) = mpsc::sync_channel::<Vec<f32>>(1);

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

        let worker = CoreGroupWorker {
            cluster,
            cfg,
            restore: PolicyRestore {
                policy_id: restore.policy_id,
                governor: restore.governor.clone(),
                min_freq: restore.min_freq,
                max_freq: restore.max_freq,
                hw_max: restore.hw_max,
            },
            core_ranges,
            load_rx,
            stop: stop.clone(),
            touch,
            thermal_cap,
            thermal_free_above,
            dev_decision: "hold",
            dev_over: 0,
            dev_under: 0,
            dev_tgt_perf: 0.0,
            dev_raw_util: 0.0,
            dev_prev_perf: 0.0,
        };

        let handle = thread::Builder::new()
            .name(format!("clg_worker_{}", pid))
            .spawn(move || worker.run())
            .ok()?;

        Some((restore, (handle, load_tx)))
    }

    /// 读取 policy 的 affected_cpus，解析成 CPU id 列表
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

// ════════════════════════════════════════════════════════════════
//  Worker 句柄（线程 + 负载通道发送端）
// ════════════════════════════════════════════════════════════════

/// 每个 Worker 的控制句柄：持有负载通道发送端和线程 JoinHandle。
struct WorkerHandle {
    policy_id: i32,
    load_tx: mpsc::SyncSender<Vec<f32>>,
    handle: Option<JoinHandle<()>>,
}

impl WorkerHandle {
    /// 发送负载数据到 Worker（非阻塞，满则丢弃本 tick）
    fn send_load(&self, core_utils: Vec<f32>) {
        let _ = self.load_tx.try_send(core_utils);
    }

    /// 等待 Worker 线程退出
    fn join(&mut self) {
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

// ════════════════════════════════════════════════════════════════
//  CpuLoadGovernor — 主控制器（Worker 线程管理器）
// ════════════════════════════════════════════════════════════════

pub struct CpuLoadGovernor {
    /// 当前生效的 CLG 配置（normalize 后的副本）
    cfg: CpuLoadGovernorConfig,
    /// Worker 句柄列表：每个 cpufreq policy 一个 Worker
    workers: Vec<WorkerHandle>,
    /// 接管前的系统状态快照：release 时恢复
    restores: Vec<PolicyRestore>,
    /// Worker 停止信号（共享，新 init 时替换为新实例）
    stop: Arc<AtomicBool>,
    /// 触摸升频状态（跨线程共享，Worker 自主读取）
    touch: Arc<AtomicTouchState>,
    /// 热保护性能上限（f32 bit pattern，1.0 = 无压制）。
    /// scheduler_ipc 按电池/CPU 温度更新，Worker 每次 flush 读取；跨 init/reload 生命周期保持
    thermal_cap: Arc<AtomicU32>,
    /// 压制豁免档（f32 bit pattern）：ceiling >= 豁免档不钳制，保证可达硬件最高频
    thermal_free_above: Arc<AtomicU32>,
    /// 是否处于接管状态（至少一个 Worker 启动成功才为 true）
    active: bool,
}

impl CpuLoadGovernor {
    /// 创建空的控制器（未激活、未接管任何 policy）。
    pub fn new() -> Self {
        Self {
            cfg: CpuLoadGovernorConfig::default(),
            workers: Vec::new(),
            restores: Vec::new(),
            stop: Arc::new(AtomicBool::new(false)),
            touch: Arc::new(AtomicTouchState::new()),
            thermal_cap: Arc::new(AtomicU32::new(1.0_f32.to_bits())),
            thermal_free_above: Arc::new(AtomicU32::new(0.80_f32.to_bits())),
            active: false,
        }
    }

    /// 下发热保护参数。scheduler_ipc 每 2s 调一次。
    /// Worker flush 时读这两个原子量：低于豁免档才压到 cap，高于就不管。
    pub fn set_thermal_limits(&self, cap: f32, free_above: f32) {
        self.thermal_cap
            .store(cap.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
        self.thermal_free_above
            .store(free_above.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }

    /// CLG 当前是否已接管 CPU 频率
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// 接管所有 cpufreq policy：停旧 Worker → 快照原始状态 → 写 schedutil →
    /// min 压到硬件最低 → max 按 perf_init 设初始值 → 为每个 policy 起 Worker 线程。
    pub fn init_policies(&mut self, gov_cfg: &CpuLoadGovernorConfig) {
        self.stop_workers();
        self.cfg = gov_cfg.clone();
        self.normalize_cfg();

        let policies = crate::chiri::get_cpu_policies();
        let core_ranges = crate::common::chiri_core_ranges();

        // 全新 stop/touch 实例；thermal_cap 跨 init 生命周期保持（字段本身复用）
        let stop = Arc::new(AtomicBool::new(false));
        let touch = Arc::new(AtomicTouchState::new());

        for policy in &policies {
            let result = CoreGroupWorker::spawn(
                policy.id,
                self.cfg.clone(),
                &policy.boost_frequencies,
                core_ranges.clone(),
                touch.clone(),
                self.thermal_cap.clone(),
                self.thermal_free_above.clone(),
                stop.clone(),
            );

            if let Some((restore, (handle, load_tx))) = result {
                self.restores.push(restore);
                self.workers.push(WorkerHandle {
                    policy_id: policy.id,
                    load_tx,
                    handle: Some(handle),
                });
            }
        }

        self.stop = stop;
        self.touch = touch;
        self.active = !self.workers.is_empty();
        if self.active {
            info!(
                "{}",
                t_with_args(
                    "clg-activated",
                    &fluent_args!("count" => self.workers.len().to_string())
                )
            );
        } else {
            warn!("{}", t("clg-no-clusters"));
        }
    }

    /// 释放接管：停止所有 Worker（Worker 线程退出前恢复系统原始状态），清空状态。
    pub fn release(&mut self) {
        if self.active {
            info!("{}", t("clg-deactivated"));
        }
        self.stop_workers();
        self.active = false;
    }

    /// 热切换配置：停止旧 Worker 并用新配置重新创建（与 init_policies 同路径，
    /// 保证 current_perf 重置到新 perf_init 并立即写频）。
    ///
    /// 切换后频率从 perf_init 起步，避免息屏 doze/scenemode 期间 current_perf 掉到 ~0
    /// 后亮屏恢复原模式时频率要从地板缓慢爬升数秒。
    pub fn reload_config(&mut self, gov_cfg: &CpuLoadGovernorConfig) {
        // 保存旧 Worker 的 policy 信息用于重建
        let policy_ids: Vec<i32> = self.workers.iter().map(|w| w.policy_id).collect();
        let policies = crate::chiri::get_cpu_policies();

        self.stop_workers();
        self.cfg = gov_cfg.clone();
        self.normalize_cfg();

        let core_ranges = crate::common::chiri_core_ranges();
        let stop = Arc::new(AtomicBool::new(false));
        let touch = Arc::new(AtomicTouchState::new());

        for policy in &policies {
            if !policy_ids.contains(&policy.id) {
                continue;
            }
            let result = CoreGroupWorker::spawn(
                policy.id,
                self.cfg.clone(),
                &policy.boost_frequencies,
                core_ranges.clone(),
                touch.clone(),
                self.thermal_cap.clone(),
                self.thermal_free_above.clone(),
                stop.clone(),
            );
            if let Some((restore, (handle, load_tx))) = result {
                self.restores.push(restore);
                self.workers.push(WorkerHandle {
                    policy_id: policy.id,
                    load_tx,
                    handle: Some(handle),
                });
            }
        }

        self.stop = stop;
        self.touch = touch;
        self.active = !self.workers.is_empty();

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

    /// 负载事件入口：将 core_utils 广播给所有 Worker（非阻塞，通道满则丢弃本 tick）。
    /// Worker 线程内自主完成决策 + 写频。
    pub fn on_load_update(&mut self, core_utils: &[f32]) {
        if !self.active {
            return;
        }
        for w in &self.workers {
            w.send_load(core_utils.to_vec());
        }
    }

    /// 触摸事件驱动入口：收到触摸按下事件时更新共享触摸升频状态，
    /// 并立即唤醒全部 Worker（广播空负载包，recv_timeout 即时返回触发 flush），
    /// 大核 Worker 在本次 flush 中读取共享状态并提升性能下限，无需等待下一个 160ms tick。
    pub fn on_touch(&mut self) {
        if !self.active || !self.cfg.touch_boost_enabled {
            return;
        }
        let floor = self.compute_touch_boost_floor();
        self.touch
            .set(floor, Duration::from_millis(self.cfg.touch_boost_ms));
        // 唤醒所有 Worker：空负载包仅触发 flush（大核 Worker 应用触摸升频地板，
        // 小核 Worker 无地板、写频去重无副作用）。try_send 满时丢弃：通道里
        // 排队的真实负载数据同样会唤醒 Worker 并应用触摸升频，不丢最终结果。
        for w in &self.workers {
            w.send_load(Vec::new());
        }
        debug!(
            "{}",
            t_with_args(
                "clg-touch-boost",
                &fluent_args!(
                    "floor" => format!("{:.2}", floor),
                    "ms" => self.cfg.touch_boost_ms.to_string()
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

    /// 停止所有 Worker：通知停止 → 等待线程退出 → 恢复系统状态。
    /// Worker 线程退出前会自行恢复其 policy，此处仅做 join 确保退出完成。
    fn stop_workers(&mut self) {
        // 通知所有 Worker 停止
        self.stop.store(true, Ordering::Release);

        // join 所有 Worker 线程（Worker 退出前自行 restore_policy）
        for w in &mut self.workers {
            w.join();
        }
        self.workers.clear();
        self.restores.clear();
    }

    /// 计算触摸升频的大核性能下限：取各覆盖大核的 Worker 当前频率在可用频率表中
    /// 向上移动 touch_boost_tiers 档后的性能比，取最大值（一个窗口期内保持不变）。
    /// 注意：Worker 架构下 Worker 持有 ClusterState，此处无法直接读取当前频率。
    /// 使用当前配置 perf_init 作为基线估算（触摸升频仅在一帧内的感知差异，实际
    /// Worker flush 时会读取共享的 touch floor 并 clamp 到正确范围）。
    fn compute_touch_boost_floor(&self) -> f32 {
        // Worker 架构下无法直接读 Worker 的 current_freq（Worker 持有 ClusterState），
        // 用 perf_init 作为保守估算；Worker 在 flush 时会正确 clamp。
        // 为精确计算，我们用 cfg.perf_init 作为基线，加上 touch_boost_tiers 档位偏移。
        let base = self
            .cfg
            .perf_init
            .clamp(self.cfg.perf_floor, self.cfg.perf_ceil);
        // 向上抬 touch_boost_tiers 档：按频率表比例估算
        // 频率表档位数量通常 15-25 个，tiers 一般 1-2 档
        // 简单估算：base + tiers * 单档步进（约 1/总档位数）
        let tier_step = 0.05; // 约 5% 性能比/档（保守估算）
        (base + self.cfg.touch_boost_tiers as f32 * tier_step).min(self.cfg.perf_ceil)
    }
}
