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

use std::sync::{Arc, Mutex, RwLock, mpsc};
use std::thread;
use std::time::{Duration, Instant};
use std::fs;
use anyhow::Result;

// CLG 看门狗：SystemLoadUpdate 常规 120ms（Chiri 特调 40ms）投喂一次，若超过 CLG_STALE_MAX 时长
// 未收到任何事件，视为负载源失效（eBPF 加载失败/探针崩溃/通道断开），主动 release()
// 回滚到系统原生调频，避免 CPU 永久锁频在最后写入值上（8550 balance 等 perf_init=1.0
// 的配置下会锁满全核高频）。
const CLG_STALE_MAX: Duration = Duration::from_secs(5);
/// 看门狗巡检间隔：事件循环无事件时的轮询周期
const CLG_STALE_POLL: Duration = Duration::from_secs(1);

pub mod config;
pub mod scheduler;
// FAS（帧感知调度）引擎：ChiRi 侧经 crate::scheduler::fas::FasController 使用；
// Yumi 调度器自身不启用 FAS（模式门控在 app_detect::determine_mode，Yumi 永不进入 fas 模式）。
pub mod fas;
pub mod cpu_load_governor;

use crate::i18n::{t, load_language, t_with_args};
use crate::fluent_args; 
use crate::utils; 
use crate::common::DaemonEvent; 
use config::Config;
use scheduler::CpuScheduler;
use crate::logger;
use crate::common;

/// CPU 频率策略簇信息
pub struct CpuPolicy {
    pub id: i32,
    /// boost 频率列表（单位 kHz），有的簇没有此文件则为空
    pub boost_frequencies: Vec<u32>,
}

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
/// 仅供 FAS 的 capacity 权重计算使用，FAS 禁用期间暂无调用，恢复时启用。
#[allow(dead_code)]
pub(super) fn probe_policy_capacity(policy_id: i32) -> Option<u32> {
    let related_str = fs::read_to_string(
        format!("/sys/devices/system/cpu/cpufreq/policy{}/related_cpus", policy_id))
        .or_else(|_| fs::read_to_string(
            format!("/sys/devices/system/cpu/cpufreq/policy{}/affected_cpus", policy_id)))
        .ok()?;
    let first_cpu: u32 = related_str.split_whitespace().next()?.parse().ok()?;
    fs::read_to_string(format!("/sys/devices/system/cpu/cpu{}/cpu_capacity", first_cpu))
        .ok()?.trim().parse::<u32>().ok()
}

/// 根据 CPU capacity 自动计算每个 cluster 的权重
/// 仅供 FAS 使用，FAS 禁用期间暂无调用，恢复时启用。
#[allow(dead_code)]
pub(super) fn auto_compute_capacity_weights(policies: &[CpuPolicy]) -> Option<Vec<(i32, f32)>> {
    let caps: Vec<(i32, u32)> = policies.iter()
        .filter(|p| p.id != -1)
        .filter_map(|p| probe_policy_capacity(p.id).map(|c| (p.id, c)))
        .collect();
    if caps.is_empty() || caps.iter().any(|&(_, c)| c == 0) { return None; }
    let min_cap = caps.iter().map(|&(_, c)| c).min().unwrap() as f32;
    Some(caps.iter().map(|&(pid, cap)| {
        let r = cap as f32 / min_cap;
        (pid, if r <= 1.01 { 1.0 } else { 1.0 + (r - 1.0).sqrt() })
    }).collect())
}

