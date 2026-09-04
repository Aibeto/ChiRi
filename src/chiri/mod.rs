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

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock, mpsc};
use std::thread;
use std::time::{Duration, Instant};
use std::fs;
use anyhow::Result;

// CLG 看门狗：SystemLoadUpdate 常规 160ms / 特调 40ms 投喂一次，超过 CLG_STALE_MAX 没收到
// 事件就认为负载源失效（eBPF 加载失败/探针崩溃/通道断开），直接 release() 回系统调频，
// 防止 CPU 锁死在上次写入的频率（如 8550 balance perf_init=1.0 会锁满全核高频）。
const CLG_STALE_MAX: Duration = Duration::from_secs(5);
/// 事件轮询间隔：主事件通道无事件时的最大阻塞时长（动态超时上限）。
/// 实际阻塞到「最近一个周期任务的 deadline」为止——空闲时不再固定 100ms 空转
/// （每秒 10 次跑循环体），而只在最近任务到期/事件到达时醒来。所有性能敏感
/// 路径（负载事件/模式切换/触摸）都是推送事件，到达即打断阻塞，零延迟损失；
/// 该值仅作为 deadline 计算的兜底上限。
const EVENT_POLL_MS: Duration = Duration::from_millis(1000);
/// 热保护温度采样间隔：2s 一次，温度变化缓慢，更密的采样只浪费 IO。
const THERMAL_CHECK_INTERVAL: Duration = Duration::from_secs(2);
/// 遥测 CSV 落盘间隔：1s 一次（功耗统计精度 1s；telemetry 线程 1s 刷新共享原子量）。
const TELEMETRY_LOG_INTERVAL: Duration = Duration::from_secs(1);
/// scenemode 饱和退出：little 簇 max_util 持续高于该值视为顶满性能上限
/// （util 是忙时占比，与频率无关——后台负载压不住小核时即饱和）
const SCENEMODE_SAT_UTIL: f32 = 0.70;
/// 饱和持续判定时长：连续满足才退出，防止瞬时突发误触发
const SCENEMODE_SAT_SECS: Duration = Duration::from_secs(10);
/// scenemode 冷却：饱和退出后 300s 内不得重新进入（防止与后台负载反复拉锯）
const SCENEMODE_COOLDOWN: Duration = Duration::from_secs(300);
/// 电池温度节点（Android 标准电源供给接口，毫摄氏度）。
/// 电池温度为主参考：反映整机持续发热，变化缓慢、不随游戏瞬时负载抖动
const BATT_TEMP_PATH: &str = "/sys/class/power_supply/battery/temp";

/// 读电池温度（毫摄氏度）→ °C；节点缺失或读失败返回 None
fn read_battery_temp() -> Option<f32> {
    std::fs::read_to_string(BATT_TEMP_PATH)
        .ok()
        .and_then(|s| s.trim().parse::<f64>().ok())
        .map(|v| (v / 1000.0) as f32)
}

/// 单传感器三级判定（带回滞）：>= 硬限压 hard_cap；>= 软限压 soft_cap；
/// 回落到 软限-hyst 以下解除；回滞带内无压制状态保持、压制中先退到软限档防阶跃。
fn eval_thermal_cap(
    temp_c: f32,
    current: f32,
    soft: f32,
    hard: f32,
    soft_cap: f32,
    hard_cap: f32,
    hyst: f32,
) -> f32 {
    if temp_c >= hard {
        hard_cap
    } else if temp_c >= soft {
        soft_cap
    } else if temp_c < soft - hyst {
        1.0
    } else if current >= 1.0 {
        1.0
    } else {
        // 回滞带内且压制中：硬限压制先退到软限档，避免阶跃解除
        current.min(soft_cap)
    }
}

pub mod config;
pub mod scheduler;
// FAS（帧感知调度）暂时禁用：功能存在 bug，需关闭调试。
// 恢复时：取消下行注释，并恢复下方所有 `FAS`/`fas_controller` 相关调用。
// pub mod fas;
pub mod affinity;
pub mod akmode;
pub mod core_ctl;
pub mod cpu_load_governor;
pub mod touch_detect;
pub mod fast;

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
    /// policy 编号，对应 /sys/devices/system/cpu/cpufreq/policy<id>
    pub id: i32,
    /// boost 频率列表（单位 kHz），有的簇没有此文件则为空
    pub boost_frequencies: Vec<u32>,
}

/// 枚举系统中实际可用的 cpufreq policy，并读取各 policy 的 boost 频率。
/// 结果按 policy id 升序返回（供 CLG 遍历初始化）。
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

/// 读取指定 policy 的 scaling_boost_frequencies（kHz）。
/// 文件不存在、为空或解析失败时返回空 Vec，不影响 policy 注册。
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

/// 按当前模式判定是否为 boost 类模式（亲和收窄/core_ctl 保大核的判定口径）：
/// performance / fast / 特调（akmode）为 boost，powersave/balance 及未知模式为 normal。
fn is_boost_mode(mode: &str) -> bool {
    mode == "performance" || mode == "fast" || crate::common::is_special_mode(mode)
}

/// 应用 CPU 亲和布局与 core_ctl 在线策略（ChiRi 专属，跟随模式/屏幕/前台 PID）。
/// 内部带去重：布局与 PID 未变化时无 sysfs 写入，可安全周期性调用。
/// `core_utils` 为最近一次 SystemLoadUpdate 的逐核 util（按核选核打分输入）。
/// `scenemode_offline` 为 scenemode 激活标志：抑制 boost（防厂商 core_ctl 把
/// 下线的核拉回来）并触发 core_ctl 离线（只留 CPU0，深度省电）。
fn apply_affinity_and_corectl(
    affinity: &mut affinity::AffinityManager,
    corectl: &mut core_ctl::CoreCtlManager,
    config: &Config,
    mode: &str,
    screen_on: bool,
    fg_pid: i32,
    core_utils: &[f32],
    scenemode_offline: bool,
) {
    // scenemode 下抑制 boost：min_cpus 抬着会让厂商 core_ctl 重新拉起被下线的核
    let boost = is_boost_mode(mode) && !scenemode_offline;
    affinity.apply(screen_on, fg_pid, &config.affinity, boost, core_utils);
    let offline_on = scenemode_offline && config.core_ctl.scenemode_offline;
    corectl.set_power_state(config.core_ctl.enabled && boost, offline_on);
}

