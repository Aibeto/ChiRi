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

use crate::common::DaemonEvent;
use crate::utils::get_ktime_ns;
use aya::maps::{HashMap as BpfHashMap, PerCpuArray};
use aya::util::online_cpus;
use aya::{Ebpf, programs::TracePoint};
use log::{debug, info, warn};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::SyncSender;
use tokio::sync::watch;

use crate::fluent_args;
use crate::i18n::{t, t_with_args};

/// 常规采样周期由调用方（main.rs 按 SoC）传入：ChiRi 160ms / Yumi 200ms。
/// 见 `start_cpu_loop` 的 `sample_ms_normal` 参数。
/// 特调采样周期（ms）：明日方舟特调激活时缩短到 40ms，保证特调档位判定的响应度
const SAMPLE_MS_TUNED: u64 = 40;
/// 前台应用 CPU 利用率计算开关：仅 FAS（帧感知调度）消费 foreground_max_util。
/// FAS 禁用（false）期间两套调度器均忽略该字段，跳过计算省掉每 tick 的 TGID/线程级开销；
/// start_cpu_loop 入口按「ChiRi 且 FAS 配置可用」动态置位，Yumi 设备恒为 false（行为零变化）。
static FAS_FG_UTIL_ENABLED: AtomicBool = AtomicBool::new(false);

/// 读取 PerCpuArray 计数 map 的全核总和（key 0 的所有 cpu 槽位累加）。
/// map 缺失（None，eBPF 产物与 daemon 版本偏差时）返回 0，保持计数可选语义。
fn percpu_total(map: Option<&PerCpuArray<&mut aya::maps::MapData, u64>>) -> u64 {
    let Some(map) = map else {
        return 0;
    };
    map.get(&0u32, 0)
        .map(|v| v.iter().sum::<u64>())
        .unwrap_or(0)
}

/// 读取前台进程的所有线程 TID（仅供 foreground 利用率降级路径使用）。
/// 仅在 FAS_FG_UTIL_ENABLED 置位（ChiRi 且 FAS 可用）时经降级路径被调用。
fn get_thread_tids(pid: u32) -> Vec<u32> {
    let task_dir = format!("/proc/{}/task", pid);
    let mut tids = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&task_dir) {
        for entry in entries.flatten() {
            if let Some(name) = entry.file_name().to_str() {
                if let Ok(tid) = name.parse::<u32>() {
                    tids.push(tid);
                }
            }
        }
    }
    tids
}

