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
///   `/sys/devices/system/cpu/cpuN/online`——**小核 + 大核全开常驻**（频率
///   上限由 scenemode CLG 配置压制），仅 prime 整簇断电消除空转漏电流；
///   另将编号最大的小核**独占**给调度服务（从业务 cpuset 组移除 + 自身线程
///   移入根组 + 全线程自钉）；逐核写后回读验证，失败的核跳过；周期重入时
///   纠偏（厂商守护进程偷偷拉起的核会被重新下线、框架加回保留核会被重新移除）；
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
/// 状态：scenemode 离线（小核+大核常驻，prime 断电，1 颗小核独占给调度服务）
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
    /// scenemode 独占给调度服务的小核（None = 未独占/设备无 cpuset 时降级）
    reserved_core: Option<usize>,
    /// 独占时被移除核的业务 cpuset 组快照（(组名, 原始 cpus)，退出恢复用）
    reserved_cpusets: Vec<(String, String)>,
    /// 自身线程被移入根组前的原 cpuset 组相对路径（退出恢复用）
    self_cpuset_group: Option<String>,
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

/// 计算 scenemode 下线目标：**小核 + 大核全开常驻**（频率上限由 scenemode
/// CLG 配置统一压制），仅 prime（超大核）整簇下线。此外 scenemode 期间会
/// 把编号最大的小核独占给调度服务（从业务 cpuset 组移除 + 自身线程移入
/// 根组 + 自钉，见 offline_cores 与 affinity 的专用核独占助手）。
fn scenemode_targets() -> Vec<u32> {
    let ranges = crate::common::chiri_core_ranges();
    let mut targets: Vec<u32> = ranges.prime.clone().map(|c| c as u32).collect();
    // 防御：引导核（CPU0）无法热拔出，永不下线（prime 不含 0，双保险）
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
            reserved_core: None,
            reserved_cpusets: Vec::new(),
            self_cpuset_group: None,
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
    /// - `scenemode`：prime 整簇下线（小核+大核常驻），独占一颗小核给调度服务。
    /// 两者互斥；切换时先退出旧状态（恢复快照）再进入新状态。
    /// scenemode 维持期每次调用都会纠偏（重新下线被外部拉起的核）。
    /// STATE_NONE 下若仍有恢复失败的核残留，周期性重试恢复
    /// （restore_online 失败不 clear，见其注释——防核永久离线）。
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
            } else if target == STATE_NONE && !self.offlined.is_empty() {
                // 亮屏恢复路径：上次 restore 有核写回失败，2s 后重试
                self.restore_online();
            }
            return;
        }

        // 先退出当前状态（恢复快照）
        match self.state {
            STATE_BOOST => self.restore_min_cpus(),
            STATE_SCENEMODE => self.restore_online(),
            _ => {}
        }
        // 残留核兜底：进入新状态前先把上次恢复失败的核拉回在线——
        // 否则重新进入 scenemode 时 offline_cores 读到 online=0 会跳过记录，
        // 这些核永远失去恢复登记
        if !self.offlined.is_empty() {
            self.restore_online();
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

    /// scenemode：下线 prime（超大核）整簇——小核 + 大核全开常驻，频率上限
    /// 由 scenemode CLG 配置统一压制。逐核写 online=0 并回读验证；写失败/
    /// 内核拒绝的核跳过（记录 warn）。成功下线的核连同原始 online 值记入
    /// offlined，供恢复。
    /// 随后**独占一颗小核给调度服务**：选编号最大的小核，从全部业务 cpuset
    /// 组移除（其他进程不可调度到该核）+ 自身线程移入根组 + 全线程自钉，
    /// 保证后台任务堵塞不了调度服务（设备无 cpuset 时降级为仅自钉）。
    fn offline_cores(&mut self) {
        for cpu in scenemode_targets() {
            // 防重复：上轮恢复失败的残留核（已在 offlined 中）跳过重复登记
            if self.offlined.iter().any(|(c, _)| *c == cpu) {
                continue;
            }
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
        // 独占小核：sched_setaffinity 自钉不排他，须同时把该核从业务 cpuset
        // 组移除 + 自身线程移入根组（钉定才不会被组掩码二次过滤）
        let ranges = crate::common::chiri_core_ranges();
        if let Some(core) = ranges.little.clone().last() {
            crate::chiri::affinity::exclude_core_from_cpusets(
                core,
                &mut self.reserved_cpusets,
            );
            self.self_cpuset_group = crate::chiri::affinity::move_self_to_cpuset_root();
            self.reserved_core = Some(core);
        }
        // 专用小核自钉
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

    /// 把守护进程自身全部线程钉到专用小核（offline_cores 已独占的
    /// reserved_core——该核已从业务 cpuset 组移除、自身线程已移入根组，
    /// sched_setaffinity 由此实现真独占）。scenemode 下调度服务
    /// （scheduler_ipc / telemetry / 触摸检测等）独占该核，后台任务堵塞不了。
    fn pin_self_dedicated(&mut self) {
        if self.self_pinned {
            return;
        }
        let Some(core) = self.reserved_core else {
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

    /// scenemode 维持期纠偏：已下线核若被外部重新拉起，重新写 0；被独占的
    /// 专用小核若被框架 CpusetManager 加回业务组（top-app 的 cpus 由框架
    /// 动态管理），重新从组内移除。只读 online 文件 + 组 cpus（每 2s 数次
    /// 小读），无写发生时零开销。
    fn reassert_offline(&mut self) {
        for (cpu, _) in &self.offlined {
            let path = format!("/sys/devices/system/cpu/cpu{}/online", cpu);
            if let Ok(v) = fs::read_to_string(&path) {
                if v.trim() != "0" {
                    let _ = crate::utils::try_write_file(&path, "0");
                }
            }
        }
        if let Some(core) = self.reserved_core {
            crate::chiri::affinity::exclude_core_from_cpusets(
                core,
                &mut self.reserved_cpusets,
            );
        }
    }

    /// 恢复被下线的核：按快照值写回 online，带回读 + 一次重试。
    /// **恢复失败的核保留在 offlined 中**（此前 clear 掉后状态机回 NONE 再无
    /// 重试路径，写回被内核拒绝/异步热插拔未完成的核会永久离线——实测亮屏后
    /// 大核 4-7 一直不上线）。残留核由 set_power_state 的 STATE_NONE 分支在
    /// 每 2s 周期调用中重试，直到全部恢复。
    fn restore_online(&mut self) {
        let offlined = std::mem::take(&mut self.offlined);
        let total = offlined.len();
        let mut failed = Vec::new();
        for (cpu, orig) in offlined {
            let path = format!("/sys/devices/system/cpu/cpu{}/online", cpu);
            let write_back = |p: &str, v: &str| {
                crate::utils::try_write_file(p, v).is_ok()
                    && fs::read_to_string(p)
                        .ok()
                        .map(|s| s.trim() == v)
                        .unwrap_or(false)
            };
            if !write_back(&path, &orig) && !write_back(&path, &orig) {
                // 周期重试期间每次尝试都会经过这里，降为 debug 防刷屏；
                // 失败汇总由下方 restore-pending 打点
                debug!(
                    "{}",
                    t_with_args("corectl-write-failed", &fluent_args!("path" => path))
                );
                failed.push((cpu, orig));
            }
        }
        let recovered = total - failed.len();
        if recovered > 0 {
            info!(
                "{}",
                t_with_args(
                    "corectl-scenemode-off",
                    &fluent_args!("count" => recovered.to_string())
                )
            );
        }
        // 失败项保留，等待下个周期重试；成功项自然丢弃
        self.offlined = failed;
        if !self.offlined.is_empty() {
            warn!(
                "{}",
                t_with_args(
                    "corectl-restore-pending",
                    &fluent_args!("count" => self.offlined.len().to_string())
                )
            );
        }
        // 全部恢复后才解除专用核钉定与独占（守护进程线程恢复全核）；
        // 有残留时保持钉定——守护线程仍应避开离线核池
        if self.offlined.is_empty() {
            // 释放独占小核：cpuset 组恢复原始值（保留核还给业务组）、自身
            // 线程移回原组、解除全线程自钉
            crate::chiri::affinity::restore_excluded_cpusets(std::mem::take(
                &mut self.reserved_cpusets,
            ));
            if let Some(group) = self.self_cpuset_group.take() {
                crate::chiri::affinity::move_self_to_cpuset_group(&group);
            }
            self.reserved_core = None;
            self.unpin_self();
        }
    }

    /// 启动时强制上线全部核心：上次运行可能在 scenemode 中途被杀，残留的
    /// 离线核会让其 cpufreq policy 目录消失（CLG/akmode/fast_lock 启动初始化
    /// 枚举不到该集群，永久失去 worker），且核本身永久离线。调度线程启动
    /// 阶段在任何 governor 接管之前调用，把调度范围内的核全部写 online=1，
    /// 同时清空 offlined 残留快照（快照对应的恢复语义已由本调用替代）。
    pub fn force_online_all(&mut self) {
        let ranges = crate::common::chiri_core_ranges();
        let write_back = |path: &str| {
            crate::utils::try_write_file(path, "1").is_ok()
                && fs::read_to_string(path)
                    .ok()
                    .map(|s| s.trim() == "1")
                    .unwrap_or(false)
        };
        for cpu in ranges
            .little
            .clone()
            .chain(ranges.big.clone())
            .chain(ranges.prime.clone())
        {
            let path = format!("/sys/devices/system/cpu/cpu{}/online", cpu);
            // 已在线（或节点不可读——核数少于布局的机型）直接跳过
            if fs::read_to_string(&path)
                .ok()
                .map(|s| s.trim() == "1")
                .unwrap_or(true)
            {
                continue;
            }
            if !write_back(&path) && !write_back(&path) {
                // 启动期失败打 warn：热插拔锁/厂商守护进程可能拒绝，
                // 后续亮屏/ModeChange 路径的 restore_online 仍会按快照兜底
                warn!(
                    "{}",
                    t_with_args("corectl-write-failed", &fluent_args!("path" => path))
                );
            }
        }
        self.offlined.clear();
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
