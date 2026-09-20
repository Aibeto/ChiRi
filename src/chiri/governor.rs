//! governor.rs: [types] [activate] [release] [residue]
//!
/// performance 调速器接管（FAS / contingency 使用）：
/// 激活时快照各 policy 的 `scaling_governor` 原值并写 `performance`，release 逐 policy 恢复。
/// 与 FastLock 同构：对象归调度线程独占、无内部锁；快照读不到原值的 policy 直接跳过
/// （宁可不管它，也不留下恢复不回去的残留）。
///
/// 健壮性边界：release 覆盖 FAS 退出 / contingency 退出 / DOWN 进入 / panic 自愈收尾 /
/// 进程收尾；SIGKILL 场景收尾不执行，残留由 [residue] 的启动清理兜底。
use log::{info, warn};
use std::fs;

use crate::fluent_args;
use crate::i18n::t_with_args;

/// 非 performance 的兜底值：残留清理与恢复失败时写回它（各模式默认调速器）
const FALLBACK_GOVERNOR: &str = "schedutil";

/// 调速器写入（区分成败，日志精简）。**不用 `try_write_file`**：它内部吞错恒返回 Ok，
/// 无法判断成功与否，且收尾会把文件 chmod 0444——对之后还要恢复的 sysfs 节点是毒药。
fn write_governor(path: &str, value: &str) -> bool {
    match fs::write(path, value) {
        Ok(()) => true,
        Err(e) => {
            warn!(
                "{}",
                t_with_args(
                    "sysfs-write-failed",
                    &fluent_args!("path" => path, "error" => e.to_string()),
                )
            );
            false
        }
    }
}

// [types]
/// 单个 policy 的调速器快照。governor 为 None 的 policy 不接管
struct GovernorSnapshot {
    policy_id: i32,
    original: String,
}

pub struct GovernorGuard {
    snapshots: Vec<GovernorSnapshot>,
    active: bool,
}

impl GovernorGuard {
    pub fn new() -> Self {
        Self {
            snapshots: Vec::new(),
            active: false,
        }
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    // [activate]
    /// 快照各 policy 原调速器并切到 performance。重复激活是 no-op（先幂等释放）。
    pub fn activate(&mut self) {
        if self.active {
            return;
        }
        let mut ok = 0usize;
        for policy in crate::chiri::get_cpu_policies() {
            let pid = policy.id;
            let path = format!(
                "/sys/devices/system/cpu/cpufreq/policy{}/scaling_governor",
                pid
            );
            // 读不到原值（节点缺失/权限）就不接管该 policy：恢复不回去的改动不做
            let Some(original) = fs::read_to_string(&path)
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
            else {
                continue;
            };
            if write_governor(&path, "performance") {
                // 逐 policy 打「由 -> 到」：原值各 policy 可能不同，聚合会丢信息
                info!(
                    "{}",
                    t_with_args(
                        "governor-switched",
                        &fluent_args!("pid" => pid.to_string(), "from" => original.as_str()),
                    )
                );
                self.snapshots.retain(|s| s.policy_id != pid);
                self.snapshots.push(GovernorSnapshot {
                    policy_id: pid,
                    original,
                });
                ok += 1;
            } else {
                warn!(
                    "{}",
                    t_with_args(
                        "governor-switch-failed",
                        &fluent_args!("pid" => pid.to_string()),
                    )
                );
            }
        }
        if ok > 0 {
            self.active = true;
        }
    }

    // [release]
    /// 恢复全部快照的原调速器；未接管时为 no-op。
    pub fn release(&mut self) {
        if !self.active {
            return;
        }
        for snap in self.snapshots.drain(..) {
            let path = format!(
                "/sys/devices/system/cpu/cpufreq/policy{}/scaling_governor",
                snap.policy_id
            );
            if !write_governor(&path, &snap.original) {
                warn!(
                    "{}",
                    t_with_args(
                        "governor-restore-failed",
                        &fluent_args!(
                            "pid" => snap.policy_id.to_string(),
                            "governor" => snap.original.as_str(),
                        ),
                    )
                );
            } else {
                info!(
                    "{}",
                    t_with_args(
                        "governor-restored",
                        &fluent_args!(
                            "pid" => snap.policy_id.to_string(),
                            "to" => snap.original.as_str(),
                        ),
                    )
                );
            }
        }
        self.active = false;
    }

    // [residue]
    /// 逐个 policy 体检：只回调「当前是 performance」的那些（pid, 节点路径）。
    /// 读不到的 policy 直接跳过（与接管口径一致）。
    fn each_residue_policy(f: &mut dyn FnMut(i32, &str)) {
        for policy in crate::chiri::get_cpu_policies() {
            let path = format!(
                "/sys/devices/system/cpu/cpufreq/policy{}/scaling_governor",
                policy.id
            );
            let Ok(cur) = fs::read_to_string(&path) else {
                continue;
            };
            if cur.trim() == "performance" {
                f(policy.id, &path);
            }
        }
    }

    /// 启动残留清理：SIGKILL 等异常退出会把 `performance` 留在节点上（收尾 release 不会
    /// 执行）。`performance` 不是任何厂商的默认调速器，启动时见到它必然是残留，一律
    /// 写回 schedutil。正常值（schedutil / walt / pelt 等）不动。
    pub fn cleanup_residue() {
        Self::each_residue_policy(&mut |pid, path| {
            if write_governor(path, FALLBACK_GOVERNOR) {
                warn!(
                    "{}",
                    t_with_args(
                        "governor-residue-cleanup",
                        &fluent_args!("pid" => pid.to_string())
                    )
                );
            }
        });
    }

    /// 停摆期的同款体检：**只报不写**。
    /// 这是「写 sysfs」的动作，而停摆的语义是把系统交回原状——残留到底是上一轮 ChiRi
    /// 留下的、还是系统/厂商自己的默认，进程无从区分（重启后节点就是内核/厂商给的
    /// 默认值，此时若仍是 performance，那就是系统自己的选择）。写了就把基线改成
    /// 「ChiRi 认为系统该有的样子」，等于停摆期还在替用户调度——上一次修的就是这类洞，
    /// 这里不能再开一个。只留日志让用户自己判断。
    pub fn report_residue() {
        Self::each_residue_policy(&mut |pid, _path| {
            warn!(
                "{}",
                t_with_args(
                    "governor-residue-down",
                    &fluent_args!("pid" => pid.to_string())
                )
            );
        });
    }
}