pub fn start_scheduler_thread(
    rx: mpsc::Receiver<DaemonEvent>,
    shared_config: Arc<RwLock<Config>>,
) -> Result<()> {
    let root = common::get_module_root();
    // 配置路径：非 Chiri 机型回退到默认 config/config.yaml；若意外命中处理器子目录则跟随之
    let config_path = common::get_config_path();
    let config_dir = root.join("config");

    // 初始模式透传 rules.yaml 的 global_mode：此前硬编码 "balance" 会导致开机到首个
    // ModeChange（约 2 秒）前按错误的模式接管 CPU（8550 上 balance perf_init=1.0 会锁满频），
    // 且与用户配置的 global_mode 不一致。global_mode 未配置或不是已注册模式时回退 balance。
    let initial_mode = {
        let rules = crate::utils::read_config::<crate::monitor::config::RulesConfig, _>(
            crate::monitor::config::get_rules_path(),
        )
        .unwrap_or_default();
        let m = rules.global_mode.clone();
        if m.is_empty() || shared_config.read().unwrap().get_mode(&m).is_none() {
            "balance".to_string()
        } else {
            m
        }
    };

    let shared_mode_name = Arc::new(Mutex::new(initial_mode));
    let sys_path_exist = Arc::new(utils::SysPathExist::new());

    // ==========================================
    // Config Watcher 线程
    // ==========================================
    let config_clone = shared_config.clone();
    let sys_path_clone = sys_path_exist.clone();
    
    thread::Builder::new()
        .name("config_watcher".to_string())
        .spawn(move || {
            loop {
                if let Err(e) = utils::watch_path(&config_dir) {
                    log::error!("{}", t_with_args("config-watch-error", &fluent_args!("error" => e.to_string())));
                    // 退避后再重试，避免持续错误时忙循环刷 CPU
                    thread::sleep(std::time::Duration::from_secs(2));
                    continue;
                }
                log::info!("{}", t("config-reloading"));

                let old_lang = config_clone.read().unwrap().meta.language.clone();
                
                match Config::load(config_path.to_str().unwrap()) {
                    Ok(new_config) => {
                        logger::update_level(&new_config.meta.loglevel);
                        *config_clone.write().unwrap() = new_config;

                        let new_lang = config_clone.read().unwrap().meta.language.clone();
                        if old_lang != new_lang { load_language(&new_lang); }

                        log::info!("{}", t("config-reloaded-success"));

                        // 快照自愈：调优参数以嵌入内容为准，磁盘文件被篡改时还原
                        // （meta 保留外部修改）。内容一致时内部跳过写入，不会成环。
                        crate::common::sync_config_snapshot(&config_path);

                        let scheduler = CpuScheduler::new(config_clone.clone(), sys_path_clone.clone());
                        if let Err(e) = scheduler.apply_system_tweaks() {
                            log::error!("{}", t_with_args("config-apply-tweaks-failed", &fluent_args!("error" => e.to_string())));
                        }
                    }
                    Err(load_err) => log::error!("{}", t_with_args("config-reload-fail", &fluent_args!("error" => load_err.to_string()))),
                }
            }
        })?;
    
    log::info!("{}", t("main-config-watch-thread-create"));

    // ==========================================
    // IPC 监听主线程 (负责所有的状态机流转与调度干预)
    // ==========================================
    let config_clone = shared_config.clone();
    let mode_clone = shared_mode_name.clone();

    thread::Builder::new()
        .name("scheduler_ipc".to_string())
        .spawn(move || {
            log::info!("{}", t("scheduler-ipc-started"));
            
            let root = common::get_module_root();
            // 当前模式持久化文件：每次模式切换时写入，供外部（如 WebUI）读取当前状态。
            // 自愈：常态下每 5 秒重写一次（见循环内 MODE_FILE_REWRITE_INTERVAL 分支），
            // 防止文件被意外清空/删除后 WebUI 读不到当前状态（清空原因多非人为，但重写兜底人为误删）。
            let mode_file_path = root.join("current_mode.txt");
            const MODE_FILE_REWRITE_INTERVAL: Duration = Duration::from_secs(5);
            let mut last_mode_file_write = Instant::now();
            // 启动时先写一次初始模式，避免开机后文件缺失/被清空时 WebUI 显示未知状态
            {
                let mode = mode_clone.lock().unwrap().clone();
                let _ = utils::try_write_file(&mode_file_path, mode.as_bytes());
            }
            
            // let mut fas_controller = crate::scheduler::fas::FasController::new(); // FAS 暂禁用
            let mut cpu_governor = crate::scheduler::cpu_load_governor::CpuLoadGovernor::new();

            // ==== FAS 暂禁用：以下变量仅服务于 FAS 调度，暂注释 ====
            // let rules_path = crate::monitor::config::get_rules_path();
            // let mut current_rules = crate::utils::read_config::<crate::monitor::config::RulesConfig, _>(&rules_path).unwrap_or_default();

            // 状态机变量
            // let mut fas_suspended_at: Option<Instant> = None;    // FAS 暂禁用
            // let mut fas_suspended_package = String::new();       // FAS 暂禁用
            // const FAS_SUSPEND_GRACE_SECS: u64 = 5;               // FAS 暂禁用
            
            let mut is_screen_on = true; // 屏幕状态标记

            // ==== FAS 暂禁用：CPU 温度采样仅用于 FAS 限温，暂注释 ====
            // let temp_sensor_path = crate::utils::find_cpu_temp_path().unwrap_or_default();
            // let mut last_temp_update = Instant::now();

            let get_clg_cfg = |config: &Config, mode: &str| -> crate::scheduler::config::CpuLoadGovernorConfig {
                config.get_mode(mode).map(|m| m.cpu_load_governor.clone()).unwrap_or_else(|| {
                    // 未知/空模式名：不意外启用 CLG，避免用默认参数接管 CPU
                    let mut cfg = crate::scheduler::config::CpuLoadGovernorConfig::default();
                    cfg.enabled = false;
                    cfg
                })
            };

            // 启动时初始化
            {
                let current_mode = mode_clone.lock().unwrap().clone();
                if current_mode != "fas" {
                    let config_lock = config_clone.read().unwrap();
                    let clg_cfg = get_clg_cfg(&config_lock, &current_mode);
                    if clg_cfg.enabled {
                        cpu_governor.init_policies(&clg_cfg);
                        log::info!("{}", t_with_args("scheduler-clg-init", &fluent_args!("mode" => current_mode.clone())));
                    }
                }
            }
            
            // 事件循环包在 catch_unwind 中：任何 panic 都被捕获并记录，
            // 避免调度线程静默死亡（进程存活但频率停在最后状态）
            // 最近一次 SystemLoadUpdate 到达时间：供 CLG 看门狗判定负载源是否失效
            let mut last_load_event = Instant::now();
            let loop_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            loop {
                // 当前模式文件自愈：每 5 秒重写一次（即便内容未变也重写），
                // 保证被外部清空/删除后 WebUI 最多 5 秒恢复读取当前状态
                if last_mode_file_write.elapsed() >= MODE_FILE_REWRITE_INTERVAL {
                    let mode = mode_clone.lock().unwrap().clone();
                    let _ = utils::try_write_file(&mode_file_path, mode.as_bytes());
                    last_mode_file_write = Instant::now();
                    // watchdog.pid 自愈（与 chiri 同口径）：logs/ 被外部删除后
                    // WebUI stopScheduler 将无法终止看门狗，旧看门狗残留会把
                    // daemon 再拉起，与重启实例形成双实例并行写两份日志
                    crate::logger::ensure_watchdog_pid_file();
                }

                let msg = match rx.recv_timeout(CLG_STALE_POLL) {
                    Ok(msg) => msg,
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        // eBPF 负载源失效自愈：超过 CLG_STALE_MAX 无负载事件 → 释放 CLG，
                        // 回滚系统原生调频，防止 CPU 永久锁频（下次 ModeChange/配置事件会重新接管）
                        if cpu_governor.is_active() && last_load_event.elapsed() >= CLG_STALE_MAX {
                            log::error!(
                                "{}",
                                t_with_args(
                                    "clg-watchdog-release",
                                    &fluent_args!("secs" => last_load_event.elapsed().as_secs().to_string())
                                )
                            );
                            cpu_governor.release();
                        }
                        continue;
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                };
                match msg {
                    // --- 1. 屏幕状态事件 (息屏深度睡眠) ---
                    DaemonEvent::ScreenStateChange(screen_on) => {
                        // 双源事件去重：uevent 线程直推 + app_detect verify 自愈兜底
                        // 都可能上报同一次屏幕切换（screen_watcher 为共享层改动），
                        // 状态未变化时只打点不处理，维持原有单事件行为
                        if screen_on == is_screen_on {
                            log::debug!("{}", t_with_args("scheduler-event-screen", &fluent_args!(
                                "on" => screen_on.to_string(),
                                "last" => is_screen_on.to_string()
                            )));
                            continue;
                        }
                        log::debug!("{}", t_with_args("scheduler-event-screen", &fluent_args!(
                            "on" => screen_on.to_string(),
                            "last" => is_screen_on.to_string()
                        )));
                        is_screen_on = screen_on;
                        let current_mode = mode_clone.lock().unwrap().clone();

                        if !is_screen_on {
                            log::info!("{}", t("scheduler-doze-enable"));
                            
                            // ==== FAS 暂禁用：息屏不再剥夺 FAS 频率控制权 ====
                            // if current_mode == "fas" {
                            //     fas_controller.reset_all_freqs();
                            //     fas_controller.clear_game();
                            //     fas_controller.policies.clear();
                            //     fas_suspended_at = None;
                            //     fas_suspended_package.clear();
                            // }

                            // 强行让 CLG 接管，并动态生成一个极致省电配置
                            let config_lock = config_clone.read().unwrap();
                            let mut doze_cfg = get_clg_cfg(&config_lock, "powersave"); 
                            doze_cfg.enabled = true;
                            doze_cfg.perf_floor = 0.0;
                            doze_cfg.perf_ceil = doze_cfg.perf_ceil.min(0.40); // 锁死天花板最高只给 40% 性能
                            doze_cfg.smoothing_up = 0.10;           // 升频极其迟钝
                            doze_cfg.smoothing_down = 1.0;          // 瞬间降频
                            
                            cpu_governor.init_policies(&doze_cfg);
                        } else {
                            log::info!("{}", t("scheduler-doze-restore"));
                            
                            let config_lock = config_clone.read().unwrap();
                            let clg_cfg = get_clg_cfg(&config_lock, &current_mode);
                            
                            if current_mode != "fas" {
                                if clg_cfg.enabled {
                                    // 息屏 doze 期间 CLG 仍持有 writer，热切换配置即可
                                    if cpu_governor.is_active() { cpu_governor.reload_config(&clg_cfg); } 
                                    else { cpu_governor.init_policies(&clg_cfg); }
                                } 
                                else { cpu_governor.release(); }
                            } else {
                                // ==== FAS 暂禁用：原恢复 fas 时释放 CLG 并清空模式，现保持 CLG 接管 ====
                                // cpu_governor.release(); 
                                // *mode_clone.lock().unwrap() = String::new();
                            }
                        }
                    },

                    // --- 2. 前台模式切换事件 ---
                    DaemonEvent::ModeChange { package_name, pid: _, mode, temperature } => {
                        let mut current_mode_lock = mode_clone.lock().unwrap();
                        let old_mode = current_mode_lock.clone();
                        log::debug!("{}", t_with_args("scheduler-event-mode-change", &fluent_args!(
                            "pkg" => package_name.as_str(),
                            "old" => old_mode.clone(),
                            "new" => mode.as_str(),
                            "temp" => temperature
                        )));
                        
                        if old_mode != mode {
                            log::info!("{}", t_with_args("scheduler-mode-change-request", &fluent_args!(
                                "old" => old_mode.clone(), "new" => mode.as_str(), "pkg" => package_name.as_str(), "temp" => temperature
                            )));
                            
                            *current_mode_lock = mode.clone();
                            drop(current_mode_lock); 

                            let _ = utils::try_write_file(&mode_file_path, mode.as_bytes());

                            // ==== FAS 暂禁用：进游戏不再释放 CLG 控制权、不再激活 FAS ====
                            if mode == "fas" {
                                // cpu_governor.release();
                                // let can_resume = fas_suspended_at.map_or(false, |at| {
                                //     at.elapsed().as_secs() < FAS_SUSPEND_GRACE_SECS && fas_suspended_package == package_name && !fas_controller.policies.is_empty()
                                // });
                                // if can_resume {
                                //     fas_suspended_at = None;
                                //     fas_suspended_package.clear();
                                //     for policy in &mut fas_controller.policies { policy.force_reapply(); }
                                // } else {
                                //     fas_suspended_at = None;
                                //     fas_suspended_package.clear();
                                //     fas_controller.load_policies(&current_rules.fas_rules);
                                // }
                                // fas_controller.set_game(pid, &package_name);
                                // fas_controller.set_temperature(temperature);
                                // fas_controller.set_temp_threshold(current_rules.fas_rules.core_temp_threshold);
                            } else {
                                // ==== FAS 暂禁用：退游戏不再挂起/清理 FAS，直接交由 CLG 接管 ====
                                // if fas_suspended_at.is_some() {
                                //     fas_controller.reset_all_freqs();
                                //     fas_controller.clear_game();
                                //     fas_controller.policies.clear();
                                //     fas_suspended_at = None;
                                //     fas_suspended_package.clear();
                                // }
                                // if old_mode == "fas" && !fas_controller.policies.is_empty() {
                                //     fas_suspended_at = Some(Instant::now());
                                //     fas_suspended_package = package_name.clone();
                                // } else if old_mode == "fas" {
                                //     fas_controller.clear_game();
                                //     fas_controller.policies.clear();
                                //     fas_suspended_at = None;
                                //     fas_suspended_package.clear();
                                // }

                                // 仅在亮屏时处理 CLG。如果息屏，Doze 配置仍在生效，这里不能覆盖它
                                if is_screen_on {
                                    let config_lock = config_clone.read().unwrap();
                                    let clg_cfg = get_clg_cfg(&config_lock, &mode);
                                    if clg_cfg.enabled {
                                        // CLG 已激活时热切换配置，避免同模式反复切换全量重建
                                        if cpu_governor.is_active() { cpu_governor.reload_config(&clg_cfg); }
                                        else { cpu_governor.init_policies(&clg_cfg); }
                                    } else {
                                        cpu_governor.release();
                                    }
                                }
                            }
                        } else if mode == "fas" {
                            // FAS 暂禁用：原用于刷新 FAS 温度
                            // fas_controller.set_temperature(temperature);
                        }
                    },

                    // --- 3. CPU 负载事件 (eBPF 驱动) ---
                    DaemonEvent::SystemLoadUpdate { core_utils, foreground_max_util: _ } => {
                        // 刷新看门狗心跳：只要有负载事件到达即视为负载源存活
                        last_load_event = Instant::now();
                        // 该事件常规 120ms / 特调 40ms 一次，仅在 DEBUG 时输出摘要便于排查
                        log::debug!("{}", t_with_args("scheduler-event-load", &fluent_args!(
                            "cores" => core_utils.iter().map(|u| format!("{:.0}", u * 100.0)).collect::<Vec<_>>().join(",")
                        )));
                        // let current_mode = mode_clone.lock().unwrap().clone(); // FAS 暂禁用
                        // ==== FAS 暂禁用：不再向 FAS 投喂 CPU 负载 ====
                        // if is_screen_on && current_mode == "fas" && fas_suspended_at.is_none() {
                        //     fas_controller.update_cpu_util(foreground_max_util);
                        //     fas_controller.update_core_utils(&core_utils);
                        // }
                        // 如果 CLG 处于活动状态（包含日常模式或息屏 Doze 模式），全权投喂
                        if cpu_governor.is_active() {
                            cpu_governor.on_load_update(&core_utils);
                        }
                    },

                    // --- 4. 帧率事件 (eBPF 驱动) ---
                    DaemonEvent::FrameUpdate { frame_delta_ns } => {
                        // ==== FAS 暂禁用：帧率事件不再参与调频 ====
                        // if !is_screen_on { continue; } // 息屏不处理渲染帧
                        // let current_mode = mode_clone.lock().unwrap().clone();
                        // if current_mode == "fas" {
                        //     if !temp_sensor_path.is_empty() && last_temp_update.elapsed().as_secs() >= 3 {
                        //         if let Ok(raw_temp) = crate::utils::read_f64_from_file(&temp_sensor_path) { 
                        //             fas_controller.set_temperature(raw_temp / 1000.0); 
                        //         }
                        //         last_temp_update = Instant::now();
                        //     }
                        //     fas_controller.update_frame(frame_delta_ns);
                        // }
                        // 帧事件在 FAS 禁用期间不参与调频，仅在 DEBUG 时周期性输出
                        log::debug!("{}", t_with_args("scheduler-event-frame", &fluent_args!(
                            "delta_ms" => format!("{:.2}", frame_delta_ns as f64 / 1_000_000.0)
                        )));
                    }

                    // --- 5. 热重载配置事件 ---
                    DaemonEvent::ConfigReload(new_rules) => {
                        let _ = new_rules; // FAS 暂禁用：原用于重载 current_rules.fas_rules
                        // current_rules = new_rules;
                        let current_mode = mode_clone.lock().unwrap().clone();
                        log::debug!("{}", t_with_args("scheduler-event-config-reload", &fluent_args!(
                            "mode" => current_mode.clone(),
                            "screen_on" => is_screen_on.to_string()
                        )));
                        
                        // ==== FAS 暂禁用：FAS 模式下不再重载 fas_rules ====
                        // if current_mode == "fas" {
                        //     if fas_controller.policies.is_empty() {
                        //         fas_controller.load_policies(&current_rules.fas_rules);
                        //     } else {
                        //         fas_controller.reload_rules(&current_rules.fas_rules);
                        //     }
                        // } else 
                        if is_screen_on { // 息屏时不要用新配置覆盖 Doze
                            let config_lock = config_clone.read().unwrap();
                            let clg_cfg = get_clg_cfg(&config_lock, &current_mode);
                            if clg_cfg.enabled {
                                if cpu_governor.is_active() { cpu_governor.reload_config(&clg_cfg); } 
                                else { cpu_governor.init_policies(&clg_cfg); }
                            } else if cpu_governor.is_active() {
                                cpu_governor.release();
                            }
                        }
                    }

                    // --- 6. eBPF 扩展探针统计 ---
                    // 仅 ChiRi SoC 的 cpu_monitor 会发送（Yumi 设备不加载扩展探针）；
                    // 此 arm 仅为枚举完备性，保证 Yumi 调度行为零变化。
                    DaemonEvent::BpfStats { .. } => {}

                    DaemonEvent::PackageSwitch { package_name, pid } => {
                        // 同模式前台包切换：仅 ChiRi FAS 消费（fas→fas 热切换），Yumi 不需要
                        let _ = (package_name, pid);
                    }
                }

                // ==== FAS 暂禁用：定期检查 FAS 挂起状态是否超时 ====
                // if let Some(suspended_at) = fas_suspended_at {
                //     if suspended_at.elapsed().as_secs() >= FAS_SUSPEND_GRACE_SECS {
                //         fas_controller.reset_all_freqs();
                //         fas_controller.clear_game();
                //         fas_controller.policies.clear();
                //         fas_suspended_at = None;
                //         fas_suspended_package.clear();
                //     }
                // }
            }
            }));
            if loop_result.is_err() {
                log::error!("{}", t("scheduler-ipc-panic"));
            }
            log::warn!("{}", t("scheduler-channel-closed"));
            // 收尾：无论 channel 关闭还是 panic，都恢复 CPU 控制状态，避免频率/governor 残留
            cpu_governor.release();
            // ==== FAS 暂禁用 ====
            // fas_controller.reset_all_freqs();
            // fas_controller.clear_game();
        })?;

    Ok(())
}