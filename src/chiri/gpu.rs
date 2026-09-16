//! gpu.rs: [detect] [types] [lock] [release]
//!
/// GPU 频率锁（contingency 使用）：启动时探测一次 devfreq 节点，能确定硬件最高频的
/// 才收录；lock 时快照当前 min/max 并把 min=max 锁到硬件最高频，release 按快照恢复。
/// 探测不到任何可用节点时保持空表，lock 为 no-op（只打一次 warn）——GPU 节点各 SoC
/// 差异很大，宁可不管也不乱写。
///
/// 节点覆盖：
/// - 高通 Adreno：`/sys/class/kgsl/kgsl-3d0/`（硬件上限 `max_gpu_clk`，写入走同名 devfreq）
/// - 通用 devfreq：`/sys/class/devfreq/` 下名字含 gpu / kgsl / mali 的设备
use crate::utils::FastWriter;
use log::{info, warn};
use std::fs;

use crate::fluent_args;
use crate::i18n::{t, t_with_args};

const GPU_DEVFREQ_ROOT: &str = "/sys/class/devfreq";
const ADRENO_DIR: &str = "/sys/class/kgsl/kgsl-3d0";
const ADRENO_MAX_CLK: &str = "/sys/class/kgsl/kgsl-3d0/max_gpu_clk";

fn read_u32(path: &str) -> Option<u32> {
    fs::read_to_string(path).ok()?.trim().parse().ok()
}

fn read_freq_range(dir: &str) -> Option<(u32, u32)> {
    // available_frequencies 是唯一可信来源：max 是硬件上限，min 是硬件最低频
    // （release 时残留防护的兜底恢复值）；devfreq 的 max_freq/min_freq 只是当前限值
    let mut freqs: Vec<u32> = fs::read_to_string(format!("{dir}/available_frequencies"))
        .ok()?
        .split_whitespace()
        .filter_map(|s| s.parse().ok())
        .collect();
    freqs.sort_unstable();
    freqs.dedup();
    Some((*freqs.first()?, *freqs.last()?))
}

// [detect]
/// 收录一个 devfreq 目录：写入器有效且能确定 hw_max/hw_min 才要
fn probe_devfreq(dir: &str, out: &mut Vec<GpuNode>) {
    // available_frequencies 缺失时，Adreno 用 max_gpu_clk 兜底硬件上限
    let (hw_min, hw_max) = match read_freq_range(dir) {
        Some(r) => r,
        None if dir.contains("kgsl") => {
            // Adreno：available_frequencies 缺失时用 max_gpu_clk 兜底硬件上限
            match read_u32(ADRENO_MAX_CLK) {
                Some(hw_max) => (read_u32(&format!("{dir}/min_freq")).unwrap_or(hw_max), hw_max),
                None => return,
            }
        }
        None => return,
    };
    if hw_max == 0 || hw_min == 0 || hw_min > hw_max {
        return;
    }
    let max_writer = FastWriter::new(format!("{dir}/max_freq"));
    let min_writer = FastWriter::new(format!("{dir}/min_freq"));
    if !max_writer.is_valid() {
        return;
    }
    let min_writer = min_writer.is_valid().then_some(min_writer);
    out.retain(|n: &GpuNode| n.label != dir);
    out.push(GpuNode {
        label: dir.to_string(),
        max_writer,
        min_writer,
        orig_max: fs::read_to_string(format!("{dir}/max_freq"))
            .ok()
            .map(|s| s.trim().to_string()),
        orig_min: fs::read_to_string(format!("{dir}/min_freq"))
            .ok()
            .map(|s| s.trim().to_string()),
        hw_min,
        hw_max,
    });
}

// [types]
struct GpuNode {
    label: String,
    max_writer: FastWriter,
    min_writer: Option<FastWriter>,
    /// 快照：恢复时按原样写回（含内核格式差异，不做数值规整）
    orig_max: Option<String>,
    orig_min: Option<String>,
    hw_min: u32,
    hw_max: u32,
}

pub struct GpuGuard {
    nodes: Vec<GpuNode>,
    active: bool,
}

impl GpuGuard {
    /// 启动时构造并探测一次；探测结果缓存，之后不再扫文件系统
    pub fn new() -> Self {
        let mut nodes = Vec::new();
        // 高通 Adreno：kgsl-3d0 在 /sys/class/kgsl 下，也常以 devfreq 形式出现
        if std::path::Path::new(ADRENO_DIR).exists() {
            probe_devfreq(ADRENO_DIR, &mut nodes);
        }
        if let Ok(entries) = fs::read_dir(GPU_DEVFREQ_ROOT) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_lowercase();
                let dir = entry.path().to_string_lossy().to_string();
                if name.contains("gpu") || name.contains("kgsl") || name.contains("mali") {
                    probe_devfreq(&dir, &mut nodes);
                }
            }
        }
        if nodes.is_empty() {
            warn!("{}", t("gpu-detect-miss"));
        }
        Self {
            nodes,
            active: false,
        }
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    // [lock]
    /// min=max 锁到硬件最高频。先升 max 再升 min（避免 min>max 被内核拒绝）。
    /// 重复激活是 no-op；无节点时也是 no-op（探测已打过 warn）。
    pub fn lock(&mut self) {
        if self.active || self.nodes.is_empty() {
            return;
        }
        let mut ok = 0usize;
        for node in &mut self.nodes {
            if !node.max_writer.write_value_force(node.hw_max) {
                continue;
            }
            if let Some(min) = &mut node.min_writer {
                min.write_value_force(node.hw_max);
            }
            ok += 1;
        }
        if ok > 0 {
            self.active = true;
            info!(
                "{}",
                t_with_args(
                    "gpu-locked",
                    &fluent_args!(
                        "khz" => self.nodes.first().map(|n| n.hw_max).unwrap_or(0).to_string(),
                        "nodes" => ok.to_string(),
                    ),
                )
            );
        }
    }

    // [release]
    /// 按快照恢复原 min/max。先降 min 再降 max（避免 max<min 被内核拒绝）。
    pub fn release(&mut self) {
        if !self.active {
            return;
        }
        for node in &mut self.nodes {
            if let (Some(min), Some(orig)) = (&mut node.min_writer, &node.orig_min) {
                // 残留防护：orig_min 若等于 hw_max（上次 SIGKILL 残留 min=max 锁频），
                // 恢复硬件最低频而不是把残留值当「原值」固化
                let restore_min = match orig.parse::<u32>() {
                    Ok(v) if v != node.hw_max => v,
                    _ => node.hw_min,
                };
                min.write_value_force(restore_min);
            }
            if let Some(orig) = &node.orig_max {
                if let Ok(v) = orig.parse::<u32>() {
                    node.max_writer.write_value_force(v);
                }
            }
        }
        self.active = false;
        info!("{}", t("gpu-released"));
    }
}
