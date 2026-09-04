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

/// 核心在线控制器接管（ChiRi 专属）。
///
/// 三态状态机（互斥，按「最近一次 apply」切换，内部去重）：
/// - **Boost**（performance/fast/特调）：各 cluster 的 core_ctl `min_cpus` 抬到
///   全组常在线，防止厂商热插拔把大核下线、与 ChiRi 升降频决策打架；
/// - **Scenemode 离线**（息屏 5 分钟后的深度省电）：解除 boost 后直接写
///   `/sys/devices/system/cpu/cpuN/online` 下线 CPU1..max（只留 CPU0 引导核），
///   大核簇/prime 整簇断电消除空转漏电流；逐核写后回读验证，失败的核跳过；
///   周期重入时纠偏（厂商守护进程偷偷拉起的核会被重新下线）；
/// - **Normal**：恢复全部快照（min_cpus / online）。
///
/// 为什么用 min_cpus/online 而不是逐核"按需唤醒"：唤醒大核要拉电压轨、重建
/// L2，为一个后台线程点亮大核净亏能；且直接写 online 会与厂商热插拔守护进程
/// 打架（对方会再下线）。需要更多在线核时的正确姿势是抬 core_ctl min_cpus
/// （Boost 态），让厂商内核按自己的回滞策略管理唤醒。
///
/// cluster 发现：遍历 cpufreq policy → related_cpus 首个 CPU 的
/// `/sys/devices/system/cpu/cpuN/core_ctl`（每个 policy 只注册一份，天然去重）。
/// 直接 sysfs 离线不依赖 core_ctl 节点（core_ctl 不可用的机型也能用，
/// 由 `CoreCtl.scenemode_offline` 配置独立门控）。
use crate::chiri::affinity::set_tid_affinity;
use crate::chiri::get_cpu_policies;
use log::{debug, info, warn};
use std::fs;

use crate::fluent_args;
use crate::i18n::{t, t_with_args};

/// 状态：无接管
const STATE_NONE: u8 = 0;
/// 状态：boost（min_cpus 全组常在线）
const STATE_BOOST: u8 = 1;
/// 状态：scenemode 离线（只留 CPU0）
const STATE_SCENEMODE: u8 = 2;

/// 单个 cluster 的 core_ctl 控制节点
struct CoreCtlCluster {
    /// core_ctl 目录（如 /sys/devices/system/cpu/cpu3/core_ctl）
    dir: String,
    /// cluster 内 CPU 数（boost 时 min_cpus 的目标值）
    cluster_size: u32,
    /// 快照的原始 min_cpus
    min_cpus: String,
}

/// 核心在线接管器
pub struct CoreCtlManager {
    /// 可控 cluster 列表；惰性发现（首次进入 boost/scenemode 前枚举一次）
    clusters: Vec<CoreCtlCluster>,
    /// 是否已完成发现（含发现结果为空的情况，避免重复枚举）
    discovered: bool,
    /// 当前状态（去重写）
    state: u8,
    /// scenemode 下被本模块下线的 CPU 及其原始 online 值（恢复用）
    offlined: Vec<(u32, String)>,
    /// scenemode 下守护进程自身线程是否已钉到专用小核
    self_pinned: bool,
}

/// 枚举守护进程自身全部线程 TID（/proc/self/task）
fn self_tids() -> Vec<i32> {
    let mut out = Vec::new();
    if let Ok(rd) = fs::read_dir("/proc/self/task") {
        for e in rd.flatten() {
            if let Some(t) = e.file_name().to_str().and_then(|s| s.parse::<i32>().ok()) {
                out.push(t);
            }
        }
    }
    out
}

/// scenemode 常驻在线核下限：保证待命响应（电源键等 PMIC 中断由任意在线核
/// µs 级处理，3 个小核留足后台任务与中断余量）
const STANDBY_CORES: usize = 3;

