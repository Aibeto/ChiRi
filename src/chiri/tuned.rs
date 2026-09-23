//! tuned.rs: [restore] [governor] [init_release] [load_freq]
//! 特调执行器（TunedGovernor）：akmode / playback / daily 等模式共用同一套连续控制，
//! 参数组按模式名从 Config.tuned_profiles 分派，差异只在参数。

use crate::chiri::config::SpecialTunedConfig;
use crate::utils::FastWriter;
use log::{debug, info, warn};
use std::fs;

/// 跨核迁移成本（ns，全局节点）：稳态场景（视频）接管期间调高、退出时恢复
const MIGRATION_COST_PATH: &str = "/proc/sys/kernel/sched_migration_cost_ns";
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::fluent_args;
use crate::i18n::t_with_args;

// [restore]
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
    /// 降频保持期记录的目标档位：目标明显回升时重置计时（防止逼近移动目标）
    down_target: u32,
    /// 用于决策的负载 EMA（util_smoothing < 1 时启用；-1 = 未初始化）
    ema_util: f32,
    // [dwell] 写频滞回状态（Phase 2，与 CLG 同口径）
    /// 上次**实际写频**成功时刻：dwell 以它计时；None = 接管初写后尚未写过
    /// （接管初写不启动时钟，首次受控写频立即生效）
    last_write_at: Option<Instant>,
    /// 上次实际写频方向：升 = 1、降 = -1、未写过 = 0（翻摆判定基准）
    last_write_dir: i8,
    /// 上次写频失败：下次写入视为防篡改补写、豁免滞回（重试时序不变）
    last_failed: bool,
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
// [governor]
pub struct TunedGovernor {
    cfg: SpecialTunedConfig,
    /// 当前接管的特调模式名（akmode / playback / daily …）：日志用，
    /// release 后保留（deactivated 日志要报是哪个模式退出）
    mode: String,
    /// 特调激活共享标志：Monitor 层（cpu_monitor）据此切换采样间隔（特调 40ms / 其余 120ms）
    ak_active: Arc<AtomicBool>,
    clusters: Vec<ClusterState>,
    /// 各 policy 的 governor/min/max 快照，release 时恢复
    restore: Vec<PolicyRestore>,
    /// 接管前 `sched_migration_cost_ns` 的原值（全局节点，只快照一次）：
    /// 仅当配置了 `migration_cost_ns` 且写成功时才非 None，release 时写回
    migration_cost_restore: Option<String>,
    active: bool,
    /// 调试日志计数，每 25 tick 打一次摘要
    log_counter: u32,
}