/// 启动 Chiri 调度线程组（由 main.rs 调用）：
/// - `config_watcher` 线程：监听 config 目录，热重载 Config 并重放一次性系统调整
/// - `scheduler_ipc` 线程：消费 `DaemonEvent` 状态机，驱动 CLG 接管/释放/配置切换
///
/// 参数 `rx` 为 Monitor 层与调度层间的有界事件通道，`shared_config` 为全局共享配置，
/// `ak_active` 为特调激活共享标志（Monitor 层据此切换采样间隔）。
pub fn start_scheduler_thread(
    rx: mpsc::Receiver<DaemonEvent>,
    shared_config: Arc<RwLock<Config>>,
    ak_active: Arc<AtomicBool>,
) -> Result<()> {
    let root = common::get_module_root();
    // 配置路径：8550 等 Chiri 目标 SoC 使用处理器子目录 config/{soc}/config.yaml，热重载跟随该文件
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

    // 当前生效模式名（跨线程共享，仅在 scheduler_ipc 线程内写）
    let shared_mode_name = Arc::new(Mutex::new(initial_mode));
    // sysfs 路径存在性缓存，避免每次 IO 调整前重复探测
    let sys_path_exist = Arc::new(utils::SysPathExist::new());
    // 触摸事件通道（事件驱动）：触摸检测线程发送触摸事件，scheduler_ipc 即时处理并触发大核升频
    let (touch_tx, touch_rx) = mpsc::sync_channel::<()>(8);
    // config.yaml 热重载联动标志：config_watcher 成功重载后置位，scheduler_ipc 轮询消费。
    // 修复此前「config.yaml 调参要等下次 ModeChange/规则重载才应用到运行中的 CLG/akmode」的
    // 热更新断链——现在调参保存后 100ms 内即按当前模式重载调度器配置。
    let config_dirty = Arc::new(AtomicBool::new(false));

    // 启动时立即应用一次性系统调整（cpuidle / IO / 屏蔽系统自带触摸升频），
    // 避免首次配置变更前这些调整处于未生效状态（config_watcher 仅在配置变化后重放）
    if let Err(e) = CpuScheduler::new(shared_config.clone(), sys_path_exist.clone())
        .apply_system_tweaks()
    {
        log::error!(
            "{}",
            t_with_args(
                "config-apply-tweaks-failed",
                &fluent_args!("error" => e.to_string())
            )
        );
    }

    // 触摸检测线程（Chiri 专属）：读取 /dev/input 触摸事件，经事件通道驱动大核触摸升频
    thread::Builder::new()
        .name("touch_detect".to_string())
        .spawn(move || {
            crate::chiri::touch_detect::monitor_touch(touch_tx);
        })?;

    // ==========================================
    // Config Watcher 线程
    // ==========================================
    let config_clone = shared_config.clone();
    let sys_path_clone = sys_path_exist.clone();
    let dirty_clone = config_dirty.clone();

    thread::Builder::new()
        .name("config_watcher".to_string())
        .spawn(move || {
            // 监听生效配置的父目录而非固定的 config/ 根目录：ChiRi 机型的生效配置在
            // 处理器子目录（如 config/8550/config.yaml），inotify 目录监听不递归，
            // 监听根目录收不到子目录内文件的 CLOSE_WRITE/MOVED_TO——导致 8550/8475/8998
            // 上 WebUI 改 meta.loglevel/language 的热重载完全失效。
            let watch_dir = config_path
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or(config_dir);
            loop {
                if let Err(e) = utils::watch_path(&watch_dir) {
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
                        common::sync_config_snapshot(&config_path);

                        let scheduler = CpuScheduler::new(config_clone.clone(), sys_path_clone.clone());
                        if let Err(e) = scheduler.apply_system_tweaks() {
                            log::error!("{}", t_with_args("config-apply-tweaks-failed", &fluent_args!("error" => e.to_string())));
                        }

                        // 通知 scheduler_ipc：运行中的 CLG/akmode/亲和/core_ctl 需按新配置重载
                        dirty_clone.store(true, Ordering::Release);
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
    let dirty_ipc = config_dirty.clone();

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
            let mut cpu_governor = crate::chiri::cpu_load_governor::CpuLoadGovernor::new();
            // 明日方舟特调（akmode）：独立于 CLG 的 4 档齿轮调度器，前台为白名单应用时接管。
            // 传入特调激活共享标志，接管/释放时联动 Monitor 层切换采样间隔
            let ak_governor_flag = ak_active.clone();
            let mut ak_governor = crate::chiri::akmode::AkmodeGovernor::new(ak_governor_flag);

            // 极速模式（fast）专属锁频器：与 CLG 完全独立，不读 yaml 调频参数，
            // 直接锁所有 cluster 的 min=max=硬件最高频，每 5 秒重写防止外部篡改。
            let mut fast_lock = crate::chiri::fast::FastLock::new();

            // CPU 亲和与线程迁移控制器 + core_ctl 核心在线接管（ChiRi 专属）
            let mut affinity_mgr = affinity::AffinityManager::new(sys_path_exist.clone());
            let mut corectl_mgr = core_ctl::CoreCtlManager::new();

            // 最近一次 eBPF 扩展探针统计（BpfStats 事件 2s 一次增量），随遥测 CSV 落盘
            let mut last_bpf_stats: (u32, u32, u32) = (0, 0, 0);
            // 遥测 CSV 落盘计时（1s 精度）与 debug 摘要计数（每 20 行 = 20s 一条）
            let mut last_telemetry_log = Instant::now();
            let mut telemetry_log_counter: u32 = 0;

            // ==== FAS 暂禁用：以下变量仅服务于 FAS 调度，暂注释 ====
            // let rules_path = crate::monitor::config::get_rules_path();
            // let mut current_rules = crate::utils::read_config::<crate::monitor::config::RulesConfig, _>(&rules_path).unwrap_or_default();

            // 状态机变量
            // let mut fas_suspended_at: Option<Instant> = None;    // FAS 暂禁用
            // let mut fas_suspended_package = String::new();       // FAS 暂禁用
            // const FAS_SUSPEND_GRACE_SECS: u64 = 5;               // FAS 暂禁用
            
            let mut is_screen_on = true; // 屏幕状态标记
            // 息屏计时：屏幕熄灭时记录，超过 scene_mode_delay_secs 后切到 scenemode 低功耗
            let mut screen_off_at: Option<Instant> = None;
            // 是否已进入 scenemode（一次性切换，亮屏/模式变更时复位）
            let mut scene_mode_active = false;
            // 特调模式冷却：init_policies 因配置缺失/硬件不支持失败后，5 分钟内不再触发
            const AKMODE_COOLDOWN: Duration = Duration::from_secs(300);
            let mut akmode_cooldown_until: Option<Instant> = None;

            // scenemode 饱和退出冷却：little 簇 util 持续顶满上限退回 powersave 后，
            // 300s 内不得重新进入 scenemode（防止与后台负载反复拉锯）
            let mut scenemode_cooldown_until: Option<Instant> = None;
            // scenemode 饱和计时起点（little 簇 max_util 连续超阈值的窗口起点）
            let mut scenemode_sat_since: Option<Instant> = None;

            // 热保护：启动时探测一次温度传感器（CPU + 电池），缺失的参考静默降级；
            // 事件循环内每 2s 采样，按 config.thermal 阈值计算压制上限下发给 CLG
            let temp_sensor_path = crate::utils::find_cpu_temp_path().ok();
            if temp_sensor_path.is_none() {
                log::debug!("{}", t("clg-thermal-no-sensor"));
            }
            let batt_sensor_exists = std::path::Path::new(BATT_TEMP_PATH).exists();
            if !batt_sensor_exists {
                log::debug!("{}", t("clg-thermal-no-battery"));
            }
            let mut last_thermal_check = Instant::now();
            // 当前生效的热保护上限（1.0 = 无压制）；变化时才写 governor，避免高频原子写
            let mut thermal_cap_current: f32 = 1.0;
            // 当前生效的压制豁免档（与 governor 内原子量同步，配置热重载时下发新值）
            let mut thermal_free_current: f32 = config_clone.read().unwrap().thermal.free_above;
            // 启动即把配置豁免档同步给 governor（内部默认 0.80，配置可能不同）
            cpu_governor.set_thermal_limits(thermal_cap_current, thermal_free_current);

            // ==== FAS 暂禁用：CPU 温度采样仅用于 FAS 限温，暂注释 ====
            // let temp_sensor_path = crate::utils::find_cpu_temp_path().unwrap_or_default();
            // let mut last_temp_update = Instant::now();

            let get_clg_cfg = |config: &Config, mode: &str| -> crate::chiri::config::CpuLoadGovernorConfig {
                config.get_mode(mode).map(|m| m.cpu_load_governor.clone()).unwrap_or_else(|| {
                    // 未知/空模式名：不意外启用 CLG，避免用默认参数接管 CPU
                    let mut cfg = crate::chiri::config::CpuLoadGovernorConfig::default();
                    cfg.enabled = false;
                    cfg
                })
            };

            // 特调起始档从 rules.yaml 识别：明日方舟 app_modes > global_mode，
            // 换算成四档（powersave=1..fast=4），配了普通模式就按对应档起步
            let get_ak_initial_tier = || -> u32 {
                let rules = crate::utils::read_config::<crate::monitor::config::RulesConfig, _>(
                    crate::monitor::config::get_rules_path(),
                )
                .unwrap_or_default();
                let mode = rules
                    .app_modes
                    .get("com.hypergryph.arknights")
                    .cloned()
                    .unwrap_or(rules.global_mode);
                crate::chiri::config::mode_to_tier(&mode)
            };

            // 启动时初始化
            {
                let current_mode = mode_clone.lock().unwrap().clone();
                if current_mode == "fast" {
                    fast_lock.init();
                } else if current_mode != "fas" {
                    let config_lock = config_clone.read().unwrap();
                    let clg_cfg = get_clg_cfg(&config_lock, &current_mode);
                    if clg_cfg.enabled {
                        cpu_governor.init_policies(&clg_cfg);
                        log::info!("{}", t_with_args("scheduler-clg-init", &fluent_args!("mode" => current_mode.clone())));
                    }
                }
                // 启动即按初始模式应用亲和布局与 core_ctl 在线策略
                {
                    let cfg = config_clone.read().unwrap();
                    // 开发记录开关初始同步（此后每 2s 周期块随配置刷新）
                    crate::logger::set_devimp_active(cfg.meta.dev_record);
                    crate::logger::set_devimp_mode(&current_mode);
                    apply_affinity_and_corectl(
                        &mut affinity_mgr,
                        &mut corectl_mgr,
                        &cfg,
                        &current_mode,
                        is_screen_on,
                        crate::monitor::app_detect::get_current_pid(),
                        &[],
                        scene_mode_active,
                    );
                }
            }

            // 事件循环包在 catch_unwind 中：panic 被捕获并记录，
            // 不会让调度线程静默挂掉（否则频率会停在最后状态）。
            // 最近一次 SystemLoadUpdate 到达时间：供 CLG 看门狗判定负载源是否失效
            let mut last_load_event = Instant::now();
            // 最近一次 SystemLoadUpdate 的逐核 util 快照（按核亲和选核输入）
            let mut last_core_utils: Vec<f32> = Vec::new();
            // 最近一次前台包名（devimp snap 行记录用，ModeChange 事件更新）
            let mut last_fg_package = String::new();
            let loop_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            loop {
                // 当前模式文件自愈：每 5 秒重写一次（即便内容未变也重写），
                // 保证被外部清空/删除后 WebUI 最多 5 秒恢复读取当前状态
                if last_mode_file_write.elapsed() >= MODE_FILE_REWRITE_INTERVAL {
                    let mode = mode_clone.lock().unwrap().clone();
                    let _ = utils::try_write_file(&mode_file_path, mode.as_bytes());
                    last_mode_file_write = Instant::now();
                }

                // config.yaml 热重载联动：config_watcher 成功重载后置位。
                // 与 ConfigReload（rules.yaml）同口径：亮屏时按当前模式把新配置应用到
                // 运行中的 CLG/akmode，并刷新亲和/core_ctl（息屏不覆盖 Doze，亮屏事件补上）。
                if dirty_ipc.swap(false, Ordering::AcqRel) && is_screen_on {
                    let current_mode = mode_clone.lock().unwrap().clone();
                    let config_lock = config_clone.read().unwrap();
                    log::debug!(
                        "{}",
                        t_with_args(
                            "scheduler-config-dirty-reload",
                            &fluent_args!("mode" => current_mode.clone())
                        )
                    );
                    if crate::common::is_special_mode(&current_mode) {
                        // 特调运行中：按新配置重载 akmode（档位仍由 rules.yaml 决定）
                        if ak_governor.is_active() {
                            let ak_cfg = config_lock.get_akmode().clone();
                            let initial_tier = get_ak_initial_tier();
                            ak_governor.reload_config(&ak_cfg, initial_tier);
                        }
                    } else if current_mode == "fast" {
                        // fast_lock 不读 yaml 调参，仅需确保 CLG 未意外持有
                        if cpu_governor.is_active() { cpu_governor.release(); }
                    } else {
                        let clg_cfg = get_clg_cfg(&config_lock, &current_mode);
                        if clg_cfg.enabled {
                            if cpu_governor.is_active() { cpu_governor.reload_config(&clg_cfg); }
                            else { cpu_governor.init_policies(&clg_cfg); }
                        } else if cpu_governor.is_active() {
                            cpu_governor.release();
                        }
                    }
                    drop(config_lock);
                    // 亲和布局/core_ctl 可能随新配置开关变化，刷新一次
                    let cfg = config_clone.read().unwrap();
                    crate::logger::set_devimp_active(cfg.meta.dev_record);
                    crate::logger::set_devimp_mode(&current_mode);
                    apply_affinity_and_corectl(
                        &mut affinity_mgr,
                        &mut corectl_mgr,
                        &cfg,
                        &current_mode,
                        is_screen_on,
                        crate::monitor::app_detect::get_current_pid(),
                        &last_core_utils,
                        scene_mode_active,
                    );
                }

                // 亲和周期刷新：boost 模式下前台 App 同模式切换不产生 ModeChange 事件，
                // 这里每 2s 用最新前台 PID 兜底重迁移（内部去重，PID 未变时无写入）；
                // 同时同步开发记录开关（meta.dev_record 热重载生效）
                if last_thermal_check.elapsed() >= THERMAL_CHECK_INTERVAL {
                    let cfg = config_clone.read().unwrap();
                    let current_mode = mode_clone.lock().unwrap().clone();
                    crate::logger::set_devimp_active(cfg.meta.dev_record);
                    apply_affinity_and_corectl(
                        &mut affinity_mgr,
                        &mut corectl_mgr,
                        &cfg,
                        &current_mode,
                        is_screen_on,
                        crate::monitor::app_detect::get_current_pid(),
                        &last_core_utils,
                        scene_mode_active,
                    );
                }

                // 状态日志 snapshot 行（1s 精度）：整合遥测 + 热保护 + 模式状态到
                // logs/status.log（替代原 telemetry.log + power.log 两条流）。
                // telemetry 线程 1s 刷新共享原子量；OPlus 机型功耗优先走 bcc_parms
                // 私有节点（标准 power_supply 节点约 10s 才刷新，1s 采样必须绕开）。
                if last_telemetry_log.elapsed() >= TELEMETRY_LOG_INTERVAL {
                    last_telemetry_log = Instant::now();
                    let tm = crate::monitor::telemetry::telemetry();
                    let fmt_opt = |v: Option<f32>, digits: usize| {
                        v.map(|x| format!("{:.*}", digits, x))
                            .unwrap_or_else(|| "-".to_string())
                    };
                    let current_mode = mode_clone.lock().unwrap().clone();
                    crate::logger::status_log_snapshot(
                        &current_mode,
                        is_screen_on,
                        read_battery_temp(),
                        temp_sensor_path
                            .as_ref()
                            .and_then(|p| crate::utils::read_f64_from_file(p).ok())
                            .map(|v| (v / 1000.0) as f32),
                        &format!("{:.0}", thermal_cap_current * 100.0),
                        &format!("{:.0}", thermal_free_current * 100.0),
                        cpu_governor.is_active(),
                        &fmt_opt(Some(tm.psi_cpu_some()), 2),
                        &fmt_opt(Some(tm.psi_io_some()), 2),
                        &fmt_opt(Some(tm.psi_mem_some()), 2),
                        &fmt_opt(tm.gpu_busy(), 0),
                        &fmt_opt(tm.batt_voltage_v(), 3),
                        &fmt_opt(tm.batt_current_ma(), 0),
                        &fmt_opt(tm.batt_power_w(), 2),
                        last_bpf_stats.0,
                        last_bpf_stats.1,
                        last_bpf_stats.2,
                    );
                    telemetry_log_counter += 1;
                    // 开发记录 snap 行（1s）：环境上下文（开启 dev_record 才有 IO）
                    if crate::logger::devimp_active() {
                        crate::logger::devimp_snap(
                            is_screen_on,
                            &last_fg_package,
                            &fmt_opt(read_battery_temp(), 1),
                            &fmt_opt(
                                temp_sensor_path
                                    .as_ref()
                                    .and_then(|p| crate::utils::read_f64_from_file(p).ok())
                                    .map(|v| (v / 1000.0) as f32),
                                1,
                            ),
                            &format!("{:.0}", thermal_cap_current * 100.0),
                            cpu_governor.is_active(),
                            &fmt_opt(Some(tm.psi_cpu_some()), 2),
                            &fmt_opt(Some(tm.psi_io_some()), 2),
                            &fmt_opt(Some(tm.psi_mem_some()), 2),
                            &fmt_opt(tm.gpu_busy(), 0),
                            &fmt_opt(tm.batt_voltage_v(), 3),
                            &fmt_opt(tm.batt_current_ma(), 0),
                            &fmt_opt(tm.batt_power_w(), 2),
                            last_bpf_stats.0,
                            last_bpf_stats.1,
                            last_bpf_stats.2,
                        );
                    }
                    if telemetry_log_counter % 20 == 0 && log::log_enabled!(log::Level::Debug) {
                        log::debug!(
                            "{}",
                            t_with_args(
                                "telemetry-summary",
                                &fluent_args!(
                                    "cpu" => format!("{:.1}", tm.psi_cpu_some()),
                                    "io" => format!("{:.1}", tm.psi_io_some()),
                                    "mem" => format!("{:.1}", tm.psi_mem_some()),
                                    "gpu" => fmt_opt(tm.gpu_busy(), 0),
                                    "wakeups" => last_bpf_stats.0.to_string(),
                                    "migrations" => last_bpf_stats.1.to_string(),
                                    "freq" => last_bpf_stats.2.to_string(),
                                    "power" => fmt_opt(tm.batt_power_w(), 2)
                                )
                            )
                        );
                    }
                }

                // 热保护（2s 周期）：电池+CPU 双源取较小值。电池温升慢、反映整机发热；
                // CPU 只在极端热时参与（阈值 75/85°C，内核 95°C 温控才是主力）。
                // 豁免档：当前性能比已高于豁免档时不压制，高负载不挡路。
                if last_thermal_check.elapsed() >= THERMAL_CHECK_INTERVAL {
                    last_thermal_check = Instant::now();
                    let (enabled, batt_soft, batt_hard, cpu_soft, cpu_hard, soft_cap, hard_cap, hyst, free_above) = {
                        let t = &config_clone.read().unwrap().thermal;
                        (
                            t.enabled,
                            t.batt_soft_temp_c,
                            t.batt_hard_temp_c,
                            t.cpu_soft_temp_c,
                            t.cpu_hard_temp_c,
                            t.soft_perf_cap,
                            t.hard_perf_cap,
                            t.hysteresis_c,
                            t.free_above,
                        )
                    };
                    let cur = thermal_cap_current;
                    let batt_t = if batt_sensor_exists { read_battery_temp() } else { None };
                    let cpu_t = temp_sensor_path
                        .as_ref()
                        .and_then(|p| crate::utils::read_f64_from_file(p).ok())
                        .map(|v| (v / 1000.0) as f32);
                    let new_cap = if !enabled {
                        1.0
                    } else {
                        match (batt_t, cpu_t) {
                            (Some(b), Some(c)) => eval_thermal_cap(
                                b, cur, batt_soft, batt_hard, soft_cap, hard_cap, hyst,
                            )
                            .min(eval_thermal_cap(
                                c, cur, cpu_soft, cpu_hard, soft_cap, hard_cap, hyst,
                            )),
                            (Some(b), None) => {
                                eval_thermal_cap(b, cur, batt_soft, batt_hard, soft_cap, hard_cap, hyst)
                            }
                            (None, Some(c)) => {
                                eval_thermal_cap(c, cur, cpu_soft, cpu_hard, soft_cap, hard_cap, hyst)
                            }
                            // 双传感器缺失/读失败：保持现状，下轮重试
                            (None, None) => cur,
                        }
                    };
                    let cap_changed = (new_cap - thermal_cap_current).abs() > f32::EPSILON;
                    let free_changed = (free_above - thermal_free_current).abs() > f32::EPSILON;
                    if cap_changed || free_changed {
                        thermal_cap_current = new_cap;
                        thermal_free_current = free_above;
                        cpu_governor.set_thermal_limits(new_cap, free_above);
                        let fmt = |v: Option<f32>| {
                            v.map(|t| format!("{:.1}", t))
                                .unwrap_or_else(|| "-".to_string())
                        };
                        log::debug!(
                            "{}",
                            t_with_args(
                                "clg-thermal-cap",
                                &fluent_args!(
                                    "batt" => fmt(batt_t),
                                    "cpu" => fmt(cpu_t),
                                    "cap" => format!("{:.0}", new_cap * 100.0),
                                    "free" => format!("{:.0}", free_above * 100.0)
                                )
                            )
                        );
                        crate::logger::devimp_event(
                            "thermal_change",
                            "-",
                            &format!(
                                "batt={} cpu={} cap={:.0} free={:.0}",
                                fmt(batt_t),
                                fmt(cpu_t),
                                new_cap * 100.0,
                                free_above * 100.0
                            ),
                        );
                    }
                    // 功耗监控已并入 status.log snapshot 行（1s），此处不再重复写
                }

                // 触摸事件（事件驱动）：每次醒来先处理触摸队列。on_touch 更新共享
                // 触摸状态并唤醒全部 Worker 立即 flush 写频，大核 Worker 在本次 flush
                // 中直接应用触摸升频地板，不等待下一个 160ms 负载决策 tick。
                // 窗口内的持续触摸会刷新截止时间（FastWriter 去重重复写频）。
                while touch_rx.try_recv().is_ok() {
                    if cpu_governor.is_active() {
                        cpu_governor.on_touch();
                        log::debug!("{}", t("touch-event-received"));
                    }
                }

                // 极速模式：每 5 秒重写一次硬件最高频，防止系统/厂商守护进程篡改；
                // 返回距下次重写的剩余时间，纳入动态超时计算
                let fast_next = fast_lock.tick();

                // 动态超时：阻塞到「最近一个周期任务的 deadline」或事件到达（先到者打断）。
                // 周期任务（telemetry 1s / thermal+亲和 2s / mode file 5s / fast 重写 5s）
                // 各自 deadline 取最小值；负载/模式/触摸等推送事件随时到达，recv_timeout
                // 立即返回，性能响应零延迟。空闲稳态下从每秒 10 次空转降为 ~1 次。
                let now_loop = Instant::now();
                let until = |last: Instant, period: Duration| -> Duration {
                    period.checked_sub(now_loop.duration_since(last)).unwrap_or(Duration::ZERO)
                };
                let mut wait = until(last_telemetry_log, TELEMETRY_LOG_INTERVAL)
                    .min(until(last_thermal_check, THERMAL_CHECK_INTERVAL))
                    .min(until(
                        last_mode_file_write,
                        MODE_FILE_REWRITE_INTERVAL,
                    ))
                    .min(EVENT_POLL_MS);
                if let Some(d) = fast_next {
                    wait = wait.min(d);
                }
                let msg = match rx.recv_timeout(wait) {
                    Ok(msg) => msg,
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        // eBPF 负载源超时自愈：超过 CLG_STALE_MAX 无负载事件 → 释放
                        // 当前接管（CLG 或 akmode），回系统原生调频，防止 CPU 锁频
                        // （下次 ModeChange/配置事件会重新接管）。
                        if last_load_event.elapsed() >= CLG_STALE_MAX {
                            if ak_governor.is_active() {
                                log::error!(
                                    "{}",
                                    t_with_args(
                                        "akmode-watchdog-release",
                                        &fluent_args!("secs" => last_load_event.elapsed().as_secs().to_string())
                                    )
                                );
                                ak_governor.release();
                            }
                            if cpu_governor.is_active() {
                                log::error!(
                                    "{}",
                                    t_with_args(
                                        "clg-watchdog-release",
                                        &fluent_args!("secs" => last_load_event.elapsed().as_secs().to_string())
                                    )
                                );
                                cpu_governor.release();
                            }
                            if fast_lock.is_active() {
                                log::error!(
                                    "{}",
                                    t_with_args(
                                        "fast-watchdog-release",
                                        &fluent_args!("secs" => last_load_event.elapsed().as_secs().to_string())
                                    )
                                );
                                fast_lock.release();
                            }
                        }
                        continue;
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                };
                match msg {
                    // --- 1. 屏幕状态事件 (息屏深度睡眠) ---
                    DaemonEvent::ScreenStateChange(screen_on) => {
                        log::debug!("{}", t_with_args("scheduler-event-screen", &fluent_args!(
                            "on" => screen_on.to_string(),
                            "last" => is_screen_on.to_string()
                        )));
                        is_screen_on = screen_on;
                        let current_mode = mode_clone.lock().unwrap().clone();

                        if !is_screen_on {
                            log::info!("{}", t("scheduler-doze-enable"));
                            // 息屏计时起点：超过 scene_mode_delay_secs 后切到 scenemode 低功耗
                            screen_off_at = Some(Instant::now());
                            scene_mode_active = false;

                            // ==== FAS 暂禁用：息屏不再剥夺 FAS 频率控制权 ====
                            // if current_mode == "fas" {
                            //     fas_controller.reset_all_freqs();
                            //     fas_controller.clear_game();
                            //     fas_controller.policies.clear();
                            //     fas_suspended_at = None;
                            //     fas_suspended_package.clear();
                            // }

                            // 特调模式下息屏保持 akmode 接管，不切换到 CLG doze：
                            // akmode 已统一为 schedutil，息屏后 schedutil 随负载自然降频省电，
                            // 无需 CLG 介入；避免 release + 亮屏 re-init 的 governor 反复切换。
                            if crate::common::is_special_mode(&current_mode) {
                                // akmode 继续运行，CLG 保持释放状态
                                log::info!("{}", t("scheduler-doze-special-keep"));
                            } else {
                                // 非特调模式：交回 CLG 处理深度睡眠
                                ak_governor.release();
                                fast_lock.release();

                                // 让 CLG 接管，动态生成一个低功耗配置
                                let config_lock = config_clone.read().unwrap();
                                let mut doze_cfg = get_clg_cfg(&config_lock, "powersave");
                                doze_cfg.enabled = true;
                                doze_cfg.perf_floor = 0.0;
                                // 息屏 doze 天花板 0.30：后台任务（sync/JobScheduler）突发时
                                // 允许短暂借力，但压住"口袋发热"；5 分钟后 scenemode 进一步压到 0.15
                                doze_cfg.perf_ceil = doze_cfg.perf_ceil.min(0.30); // 锁死天花板最高只给 30% 性能
                                doze_cfg.smoothing_up = 0.10;           // 升频极其迟钝
                                doze_cfg.touch_boost_enabled = false;   // 息屏无触摸，关闭触摸升频

                                cpu_governor.init_policies(&doze_cfg);
                            }
                            // 亲和/core_ctl 跟随息屏：top-app 恢复快照、后台压小核（特调保持 boost 布局）
                            {
                                let cfg = config_clone.read().unwrap();
                                apply_affinity_and_corectl(
                                    &mut affinity_mgr,
                                    &mut corectl_mgr,
                                    &cfg,
                                    &current_mode,
                                    false,
                                    crate::monitor::app_detect::get_current_pid(),
                                    &last_core_utils,
                                    scene_mode_active,
                                );
                            }
                            crate::logger::devimp_event("screen", "-", "off");
                        } else {
                            log::info!("{}", t("scheduler-doze-restore"));
                            // 亮屏：清空息屏计时与 scenemode 状态（恢复逻辑在下方重放原模式）
                            screen_off_at = None;
                            scene_mode_active = false;
                            let config_lock = config_clone.read().unwrap();
                            
                            if crate::common::is_special_mode(&current_mode) {
                                // 亮屏恢复特调：akmode 息屏期间通常保持接管（息屏分支不释放）；
                                // 但若息屏时负载事件停止触发看门狗释放过 akmode，这里必须重新接管，
                                // 否则特调限频失效、采样间隔也不会切回 40ms。
                                // 冷却期内跳过特调，直接走 CLG。
                                let in_cooldown = akmode_cooldown_until
                                    .map_or(false, |until| Instant::now() < until);
                                if !ak_governor.is_active() && !in_cooldown {
                                    cpu_governor.release();
                                    let ak_cfg = config_lock.get_akmode().clone();
                                    let initial_tier = get_ak_initial_tier();
                                    if !ak_governor.init_policies(&ak_cfg, initial_tier) {
                                        // init 失败（配置缺失/硬件不支持）：冷却 5 分钟，CLG 接管
                                        akmode_cooldown_until = Some(Instant::now() + AKMODE_COOLDOWN);
                                        log::warn!("{}", t_with_args(
                                            "scheduler-akmode-cooldown",
                                            &fluent_args!("secs" => AKMODE_COOLDOWN.as_secs().to_string())
                                        ));
                                        let clg_cfg = get_clg_cfg(&config_lock, &current_mode);
                                        if clg_cfg.enabled { cpu_governor.init_policies(&clg_cfg); }
                                    }
                                } else if !ak_governor.is_active() {
                                    // 冷却中：CLG 接管
                                    let clg_cfg = get_clg_cfg(&config_lock, &current_mode);
                                    if clg_cfg.enabled { cpu_governor.init_policies(&clg_cfg); }
                                }
                            } else if current_mode == "fast" {
                                // 亮屏恢复极速模式：释放 doze CLG，由 fast_lock 接管
                                ak_governor.release();
                                cpu_governor.release();
                                fast_lock.init();
                            } else if current_mode != "fas" {
                                ak_governor.release();
                                fast_lock.release();
                                let clg_cfg = get_clg_cfg(&config_lock, &current_mode);
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
                            // 亲和/core_ctl 跟随亮屏恢复：亮屏强制重迁移前台线程
                            {
                                let cfg = config_clone.read().unwrap();
                                apply_affinity_and_corectl(
                                    &mut affinity_mgr,
                                    &mut corectl_mgr,
                                    &cfg,
                                    &current_mode,
                                    true,
                                    crate::monitor::app_detect::get_current_pid(),
                                    &last_core_utils,
                                    scene_mode_active,
                                );
                            }
                            crate::logger::devimp_event("screen", "-", "on");
                        }
                    },

                    // --- 2. 前台模式切换事件 ---
                    DaemonEvent::ModeChange { package_name, pid, mode, temperature } => {
                        let mut current_mode_lock = mode_clone.lock().unwrap();
                        let old_mode = current_mode_lock.clone();
                        log::debug!("{}", t_with_args("scheduler-event-mode-change", &fluent_args!(
                            "pkg" => package_name.as_str(),
                            "old" => old_mode.clone(),
                            "new" => mode.as_str(),
                            "temp" => temperature
                        )));
                        // 前台切换已改由 app_detect 直写 status.log fg 行（含同模式切换），
                        // ModeChange 事件仅在模式变化时产生，此处不再重复记录

                        if old_mode != mode {
                            log::info!("{}", t_with_args("scheduler-mode-change-request", &fluent_args!(
                                "old" => old_mode.clone(), "new" => mode.as_str(), "pkg" => package_name.as_str(), "temp" => temperature
                            )));

                            *current_mode_lock = mode.clone();
                            drop(current_mode_lock);

                            last_fg_package = package_name.clone();
                            crate::logger::set_devimp_mode(&mode);
                            crate::logger::devimp_event(
                                "mode_change",
                                &package_name,
                                &format!("{old_mode}->{mode}"),
                            );

                            let _ = utils::try_write_file(&mode_file_path, mode.as_bytes());

                            // 亲和布局/core_ctl 跟随模式切换（含前台 PID 变化的线程重迁移）
                            {
                                let cfg = config_clone.read().unwrap();
                                apply_affinity_and_corectl(
                                    &mut affinity_mgr,
                                    &mut corectl_mgr,
                                    &cfg,
                                    &mode,
                                    is_screen_on,
                                    pid,
                                    &last_core_utils,
                                    scene_mode_active,
                                );
                            }

                            // 特调模式激活打点：仅 ChiRi 白名单应用可进入，info 级便于用户定位
                            if crate::common::is_special_mode(&mode) {
                                log::info!("{}", t_with_args("scheduler-special-mode-active", &fluent_args!(
                                    "pkg" => package_name.as_str(),
                                    "mode" => mode.as_str()
                                )));
                            }

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

                                // 仅在亮屏时处理调度接管。如果息屏，Doze 配置仍在生效，这里不能覆盖它
                                if is_screen_on {
                                    let config_lock = config_clone.read().unwrap();
                                    if crate::common::is_special_mode(&mode) {
                                        // 进入特调模式：停止 CLG，改由 akmode 独立接管。
                                        // 起始档从 rules.yaml 的生效模式识别（档位与全局统一）。
                                        // 冷却期内跳过特调，直接走 CLG。
                                        let in_cooldown = akmode_cooldown_until
                                            .map_or(false, |until| Instant::now() < until);
                                        if !in_cooldown {
                                            cpu_governor.release();
                                            let ak_cfg = config_lock.get_akmode().clone();
                                            let initial_tier = get_ak_initial_tier();
                                            if !ak_governor.init_policies(&ak_cfg, initial_tier) {
                                                // init 失败：冷却 5 分钟，CLG 接管
                                                akmode_cooldown_until = Some(Instant::now() + AKMODE_COOLDOWN);
                                                log::warn!("{}", t_with_args(
                                                    "scheduler-akmode-cooldown",
                                                    &fluent_args!("secs" => AKMODE_COOLDOWN.as_secs().to_string())
                                                ));
                                                let clg_cfg = get_clg_cfg(&config_lock, &mode);
                                                if clg_cfg.enabled { cpu_governor.init_policies(&clg_cfg); }
                                            }
                                        } else {
                                            // 冷却中：CLG 接管
                                            let clg_cfg = get_clg_cfg(&config_lock, &mode);
                                            if clg_cfg.enabled { cpu_governor.init_policies(&clg_cfg); }
                                        }
                                    } else if mode == "fast" {
                                        // 极速模式：停止特调/CLG，由 fast_lock 独立锁满频
                                        ak_governor.release();
                                        cpu_governor.release();
                                        fast_lock.init();
                                    } else {
                                        // 退出特调/极速模式：停止 akmode/fast_lock，交回 CLG 接管
                                        ak_governor.release();
                                        fast_lock.release();
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
                        // 快照逐核 util：按核亲和选核打分与 devimp tick 行的输入
                        last_core_utils = core_utils.clone();
                        // 该事件常规 160ms / 特调 40ms 一次，仅在 DEBUG 时输出摘要便于排查。
                        // 字符串构造在宏外会被无条件求值（每 tick 分配一次），
                        // 用 log_enabled! 门控——INFO 级别下零分配。
                        if log::log_enabled!(log::Level::Debug) {
                            log::debug!("{}", t_with_args("scheduler-event-load", &fluent_args!(
                                "cores" => core_utils.iter().map(|u| format!("{:.0}", u * 100.0)).collect::<Vec<_>>().join(",")
                            )));
                        }
                        // let current_mode = mode_clone.lock().unwrap().clone(); // FAS 暂禁用
                        // ==== FAS 暂禁用：不再向 FAS 投喂 CPU 负载 ====
                        // if is_screen_on && current_mode == "fas" && fas_suspended_at.is_none() {
                        //     fas_controller.update_cpu_util(foreground_max_util);
                        //     fas_controller.update_core_utils(&core_utils);
                        // }
                        // 特调（akmode）优先：白名单应用前台时投喂 akmode 做动态限频
                        // （档位固定不切换，max 随负载在 [最低档, 生效档] 间变化）；
                        // 否则若 CLG 处于活动状态（日常模式或息屏 Doze），投喂 CLG。
                        if ak_governor.is_active() {
                            ak_governor.on_load_update(&core_utils);
                        } else if cpu_governor.is_active() {
                            // Worker 架构：on_load_update 广播给各核心组 Worker，
                            // Worker 线程内自主完成决策 + 写频，无需外部 flush
                            cpu_governor.on_load_update(&core_utils);
                        }

                        // scenemode：息屏超过 scene_mode_delay_secs 后把 CLG 切到低功耗配置
                        // （一次性）。特调模式由 akmode 独立接管不参与；亮屏后恢复原模式。
                        // scenemode 未启用时释放 CLG 回系统默认。
                        // 饱和退出冷却期内不得重进（防止与后台负载反复拉锯）。
                        let scenemode_cooldown_ok = scenemode_cooldown_until
                            .map_or(true, |u| Instant::now() >= u);
                        if !is_screen_on && !scene_mode_active && scenemode_cooldown_ok {
                            // 先用免锁的计时预判（最低 60s），避免息屏期间每个负载 tick 都抢锁
                            let delay_hit = screen_off_at
                                .map_or(false, |off| off.elapsed().as_secs() >= 60);
                            if delay_hit {
                                let current_mode = mode_clone.lock().unwrap().clone();
                                if !crate::common::is_special_mode(&current_mode) {
                                    let config_read = config_clone.read().unwrap();
                                    let delay = config_read.scene_mode_delay_secs.max(60);
                                    let scene_cfg =
                                        config_read.scenemode.cpu_load_governor.clone();
                                    drop(config_read);
                                    if screen_off_at
                                        .map_or(false, |off| off.elapsed().as_secs() >= delay)
                                    {
                                        if scene_cfg.enabled {
                                            if cpu_governor.is_active() {
                                                cpu_governor.reload_config(&scene_cfg);
                                            } else {
                                                cpu_governor.init_policies(&scene_cfg);
                                            }
                                        } else {
                                            cpu_governor.release();
                                        }
                                        log::info!("{}", t("scheduler-scene-mode-enter"));
                                        scene_mode_active = true;
                                        // 立即应用 scenemode 离线核（不等 2s 周期块）：
                                        // 小核全开 + 大核/prime 下线 + 专用小核自钉
                                        {
                                            let cfg = config_clone.read().unwrap();
                                            apply_affinity_and_corectl(
                                                &mut affinity_mgr,
                                                &mut corectl_mgr,
                                                &cfg,
                                                &current_mode,
                                                is_screen_on,
                                                crate::monitor::app_detect::get_current_pid(),
                                                &last_core_utils,
                                                scene_mode_active,
                                            );
                                        }
                                    }
                                }
                            }
                        }

                        // scenemode 饱和退出：little 簇 max_util 持续顶满性能上限
                        // → 退回 powersave（恢复全部在线核）+ 300s 冷却不得重进，
                        // 防止后台负载压不死小核时反复拉锯。util 是忙时占比（与
                        // 频率无关），饱和即真饱和，与 perf_ceil 数值无耦合。
                        if scene_mode_active && !is_screen_on {
                            let ranges = crate::common::chiri_core_ranges();
                            let little_max = ranges
                                .little
                                .clone()
                                .filter_map(|c| last_core_utils.get(c).copied())
                                .fold(0.0_f32, f32::max);
                            if little_max >= SCENEMODE_SAT_UTIL {
                                let sustained = match scenemode_sat_since {
                                    Some(since) => since.elapsed() >= SCENEMODE_SAT_SECS,
                                    None => {
                                        scenemode_sat_since = Some(Instant::now());
                                        false
                                    }
                                };
                                if sustained {
                                    scenemode_sat_since = None;
                                    scene_mode_active = false;
                                    scenemode_cooldown_until =
                                        Some(Instant::now() + SCENEMODE_COOLDOWN);
                                    let current_mode = mode_clone.lock().unwrap().clone();
                                    // 退回 powersave：给后台负载更大余量
                                    let ps_cfg = get_clg_cfg(&config_clone.read().unwrap(), "powersave");
                                    if cpu_governor.is_active() {
                                        cpu_governor.reload_config(&ps_cfg);
                                    } else {
                                        cpu_governor.init_policies(&ps_cfg);
                                    }
                                    // 立即恢复全部在线核 + 解除专用核钉定
                                    {
                                        let cfg = config_clone.read().unwrap();
                                        apply_affinity_and_corectl(
                                            &mut affinity_mgr,
                                            &mut corectl_mgr,
                                            &cfg,
                                            &current_mode,
                                            is_screen_on,
                                            crate::monitor::app_detect::get_current_pid(),
                                            &last_core_utils,
                                            scene_mode_active,
                                        );
                                    }
                                    log::info!(
                                        "{}",
                                        t_with_args(
                                            "scheduler-scene-mode-saturation",
                                            &fluent_args!("util" => format!("{:.0}", little_max * 100.0))
                                        )
                                    );
                                    crate::logger::devimp_event(
                                        "scene_exit",
                                        "-",
                                        "saturation->powersave+300s",
                                    );
                                }
                            } else {
                                scenemode_sat_since = None;
                            }
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
                            if crate::common::is_special_mode(&current_mode) {
                                // 特调模式：按 rules.yaml 重新换算档位并重载 akmode 配置
                                let ak_cfg = config_lock.get_akmode().clone();
                                let initial_tier = get_ak_initial_tier();
                                if ak_governor.is_active() {
                                    ak_governor.reload_config(&ak_cfg, initial_tier);
                                } else {
                                    let in_cooldown = akmode_cooldown_until
                                        .map_or(false, |until| Instant::now() < until);
                                    if !in_cooldown {
                                        if !ak_governor.init_policies(&ak_cfg, initial_tier) {
                                            akmode_cooldown_until = Some(Instant::now() + AKMODE_COOLDOWN);
                                            log::warn!("{}", t_with_args(
                                                "scheduler-akmode-cooldown",
                                                &fluent_args!("secs" => AKMODE_COOLDOWN.as_secs().to_string())
                                            ));
                                            let clg_cfg = get_clg_cfg(&config_lock, &current_mode);
                                            if clg_cfg.enabled { cpu_governor.init_policies(&clg_cfg); }
                                        }
                                    } else {
                                        let clg_cfg = get_clg_cfg(&config_lock, &current_mode);
                                        if clg_cfg.enabled { cpu_governor.init_policies(&clg_cfg); }
                                    }
                                }
                            } else if current_mode == "fast" {
                                // 极速模式不读 yaml 参数，ConfigReload 无需处理；
                                // fast_lock 保持活跃，仅确保 CLG 未意外启动
                                if cpu_governor.is_active() { cpu_governor.release(); }
                            } else {
                                // 非特调、非极速模式：确保 fast_lock 释放，CLG 接管
                                ak_governor.release();
                                fast_lock.release();
                                let clg_cfg = get_clg_cfg(&config_lock, &current_mode);
                                if clg_cfg.enabled {
                                    if cpu_governor.is_active() { cpu_governor.reload_config(&clg_cfg); } 
                                    else { cpu_governor.init_policies(&clg_cfg); }
                                } else if cpu_governor.is_active() {
                                    cpu_governor.release();
                                }
                            }
                        }
                        // 亲和/core_ctl 与配置联动（开关变化时切换布局；内部去重）
                        {
                            let cfg = config_clone.read().unwrap();
                            crate::logger::set_devimp_active(cfg.meta.dev_record);
                            crate::logger::set_devimp_mode(&current_mode);
                            apply_affinity_and_corectl(
                                &mut affinity_mgr,
                                &mut corectl_mgr,
                                &cfg,
                                &current_mode,
                                is_screen_on,
                                crate::monitor::app_detect::get_current_pid(),
                                &last_core_utils,
                                scene_mode_active,
                            );
                        }
                        crate::logger::devimp_event("config_reload", "-", "rules.yaml");
                    }

                    // --- 6. eBPF 扩展探针统计（ChiRi 专属遥测，2s 一次增量）---
                    DaemonEvent::BpfStats { wakeups, migrations, freq_transitions } => {
                        // 仅缓存供遥测 CSV/摘要落盘，不参与调频决策，也不刷新 CLG 看门狗心跳
                        // （探针加载失败时增量为 0，不影响任何控制路径）
                        last_bpf_stats = (wakeups, migrations, freq_transitions);
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
            ak_governor.release();
            fast_lock.release();
            // 亲和布局与 core_ctl 同步恢复系统原始状态
            corectl_mgr.release();
            affinity_mgr.release();
            // ==== FAS 暂禁用 ====
            // fas_controller.reset_all_freqs();
            // fas_controller.clear_game();
        })?;

    Ok(())
}