pub async fn start_cpu_loop(
    tx: SyncSender<DaemonEvent>,
    rx_pid: watch::Receiver<u32>,
    ak_active: Arc<AtomicBool>,
    sample_ms_normal: u64,
) -> Result<(), anyhow::Error> {
    // 与 fps_monitor 保持一致：debug 构建嵌 debug 产物，release 构建嵌 release 产物
    #[cfg(debug_assertions)]
    let bpf = Box::leak(Box::new(Ebpf::load(include_bytes!(concat!(
        env!("OUT_DIR"),
        "/ebpf_target/bpfel-unknown-none/debug/yumi-ebpf"
    )))?));
    #[cfg(not(debug_assertions))]
    let bpf = Box::leak(Box::new(Ebpf::load(include_bytes!(concat!(
        env!("OUT_DIR"),
        "/ebpf_target/bpfel-unknown-none/release/yumi-ebpf"
    )))?));
    let program: &mut TracePoint = bpf.program_mut("handle_sched_switch").unwrap().try_into()?;
    program.load()?;
    program.attach("sched", "sched_switch")?;
    info!("{}", t("cpu-monitor-started"));

    // FAS 恢复前台利用率计算：仅 ChiRi 且 FAS 配置可用时置位（Yumi 设备保持 false，
    // 两套调度器对该字段均忽略，行为零变化）
    if crate::common::is_chiri_soc() && crate::common::fas_available() {
        FAS_FG_UTIL_ENABLED.store(true, Ordering::Relaxed);
    }

    // ChiRi 专属：附加可选扩展探针（唤醒/线程迁移/频率切换计数，遥测观测用）。
    // 内核缺少对应 tracepoint 时挂载失败，warn 一次后跳过，不影响主探针；
    // 仅 ChiRi SoC 尝试挂载，Yumi 设备保持原有单探针行为不变。
    let chiri_telemetry = crate::common::is_chiri_soc();
    if chiri_telemetry {
        for (name, cat, tp) in [
            ("handle_sched_wakeup", "sched", "sched_wakeup"),
            ("handle_sched_migrate_task", "sched", "sched_migrate_task"),
            ("handle_cpufreq_transition", "cpufreq", "cpufreq_transition"),
        ] {
            let result = (|| -> anyhow::Result<()> {
                let prog: &mut TracePoint = bpf
                    .program_mut(name)
                    .ok_or_else(|| anyhow::anyhow!("program {name} not found in ELF"))?
                    .try_into()?;
                prog.load()?;
                prog.attach(cat, tp)?;
                Ok(())
            })();
            match result {
                Ok(()) => info!(
                    "{}",
                    t_with_args("telemetry-probe-attached", &fluent_args!("name" => name))
                ),
                Err(e) => warn!(
                    "{}",
                    t_with_args(
                        "telemetry-probe-failed",
                        &fluent_args!("name" => name, "error" => e.to_string())
                    )
                ),
            }
        }
    }

    // 获取准确的物理在线核心列表
    let online_cpus_list = online_cpus().map_err(|e| {
        anyhow::anyhow!(
            "{}",
            t_with_args(
                "cpu-monitor-online-cpus-failed",
                &fluent_args!("error" => format!("{:?}", e))
            )
        )
    })?;
    let max_cpu_id = online_cpus_list.iter().copied().max().unwrap_or(0) as usize;
    info!(
        "{}",
        t_with_args(
            "cpu-monitor-online-cpus",
            &fluent_args!("cpus" => format!("{:?}", online_cpus_list))
        )
    );

    let bpf_ptr = bpf as *mut Ebpf;

    let core_idle_map: PerCpuArray<_, u64> =
        PerCpuArray::try_from(unsafe { &mut *bpf_ptr }.map_mut("CORE_IDLE_TIME").unwrap())?;
    let core_busy_map: PerCpuArray<_, u64> =
        PerCpuArray::try_from(unsafe { &mut *bpf_ptr }.map_mut("CORE_BUSY_TIME").unwrap())?;
    let core_last_time_map: PerCpuArray<_, u64> =
        PerCpuArray::try_from(unsafe { &mut *bpf_ptr }.map_mut("CORE_LAST_TIME").unwrap())?;
    let core_current_tid_map: PerCpuArray<_, u32> = PerCpuArray::try_from(
        unsafe { &mut *bpf_ptr }
            .map_mut("CORE_CURRENT_TID")
            .unwrap(),
    )?;
    let thread_run_map: BpfHashMap<_, u32, u64> =
        BpfHashMap::try_from(unsafe { &mut *bpf_ptr }.map_mut("THREAD_RUN_TIME").unwrap())?;

    // TGID 级聚合运行时间 map
    let tgid_run_map: BpfHashMap<_, u32, u64> =
        BpfHashMap::try_from(unsafe { &mut *bpf_ptr }.map_mut("TGID_RUN_TIME").unwrap())?;

    // 每核当前 TGID map (用于 pending delta 补偿)
    let core_current_tgid_map: PerCpuArray<_, u32> = PerCpuArray::try_from(
        unsafe { &mut *bpf_ptr }
            .map_mut("CORE_CURRENT_TGID")
            .unwrap(),
    )?;

    // 扩展探针计数 map：可选语义——ELF 中缺失（产物与 daemon 版本偏差）时计数
    // 恒为 0 并 warn 一次，绝不 panic（与探针挂载失败的容忍路径语义一致）。
    let fetch_counter_map =
        |name: &'static str| -> Option<PerCpuArray<&mut aya::maps::MapData, u64>> {
            match unsafe { &mut *bpf_ptr }
                .map_mut(name)
                .and_then(|m| PerCpuArray::try_from(m).ok())
            {
                Some(m) => Some(m),
                None => {
                    warn!(
                        "{}",
                        t_with_args("telemetry-map-missing", &fluent_args!("name" => name))
                    );
                    None
                }
            }
        };
    let wakeup_map = fetch_counter_map("WAKEUP_COUNT");
    let migrate_map = fetch_counter_map("MIGRATE_COUNT");
    let freq_trans_map = fetch_counter_map("FREQ_TRANS_COUNT");

    tokio::spawn(async move {
        let mut rx_pid = rx_pid;
        // 前台 PID 由 monitor/mod.rs 的 pid_watcher 统一广播，这里只消费最新值
        let mut fg_pid: u32 = *rx_pid.borrow();
        // 根据最大 CPU ID 初始化历史记录向量，避免越界
        let mut last_idle_times = vec![0u64; max_cpu_id + 1];
        let mut last_busy_times = vec![0u64; max_cpu_id + 1];
        let mut last_check_time = get_ktime_ns();

        debug!(
            "{}",
            t_with_args(
                "cpu-monitor-baseline",
                &fluent_args!(
                    "cpus" => format!("{:?}", online_cpus_list),
                    "max_cpu" => max_cpu_id.to_string()
                )
            )
        );

        // TGID 级聚合数据：per-PID 的历史值
        let mut last_tgid_run: u64 = 0;
        let mut last_tgid_pid: u32 = 0; // 上一次采样时的前台 PID
        // 备用: 线程级数据 (当 TGID map 不可用时)
        let mut last_thread_run: std::collections::HashMap<u32, u64> =
            std::collections::HashMap::new();

        let mut log_counter: u32 = 0;

        // 扩展探针统计：2s 读取一次累计计数并发送周期增量事件（ChiRi 专属）
        const STATS_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);
        let mut last_stats_check = std::time::Instant::now();
        let mut last_wakeup_total: u64 = 0;
        let mut last_migrate_total: u64 = 0;
        let mut last_freq_total: u64 = 0;

        let mut interval =
            tokio::time::interval(std::time::Duration::from_millis(sample_ms_normal));

        loop {
            interval.tick().await;
            // 消费 pid_watcher 广播的前台 PID（500ms 源 + 常规/特调40ms 采样，足够及时）
            if rx_pid.has_changed().unwrap_or(false) {
                fg_pid = *rx_pid.borrow_and_update();
            }
            let now_ktime = get_ktime_ns();
            let real_delta_ns = now_ktime.saturating_sub(last_check_time);
            last_check_time = now_ktime;

            if real_delta_ns == 0 {
                continue;
            }

            let zero_key: u32 = 0;
            let per_cpu_idle_values = core_idle_map.get(&zero_key, 0);
            let per_cpu_busy_values = core_busy_map.get(&zero_key, 0);
            let per_cpu_last_time = core_last_time_map.get(&zero_key, 0);
            let per_cpu_current_tid = core_current_tid_map.get(&zero_key, 0);
            let per_cpu_current_tgid = core_current_tgid_map.get(&zero_key, 0);

            let mut core_utils = vec![0.0_f32; max_cpu_id + 1];

            // 1. 全局单核利用率计算（带有实时状态补偿）
            //    注意：core_utils 按「真实 CPU ID」索引（长度 max_cpu_id + 1），
            //    与 CLG 端 core_utils.get(cpu_id) 保持一致；若按在线列表顺序 push，
            //    在线核不连续（如 [0,2,4,6]）时 CLG 会取到错误的核/取不到，负载归零。
            for &cpu_id in &online_cpus_list {
                let idx = cpu_id as usize;

                let raw_idle = per_cpu_idle_values
                    .as_ref()
                    .ok()
                    .and_then(|v| v.get(idx))
                    .copied()
                    .unwrap_or(0);
                let raw_busy = per_cpu_busy_values
                    .as_ref()
                    .ok()
                    .and_then(|v| v.get(idx))
                    .copied()
                    .unwrap_or(0);
                let last_switch_time = per_cpu_last_time
                    .as_ref()
                    .ok()
                    .and_then(|v| v.get(idx))
                    .copied()
                    .unwrap_or(0);
                let current_tid = per_cpu_current_tid
                    .as_ref()
                    .ok()
                    .and_then(|v| v.get(idx))
                    .copied()
                    .unwrap_or(0);

                let mut adj_idle = raw_idle;
                let mut adj_busy = raw_busy;

                // 计算当前正在执行的任务积累但未触发 sched_switch 的时间
                let mut pending_delta = now_ktime.saturating_sub(last_switch_time);
                if pending_delta > 1_000_000_000 {
                    pending_delta = 0; // 防御性保护，剔除极大异常值
                }

                if current_tid == 0 {
                    adj_idle += pending_delta;
                } else {
                    adj_busy += pending_delta;
                }

                let idle_diff = adj_idle.saturating_sub(last_idle_times[idx]);
                let busy_diff = adj_busy.saturating_sub(last_busy_times[idx]);
                let total_diff = idle_diff + busy_diff;

                let util = if total_diff > 0 {
                    (busy_diff as f32 / total_diff as f32).clamp(0.0, 1.0)
                } else {
                    0.0
                };

                core_utils[idx] = util;
                last_idle_times[idx] = adj_idle;
                last_busy_times[idx] = adj_busy;
            }

            // 2. 前台应用利用率计算
            //    主路径: 使用 tgid_run_time map (TGID 级聚合)
            //    只需查询 1 个 key，不受 thread_run_time HASH 驱逐影响
            // FAS 禁用时 foreground_max_util 无消费方，跳过计算（见 FAS_FG_UTIL_ENABLED）
            let foreground_max_util = if FAS_FG_UTIL_ENABLED.load(Ordering::Relaxed) {
                if fg_pid == 0 {
                    0.0_f32
                } else {
                    // PID 切换时重置 TGID 基线，避免跨进程的累计值比较
                    if fg_pid != last_tgid_pid {
                        debug!(
                            "{}",
                            t_with_args(
                                "cpu-monitor-fg-baseline-reset",
                                &fluent_args!(
                                    "old" => last_tgid_pid.to_string(),
                                    "new" => fg_pid.to_string()
                                )
                            )
                        );
                        last_tgid_run = 0;
                        last_tgid_pid = fg_pid;
                        // 同时清空线程级缓存（PID 变了，旧 TID 无意义）
                        last_thread_run.clear();
                    }

                    // ── 主路径: TGID 级聚合 ──
                    let tgid_util = compute_tgid_util(
                        fg_pid,
                        &tgid_run_map,
                        &per_cpu_current_tgid,
                        &per_cpu_last_time,
                        &online_cpus_list,
                        now_ktime,
                        real_delta_ns,
                        &mut last_tgid_run,
                    );

                    if let Some(util) = tgid_util {
                        util
                    } else {
                        // ── 降级路径: 逐 TID 遍历 (原始逻辑，作为 fallback) ──
                        debug!(
                            "{}",
                            t_with_args(
                                "cpu-monitor-util-fallback",
                                &fluent_args!(
                                    "pid" => fg_pid.to_string(),
                                    "raw" => tgid_run_map.get(&fg_pid, 0).unwrap_or(0).to_string()
                                )
                            )
                        );
                        compute_thread_level_util(
                            fg_pid,
                            &thread_run_map,
                            &core_current_tid_map,
                            &per_cpu_last_time,
                            &online_cpus_list,
                            now_ktime,
                            real_delta_ns,
                            &mut last_thread_run,
                        )
                    }
                }
            } else {
                0.0_f32
            };

            log_counter += 1;
            // format! 在宏外求值，用 log_enabled! 门控省掉 INFO 级别下的分配
            if log_counter % 25 == 0 && log::log_enabled!(log::Level::Debug) {
                let cores_str = core_utils
                    .iter()
                    .map(|u| format!("{:.0}", u * 100.0))
                    .collect::<Vec<_>>()
                    .join(", ");

                debug!(
                    "{}",
                    t_with_args(
                        "cpu-monitor-tick-log",
                        &fluent_args!(
                            "cores" => cores_str,
                            "pid" => fg_pid.to_string(),
                            "util" => format!("{:.1}", foreground_max_util * 100.0),
                            "threads" => last_thread_run.len().to_string(),
                            "delta" => (real_delta_ns / 1_000_000).to_string()
                        )
                    )
                );
            }

            if tx
                .send(DaemonEvent::SystemLoadUpdate {
                    core_utils,
                    foreground_max_util,
                })
                .is_err()
            {
                warn!("{}", t("cpu-monitor-channel-closed"));
                break;
            }

            // ChiRi 遥测：读取扩展探针累计计数，按周期发送增量（探针未挂载时增量恒 0，
            // 事件照发保持下游 CSV 列对齐；watchdog 不消费该事件，不影响负载源判定）
            if chiri_telemetry && last_stats_check.elapsed() >= STATS_INTERVAL {
                last_stats_check = std::time::Instant::now();
                let w = percpu_total(wakeup_map.as_ref());
                let m = percpu_total(migrate_map.as_ref());
                let f = percpu_total(freq_trans_map.as_ref());
                let stats = DaemonEvent::BpfStats {
                    wakeups: w.saturating_sub(last_wakeup_total) as u32,
                    migrations: m.saturating_sub(last_migrate_total) as u32,
                    freq_transitions: f.saturating_sub(last_freq_total) as u32,
                };
                last_wakeup_total = w;
                last_migrate_total = m;
                last_freq_total = f;
                if tx.send(stats).is_err() {
                    warn!("{}", t("cpu-monitor-channel-closed"));
                    break;
                }
            }

            // 按特调状态动态切换采样周期：akmode 激活时 40ms 快速跟随负载，
            // 其余用传入的常规间隔（ChiRi 160ms / Yumi 200ms）。
            // interval 周期固定，切换时按新周期重建（相位以本轮处理完成为基准，采样点间隔精确）。
            let target = if ak_active.load(Ordering::Relaxed) {
                std::time::Duration::from_millis(SAMPLE_MS_TUNED)
            } else {
                std::time::Duration::from_millis(sample_ms_normal)
            };
            if interval.period() != target {
                interval = tokio::time::interval_at(tokio::time::Instant::now() + target, target);
            }
        }
    });

    std::future::pending::<()>().await;
    Ok(())
}