impl TunedGovernor {
    pub fn new(ak_active: Arc<AtomicBool>) -> Self {
        Self {
            cfg: SpecialTunedConfig::default(),
            mode: String::new(),
            ak_active,
            clusters: Vec::new(),
            restore: Vec::new(),
            migration_cost_restore: None,
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
    // [init_release]
    pub fn init_policies(&mut self, mode: &str, cfg: &SpecialTunedConfig) -> bool {
        self.release();
        self.cfg = cfg.clone();
        self.cfg.normalize();
        // 模式名只用于日志可读性（同一套机制被多个模式共用，日志必须能区分）
        self.mode = mode.to_string();

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
                            "tuned-cluster-skipped",
                            &fluent_args!(
                                "mode" => self.mode.clone(),
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
                        "tuned-cluster-skipped",
                        &fluent_args!(
                            "mode" => self.mode.clone(),
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
                ema_util: -1.0,
                // [dwell] 写频滞回状态：接管初写不启动 dwell 时钟
                last_write_at: None,
                last_write_dir: 0,
                last_failed: false,
            });
        }

        // 稳态负载下按需调高迁移成本（可选：未配置则该节点完全不动）
        self.apply_migration_cost();

        self.active = !self.clusters.is_empty();
        if self.active {
            info!(
                "{}",
                t_with_args("tuned-init", &fluent_args!("mode" => self.mode.clone()))
            );
            info!(
                "{}",
                t_with_args(
                    "tuned-activated",
                    &fluent_args!("mode" => self.mode.clone())
                )
            );
            // 特调激活通知 Monitor 层切换到 40ms 快速采样
            self.ak_active.store(true, Ordering::Relaxed);
        } else {
            warn!(
                "{}",
                t_with_args(
                    "tuned-no-clusters",
                    &fluent_args!("mode" => self.mode.clone())
                )
            );
        }
        self.active
    }

    /// 释放接管：恢复各 policy 的 governor/min/max，清空状态
    pub fn release(&mut self) {
        if self.active {
            info!(
                "{}",
                t_with_args(
                    "tuned-deactivated",
                    &fluent_args!("mode" => self.mode.clone())
                )
            );
        }
        self.active = false;
        // 恢复原 governor/min/max（快照读取失败的字段跳过）
        self.restore.retain(|r| !Self::restore_policy(r));
        self.restore_migration_cost();
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

    /// 接管期间按需调高 `sched_migration_cost_ns`：稳态负载（视频播放）下该值偏小
    /// 会让调度器频繁跨核搬迁（8550 实测 ≈557 次/s），调高可省下搬迁开销与 cache
    /// 失效。原值进 `migration_cost_restore`，release 时写回；写失败就当没配——
    /// 这是锦上添花项，不该影响接管本身。
    fn apply_migration_cost(&mut self) {
        let Some(want) = self.cfg.migration_cost_ns.filter(|v| *v > 0) else {
            return;
        };
        let orig = fs::read_to_string(MIGRATION_COST_PATH)
            .ok()
            .map(|s| s.trim().to_string());
        if crate::utils::try_write_file(MIGRATION_COST_PATH, &want.to_string()).is_ok() {
            self.migration_cost_restore = orig;
        }
    }

    /// 恢复接管前的 `sched_migration_cost_ns`（只对曾经写成功的那次生效）
    fn restore_migration_cost(&mut self) {
        if let Some(orig) = self.migration_cost_restore.take() {
            let _ = crate::utils::try_write_file(MIGRATION_COST_PATH, &orig);
        }
    }

    /// 热重载：tuned_profiles.yaml 参数变化后更新控制参数（max 动态状态保持不变）。
    // [load_freq]
    pub fn reload_config(&mut self, cfg: &SpecialTunedConfig) {
        self.cfg = cfg.clone();
        self.cfg.normalize();
        debug!(
            "{}",
            t_with_args(
                "tuned-config-reloaded",
                &fluent_args!("mode" => self.mode.clone())
            )
        );
    }

    /// 频率表中找「不低于 ratio × 硬件最高」的最低档位：
    /// 升频响应性优先，落点略高于理论值无妨；降频由 hysteresis + hold 防抖兜住。
    fn freq_for_ratio(freqs: &[u32], ratio: f32) -> u32 {
        let hw_max = *freqs.last().unwrap_or(&0);
        let want = (hw_max as f32 * ratio) as u32;
        freqs.iter().copied().find(|&f| f >= want).unwrap_or(hw_max)
    }

    // [dwell] 受控写频（与 CLG write_freq 同口径）：死区由调用方 hysteresis 保证
    // （|target−current| 超死区才调用），这里做「最小驻留内方向翻摆延迟」与写失败
    // 防篡改补写豁免；接管初写/恢复不走本函数。返回是否写入成功（成功才前移
    // current_max 并记录实际写频；失败/被延迟由下一 tick 补写当前 target）。
    fn gated_write(c: &mut ClusterState, target: u32, dwell_ms: u64) -> bool {
        let dir: i8 = if target > c.current_max { 1 } else { -1 };
        if !c.last_failed && c.last_write_dir != 0 && dir != c.last_write_dir {
            if let Some(at) = c.last_write_at {
                if at.elapsed() < Duration::from_millis(dwell_ms) {
                    return false;
                }
            }
        }
        let old = c.current_max;
        let ok = c.max_writer.write_value_force(target);
        if ok {
            c.current_max = target;
            c.last_write_at = Some(Instant::now());
            c.last_write_dir = if target > old { 1 } else { -1 };
            c.last_failed = false;
        } else {
            c.last_failed = true;
        }
        ok
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

        // main_ tick 行数据（每核心组一行）：cluster / util / decision / cur_max / hw_max
        let mut main_rows: Vec<(&'static str, String, &str, u32, u32)> = Vec::new();

        for c in &mut self.clusters {
            let range: &std::ops::Range<usize> = if c.core_name == "little" {
                &ranges.little
            } else if c.core_name == "big" {
                &ranges.big
            } else {
                &ranges.prime
            };
            // 该核心组的有效参数（per_cluster 覆盖后的最终值）：三簇负载形态不同
            // （视频实测 little 过载 / big 合理 / prime 空转），必须按组取
            let p = cfg.for_cluster(c.core_name);
            let group_util = range
                .clone()
                .filter_map(|cpu| core_utils.get(cpu).copied())
                .fold(0.0_f32, f32::max);
            // 负载平滑（EMA，util_smoothing=1.0 关闭）：抖动的瞬时 util 尖峰会
            // 把「升频立即执行」的上限反复推高、降频又被 hold 拖住，上限均值
            // 反而高于带平滑的 CLG（2026-09-17 8550 日志回放证实）。视频/轻载
            // 这类「噪声型抖动」负载用平滑滤尖峰；游戏（默认 1.0）保持原始值，
            // 瞬时升频是响应性的一部分。
            // 时间常数（α=0.35）≈ 采样间隔 × (1-α)/α：实机 40ms tick ≈ 75ms
            // （跟得上真实负载、滤掉单 tick 尖峰）；日志回放是 160ms 采样
            // （≈300ms）——回放给出的收紧幅度因此偏乐观，实机效果待日志验证。
            let util = if p.util_smoothing >= 0.999 || c.ema_util < 0.0 {
                group_util
            } else {
                p.util_smoothing * group_util + (1.0 - p.util_smoothing) * c.ema_util
            };
            c.ema_util = util;

            let hw_max = *c.available_freqs.last().unwrap_or(&0);
            // perf_ceil 是天花板（默认 1.0 = 与加该字段前完全一致）
            let target_ratio = (util * p.headroom).clamp(p.perf_floor, p.perf_ceil);
            let target_max = Self::freq_for_ratio(&c.available_freqs, target_ratio);
            let hyst_freq = (hw_max as f32 * p.hysteresis) as u32;

            let decision;
            if target_max > c.current_max + hyst_freq {
                // 升频：立即执行（响应性优先），并取消进行中的降频等待。
                // 写成功才前移状态：失败时下一 tick target 仍超死区 → up 重试
                //（与 CLG「写失败下次 tick 自动重试」语义对齐）
                // [dwell] 经写频滞回（同 CLG）：翻摆延迟、写失败补写豁免
                Self::gated_write(c, target_max, cfg.write_dwell_ms);
                c.down_since = None;
                decision = "up";
            } else if target_max < c.current_max.saturating_sub(hyst_freq) {
                // 降频：目标须持续 down_hold_ms 才执行。
                // 计时重置只在目标**明显回升**（> down_target + hyst）时发生：抖动负载下
                // target 在相邻档位小幅跳动不应重置计时——原「档位不等即重置」在
                // 2026-09-17 的 8550 日志回放中被证实会让上限卡在高位降不下来
                // （160ms 平滑 util 下都几乎降不动，真实原始 util 更糟），
                // 连续控制退化成「只升不降」。目标继续下探时只更新 down_target
                // （等待更深的降频，执行时用最新档），不重置计时。
                match c.down_since {
                    None => {
                        c.down_since = Some(now);
                        c.down_target = target_max;
                        decision = "down_wait";
                    }
                    Some(_) if target_max > c.down_target.saturating_add(hyst_freq) => {
                        c.down_since = Some(now);
                        c.down_target = target_max;
                        decision = "down_wait";
                    }
                    Some(since) => {
                        c.down_target = target_max;
                        if now.duration_since(since).as_millis() as u64 >= p.down_hold_ms {
                            // 写成功才前移状态并结束等待：失败/被 dwell 延迟保留计时起点，
                            // 下一 tick（elapsed 仍满）立即重试
                            // [dwell] 经写频滞回（同 CLG）：翻摆延迟、写失败补写豁免
                            if Self::gated_write(c, target_max, cfg.write_dwell_ms) {
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
                // [dwell] 目标收敛进死区：上次写失败的补写诉求消失，清标记防悬挂
                //（否则任意久之后的下一次真实写频会白豁免滞回一次）
                c.last_failed = false;
                decision = "hold";
            }

            main_rows.push((
                c.core_name,
                format!("{:.2}", util),
                decision,
                c.current_max,
                hw_max,
            ));
        }

        // main_ tick 行（开发记录开启时才有 IO）：
        // cur_freq_khz 列写当前动态 max（kHz），max_freq_khz 列写硬件最高（kHz），
        // 与旧版一致；over/under 列无档位阈值语义，恒 0。
        // **util 列写决策用的负载**：util_smoothing < 1 时为 EMA 平滑后的值
        // （见上方 util 计算处的注释）——2026-09-17 起语义变化，离线回放该列时
        // 不能再当原始 util 二次平滑。
        if crate::logger::diag_active() {
            for (name, util, decision, cur_max, hw_max) in &main_rows {
                crate::logger::main_tick(
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
        // debug 心跳（独立于开发诊断，日志通道随时可用）：每 25 tick 汇总各簇
        // util/max，供诊断关闭时观测负载直拉行为
        if self.log_counter % 25 == 0 && log::log_enabled!(log::Level::Debug) {
            let summary = main_rows
                .iter()
                .map(|(name, util, _d, cur_max, _hw)| {
                    format!("{}={}MHz({})", name, cur_max / 1000, util)
                })
                .collect::<Vec<_>>()
                .join(" ");
            debug!(
                "{}",
                t_with_args(
                    "tuned-tick-log",
                    &fluent_args!("mode" => self.mode.clone(), "state" => summary)
                )
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
