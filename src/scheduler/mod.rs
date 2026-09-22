//! mod.rs: [mods] [policy]
//!
//! 原 Yumi 调度兜底（`cpu_load_governor.rs` / `scheduler.rs` / `config.rs` 与本模块的
//! thread/cfgwatch/ipc 接线）已于 2026-09-22 移除——非 ChiRi SoC 不再接管 CPU。
//! 本模块只保留 FAS 引擎（ChiRi 的 fas 模式在用）与 FAS 依赖的 policy 探测工具。

use std::fs;

// [mods]
// FAS（帧感知调度）引擎：ChiRi 侧经 crate::scheduler::fas::FasController 使用。
pub mod fas;

/// CPU 频率策略簇信息
pub struct CpuPolicy {
    pub id: i32,
    /// boost 频率列表（单位 kHz），有的簇没有此文件则为空
    pub boost_frequencies: Vec<u32>,
}

// [policy]
// 动态获取系统中实际可用的 CPU Policy，并读取 boost 频率
pub fn get_cpu_policies() -> Vec<CpuPolicy> {
    let mut policies = Vec::new();
    if let Ok(entries) = std::fs::read_dir("/sys/devices/system/cpu/cpufreq") {
        for entry in entries.flatten() {
            if let Some(name) = entry.file_name().to_str() {
                if name.starts_with("policy") {
                    if let Ok(pid) = name["policy".len()..].parse::<i32>() {
                        let boost_freqs = read_boost_frequencies(pid);
                        policies.push(CpuPolicy {
                            id: pid,
                            boost_frequencies: boost_freqs,
                        });
                    }
                }
            }
        }
    }
    policies.sort_unstable_by_key(|p| p.id);
    policies
}

fn read_boost_frequencies(pid: i32) -> Vec<u32> {
    let path = format!(
        "/sys/devices/system/cpu/cpufreq/policy{}/scaling_boost_frequencies",
        pid
    );
    std::fs::read_to_string(&path)
        .unwrap_or_default()
        .split_whitespace()
        .filter_map(|s| s.parse().ok())
        .collect()
}

/// 通过 sysfs 探测指定 policy 的 capacity 值
/// 供 FAS 的 capacity 权重计算使用（fas/policy_mgmt.rs）。
pub(super) fn probe_policy_capacity(policy_id: i32) -> Option<u32> {
    let related_str = fs::read_to_string(format!(
        "/sys/devices/system/cpu/cpufreq/policy{}/related_cpus",
        policy_id
    ))
    .or_else(|_| {
        fs::read_to_string(format!(
            "/sys/devices/system/cpu/cpufreq/policy{}/affected_cpus",
            policy_id
        ))
    })
    .ok()?;
    let first_cpu: u32 = related_str.split_whitespace().next()?.parse().ok()?;
    fs::read_to_string(format!(
        "/sys/devices/system/cpu/cpu{}/cpu_capacity",
        first_cpu
    ))
    .ok()?
    .trim()
    .parse::<u32>()
    .ok()
}

/// 根据 CPU capacity 自动计算每个 cluster 的权重
/// 供 FAS 使用（fas/policy_mgmt.rs）。
pub(super) fn auto_compute_capacity_weights(policies: &[CpuPolicy]) -> Option<Vec<(i32, f32)>> {
    let caps: Vec<(i32, u32)> = policies
        .iter()
        .filter(|p| p.id != -1)
        .filter_map(|p| probe_policy_capacity(p.id).map(|c| (p.id, c)))
        .collect();
    if caps.is_empty() || caps.iter().any(|&(_, c)| c == 0) {
        return None;
    }
    let min_cap = caps.iter().map(|&(_, c)| c).min().unwrap() as f32;
    Some(
        caps.iter()
            .map(|&(pid, cap)| {
                let r = cap as f32 / min_cap;
                (
                    pid,
                    if r <= 1.01 {
                        1.0
                    } else {
                        1.0 + (r - 1.0).sqrt()
                    },
                )
            })
            .collect(),
    )
}