/// 计算 scenemode 下线目标：**小核全开**；大核簇 + prime 整簇下线；
/// 若小核数量不足 3 个，从大核簇按编号从小到大补足差额（同簇核心同质，
/// 编号最小者即最省电的大核）。CPU0（引导核）永不下线。
fn scenemode_targets() -> Vec<u32> {
    let ranges = crate::common::chiri_core_ranges();
    let little_n = ranges.little.clone().count();
    // 常驻核：全部小核；不足 STANDBY_CORES 时从大核补足
    let keep_big = STANDBY_CORES.saturating_sub(little_n);
    let mut targets = Vec::new();
    let mut kept_big = 0usize;
    for cpu in ranges.big.clone() {
        if kept_big < keep_big {
            kept_big += 1;
            continue;
        }
        targets.push(cpu as u32);
    }
    for cpu in ranges.prime.clone() {
        targets.push(cpu as u32);
    }
    // 防御：引导核永不下线
    targets.retain(|&c| c != 0);
    targets
}

impl CoreCtlManager {
    pub fn new() -> Self {
        Self {
            clusters: Vec::new(),
            discovered: false,
            state: STATE_NONE,
            offlined: Vec::new(),
            self_pinned: false,
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

    /// 统一状态入口（内部去重，可被 2s 周期安全调用）。
    /// - `boost`：min_cpus 抬到全组常在线（性能模式）；
    /// - `scenemode`：下线 CPU1..max 只留引导核（息屏深度省电）。
    /// 两者互斥；切换时先退出旧状态（恢复快照）再进入新状态。
    /// scenemode 维持期每次调用都会纠偏（重新下线被外部拉起的核）。
    pub fn set_power_state(&mut self, boost: bool, scenemode: bool) {
        let target = if boost {
            STATE_BOOST
        } else if scenemode {
            STATE_SCENEMODE
        } else {
            STATE_NONE
        };
        if target == self.state {
            if target == STATE_SCENEMODE {
                // 维持期纠偏：厂商热插拔守护进程可能把核悄悄拉回来
                self.reassert_offline();
            }
            return;
        }

        // 先退出当前状态（恢复快照）
        match self.state {
            STATE_BOOST => self.restore_min_cpus(),
            STATE_SCENEMODE => self.restore_online(),
            _ => {}
        }

        // 进入目标状态
        match target {
            STATE_BOOST => {
                self.discover();
                for c in &self.clusters {
                    let path = format!("{}/min_cpus", c.dir);
                    if crate::utils::try_write_file(&path, &c.cluster_size.to_string()).is_err() {
                        warn!(
                            "{}",
                            t_with_args("corectl-write-failed", &fluent_args!("path" => path))
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
            }
            STATE_SCENEMODE => {
                self.offline_cores();
            }
            _ => {}
        }
        self.state = target;
    }

    /// scenemode：下线大核簇 + prime（小核全开；小核不足 3 个时按编号从小到大
    /// 保留大核补足常驻数）。逐核写 online=0 并回读验证；写失败/内核拒绝的核
    /// 跳过（记录 warn）。成功下线的核连同原始 online 值记入 offlined，供恢复。
    /// 随后把守护进程自身全部线程钉到专用小核，避免后台任务堵塞调度服务。
    fn offline_cores(&mut self) {
        for cpu in scenemode_targets() {
            let path = format!("/sys/devices/system/cpu/cpu{}/online", cpu);
            let orig = fs::read_to_string(&path)
                .ok()
                .map(|s| s.trim().to_string())
                .unwrap_or_default();
            // 本来就离线（或节点不存在/不可读）：不记录、不恢复
            if orig != "1" {
                continue;
            }
            if crate::utils::try_write_file(&path, "0").is_err() {
                warn!(
                    "{}",
                    t_with_args("corectl-write-failed", &fluent_args!("path" => path))
                );
                continue;
            }
            // 回读验证：防内核静默拒绝（热插拔锁/厂商守护进程）
            let now = fs::read_to_string(&path)
                .ok()
                .map(|s| s.trim().to_string())
                .unwrap_or_default();
            if now != "0" {
                warn!(
                    "{}",
                    t_with_args("corectl-write-failed", &fluent_args!("path" => path))
                );
                continue;
            }
            self.offlined.push((cpu, orig));
        }
        // 专用小核：调度服务独占一颗小核，防止后台任务堵塞遥测/决策线程
        self.pin_self_dedicated();
        if !self.offlined.is_empty() {
            info!(
                "{}",
                t_with_args(
                    "corectl-scenemode-on",
                    &fluent_args!("count" => self.offlined.len().to_string())
                )
            );
        }
    }

    /// 把守护进程自身全部线程钉到专用小核（编号最大的 little 核——CPU0 承担
    /// 最多外部中断与内核家务，编号大者更安静）。scenemode 下调度服务
    /// （scheduler_ipc / telemetry / 触摸检测等）独占该核，免受后台任务挤占。
    fn pin_self_dedicated(&mut self) {
        if self.self_pinned {
            return;
        }
        let ranges = crate::common::chiri_core_ranges();
        let Some(core) = ranges.little.clone().last() else {
            return;
        };
        let mut all_ok = true;
        for tid in self_tids() {
            if !set_tid_affinity(tid, &[core]) {
                all_ok = false;
            }
        }
        self.self_pinned = all_ok;
        debug!(
            "{}",
            t_with_args(
                "corectl-self-pinned",
                &fluent_args!("core" => core.to_string())
            )
        );
    }

    /// 解除专用核钉定：守护进程线程恢复全核掩码
    fn unpin_self(&mut self) {
        if !self.self_pinned {
            return;
        }
        let ranges = crate::common::chiri_core_ranges();
        let all: Vec<usize> = (0..ranges.prime.end.max(ranges.big.end)).collect();
        for tid in self_tids() {
            let _ = set_tid_affinity(tid, &all);
        }
        self.self_pinned = false;
    }

    /// scenemode 维持期纠偏：已下线核若被外部重新拉起，重新写 0。
    /// 只读 online 文件（每 2s 最多 ~7 次小读），无写发生时零开销。
    fn reassert_offline(&mut self) {
        for (cpu, _) in &self.offlined {
            let path = format!("/sys/devices/system/cpu/cpu{}/online", cpu);
            if let Ok(v) = fs::read_to_string(&path) {
                if v.trim() != "0" {
                    let _ = crate::utils::try_write_file(&path, "0");
                }
            }
        }
    }

    /// 恢复全部被下线的核：按快照值写回 online，带回读 + 一次重试
    /// （亮屏后核必须回来，失败必须闹响）。
    fn restore_online(&mut self) {
        for (cpu, orig) in &self.offlined {
            let path = format!("/sys/devices/system/cpu/cpu{}/online", cpu);
            let write_back = |p: &str, v: &str| {
                crate::utils::try_write_file(p, v).is_ok()
                    && fs::read_to_string(p)
                        .ok()
                        .map(|s| s.trim() == v)
                        .unwrap_or(false)
            };
            if !write_back(&path, orig) && !write_back(&path, orig) {
                warn!(
                    "{}",
                    t_with_args("corectl-write-failed", &fluent_args!("path" => path))
                );
            }
        }
        if !self.offlined.is_empty() {
            info!(
                "{}",
                t_with_args(
                    "corectl-scenemode-off",
                    &fluent_args!("count" => self.offlined.len().to_string())
                )
            );
        }
        self.offlined.clear();
        // 解除专用核钉定（守护进程线程恢复全核）
        self.unpin_self();
    }

    /// 恢复各 cluster 的 min_cpus 快照（boost 退出）。
    fn restore_min_cpus(&mut self) {
        for c in &self.clusters {
            let path = format!("{}/min_cpus", c.dir);
            let _ = crate::utils::try_write_file(&path, &c.min_cpus);
        }
        if !self.clusters.is_empty() {
            info!("{}", t("corectl-boost-off"));
        }
    }

    /// 释放接管：恢复全部快照（调度线程收尾时调用）。
    pub fn release(&mut self) {
        if self.state != STATE_NONE {
            match self.state {
                STATE_BOOST => self.restore_min_cpus(),
                STATE_SCENEMODE => self.restore_online(),
                _ => {}
            }
            self.state = STATE_NONE;
        }
    }
}