/// 主路径: 使用 TGID 级聚合 map 计算前台进程的 CPU 利用率
///
/// 优势:
/// - 只需查询 1 个 key (TGID)，不依赖逐 TID 遍历
/// - tgid_run_time map 容量 1024，远够用（系统不会有 1024 个活跃进程）
/// - 完全规避 thread_run_time HASH 容量不足 / LRU 驱逐问题
///
/// 关键设计: 基线只保存 raw 值（不含 pending delta），避免 pending 累积漂移
///
/// 返回 Some(util) 表示成功，None 表示需要走降级路径
/// 仅在 FAS_FG_UTIL_ENABLED 置位（ChiRi 且 FAS 可用）时被 foreground 计算调用。
fn compute_tgid_util(
    fg_pid: u32,
    tgid_run_map: &BpfHashMap<&mut aya::maps::MapData, u32, u64>,
    per_cpu_current_tgid: &Result<aya::maps::PerCpuValues<u32>, aya::maps::MapError>,
    per_cpu_last_time: &Result<aya::maps::PerCpuValues<u64>, aya::maps::MapError>,
    online_cpus: &[u32],
    now_ktime: u64,
    real_delta_ns: u64,
    last_tgid_run: &mut u64,
) -> Option<f32> {
    // 读取 TGID 的累计运行时间 (BPF 侧只在 sched_switch 时更新)
    let raw_tgid_time = tgid_run_map.get(&fg_pid, 0).unwrap_or(0);

    // 如果 TGID 在 map 中完全不存在，且没有历史基线
    if raw_tgid_time == 0 && *last_tgid_run == 0 {
        return None;
    }

    // 计算当前 pending delta：正在核心上运行但还没经过 sched_switch 的时间
    // 这是一个瞬时快照值，每轮独立计算，不累积到基线中
    let mut current_pending: u64 = 0;
    if let Ok(per_cpu_tgids) = per_cpu_current_tgid.as_ref() {
        for &cpu_id in online_cpus {
            let idx = cpu_id as usize;
            let current_tgid = per_cpu_tgids.get(idx).copied().unwrap_or(0);

            if current_tgid == fg_pid {
                let last_switch = per_cpu_last_time
                    .as_ref()
                    .ok()
                    .and_then(|v| v.get(idx))
                    .copied()
                    .unwrap_or(0);
                let pending = now_ktime.saturating_sub(last_switch);
                if pending < 1_000_000_000 {
                    current_pending += pending;
                }
            }
        }
    }

    // 基线只用 raw 值（不含 pending），避免 pending 累积漂移
    // adj = raw + pending 只用于本轮差值计算
    let prev_raw = *last_tgid_run;
    *last_tgid_run = raw_tgid_time; // 保存 raw，不保存 adj

    if prev_raw == 0 {
        // 第一次采样（PID 刚切换或首次运行），只建立基线
        return Some(0.0);
    }

    // raw 值是单调递增的（BPF 侧只做 += delta）
    // 如果 raw < prev_raw 说明 map 被重置或异常
    if raw_tgid_time < prev_raw {
        return Some(0.0);
    }

    // 总增量 = (raw 增量) + (当前 pending)
    // 注意：不减去"上次 pending"，因为上次的 pending 在这轮的 raw 增量中
    // 已经被 sched_switch 消化了。如果上次 pending 的线程还在跑（没有
    // sched_switch），那它的时间会同时出现在 raw 增量和 current_pending 中，
    // 但 raw 增量中不会包含它（因为没有 sched_switch 来触发累加）。
    // 所以：total_delta = raw_delta + current_pending 是正确的。
    let raw_delta = raw_tgid_time - prev_raw;
    let total_delta = raw_delta + current_pending;

    // 利用率 = 进程总 CPU 时间增量 / 实际墙钟时间
    let util = (total_delta as f32 / real_delta_ns as f32).clamp(0.0, 1.0);

    Some(util)
}

