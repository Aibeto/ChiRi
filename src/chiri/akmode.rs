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
use log::{debug, info};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::fluent_args;
use crate::i18n::{t, t_with_args};

/// 固定 4 档，对应 powersave/balance/performance/fast，powersave 最低 fast 最高
const TIER_COUNT: usize = 4;

/// 明日方舟特调（akmode）控制器：仅按负载在四档模式间升降档，不干预任何频率。
/// 特调期间 CPU 频率由系统原生 governor 自行管理，本控制器只维护档位状态机并打点，
/// 供日志观察特调档位判定是否合理（固定频率锁频功能已按需求关闭）。
pub struct AkmodeGovernor {
    cfg: SpecialTunedConfig,
    /// 特调激活共享标志：Monitor 层（cpu_monitor）据此切换采样间隔（特调 40ms / 其余 120ms）。
    /// 接管时置 true、释放时置 false，由 init_policies / release 维护。
    ak_active: Arc<AtomicBool>,
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

    /// 接管：设置起始档位并激活状态机。
    /// 不做任何 sysfs 写入（目标频率功能已关闭），CPU 频率由系统原生 governor 管理。
    /// initial_tier 由调用方从 rules.yaml 的生效模式换算（powersave=1..fast=4）。
    pub fn init_policies(&mut self, cfg: &SpecialTunedConfig, initial_tier: u32) {
        self.release();
        self.cfg = cfg.clone();
        self.cfg.normalize();
        self.current_tier = initial_tier.clamp(1, TIER_COUNT as u32);
        self.pending_tier = None;
        self.pending_since = None;
        self.active = true;
        // 特调激活通知 Monitor 层切换到 40ms 快速采样
        self.ak_active.store(true, Ordering::Relaxed);
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
    }

    /// 释放接管：复位状态机
    pub fn release(&mut self) {
        if self.active {
            info!("{}", t("akmode-deactivated"));
        }
        self.active = false;
        // 特调退出通知 Monitor 层恢复常规采样
        self.ak_active.store(false, Ordering::Relaxed);
        self.pending_tier = None;
        self.pending_since = None;
        self.fast_wait_until = None;
        self.log_counter = 0;
    }

    /// 热切换配置：换阈值，当前档位不变
    pub fn reload_config(&mut self, cfg: &SpecialTunedConfig) {
        self.cfg = cfg.clone();
        self.cfg.normalize();
        self.current_tier = self.current_tier.clamp(1, TIER_COUNT as u32);
        let tc = self.cfg.tier(self.current_tier);
        debug!(
            "{}",
            t_with_args(
                "akmode-config-reloaded",
                &fluent_args!("wait" => tc.wait_ms.to_string())
            )
        );
    }

    /// 档位判定入口，每个 SystemLoadUpdate（常规 120ms / 特调 40ms）触发一次。
    /// 按核心组（little/big/prime）分别统计忙/闲核心数，每组用本组独立条件判定：
    ///   升档 = 任一组内超过 up_core_count 个核心占用率 > up_util_percent
    ///   降档 = 任一组内超过 down_core_count 个核心占用率 < down_util_percent
    /// 升档优先于降档，条件成立后等 wait_ms 防抖再切档。
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
            let wait = if fast_wait {
                tc.wait_ms / 2
            } else {
                tc.wait_ms
            };
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

    /// 切档（不干预频率，仅更新档位状态并打点）
    fn apply_tier(&mut self, tier: u32) {
        let old = self.current_tier;
        self.current_tier = tier;
        // 切档后启动临时加速窗口：此后 after_change_duration_ms 内防抖等待减半执行，
        // 让连续跳档更跟手；每次切档都会重置该窗口。
        self.fast_wait_until =
            Some(Instant::now() + Duration::from_millis(self.cfg.after_change_duration_ms));
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
}
