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

/// core_ctl（厂商核心在线控制器）接管（ChiRi 专属）。
///
/// boost 模式（performance/fast/特调）下把各 cluster 的 core_ctl `min_cpus`
/// 抬到全组常在线，防止厂商热插拔把大核下线、与 ChiRi 升降频决策打架；
/// 退出 boost 恢复快照值。只动 min_cpus，不改 max_cpus / busy 阈值，改动面最小。
/// cluster 发现：遍历 cpufreq policy → related_cpus 首个 CPU 的
/// `/sys/devices/system/cpu/cpuN/core_ctl`（同 cluster 各 CPU 目录指向同一控制器，
/// 每个 policy 只注册一份，天然去重）。

use crate::chiri::get_cpu_policies;
use log::{info, warn};
use std::fs;

use crate::fluent_args;
use crate::i18n::{t, t_with_args};

/// 单个 cluster 的 core_ctl 控制节点
struct CoreCtlCluster {
    /// core_ctl 目录（如 /sys/devices/system/cpu/cpu3/core_ctl）
    dir: String,
    /// cluster 内 CPU 数（boost 时 min_cpus 的目标值）
    cluster_size: u32,
    /// 快照的原始 min_cpus
    min_cpus: String,
}

/// core_ctl 接管器
pub struct CoreCtlManager {
    /// 可控 cluster 列表；惰性发现（首次 boost 前枚举一次）
    clusters: Vec<CoreCtlCluster>,
    /// 是否已完成发现（含发现结果为空的情况，避免重复枚举）
    discovered: bool,
    /// 当前是否处于 boost 状态（去重写）
    boosted: bool,
}

impl CoreCtlManager {
    pub fn new() -> Self {
        Self {
            clusters: Vec::new(),
            discovered: false,
            boosted: false,
        }
    }

    /// 枚举各 cluster 的 core_ctl 控制节点并快照 min_cpus。
    /// 无任何 core_ctl 节点（内核不支持/未启用）时打点一次，之后保持空表。
    fn discover(&mut self) {
        if self.discovered {
            return;
        }
        self.discovered = true;
        for policy in get_cpu_policies() {
            // related_cpus 首个 CPU 即该 cluster 的代表（core_ctl 挂在 cluster 首 CPU 下）
            let related = fs::read_to_string(format!(
                "/sys/devices/system/cpu/cpufreq/policy{}/related_cpus",
                policy.id
            ))
            .or_else(|_| {
                fs::read_to_string(format!(
                    "/sys/devices/system/cpu/cpufreq/policy{}/affected_cpus",
                    policy.id
                ))
            })
            .unwrap_or_default();
            let cpus: Vec<u32> = related
                .split_whitespace()
                .filter_map(|s| s.parse().ok())
                .collect();
            let Some(first) = cpus.first() else {
                continue;
            };
            let dir = format!("/sys/devices/system/cpu/cpu{}/core_ctl", first);
            let min_path = format!("{}/min_cpus", dir);
            if let Ok(val) = fs::read_to_string(&min_path) {
                let val = val.trim().to_string();
                if val.is_empty() {
                    continue;
                }
                self.clusters.push(CoreCtlCluster {
                    dir,
                    cluster_size: cpus.len() as u32,
                    min_cpus: val,
                });
            }
        }
        if self.clusters.is_empty() {
            info!("{}", t("corectl-unavailable"));
        }
    }

    /// 设置/解除 boost：boost 时 min_cpus 抬到全组在线，解除时恢复快照。
    /// `on` 未变化时为无写空操作（可由 2s 周期刷新安全调用）。
    pub fn set_boost(&mut self, on: bool) {
        if on == self.boosted {
            return;
        }
        if on {
            self.discover();
            for c in &self.clusters {
                let path = format!("{}/min_cpus", c.dir);
                if crate::utils::try_write_file(&path, &c.cluster_size.to_string()).is_err() {
                    warn!(
                        "{}",
                        t_with_args(
                            "corectl-write-failed",
                            &fluent_args!("path" => path)
                        )
                    );
                }
            }
            if !self.clusters.is_empty() {
                info!(
                    "{}",
                    t_with_args(
                        "corectl-boost-on",
                        &fluent_args!("count" => self.clusters.len().to_string())
                    )
                );
            }
        } else {
            for c in &self.clusters {
                let path = format!("{}/min_cpus", c.dir);
                let _ = crate::utils::try_write_file(&path, &c.min_cpus);
            }
            if !self.clusters.is_empty() {
                info!("{}", t("corectl-boost-off"));
            }
        }
        self.boosted = on;
    }

    /// 释放接管：恢复全部快照（调度线程收尾时调用）。
    pub fn release(&mut self) {
        if self.boosted {
            self.set_boost(false);
        }
    }
}