/// 降级路径: 逐 TID 遍历计算前台最重线程的利用率 (原始逻辑)
/// 增加防驱逐保护：如果 map 返回值 < 上次记录值，跳过该 TID
/// 仅在 FAS_FG_UTIL_ENABLED 置位（ChiRi 且 FAS 可用）且 TGID 主路径失败时被调用。
fn compute_thread_level_util(
    fg_pid: u32,
    thread_run_map: &BpfHashMap<&mut aya::maps::MapData, u32, u64>,
    core_current_tid_map: &PerCpuArray<&mut aya::maps::MapData, u32>,
    per_cpu_last_time: &Result<aya::maps::PerCpuValues<u64>, aya::maps::MapError>,
    online_cpus: &[u32],
    now_ktime: u64,
    real_delta_ns: u64,
    last_thread_run: &mut std::collections::HashMap<u32, u64>,
) -> f32 {
    let tids = get_thread_tids(fg_pid);
    let mut max_util: f32 = 0.0;
    let mut current_thread_run = std::collections::HashMap::with_capacity(tids.len());
    let zero_key: u32 = 0;

    let per_cpu_current_tid = core_current_tid_map.get(&zero_key, 0);

    for &tid in &tids {
        let mut adj_thread_time = thread_run_map.get(&tid, 0).unwrap_or(0);

        // 如果该线程正在某个核心上跑，补上它的 Pending Delta
        for &cpu_id in online_cpus {
            let idx = cpu_id as usize;
            let current_tid_on_core = per_cpu_current_tid
                .as_ref()
                .ok()
                .and_then(|v| v.get(idx))
                .copied()
                .unwrap_or(0);

            if current_tid_on_core == tid {
                let last_switch_time = per_cpu_last_time
                    .as_ref()
                    .ok()
                    .and_then(|v| v.get(idx))
                    .copied()
                    .unwrap_or(0);
                let pending_delta = now_ktime.saturating_sub(last_switch_time);
                if pending_delta < 1_000_000_000 {
                    adj_thread_time += pending_delta;
                }
            }
        }

        current_thread_run.insert(tid, adj_thread_time);

        if let Some(&last_run) = last_thread_run.get(&tid) {
            // 防驱逐保护：如果新值 < 旧值，说明 HASH map 条目被驱逐后
            // 重新创建，数据不连续，跳过此 TID 本轮的计算
            if adj_thread_time >= last_run {
                let thread_delta = adj_thread_time - last_run;
                let util = (thread_delta as f32 / real_delta_ns as f32).clamp(0.0, 1.0);
                if util > max_util {
                    max_util = util;
                }
            }
        }
    }

    *last_thread_run = current_thread_run;
    max_util
}
