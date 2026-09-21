//! mod.rs: [consts] [thermal] [policies] [affinity] [threads] [config_watcher] [ipc_main] [ipc_state] [evt_loop] [evt_screen] [evt_mode] [evt_pkg_switch] [evt_load] [evt_frame] [evt_reload] [evt_bpf] [panic_recovery]

use anyhow::Result;
use std::fs;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock, mpsc};
use std::thread;
use std::time::{Duration, Instant};

// [consts]
// CLG 看门狗：SystemLoadUpdate 常规 160ms / 特调 40ms 投喂一次，超过 CLG_STALE_MAX 没收到
// 事件就认为负载源失效（eBPF 加载失败/探针崩溃/通道断开），直接 release() 回系统调频，
// 防止 CPU 锁死在上次写入的频率（如 8550 default perf_init=1.0 会锁满全核高频）。
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
/// scenemode 饱和退出：**常驻簇（小核 + 大核）** max_util 持续高于该值视为
/// 顶满性能上限（util 是忙时占比，与频率无关——后台负载压不住常驻核时即饱和）
const SCENEMODE_SAT_UTIL: f32 = 0.75;
/// 饱和持续判定时长：连续满足才退出，防止瞬时突发误触发
const SCENEMODE_SAT_SECS: Duration = Duration::from_secs(10);
/// scenemode 冷却：饱和退出后 300s 内不得重新进入（防止与后台负载反复拉锯）
const SCENEMODE_COOLDOWN: Duration = Duration::from_secs(300);
/// scheduler_ipc 事件循环 panic 自愈：连续崩溃超过该次数后放弃重启（防止
/// poisoned lock 等确定性 panic 变成打满 CPU 的重启风暴），仅保留最终清理
const SCHEDULER_IPC_RESTART_MAX: u32 = 5;
/// panic 重启退避：每次重启前等待，错开引发 panic 的外部状态（如负载风暴）
const SCHEDULER_IPC_RESTART_BACKOFF: Duration = Duration::from_secs(1);
/// 电池状态节点（标准 power_supply 接口）：区分充电/放电
const BATT_STATUS_PATH: &str = "/sys/class/power_supply/battery/status";

/// status 节点取值不认识时的告警去重（值恢复为认识的内容后重新武装）
static BATT_STATUS_UNKNOWN_WARNED: AtomicBool = AtomicBool::new(false);
/// 热保护解除斜坡步长：压制加深立即生效，解除方向每个采样周期（2s）最多
/// 恢复 0.15。温度围绕阈值震荡时，若解除立即全量恢复，温度马上反弹触发
/// 再压制——cap 在 40/70/100 间以 ~8s 周期反复跳变（bang-bang 振荡，
/// devimp 实测 8475 高负载游戏 84% 时间处于压制），每次跳变都把
/// current_perf 砸回低位再缓慢爬升，表现为周期性掉帧卡顿。限速解除后
/// 恢复过程 8s 级渐进，温度反弹会在中途重新压制，不再打穿阈值。
const THERMAL_UNPRESS_STEP: f32 = 0.15;

// [thermal]
/// 读电池充放电状态（1s snap 处消费）：归一为小写短词；节点缺失或
/// 未知值返回 "-"（与 CSV 缺失占位一致）。电流符号因厂商节点方向不一，
/// 不可靠，故直接读 status 字符串。
fn read_battery_charge_state() -> String {
    let state = match std::fs::read_to_string(BATT_STATUS_PATH) {
        Ok(s) => {
            let raw = s.trim();
            // 大小写与多余空白都容忍：不同内核会写 Charging / charging，规范化后再比对
            match raw.to_ascii_lowercase().as_str() {
                "charging" => "charging",
                "discharging" => "discharging",
                "full" => "full",
                "not charging" => "not_charging",
                _ => {
                    // 取值不认识 = 功耗均值永远不会取样（门控只认 discharging），
                    // 必须把原文打出来，否则只看得到「PowerAVG.chr 一直是空的」
                    if !BATT_STATUS_UNKNOWN_WARNED.swap(true, Ordering::Relaxed) {
                        log::warn!(
                            "{}",
                            crate::i18n::t_with_args(
                                "battery-status-unknown",
                                &crate::fluent_args!("raw" => raw.to_string())
                            )
                        );
                    }
                    "-"
                }
            }
        }
        Err(_) => "-",
    };
    if state != "-" {
        BATT_STATUS_UNKNOWN_WARNED.store(false, Ordering::Relaxed);
    }
    state.to_string()
}

/// 温度传感器滤波器：物理范围门 + 毛刺丢弃 + 3 样本中值 + 斜率限制。
/// MTK soc_max 等合成温区读数跳变极大（devimp 实测 2s 内 91.5→57.2°C，
/// 物理上不可能），直接用于带回滞的阈值判定会让 cap 以 ~8s 周期反复跳变。
/// 逐级平滑后才能作为秒级热判定的输入。
struct TempFilter {
    /// 物理合理区间 [min_c, max_c]：节点异常/未初始化的读数直接丢弃
    min_c: f32,
    max_c: f32,
    /// 相对上次有效输出的最大可信跳变（°C），超过视为传感器毛刺丢弃
    max_jump: f32,
    /// 输出斜率限制（°C/样本）：真实热质量决定温度不可能瞬间大幅变化
    max_step: f32,
    window: [f32; 3],
    win_len: usize,
    win_idx: usize,
    last_out: Option<f32>,
}

impl TempFilter {
    fn new(min_c: f32, max_c: f32, max_jump: f32, max_step: f32) -> Self {
        Self {
            min_c,
            max_c,
            max_jump,
            max_step,
            window: [0.0; 3],
            win_len: 0,
            win_idx: 0,
            last_out: None,
        }
    }

    /// 喂入一个原始样本，返回滤波后的温度。样本被丢弃时返回上次输出
    /// （首个有效样本到来前返回 None）。
    fn push(&mut self, raw: f32) -> Option<f32> {
        // 1) 物理范围门：明显不合理的读数（如 0.3°C 的电池温度节点）丢弃
        if !(raw >= self.min_c && raw <= self.max_c) {
            return self.last_out;
        }
        // 2) 毛刺丢弃：相对上次输出的跳变超过真实热质量的上限（如 2s 内
        //    降 25°C 实为压制后频率骤降叠加合成温区切换热点），丢弃防止
        //    误触发"温度骤降→解除→反弹"的振荡
        if let Some(prev) = self.last_out {
            if (raw - prev).abs() > self.max_jump {
                return self.last_out;
            }
        }
        // 3) 3 样本中值：滤掉单点毛刺
        self.window[self.win_idx] = raw;
        self.win_idx = (self.win_idx + 1) % 3;
        self.win_len = (self.win_len + 1).min(3);
        let mut sorted = self.window[..self.win_len].to_vec();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let median = sorted[sorted.len() / 2];
        // 4) 斜率限制：压制/解除两个方向都平滑，消除控制环振荡
        let out = match self.last_out {
            None => median,
            Some(prev) => prev + (median - prev).clamp(-self.max_step, self.max_step),
        };
        self.last_out = Some(out);
        self.last_out
    }
}

/// 单传感器三级判定（带回滞）：>= 硬限压 hard_cap；>= 软限压 soft_cap；
/// 回落到 软限-hyst 以下解除；回滞带内无压制状态保持、压制中先退到软限档防阶跃。
///
/// `temp_c` 必须传入 `TempFilter::push` 的输出——1s snap 块把滤波值写入
/// `last_batt_temp`/`last_cpu_temp`，本函数所在的 2s 热块复用这两个变量。
/// 不要直接喂原始读数：soc_max 等合成温区跳变极大（实测 2s 内 ±25°C），
/// 绕过滤波会让 cap 以 ~8s 周期 bang-bang 振荡。新增调用方同样必须传滤波值。
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
// FAS（帧感知调度）：引擎位于 crate::scheduler::fas（算法层），ChiRi 侧由 fas_manager 提供多实例生命周期管理。
pub mod affinity;
pub mod core_ctl;
pub mod cpu_load_governor;
pub mod fas_manager;
pub mod fast;
pub mod governor;
pub mod gpu;
pub mod touch_detect;
pub mod tuned;

use crate::common;
use crate::common::DaemonEvent;
use crate::fluent_args;
use crate::i18n::{load_language, t, t_with_args};
use crate::logger;
use crate::utils;
use config::Config;
use scheduler::CpuScheduler;

// [policies]
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

/// devimp snap 行用：各 policy 的**实际**当前频率 / 上限 / 下限 / 调速器
/// （`scaling_cur_freq` / `scaling_max_freq` / `scaling_min_freq` / `scaling_governor`）。
/// 多 policy 以 `;` 分隔，每项 `policy<id>:<值>`；节点读不到写 `-`。
/// **只在 devimp 开启时调用**（每秒一次 sysfs 读），不做缓存——这些值会被内核
/// governor 与 TunedGovernor 改写，缓存只会给出过期数据。与 devimp 既有的
/// `cur_freq_khz`/`max_freq_khz`（调度器决策值）互补：那两列是「我们写了多少」，
/// 这里是「内核现在实际是多少」。
pub fn cpu_freq_snapshot() -> (String, String, String, String) {
    let (mut cur, mut max, mut min, mut gov) = (
        String::new(),
        String::new(),
        String::new(),
        String::new(),
    );
    for p in get_cpu_policies() {
        let base = format!("/sys/devices/system/cpu/cpufreq/policy{}", p.id);
        let read = |f: &str| -> String {
            std::fs::read_to_string(format!("{base}/{f}"))
                .map(|s| s.trim().to_string())
                .unwrap_or_else(|_| "-".to_string())
        };
        let push = |dst: &mut String, v: String| {
            if !dst.is_empty() {
                dst.push(';');
            }
            dst.push_str(&format!("policy{}:{}", p.id, v));
        };
        push(&mut cur, read("scaling_cur_freq"));
        push(&mut max, read("scaling_max_freq"));
        push(&mut min, read("scaling_min_freq"));
        push(&mut gov, read("scaling_governor"));
    }
    (cur, max, min, gov)
}

/// GPU 频率快照的节流间隔：读 GPU 频率节点（尤其 Adreno 的 `gpuclk`）会把 GPU
/// 从低功耗状态拉起来，跟着 1s 的 snap 走等于**每秒唤醒一次 GPU**。
/// 8550 实测：加上每秒 GPU 采集后，同一 playback 场景（gpu_busy<12）功耗从
/// 1.76 W 涨到 2.59 W、util 与 psi 同步上升——采集本身的代价盖过了收益。
/// 故 GPU 单独按 10s 采一次；CPU 的 cpufreq 节点读取是廉价的，仍每秒采。
const GPU_SNAP_INTERVAL_MS: u64 = 10_000;

/// GPU 频率/调速器快照总开关：**当前默认关闭**。
/// 关闭原因（8550 实测）：读 GPU 频率节点（Adreno `gpuclk` / devfreq `cur_freq`）会把
/// GPU 从低功耗状态拉起来，跟着 1s 的 snap 走等于每秒唤醒一次 GPU——同一 playback
/// 场景（gpu_busy<12）功耗由 1.76 W 涨到 2.59 W（+47%），util 与 psi 同步上升，
/// 采集本身的代价盖过了数据收益。CPU 那 4 列不受影响（cpufreq 节点读取廉价）。
/// **恢复方式**：确需 GPU 频率数据时把这里改回 true（节流间隔见 GPU_SNAP_INTERVAL_MS）；
/// 彻底避免唤醒需要另找不触碰 GPU 硬件的读数路径，本开关只是先止血。
const GPU_SNAPSHOT_ENABLED: bool = false;
static LAST_GPU_SNAP_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// 距上次 GPU 快照是否已超过节流间隔（到点则顺带更新时间戳）
fn gpu_snapshot_due() -> bool {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let prev = LAST_GPU_SNAP_MS.load(std::sync::atomic::Ordering::Relaxed);
    if now.saturating_sub(prev) < GPU_SNAP_INTERVAL_MS {
        return false;
    }
    LAST_GPU_SNAP_MS.store(now, std::sync::atomic::Ordering::Relaxed);
    true
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
/// 仅供 FAS 使用，FAS 禁用期间暂无调用，恢复时启用。
#[allow(dead_code)]
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

/// 按当前模式判定是否为 boost 类模式（亲和收窄/core_ctl 保大核的判定口径）：
/// boost / vector / 特调（akmode）为 boost 类，reduce/default 及未知模式为 normal。
/// contingency / babel 有独立分支，不走 boost 亲和收窄。
fn is_boost_mode(mode: &str) -> bool {
    mode == "boost" || mode == "vector" || crate::common::is_special_mode(mode)
}

/// 特调的 boost 亲和开关（参数组 boost_affinity）：游戏特调 true（保响应），
/// 视频/轻载省电特调 false（不保大核在线、不收窄 cpuset）。非特调恒 true
/// （实际只对 is_boost_mode 命中的模式有意义）。
fn tuned_boost_affinity(config: &crate::chiri::config::Config, mode: &str) -> bool {
    if crate::common::is_special_mode(mode) {
        config.get_tuned_profile(mode).boost_affinity
    } else {
        true
    }
}

/// governor/GPU/极速锁频与模式的同步（幂等，可在任意入口调用）：
/// contingency → performance 调速器 + GPU 锁最高频 + 极速锁频（min=max=硬件最高，
/// 与 vector 同口径；governor 节点只读的机型写不进 performance，靠锁频窗口等效）；
/// babel → 仅 performance；其他模式 → governor/GPU 按快照恢复
/// （fast_lock 不在这里释放：vector 也走 `_` 分支，误释放会锁不住）。
fn sync_lab_governor_gpu(
    mode: &str,
    governor: &mut governor::GovernorGuard,
    gpu: &mut gpu::GpuGuard,
    fast_lock: &mut crate::chiri::fast::FastLock,
) {
    match mode {
        "contingency" => {
            if !governor.is_active() {
                governor.activate();
            }
            if !gpu.is_active() {
                gpu.lock();
            }
            // 全核最高频硬锁：不能只靠 performance——部分机型 scaling_governor
            // 节点只读（写不进），min=max=硬件最高在任意 governor 下都等效锁频
            if !fast_lock.is_active() {
                fast_lock.init();
            }
        }
        "babel" => {
            if !governor.is_active() {
                governor.activate();
            }
            if gpu.is_active() {
                gpu.release();
            }
        }
        _ => {
            if governor.is_active() {
                governor.release();
            }
            if gpu.is_active() {
                gpu.release();
            }
        }
    }
}

/// CLG 档位参数获取（未知/空模式名禁用 CLG，避免默认参数意外接管）。
/// 与调度线程内的 get_clg_cfg 闭包同逻辑；文件级供 FAS 延迟退出等辅助函数使用。
fn clg_cfg_for(
    config: &crate::chiri::config::Config,
    mode: &str,
) -> crate::chiri::config::CpuLoadGovernorConfig {
    config
        .get_mode(mode)
        .map(|m| m.cpu_load_governor.clone())
        .unwrap_or_else(|| {
            let mut cfg = crate::chiri::config::CpuLoadGovernorConfig::default();
            cfg.enabled = false;
            cfg
        })
}

/// 按目标模式接管频率（特调 / vector / CLG）。FAS 正常退出与延迟退出共用。
/// 调用前提：FAS 已完全退出（频率 + governor 已恢复）、各 governor 处于安全态、亮屏。
/// akmode init 失败不设冷却（冷却语义只属于前台切换路径），直接 CLG 回退。
fn apply_mode_takeover(
    mode: &str,
    config: &crate::chiri::config::Config,
    cpu_governor: &mut crate::chiri::cpu_load_governor::CpuLoadGovernor,
    ak_governor: &mut crate::chiri::tuned::TunedGovernor,
    fast_lock: &mut crate::chiri::fast::FastLock,
) {
    if crate::common::is_special_mode(mode) {
        let ak_cfg = config.get_tuned_profile(mode);
        if !ak_governor.init_policies(mode, &ak_cfg) {
            let clg_cfg = clg_cfg_for(config, mode);
            if clg_cfg.enabled {
                cpu_governor.init_policies(&clg_cfg);
            }
        }
    } else if mode == "vector" {
        // vector 档由 fast_lock 硬锁（不注册 CLG 参数），漏 init 就是频率零接管
        ak_governor.release();
        cpu_governor.release();
        fast_lock.init();
    } else {
        ak_governor.release();
        fast_lock.release();
        let clg_cfg = clg_cfg_for(config, mode);
        if clg_cfg.enabled {
            if cpu_governor.is_active() {
                cpu_governor.reload_config(&clg_cfg);
            } else {
                cpu_governor.init_policies(&clg_cfg);
            }
        } else {
            cpu_governor.release();
        }
    }
}

// [affinity]
/// 应用 CPU 亲和布局与 core_ctl 在线策略（ChiRi 专属，跟随模式/屏幕/前台 PID）。
/// 内部带去重：布局与 PID 未变化时无 sysfs 写入，可安全周期性调用。
/// `core_utils` 为最近一次 SystemLoadUpdate 的逐核 util（按核选核打分输入）。
/// `scenemode_offline` 为 scenemode 激活标志：抑制 boost（防厂商 core_ctl 把
/// 下线的核拉回来）并触发 core_ctl 离线（prime 下线深度省电）。
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
    // 实验室静态分组模式（contingency/babel）：停线程迁移、按模式写组级 cpus；
    // governor/GPU 由调用方 sync（sync_lab_governor_gpu）。周期重入即纠偏
    // （框架写回的 top-app/foreground 会被重写）。core_ctl 交回系统（NONE）。
    if mode == "contingency" || mode == "babel" {
        // thread_bind=false（线程调整关闭）时不接管摆放：lab 静态分组本质是
        // 线程/核心摆放，与普通亲和一样受总闸约束（config.affinity.enabled 已在
        // Config::load 与 meta.thread_bind 取与），否则 lab 模式下总闸关不掉
        if config.affinity.enabled {
            affinity.lab_static_apply(mode);
        } else {
            affinity.lab_static_deactivate();
        }
        corectl.set_power_state(false, false);
        return;
    }
    // stardust 家族（scenemode）语义：停线程迁移与动态分组、全部 cpuset 恢复全核——
    // 压频只压 CLG 频率上限，不做任何核心/线程特化（原「prime 整簇下线 + 保留核
    // 独占」已废弃，core_ctl 回 Normal）。affinity.release 会把此前收窄的组按快照恢复。
    // 本分支由 2s 周期块与场景事件反复进入：仅在持有接管时 release 一次，
    // 避免息屏全程每 2s 重复回写后台组 uclamp.max 并刷「已释放接管」日志
    if scenemode_offline {
        if affinity.is_active() {
            affinity.release();
        }
        corectl.set_power_state(false, false);
        return;
    }
    // fas 模式按 boost 处理：FAS 只负责调频，线程摆放沿用 boost 布局
    // （top-app/foreground 收窄 prime∪big + 前台钉核 + core_ctl 保大核）。
    // FAS 息屏省电已完全移除（2026-09）：FAS 与屏幕状态完全解耦——息屏不再
    // 释放实例，故 fas 全时段入 boost，不再按 screen_on 回退 normal 布局。
    // 语义声明：boost 只看 mode、不查 FAS 引擎是否活跃——初始化
    // 失败冷却期（最长 FAS_COOLDOWN=300s）与重激活间隙内，
    // mode 仍为 fas，boost 布局照常生效（调频为 CLG default）。这是有意的：
    // mode=="fas" 时前台必为白名单游戏，摆放对游戏非负收益，且 gating
    // is_active 会引入激活边界的布局抖动
    // 省电型特调（参数组 boost_affinity=false，如 playback/daily）不走 boost：不收窄
    // cpuset、不保大核常在线——那与「贴负载降频」的省电目标相反（大核空转漏电），
    // 也会把视频解码线程钉到大核组被低上限压住
    let boost = ((is_boost_mode(mode) && tuned_boost_affinity(config, mode)) || mode == "fas")
        && !scenemode_offline;
    // top-app uclamp.max 放开（激活期写 100 让重线程可被 EAS 放到 prime）的管理
    // 归属：特调（akmode）由本函数按当前模式同步 Some(true)；fas 交给
    // fas_affinity_hook（None = 本函数不干预，避免与其时序打架）；其余 boost 模式
    // Some(false)——保证离开特调后不残留 100。省电型特调同样不放开（不抬 prime 上限）
    let uclamp_override = if mode == "fas" {
        None
    } else {
        Some(crate::common::is_special_mode(mode) && tuned_boost_affinity(config, mode))
    };
    affinity.apply(
        screen_on,
        fg_pid,
        &config.affinity,
        boost,
        core_utils,
        uclamp_override,
    );
    // core_ctl 在线接管随 boost 走；scenemode 不再触发 prime 下线（stardust 家族
    // 语义 = 全核心 + 仅压频，见上方分支）
    corectl.set_power_state(config.core_ctl.enabled && boost, false);
}

/// FAS 亲和接入点：FAS 激活/去激活时调整线程摆放相关状态。
/// 当前职责：激活期放开 top-app uclamp.max（boost 进入时按机型配置写入的
/// 85 会钳制 EAS 对重线程的 capacity 视图、抑制 prime 放置，与 FAS 让
/// prime 承接负载的目标相悖；FAS 激活期锁频绕过 schedutil，85 对调频
/// 无效，故激活期写 100、去激活还原）。后续 FAS 线程钉核扩展也在此实现。
fn fas_affinity_hook(
    affinity_mgr: &mut affinity::AffinityManager,
    corectl_mgr: &mut core_ctl::CoreCtlManager,
    active: bool,
    fg_pid: i32,
) {
    let _ = (corectl_mgr, fg_pid);
    affinity_mgr.set_boost_uclamp_override(active);
}

// [threads]
/// 启动 Chiri 调度线程组（由 main.rs 调用）：
/// - `config_watcher` 线程：监听 config 目录，热重载 Config 并重放一次性系统调整
/// - `scheduler_ipc` 线程：消费 `DaemonEvent` 状态机，驱动 CLG 接管/释放/配置切换
///
/// 参数 `rx` 为 Monitor 层与调度层间的有界事件通道，`shared_config` 为全局共享配置，
/// `ak_active` 为特调激活共享标志（Monitor 层据此切换采样间隔），
/// `fas_active` 为 FAS 前台激活共享标志（FasManager 置位，Monitor 层 fps_monitor
/// 据此门控 eBPF 探针加载与 uprobe 挂载——反偷跑）。
pub fn start_scheduler_thread(
    rx: mpsc::Receiver<DaemonEvent>,
    shared_config: Arc<RwLock<Config>>,
    ak_active: Arc<AtomicBool>,
    fas_active: Arc<AtomicBool>,
    rhine_initial: Option<String>,
) -> Result<()> {
    let root = common::get_module_root();
    // 配置路径：8550 等 Chiri 目标 SoC 使用处理器子目录 config/{soc}/meta.yaml，热重载跟随该文件
    let config_path = common::get_config_path();
    let config_dir = root.join("config");

    // 初始模式透传 rules.yaml 的 global_mode：此前硬编码 "default" 会导致开机到首个
    // ModeChange（约 2 秒）前按错误的模式接管 CPU（8550 上 default perf_init=1.0 会锁满频），
    // 且与用户配置的 global_mode 不一致。global_mode 未配置或不是已注册模式时回退 default。
    let initial_mode = {
        // 嵌入 rules.yaml 为唯一规则来源（编译期打包，防篡改；磁盘文件仅展示副本）。
        // 实验室启用时它的全局模式覆盖优先——否则开机后到首个 ModeChange（约 2 秒）
        // 会按 rules 里的旧值接管 CPU，与实验室语义不符。
        let rules = crate::common::embedded_rules();
        let m = crate::common::lab_global_mode().unwrap_or_else(|| rules.global_mode.clone());
        if m.is_empty() || shared_config.read().unwrap().get_mode(&m).is_none() {
            "default".to_string()
        } else {
            m
        }
    };

    // 当前生效模式名（跨线程共享，仅在 scheduler_ipc 线程内写）
    // 留一份给 DOWN 停摆退出时恢复用（initial_mode 随后被 move 进共享锁）；
    // 初值只是启动缺省值，真正进入停摆时会被当时的实际模式覆盖
    let mut down_resume_mode = initial_mode.clone();
    let shared_mode_name = Arc::new(Mutex::new(initial_mode));
    // sysfs 路径存在性缓存，避免每次 IO 调整前重复探测
    let sys_path_exist = Arc::new(utils::SysPathExist::new());
    // 触摸事件通道（事件驱动）：触摸检测线程发送触摸事件，scheduler_ipc 即时处理并触发大核升频
    let (touch_tx, touch_rx) = mpsc::sync_channel::<()>(8);
    // config.yaml 热重载联动标志：config_watcher 成功重载后置位，scheduler_ipc 轮询消费。
    // 修复此前「config.yaml 调参要等下次 ModeChange/规则重载才应用到运行中的 CLG/akmode」的
    // 热更新断链——现在调参保存后 100ms 内即按当前模式重载调度器配置。
    let config_dirty = Arc::new(AtomicBool::new(false));

    // [down_watcher]
    // DOWN 停摆状态监听：down.chr 一被写入/清空就切换停摆，与 meta.yaml、rhine.chr 同语义。
    // 这里只维护「该不该停摆」这个事实；真正的释放/恢复在调度循环里做（governor 归它独占）。
    // **必须排在所有会写 sysfs 的线程与首次下发之前**：下面的一次性系统调整、config/rhine
    // 两条监听都要按它决定是否下发。down.chr 落盘持久化，「重启后仍停摆」的那次启动若
    // 先下发再判定，就等于停摆期又把 tweak 写了一遍。
    let down_root = root.clone();
    crate::down::on_startup(&down_root);
    thread::Builder::new()
        .name("down_watcher".to_string())
        .spawn(move || crate::down::watch_loop(down_root))?;

    // 启动时立即应用一次性系统调整（cpuidle / IO / 屏蔽系统自带触摸升频 / 内核 sched 参数），
    // 避免首次配置变更前这些调整处于未生效状态（config_watcher 仅在配置变化后重放）。
    // **停摆启动期跳过**：DOWN 的语义是「调度不工作、一切交回系统」，这些节点里正有
    // 一批是与调度直接相关的（cpu_boost 输入升频、cpuidle governor、sched_migration_cost 等），
    // 下发等于停摆期还在替用户调度，采集到的基线也就不再是系统原状。
    // 停摆判定在 `apply_system_tweaks` 内部（那里才是唯一下发入口，且带互斥闸），
    // 这里不再重复判一次，避免两处口径漂移
    if let Err(e) =
        CpuScheduler::new(shared_config.clone(), sys_path_exist.clone()).apply_system_tweaks()
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

    // [config_watcher]
    let config_clone = shared_config.clone();
    let sys_path_clone = sys_path_exist.clone();
    let dirty_clone = config_dirty.clone();
    // config_path 下面要被 config_watcher 的闭包吃掉，实验室监听线程留一份
    let config_path_for_rhine = config_path.clone();
    // 只关心生效 meta 文件自身的事件：同目录下的临时文件（WebUI 的
    // `meta.yaml.webui.tmp`、daemon 自愈的 `meta.yaml.tmp`）必须忽略，
    // 否则会在原子替换完成前提前重载，读到旧内容（详见 utils::DirWatcher）
    let config_file_name = config_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "meta.yaml".to_string());

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
            // inotify 实例跨重载复用（旧实现每轮重建，重载窗口内到达的事件被丢弃）
            let mut watcher: Option<utils::DirWatcher> = None;
            loop {
                if watcher.is_none() {
                    match utils::DirWatcher::new(&watch_dir) {
                        Ok(w) => watcher = Some(w),
                        Err(e) => {
                            log::error!(
                                "{}",
                                t_with_args(
                                    "config-watch-error",
                                    &fluent_args!("error" => e.to_string())
                                )
                            );
                            // 退避后再重试，避免持续错误时忙循环刷 CPU
                            thread::sleep(std::time::Duration::from_secs(2));
                            continue;
                        }
                    }
                }
                let waited = watcher.as_mut().unwrap().wait_change(&config_file_name);
                if let Err(e) = waited {
                    log::error!(
                        "{}",
                        t_with_args(
                            "config-watch-error",
                            &fluent_args!("error" => e.to_string())
                        )
                    );
                    // 丢弃异常实例（目录被重建/ fd 异常），下轮重新建立监听
                    watcher = None;
                    thread::sleep(std::time::Duration::from_secs(2));
                    continue;
                }
                log::info!("{}", t("config-reloading"));

                // meta.yaml 自愈先于重载：字段非法时用嵌入默认整体覆盖并追加警告注释，
                // 文件缺失则重建，然后再加载（Config::load 读到的一定是合法 meta）。
                // nofix 防篡改开关开启时跳过：文件保持用户原样，字段非法由
                // Config::load/read_external_meta 自行回退内嵌默认（不落盘）
                if !common::nofix_active() {
                    common::sync_meta_snapshot(&config_path);
                }

                let old_lang = config_clone.read().unwrap().meta.language.clone();

                match Config::load(config_path.to_str().unwrap()) {
                    Ok(new_config) => {
                        logger::update_level(&new_config.meta.loglevel);
                        *config_clone.write().unwrap() = new_config;

                        let new_lang = config_clone.read().unwrap().meta.language.clone();
                        if old_lang != new_lang {
                            load_language(&new_lang);
                        }

                        log::info!("{}", t("config-reloaded-success"));

                        // 一次性系统调整的下发要过停摆判定：本线程独立于调度循环，
                        // 拿不到那里的 `halted`，判定放在唯一的入口
                        // `CpuScheduler::apply_system_tweaks` 里（读进程级原子标志 + 互斥闸）。
                        // 停摆期仍照常重载配置（日志级别/语言要生效），只是不往系统里写；
                        // 退出停摆时由调度循环补发一次。
                        let scheduler =
                            CpuScheduler::new(config_clone.clone(), sys_path_clone.clone());
                        if let Err(e) = scheduler.apply_system_tweaks() {
                            log::error!(
                                "{}",
                                t_with_args(
                                    "config-apply-tweaks-failed",
                                    &fluent_args!("error" => e.to_string())
                                )
                            );
                        }

                        // 通知 scheduler_ipc：运行中的 CLG/akmode/亲和/core_ctl 需按新配置重载
                        dirty_clone.store(true, Ordering::Release);
                    }
                    Err(load_err) => log::error!(
                        "{}",
                        t_with_args(
                            "config-reload-fail",
                            &fluent_args!("error" => load_err.to_string())
                        )
                    ),
                }
            }
        })?;

    log::info!("{}", t("main-config-watch-thread-create"));

    // [rhine_watcher]
    // 实验室状态监听：rhine.chr 一被写入就套用/还原，和 meta.yaml 热重载同语义，
    // 不用重启调度。盯的是模块根下的 rhine.chr（config_watcher 盯的是 config 子目录里的
    // meta.yaml，两者不是一回事）。套用/还原时会改写 meta.yaml，写入方只有这一个线程，
    // 不存在并发改写。
    let rhine_root = root.clone();
    thread::Builder::new()
        .name("rhine_watcher".to_string())
        .spawn(move || {
            crate::rhine::watch_loop(rhine_root, config_path_for_rhine, rhine_initial)
        })?;

    // [ipc_main]
    let config_clone = shared_config.clone();
    let mode_clone = shared_mode_name.clone();
    let dirty_ipc = config_dirty.clone();
    // sysfs 存在性缓存的另一份：停摆退出后由本线程补发一次性系统调整
    // （ config_watcher 那条路径只在配置文件变化时才重放）
    let tweaks_sys_path = sys_path_exist.clone();

    thread::Builder::new()
        .name("scheduler_ipc".to_string())
        .spawn(move || {
            log::info!("{}", t("scheduler-ipc-started"));

            // [ipc_state] 
            let root = common::get_module_root();
            // 当前模式持久化文件：每次模式切换时写入，供外部（如 WebUI）读取当前状态。
            // 自愈：常态下每 5 秒重写一次（见循环内 MODE_FILE_REWRITE_INTERVAL 分支），
            // 防止文件被意外清空/删除后 WebUI 读不到当前状态（清空原因多非人为，但重写兜底人为误删）。
            let mode_file_path = root.join("current_mode.chr");
            const MODE_FILE_REWRITE_INTERVAL: Duration = Duration::from_secs(5);
            let mut last_mode_file_write = Instant::now();
            // 停摆期心跳间隔：停摆中本线程不打任何日志，按这个间隔落一条状态行
            // （既是「停摆仍在生效」的证据，也是「采集与日志照常」的证据）。
            // 取 60s 而不是更长：停摆期的日志**完全静止**，用户只能靠这一行区分
            // 「停摆生效中」与「进程已经死了」，间隔太长等于没有。
            const DOWN_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(60);
            let mut last_down_heartbeat = Instant::now();
            // 停摆起始时刻（启动即停摆时就是现在）：心跳里带上已持续分钟数
            let mut halt_since: Option<Instant> = None;
            // [halt] 停摆状态机：进入/退出 DOWN 时做一次性的释放与恢复。
            // 放在调度循环里而不是监听线程里——这些 governor 对象归本线程独占。
            let mut halted = crate::down::is_down();
            // 停摆状态下启动：内存里的当前模式直接是 down，下面写文件、status.csv 的
            // mode 列都用它（rules 里的真实模式不受影响，退出停摆时恢复）
            if halted {
                *mode_clone.lock().unwrap() = crate::down::DOWN_MODE.to_string();
                // **开机就停摆必须显式打点**：down.rs 只在「运行期切换」时打日志，而
                // 启动时 `halted` 初值就是 is_down()、状态机不会走「进入停摆」分支，
                // 于是整份日志里没有任何停摆字样——与「调度线程根本没起来」完全同形，
                // 只能靠推断（旁证是 tweaks 被跳过的那行），必须留痕
                log::warn!("{}", t("down-boot-halted"));
                halt_since = Some(Instant::now());
            }
            // 启动时先写一次初始模式，避免开机后文件缺失/被清空时 WebUI 显示未知状态
            {
                let mode = mode_clone.lock().unwrap().clone();
                let _ = utils::try_write_file(&mode_file_path, mode.as_bytes());
            }

            // let mut fas_controller = crate::scheduler::fas::FasController::new(); // 已由 fas_manager 取代
            let mut cpu_governor = crate::chiri::cpu_load_governor::CpuLoadGovernor::new();
            // 明日方舟特调（akmode）：独立于 CLG 的 4 档齿轮调度器，前台为白名单应用时接管。
            // 传入特调激活共享标志，接管/释放时联动 Monitor 层切换采样间隔
            let ak_governor_flag = ak_active.clone();
            let mut ak_governor = crate::chiri::tuned::TunedGovernor::new(ak_governor_flag);

            // 极速模式（fast）专属锁频器：与 CLG 完全独立，不读 yaml 调频参数，
            // 直接锁所有 cluster 的 min=max=硬件最高频，每 5 秒重写防止外部篡改。
            let mut fast_lock = crate::chiri::fast::FastLock::new();

            // governor/GPU 接管层（contingency/babel 用；FAS 的 governor 在 FasManager 内部）。
            // GpuGuard::new 启动探测一次 devfreq 节点并缓存结果。
            let mut governor_guard = governor::GovernorGuard::new();
            let mut gpu_guard = gpu::GpuGuard::new();

            // FAS 实例管理器：温度源独立探测（与下方 thermal 的 temp_sensor_path 分开，语义不同）。
            // 温度看电池不看处理器：电池温度是热安全边界，处理器长期 95℃ 属正常工作区；
            // 无电池温度节点传 None，FAS 内部限温关闭，不影响其他功能
            // 电池温度刻度预识别（一次）：`/sys/class/power_supply/battery/temp`
            // 的单位因内核/厂商而异（0.1°C / 毫摄氏度 / 直读 °C），结果全局缓存，
            // CLG 热保护与下方 FAS 温度护栏共用同一结论，杜绝两处口径漂移。
            // 探测不出（节点缺失/未初始化）时打 debug，读取侧会退化为仅 CPU 温度
            match crate::utils::battery_temp_scale() {
                Some(scale) => log::info!(
                    "{}",
                    t_with_args(
                        "battery-temp-scale",
                        &fluent_args!(
                            "unit" => scale.label().to_string(),
                            "divisor" => scale.divisor().to_string()
                        )
                    )
                ),
                None => log::debug!("{}", t("battery-temp-scale-unknown")),
            }
            // FAS 温度源只记节点路径，除数每次刷新取全局预识别结论（勿在此固化）
            let fas_temp_path =
                crate::utils::find_battery_temp_path().map(std::path::PathBuf::from);
            let mut fas_mgr = fas_manager::FasManager::new(fas_temp_path, fas_active.clone());

            // CPU 亲和与线程迁移控制器 + core_ctl 核心在线接管（ChiRi 专属）
            let mut affinity_mgr = affinity::AffinityManager::new(sys_path_exist.clone());
            let mut corectl_mgr = core_ctl::CoreCtlManager::new();

            // 最近一次 eBPF 扩展探针统计（BpfStats 事件 2s 一次增量），随遥测 CSV 落盘
            let mut last_bpf_stats: (u32, u32, u32) = (0, 0, 0);
            // 遥测 CSV 落盘计时（1s 精度）与 debug 摘要计数（每 20 行 = 20s 一条）
            let mut last_telemetry_log = Instant::now();
            let mut telemetry_log_counter: u32 = 0;
            // 上一次「PowerAVG 取样被跳过」的原因：只在原因变化时打点，避免每秒刷屏
            let mut sample_skip_reason = String::new();

            let mut is_screen_on = true; // 屏幕状态标记
            // 息屏计时：屏幕熄灭时记录，超过 scene_mode_delay_secs 后切到 scenemode 低功耗
            let mut screen_off_at: Option<Instant> = None;
            // 是否已进入 scenemode（一次性切换，亮屏/模式变更时复位）
            let mut scene_mode_active = false;
            // 特调模式冷却：init_policies 因配置缺失/硬件不支持失败后，5 分钟内不再触发
            const TUNED_COOLDOWN: Duration = Duration::from_secs(300);
            let mut tuned_cooldown_until: Option<Instant> = None;

            // FAS 初始化失败冷却：load_policies 无可用 policy 后 5 分钟内不再触发，CLG default 接管
            const FAS_COOLDOWN: Duration = Duration::from_secs(300);
            let mut fas_cooldown_until: Option<Instant> = None;

            // scenemode 饱和退出冷却：常驻簇（大核簇）util 持续顶满上限退回
            // reduce 后，300s 内不得重新进入 scenemode（防止与后台负载反复拉锯）
            let mut scenemode_cooldown_until: Option<Instant> = None;
            // scenemode 饱和计时起点（常驻大核簇 max_util 连续超阈值的窗口起点）
            let mut scenemode_sat_since: Option<Instant> = None;

            // scenemode 负载门槛打点去重：持续被拒期间只记一条 scene_hold
            let mut scene_hold_logged = false;

            // 热保护：启动时探测一次温度传感器（CPU + 电池），缺失的参考静默降级；
            // 事件循环内每 2s 采样，按 config.thermal 阈值计算压制上限下发给 CLG
            let temp_sensor_path = crate::utils::find_cpu_temp_path().ok();
            if temp_sensor_path.is_none() {
                log::debug!("{}", t("clg-thermal-no-sensor"));
            }
            let batt_sensor_exists = crate::utils::find_battery_temp_path().is_some();
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

            let get_clg_cfg = |config: &Config, mode: &str| -> crate::chiri::config::CpuLoadGovernorConfig {
                config.get_mode(mode).map(|m| m.cpu_load_governor.clone()).unwrap_or_else(|| {
                    // 未知/空模式名：不意外启用 CLG，避免用默认参数接管 CPU
                    let mut cfg = crate::chiri::config::CpuLoadGovernorConfig::default();
                    cfg.enabled = false;
                    cfg
                })
            };

            // 启动时初始化
            {
                // 强制上线全部核心：上次运行可能在 scenemode 中途被杀，残留的
                // 离线核会让 cpufreq policy 目录消失，下方 CLG/fast_lock 初始化
                // 枚举不到对应集群（该簇永久失去 worker）。必须先于任何接管。
                // **停摆期不做**：它写 `cpuN/online`，是最直接的调度干预；停摆时下面
                // 的 CLG/fast_lock 初始化本来就不执行，这一步在停摆期没有存在理由
                if !halted {
                    corectl_mgr.force_online_all();
                }
                // 调速器残留清理：SIGKILL 等异常退出会把 performance 留在节点上
                // （FAS/contingency 的收尾 release 不会执行），启动时一律恢复 schedutil。
                // **停摆期改成只报不写**——理由见 `GovernorGuard::report_residue`：
                // 停摆要记录的是系统原状，而「残留还是厂商默认」进程无从区分
                if halted {
                    crate::chiri::governor::GovernorGuard::report_residue();
                } else {
                    crate::chiri::governor::GovernorGuard::cleanup_residue();
                }
                let current_mode = mode_clone.lock().unwrap().clone();
                // 启动接管（极速锁频 / CLG）：**停摆期一律不做**。
                // 此前只靠「停摆时内存模式是 down → 既不等于 vector、get_clg_cfg 又
                // 返回 enabled=false」间接跳过——这是个脆弱不变量：feature.yaml 里
                // 一旦出现名为 `down` 的模式段，停摆期开机就会直接锁频/接管 CLG。
                // 显式门控，不再依赖模式名恰好不命中。
                if !halted {
                    if current_mode == "vector" {
                        fast_lock.init();
                    } else if current_mode != "fas" {
                        let config_lock = config_clone.read().unwrap();
                        let clg_cfg = get_clg_cfg(&config_lock, &current_mode);
                        if clg_cfg.enabled {
                            cpu_governor.init_policies(&clg_cfg);
                            log::info!("{}", t_with_args("scheduler-clg-init", &fluent_args!("mode" => current_mode.clone())));
                        }
                    }
                }
                // 开发记录开关初始同步：**与停摆无关**（停摆只停调度，采集照常），
                // 必须先于下面的 halted 门控——停摆启动时若跳过，devimp 会用 logger
                // 的默认值，与 meta.dev_record 配置不一致
                {
                    let cfg = config_clone.read().unwrap();
                    crate::logger::set_devimp_active(cfg.meta.dev_record);
                }
                crate::logger::set_devimp_mode(&current_mode);
                // 启动即按初始模式应用亲和布局与 core_ctl 在线策略。
                // **停摆启动时跳过**：DOWN 的语义是「调度不工作、一切交回系统」，此时
                // 接管亲和布局等于停摆期还在写 cpuset；而 `halted` 初值就是 is_down()，
                // 循环里那次「进入停摆」分支永远不会执行，没有任何地方会把它收回去。
                if !halted {
                    let cfg = config_clone.read().unwrap();
                    sync_lab_governor_gpu(&current_mode, &mut governor_guard, &mut gpu_guard, &mut fast_lock);
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
            // FAS 延迟退出期间记住的目标模式（fas → X 的 ModeChange 被延迟时记录），
            // 超时退出完成后按它重新接管（1s tick 巡检消费）
            let mut pending_mode_after_fas: Option<String> = None;
            // 最近一次 SystemLoadUpdate 的逐核 util 快照（按核亲和选核输入）
            let mut last_core_utils: Vec<f32> = Vec::new();
            // 温度缓存（1s snap 块读取一次，status/devimp/thermal 三处共用）：
            // 避免电池/CPU 温度文件每秒被读 2 遍（thermal 2s 块复用 ≤1s 旧值，
            // 温度变化秒级，对带回滞的判定无影响）
            let mut last_batt_temp: Option<f32> = None;
            let mut last_cpu_temp: Option<f32> = None;
            // 温度滤波：MTK soc_max 合成温区跳变极大（实测 2s ±25°C），不滤波
            // 会让热保护 cap 以 ~8s 周期在 40/70/100 间震荡，高负载游戏周期性卡顿
            let mut batt_filter = TempFilter::new(-10.0, 70.0, 10.0, 1.0);
            let mut cpu_filter = TempFilter::new(5.0, 110.0, 12.0, 3.0);
            // panic 自愈：事件循环 panic 被捕获后不退出线程，而是清理到安全态并
            // 重新进入事件循环（退避 + 连续崩溃上限）；仅 channel 关闭才正常退出。
            // 此前 catch_unwind 捕获后直接收尾退出，进程存活但调度永久死亡。
            // [evt_loop] 
            let mut ipc_restart_count: u32 = 0;
            loop {
            let loop_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            loop {
                // ── DOWN 停摆 ──
                // 状态机：进入时释放全部接管并把 current_mode.chr 写成 down，
                // 退出时删掉那个文件，让下一次 ModeChange 或 5s 自愈把真实模式写回去。
                let down = crate::down::is_down();
                if down != halted {
                    if down {
                        // 先记下进入停摆前的**实际**模式：`down_resume_mode` 初值是启动时
                        // 那份 rules 缺省值，此后可能已被 app_modes / 特调 / FAS 改掉，
                        // 退出时要按真实值重建，不能拿启动快照顶替
                        down_resume_mode = mode_clone.lock().unwrap().clone();
                        // FAS 延迟退出的目标模式随之失效（DOWN 退出按 down_resume_mode 接管）
                        pending_mode_after_fas = None;
                        halt_since = Some(Instant::now());
                        // 顺序：先把频率与布局交回系统（各 release 会恢复自己的快照），
                        // 再写状态文件——反了会有一瞬间「既停摆又还持有着」
                        cpu_governor.release();
                        ak_governor.release();
                        fast_lock.release();
                        fas_mgr.deactivate_all();
                        affinity_mgr.release();
                        affinity_mgr.lab_static_deactivate();
                        governor_guard.release();
                        gpu_guard.release();
                        corectl_mgr.set_power_state(false, false);
                        // 一次性系统调整（cpuidle / IO / cpu_boost 输入升频 / 内核 sched 参数）
                        // 也是「调度干预」，一并按快照还原：它们不被任何 governor 持有，
                        // release 清单碰不到，不还原就残留整段停摆期——记录到的基线
                        // 是被 ChiRi 改过的系统，而不是系统自身。
                        CpuScheduler::restore_system_tweaks();
                        *mode_clone.lock().unwrap() = crate::down::DOWN_MODE.to_string();
                        crate::logger::set_devimp_mode(crate::down::DOWN_MODE);
                        let _ = utils::try_write_file(
                            &mode_file_path,
                            crate::down::DOWN_MODE.as_bytes(),
                        );
                        log::warn!("{}", t("scheduler-down-enter"));
                    } else {
                        // 恢复目标模式：优先用 monitor 的**实时判定**而不是停摆前的快照——
                        // 停摆期间前台变化的事件被本线程丢弃且不补发（monitor 照常判定），
                        // 停摆前是 fas/特调时若沿用旧快照，退出后会因「模式没变 → 不发
                        // ModeChange」而一直空窗；实时值把「前台已变/未变」两种情况一起解决
                        // （快照仅在 monitor 尚未判定/刚亮屏清空时兜底）。
                        halt_since = None;
                        let live_mode = crate::monitor::app_detect::last_determined_mode();
                        let resume_mode = if live_mode.is_empty() {
                            down_resume_mode.clone()
                        } else {
                            live_mode
                        };
                        *mode_clone.lock().unwrap() = resume_mode.clone();
                        crate::logger::set_devimp_mode(&resume_mode);
                        let _ = std::fs::remove_file(&mode_file_path);
                        // 停摆期 governors 全被 release 过，退出时**必须按恢复出来的模式重新
                        // 接管**：只复位内存模式不够——ModeChange 只在「模式真的变了」时才重
                        // 接管，前台应用没换就会一直空着（全是释放态，却不会有任何异常提示）。
                        // 口径与 panic 自愈后的重建一致；fas/特调不再「交给后续事件」而在这里
                        // 直接重建——事件不会再来（见上），它们恰恰是最需要重建的两个。
                        {
                            let mode = resume_mode.clone();
                            let cfg = config_clone.read().unwrap();
                            let pkg = crate::monitor::app_detect::get_current_package();
                            let pid = crate::monitor::app_detect::get_current_pid();
                            if mode == "vector" {
                                fast_lock.init();
                            } else if mode == "fas" {
                                // 与 ModeChange 的 fas 分支同款重建：三 governor 互斥释放
                                // → activate（内部复查白名单，竞态下前台刚变即拒绝）→
                                // 亲和 hook；失败回退 CLG default 并进入冷却
                                ak_governor.release();
                                fast_lock.release();
                                cpu_governor.release();
                                if !pkg.is_empty() && fas_mgr.activate(&pkg, pid) {
                                    fas_affinity_hook(&mut affinity_mgr, &mut corectl_mgr, true, pid);
                                } else {
                                    fas_mgr.deactivate_all();
                                    fas_cooldown_until = Some(Instant::now() + FAS_COOLDOWN);
                                    log::warn!(
                                        "{}",
                                        t_with_args("scheduler-fas-init-failed", &fluent_args!("pkg" => pkg.clone()))
                                    );
                                    let clg_cfg = get_clg_cfg(&cfg, "default");
                                    if clg_cfg.enabled {
                                        cpu_governor.init_policies(&clg_cfg);
                                    }
                                }
                            } else if crate::common::is_special_mode(&mode) {
                                // 特调：按实时包名复查白名单（竞态下前台刚变则拒绝接管，
                                // 与事件路径同语义），接管失败进冷却
                                if crate::common::is_special_mode_allowed(&pkg, &mode)
                                    && !ak_governor.init_policies(&mode, &cfg.get_tuned_profile(&mode))
                                {
                                    tuned_cooldown_until = Some(Instant::now() + TUNED_COOLDOWN);
                                    log::warn!(
                                        "{}",
                                        t_with_args("scheduler-tuned-cooldown", &fluent_args!("secs" => TUNED_COOLDOWN.as_secs().to_string()))
                                    );
                                }
                            } else {
                                let clg_cfg = get_clg_cfg(&cfg, &mode);
                                if clg_cfg.enabled {
                                    cpu_governor.init_policies(&clg_cfg);
                                }
                            }
                            // lab 模式的 governor/GPU 同步（退出停摆若回到 contingency/babel）
                            sync_lab_governor_gpu(&mode, &mut governor_guard, &mut gpu_guard, &mut fast_lock);
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
                        // 停摆期进 DOWN 时还原过的一次性系统调整在这里补发回来
                        // （快照已在进入时清空，本次下发会重新记一份用于下次还原）
                        if let Err(e) =
                            CpuScheduler::new(config_clone.clone(), tweaks_sys_path.clone())
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
                        // 停摆期负载事件被丢弃、last_load_event 停在进入之前，不重置会让
                        // 「负载源超时」看门狗把刚重建的 CLG / vector 立刻判 stale 释放
                        last_load_event = Instant::now();
                        log::info!("{}", t("scheduler-down-exit"));
                    }
                    halted = down;
                }

                // 当前模式文件自愈：每 5 秒重写一次（即便内容未变也重写），
                // 保证被外部清空/删除后 WebUI 最多 5 秒恢复读取当前状态。
                // 停摆期间只推进计时、不落盘——里面是 down，覆盖它等于退出停摆
                if last_mode_file_write.elapsed() >= MODE_FILE_REWRITE_INTERVAL {
                    last_mode_file_write = Instant::now();
                    if !halted {
                        let mode = mode_clone.lock().unwrap().clone();
                        let _ = utils::try_write_file(&mode_file_path, mode.as_bytes());
                    }
                    // watchdog.pid 自愈：logs/ 被外部删除后该文件随目录消失，
                    // WebUI stopScheduler 将无法终止看门狗（旧看门狗残留会把
                    // daemon 再拉起，与重启实例形成双实例并行写两份日志）
                    crate::logger::ensure_watchdog_pid_file();
                }

                // 停摆期心跳（5 分钟一条）：停摆中调度动作全停，上面那些周期块也大多
                // 被门控跳过，日志会长时间静默——没有这条就分不清「停摆生效中」和
                // 「调度线程已死」。采集（status.csv/devimp/心跳文件）由各自的周期块
                // 与独立线程维持，与本心跳无关。
                if halted && last_down_heartbeat.elapsed() >= DOWN_HEARTBEAT_INTERVAL {
                    last_down_heartbeat = Instant::now();
                    let mins = halt_since
                        .map(|t| t.elapsed().as_secs() / 60)
                        .unwrap_or(0);
                    log::info!(
                        "{}",
                        t_with_args("down-heartbeat", &fluent_args!("mins" => mins.to_string()))
                    );
                }

                // config.yaml 热重载联动：config_watcher 成功重载后置位。
                // 与 ConfigReload（rules.yaml）同口径：亮屏时按当前模式把新配置应用到
                // 运行中的 CLG/akmode，并刷新亲和/core_ctl（息屏不覆盖 Doze，亮屏事件补上）。
                // 停摆期间配置照重载（meta 的日志开关等仍要生效），只是不下发到调度器
                let config_dirty = dirty_ipc.swap(false, Ordering::AcqRel);
                if config_dirty {
                    // devimp 采集开关与停摆无关（停摆只停调度）：无条件同步，
                    // 保证停摆期间改 meta.dev_record 也即时生效（与 down.rs
                    // 「采集照常」的语义一致）
                    let cfg = config_clone.read().unwrap();
                    crate::logger::set_devimp_active(cfg.meta.dev_record);
                }
                if config_dirty && !halted && is_screen_on {
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
                        // 特调运行中：按新配置重载 akmode
                        if ak_governor.is_active() {
                            let ak_cfg = config_lock.get_tuned_profile(&current_mode);
                            ak_governor.reload_config(&ak_cfg);
                        }
                    } else if current_mode == "vector" {
                        // fast_lock 不读 yaml 调参，仅需确保 CLG 未意外持有
                        if cpu_governor.is_active() { cpu_governor.release(); }
                    } else if current_mode != "fas" {
                        // fas 模式下 CLG fallback 不参与热重载（FAS 配置编译期嵌入静态）
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
                if !halted && last_thermal_check.elapsed() >= THERMAL_CHECK_INTERVAL {
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
                // logs/status.csv（替代原 telemetry.log + power.log 两条流）。
                // telemetry 线程 1s 刷新共享原子量；OPlus 机型功耗优先走 bcc_parms
                // 私有节点（标准 power_supply 节点约 10s 才刷新，1s 采样必须绕开）。
                if last_telemetry_log.elapsed() >= TELEMETRY_LOG_INTERVAL {
                    last_telemetry_log = Instant::now();
                    let tm = crate::monitor::telemetry::telemetry();
                    let fmt_opt = |v: Option<f32>, digits: usize| {
                        v.map(|x| format!("{:.*}", digits, x))
                            .unwrap_or_else(|| "-".to_string())
                    };
                    // status.csv 列全精度（2026-09-18 用户口径：文件保留全部小数位，
                    // 取整交给显示层——WebUI 状态快照统一 toFixed(1)）；fmt_opt 的
                    // 固定位数留给 devimp 与调试打点
                    let fmt_opt_full = |v: Option<f32>| {
                        v.map(|x| x.to_string())
                            .unwrap_or_else(|| "-".to_string())
                    };
                    let current_mode = mode_clone.lock().unwrap().clone();
                    // 温度本秒读取一次，status/devimp/thermal（2s）三处共用；
                    // 滤波后的值参与热判定，原始值不再直供任何控制路径
                    last_batt_temp = if batt_sensor_exists {
                        crate::utils::read_battery_temp_celsius().and_then(|t| batt_filter.push(t))
                    } else {
                        None
                    };
                    last_cpu_temp = temp_sensor_path
                        .as_ref()
                        .and_then(|p| crate::utils::read_f64_from_file(p).ok())
                        .map(|v| (v / 1000.0) as f32)
                        .and_then(|t| cpu_filter.push(t));
                    // 前台包名实时取自 app_detect（含同模式切换；ModeChange 仅在
                    // 模式变化时才有事件，用事件维护会写过期包名）
                    let fg_package = crate::monitor::app_detect::get_current_package();
                    // devimp 按前台包名分组：包名变化即切换新诊断文件（内部去重；
                    // 空包名不切换，避免启动初期/瞬时空读反复开文件）
                    crate::logger::set_devimp_package(&fg_package);
                    // 充放电状态：1s 一次读 status 节点（电流符号厂商方向不一，不可靠）
                    let charge_state = read_battery_charge_state();
                    // fps 预留列：仅 FAS 激活时取活跃实例的窗口均值，
                    // 其余（FAS 未启动/后台保留实例/窗口无样本）为 None → 写 "-"
                    let fas_fps = fas_mgr.current_fps();
                    crate::logger::status_log_snapshot(
                        &current_mode,
                        &fg_package,
                        &charge_state,
                        is_screen_on,
                        last_batt_temp,
                        last_cpu_temp,
                        &format!("{}", thermal_cap_current * 100.0),
                        &format!("{}", thermal_free_current * 100.0),
                        cpu_governor.is_active(),
                        &fmt_opt_full(Some(tm.psi_cpu_some())),
                        &fmt_opt_full(Some(tm.psi_io_some())),
                        &fmt_opt_full(Some(tm.psi_mem_some())),
                        &fmt_opt_full(tm.gpu_busy()),
                        &fmt_opt_full(tm.batt_voltage_v()),
                        // 全精度：电流值的量级由校准值决定（可能是 0.5 这种小数量级），
                        // 取整会把真值抹平，归档里没法反推单位
                        &fmt_opt_full(tm.batt_current_ma()),
                        &fmt_opt_full(tm.batt_power_w()),
                        last_bpf_stats.0,
                        last_bpf_stats.1,
                        last_bpf_stats.2,
                        fas_fps,
                    );
                    // PowerAVG（耗电参考/平均）：紧跟 status 行写入之后**顺序**计算并
                    // 写 PowerAVG.chr（用户口径：计算值直接加在 csv 后，不并行处理）。
                    // 取样口径三条（按用户要求逐次收窄）：
                    // ① **仅电池放电**：插电/充满/未充电时功率由充电链路决定，而
                    //    batt_power_w 取的是电流绝对值，计入会把充电功率混进耗电均值；
                    // ② **平均模式再排除息屏**：息屏功耗低但占时长大（常整夜），等权全史
                    //    会把「亮屏放电」读数整体拉低——均值只统计亮屏放电样本；
                    // ③ **参考值保留息屏**：它是偏历史的滑动参考，要反映整段放电过程。
                    // 判定式记法：`!use_average || is_screen_on` —— 平均模式要求亮屏，
                    // 参考值不看屏幕。展开：平均 = 放电 && 亮屏，参考 = 放电（含息屏）。
                    // 2026-09-18 修正：此式原先写作 `use_average || is_screen_on`，两个
                    // 模式正好互换（平均收了息屏、参考丢了息屏），与 ②③ 相反。屏态来自
                    // 屏幕检测仲裁（息屏要两个有效节点投票确认，见 monitor::screen_detect）：
                    // 仲裁判 OFF 的时长直接决定平均值的样本多少——误判息屏只是少收样本，
                    // 误判断亮屏会把息屏样本混进平均值。
                    // 不满足条件时传 None → 递推整体跳过（不推进留存次数、不改文件）。
                    // 口径开关由 meta.power_avg 决定（热重载即时生效）
                    let (use_average, notify_on) = {
                        let cfg = config_clone.read().unwrap();
                        (cfg.meta.power_avg, cfg.meta.notify)
                    };
                    let sample_ok =
                        charge_state == "discharging" && (!use_average || is_screen_on);
                    let power_reading = if sample_ok { tm.batt_power_w() } else { None };
                    // 跳过时 PowerAVG.chr 不更新（界面只看得到「文件为空」）——把原因说清：
                    // 同一原因只报一次，真正写入样本后重新武装（成功时清空 last_skip）
                    let skip_reason = if !sample_ok {
                        if charge_state != "discharging" {
                            Some(format!("not-discharging({charge_state})"))
                        } else {
                            Some("screen-on".to_string())
                        }
                    } else if power_reading.is_none() {
                        Some("no-reading".to_string())
                    } else {
                        None
                    };
                    match skip_reason {
                        Some(reason) => {
                            if sample_skip_reason != reason {
                                log::info!(
                                    "{}",
                                    crate::i18n::t_with_args(
                                        "power-avg-skip",
                                        &crate::fluent_args!("reason" => reason.clone())
                                    )
                                );
                                sample_skip_reason = reason;
                            }
                        }
                        None => sample_skip_reason.clear(),
                    }
                    let power_now = crate::logger::power_avg_update(power_reading, use_average);
                    // 首轮标记：必须在自增**之前**取——第一 tick 时计数器还是 0。此前
                    // 把 `telemetry_log_counter == 0` 放在自增之后判断，恒为 false，
                    // 启动首轮清残留通知那条永远不会执行
                    let first_tick = telemetry_log_counter == 0;
                    telemetry_log_counter += 1;
                    // 常驻状态通知：每 5s 更新一次（内容不变不重投；内部自带失败候选与
                    // 告警去重）。标题 = 前台包名，正文 = 模式/家族/子模式/温度/功耗
                    // ——功耗口径随 meta.power_avg（电池读数页里的开关）。
                    // 这里只组装并**非阻塞投递**到 notify 线程（拿不到 `cmd` 的进程创建
                    // 与等待，调度循环照常跑）
                    if !notify_on {
                        // 开关关闭（meta.yaml `notify`）：撤销已投递的通知；首轮无条件，
                        // 清掉上一次运行残留的那条（此后每 tick 调用都幂等）
                        crate::notify::cancel(first_tick);
                    } else if telemetry_log_counter % crate::notify::INTERVAL_SECS == 0 {
                        crate::notify::update(&crate::notify::Snapshot {
                            pkg: &fg_package,
                            mode: &current_mode,
                            batt_temp: last_batt_temp,
                            cpu_temp: last_cpu_temp,
                            power_w: power_now,
                        });
                    }
                    // 开发记录 snap 行（1s）：环境上下文（开启 dev_record 才有 IO；
                    // 前台包名由 set_devimp_package 已同步，行内自动填充）
                    if crate::logger::devimp_active() {
                        // 频率/调速器快照只在 devimp 开启时采集（常态零开销）；
                        // 不缓存——内核与其它进程随时会改这些值。
                        // CPU 每秒采（cpufreq 节点读取廉价）；GPU 受 `GPU_SNAPSHOT_ENABLED`
                        // 总开关控制（当前关闭，见其注释）、开启时再叠加 `gpu_snapshot_due()`
                        // 的 10s 节流。关闭期间这 4 列恒为 "-"。
                        let (cpu_cur, cpu_max, cpu_min, cpu_gov) = crate::chiri::cpu_freq_snapshot();
                        let (gpu_cur, gpu_max, gpu_min, gpu_gov) = if GPU_SNAPSHOT_ENABLED
                            && gpu_snapshot_due()
                        {
                            crate::chiri::gpu::devfreq_snapshot()
                        } else {
                            ("-".to_string(), "-".to_string(), "-".to_string(), "-".to_string())
                        };
                        crate::logger::devimp_snap(
                            is_screen_on,
                            &fmt_opt(last_batt_temp, 1),
                            &fmt_opt(last_cpu_temp, 1),
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
                            &cpu_cur,
                            &cpu_max,
                            &cpu_min,
                            &cpu_gov,
                            &gpu_cur,
                            &gpu_max,
                            &gpu_min,
                            &gpu_gov,
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

                    // FAS 延迟退出巡检（1s）：到期完成退出（频率 + governor 已恢复），
                    // 按离开 FAS 时记住的目标模式重新接管；延迟期内切回白名单应用
                    // 会由 activate 取消延迟（无缝续期），这里 no-op。
                    // 这个分支在停摆期不可达（所以不需要另加 halted 门控）：tick 只在
                    // exit_deadline 到期时返回 true，而进入 DOWN 的 deactivate_all()
                    // 已经把它清空——改这一段时别破坏这个不变量，否则停摆会被 FAS
                    // 的延迟退出重新接管调度
                    if !halted && fas_mgr.tick() {
                        if let Some(mode) = pending_mode_after_fas.take() {
                            *mode_clone.lock().unwrap() = mode.clone();
                            crate::logger::set_devimp_mode(&mode);
                            let _ = utils::try_write_file(&mode_file_path, mode.as_bytes());
                            // governor/GPU：目标若是 contingency/babel 则接管，否则恢复
                            sync_lab_governor_gpu(&mode, &mut governor_guard, &mut gpu_guard, &mut fast_lock);
                            fas_affinity_hook(&mut affinity_mgr, &mut corectl_mgr, false, 0);
                            if mode == "contingency" || mode == "babel" {
                                // 进入 lab 静态模式：先释放全部其它调频接管。CLG/特调的
                                // tick 与防篡改会继续压 scaling_max_freq（大核/prime 锁
                                // 不到最高频），它们的 release 还会把 governor 恢复成
                                // schedutil 覆盖 performance。全部释放（含 Guard 自身复位）
                                // 后再 sync，performance/锁频才是终值。
                                cpu_governor.release();
                                ak_governor.release();
                                fast_lock.release();
                                governor_guard.release();
                                gpu_guard.release();
                                sync_lab_governor_gpu(&mode, &mut governor_guard, &mut gpu_guard, &mut fast_lock);
                                // lab 静态分组与屏幕状态无关：分组重建不门控亮屏
                                let cfg = config_clone.read().unwrap();
                                apply_affinity_and_corectl(
                                    &mut affinity_mgr,
                                    &mut corectl_mgr,
                                    &cfg,
                                    &mode,
                                    is_screen_on,
                                    crate::monitor::app_detect::get_current_pid(),
                                    &last_core_utils,
                                    scene_mode_active,
                                );
                            } else if is_screen_on {
                                apply_mode_takeover(
                                    &mode,
                                    &config_clone.read().unwrap(),
                                    &mut cpu_governor,
                                    &mut ak_governor,
                                    &mut fast_lock,
                                );
                            } else {
                                // 息屏：FAS 退出后交 CLG doze（与 fas_enabled=false 息屏
                                // 分支同款低功耗配置）——否则延迟期结束到亮屏之间零接管
                                let cfg = config_clone.read().unwrap();
                                let mut doze_cfg = get_clg_cfg(&cfg, "reduce");
                                doze_cfg.enabled = true;
                                doze_cfg.perf_floor = 0.0;
                                doze_cfg.perf_ceil = doze_cfg.perf_ceil.min(0.30);
                                doze_cfg.smoothing_up = 0.10;
                                doze_cfg.touch_boost_enabled = false;
                                cpu_governor.init_policies(&doze_cfg);
                            }
                            log::info!("{}", t_with_args(
                                "scheduler-fas-delayed-exit",
                                &fluent_args!("mode" => mode.as_str())
                            ));
                        }
                    }

                    // PackageSwitch 兜底（事件丢失/启动即 fas 自愈）：1s 巡检一次，
                    // 包名实际变化才触发热切换，天然幂等。热切换逻辑与 PackageSwitch
                    // arm 同款内联（借用检查下两处各自持有 governor 可变引用，无法
                    // 提取公共闭包——与文件内既有风格一致）。
                    // FAS 息屏省电已完全移除（2026-09）：原「仅亮屏时执行」门控删除——
                    // FAS 息屏保持接管，兜底激活（失效自愈）全时段生效。app_detect 息屏期
                    // 不更新前台包名，此处比较的是缓存包名，稳态为 no-op。
                    if !halted
                        && mode_clone.lock().unwrap().as_str() == "fas"
                        && !crate::common::fas_enabled()
                    {
                        // fas_enabled=false 热重载生效：立即注销全部 FAS 实例并按屏幕状态
                        // 恢复调度接管。fas_available 已为 false，determine_mode 不再产生
                        // fas 模式，包名/模式切换后自然收尾；无实例且 governor 已接管时本分支为 no-op。
                        let had_instance = fas_mgr.has_any_instance();
                        if had_instance {
                            fas_mgr.deactivate_all();
                            fas_affinity_hook(&mut affinity_mgr, &mut corectl_mgr, false, 0);
                            log::warn!("{}", t("scheduler-fas-switch-off"));
                        }
                        // 恢复接管：有实例被注销，或三 governor 全停（启动残留/冷却期自愈）
                        if had_instance
                            || (!cpu_governor.is_active()
                                && !ak_governor.is_active()
                                && !fast_lock.is_active())
                        {
                            if is_screen_on {
                                // 亮屏：CLG default 回退（与 FAS 初始化失败同口径）
                                let config_lock = config_clone.read().unwrap();
                                let clg_cfg = get_clg_cfg(&config_lock, "default");
                                if clg_cfg.enabled {
                                    cpu_governor.init_policies(&clg_cfg);
                                }
                            } else {
                                // 息屏：交回 CLG doze（与息屏事件同款低功耗配置）
                                ak_governor.release();
                                fast_lock.release();
                                let config_lock = config_clone.read().unwrap();
                                let mut doze_cfg = get_clg_cfg(&config_lock, "reduce");
                                doze_cfg.enabled = true;
                                doze_cfg.perf_floor = 0.0;
                                doze_cfg.perf_ceil = doze_cfg.perf_ceil.min(0.30);
                                doze_cfg.smoothing_up = 0.10;
                                doze_cfg.touch_boost_enabled = false;
                                cpu_governor.init_policies(&doze_cfg);
                            }
                        }
                    } else if !halted && mode_clone.lock().unwrap().as_str() == "fas" {
                        // 取 Arc<str> 快照：只做引用计数递增，不再每次克隆 String
                        let cur_pkg_arc = crate::monitor::app_detect::current_package_arc();
                        let cur_pkg: &str = &cur_pkg_arc;
                        if !cur_pkg.is_empty() {
                            if fas_mgr.is_active() {
                                if let Some(active) = fas_mgr.active_pkg().map(str::to_string) {
                                    // 同包（延迟期内切回同一白名单应用）：续期取消退出
                                    if active == cur_pkg {
                                        fas_mgr.renew_if_same_pkg(&cur_pkg);
                                    }
                                    if active != cur_pkg {
                                        if crate::common::fas_whitelist_entry(&cur_pkg)
                                            .and_then(|cfg| crate::common::fas_app_config(cfg))
                                            .is_some()
                                        {
                                            fas_mgr.deactivate_active();
                                            if !fas_mgr.activate(&cur_pkg, crate::monitor::app_detect::get_current_pid()) {
                                                fas_mgr.deactivate_all();
                                                fas_cooldown_until = Some(Instant::now() + FAS_COOLDOWN);
                                                log::warn!("{}", t_with_args("scheduler-fas-init-failed", &fluent_args!("pkg" => cur_pkg)));
                                                // CLG default 回退
                                                let config_lock = config_clone.read().unwrap();
                                                let clg_cfg = get_clg_cfg(&config_lock, "default");
                                                if clg_cfg.enabled { cpu_governor.init_policies(&clg_cfg); }
                                            } else {
                                                fas_affinity_hook(&mut affinity_mgr, &mut corectl_mgr, true,
                                                    crate::monitor::app_detect::get_current_pid());
                                            }
                                        }
                                        // 非白名单包（不应出现，determine 门控保证）：忽略，
                                        // ModeChange 会在模式变化时退出 fas
                                    }
                                }
                            } else if fas_cooldown_until.map_or(true, |until| Instant::now() >= until) {
                                if crate::common::fas_whitelist_entry(&cur_pkg)
                                    .and_then(|cfg| crate::common::fas_app_config(cfg))
                                    .is_some()
                                {
                                    // FAS 激活优先于 scenemode（2026-09）：scenemode 激活期间
                                    // prime 离线 + 专用小核独占，不允许 FAS 在残缺 CPU 拓扑上
                                    // 接管——先恢复全部在线核再激活（与亮屏恢复「先恢复核再
                                    // init」同序）。守卫维持「FAS 实例存在 ⇒ 非 scenemode」不变量
                                    if scene_mode_active {
                                        scene_mode_active = false;
                                        scene_hold_logged = false;
                                        log::info!("{}", t("scheduler-scene-mode-exit-fas"));
                                        {
                                            let cfg = config_clone.read().unwrap();
                                            let cur_mode = mode_clone.lock().unwrap().clone();
                                            apply_affinity_and_corectl(
                                                &mut affinity_mgr,
                                                &mut corectl_mgr,
                                                &cfg,
                                                &cur_mode,
                                                is_screen_on,
                                                crate::monitor::app_detect::get_current_pid(),
                                                &last_core_utils,
                                                false,
                                            );
                                        }
                                        crate::logger::devimp_event("scene_exit", "-", "fas_activate_preempt");
                                    }
                                    ak_governor.release();
                                    fast_lock.release();
                                    cpu_governor.release();
                                    if !fas_mgr.activate(&cur_pkg, crate::monitor::app_detect::get_current_pid()) {
                                        fas_mgr.deactivate_all();
                                        fas_cooldown_until = Some(Instant::now() + FAS_COOLDOWN);
                                        log::warn!("{}", t_with_args("scheduler-fas-init-failed", &fluent_args!("pkg" => cur_pkg)));
                                        // CLG default 回退
                                        let config_lock = config_clone.read().unwrap();
                                        let clg_cfg = get_clg_cfg(&config_lock, "default");
                                        if clg_cfg.enabled { cpu_governor.init_policies(&clg_cfg); }
                                    } else {
                                        fas_affinity_hook(&mut affinity_mgr, &mut corectl_mgr, true,
                                            crate::monitor::app_detect::get_current_pid());
                                    }
                                } else if !cpu_governor.is_active() && !ak_governor.is_active() && !fast_lock.is_active() {
                                    // 启动残留 fas 模式且前台非 FAS 应用：CLG default 自愈接管
                                    let config_lock = config_clone.read().unwrap();
                                    let clg_cfg = get_clg_cfg(&config_lock, "default");
                                    if clg_cfg.enabled { cpu_governor.init_policies(&clg_cfg); }
                                }
                            }
                        }
                    }
                }

                // 热保护（2s 周期）：电池+CPU 双源取较小值。电池温升慢、反映整机发热；
                // CPU 只在极端热时参与（阈值 75/85°C，内核 95°C 温控才是主力）。
                // 豁免档：当前性能比已高于豁免档时不压制，高负载不挡路。
                if last_thermal_check.elapsed() >= THERMAL_CHECK_INTERVAL {
                    last_thermal_check = Instant::now();
                    // FAS 活跃时整体跳过 ChiRi 热保护：温控移除，一切交由 FAS 引擎
                    // （FasManager 内部独立刷新温度喂给引擎）。cap 冻结在当前值；
                    // 2s 亲和块共用本计时器不受影响，1s 遥测块温度读数照常刷新。
                    // fas 模式但实例未活跃（息屏已释放/冷却/初始化失败）时照常生效，
                    // 否则 CLG doze 期间将失去热保护
                    if !halted
                        && !(mode_clone.lock().unwrap().as_str() == "fas" && fas_mgr.is_active())
                    {
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
                        // 复用 snap 块（1s）缓存的温度，不再重复读文件（≤1s 旧值，
                        // 对带回滞的秒级热判定无影响）
                        let batt_t = last_batt_temp;
                        let cpu_t = last_cpu_temp;
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
                        // 解除方向限速：压制加深立即生效，解除每周期最多 +0.15 逐级恢复。
                        // 立即全量解除会让温度马上反弹再触发深压——cap 以 ~8s 周期在
                        // 40/70/100 间跳变（bang-bang 振荡），每次跳变都把 current_perf
                        // 砸回低位再缓慢爬升，是高负载游戏周期性卡顿的直接来源
                        let new_cap = if new_cap > thermal_cap_current {
                            (thermal_cap_current + THERMAL_UNPRESS_STEP).min(new_cap)
                        } else {
                            new_cap
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
                        // 功耗监控已并入 status.csv snapshot 行（1s），此处不再重复写
                    }
                }

                // 触摸事件（事件驱动）：每次醒来先处理触摸队列。on_touch 更新共享
                // 触摸状态并唤醒全部 Worker 立即 flush 写频，大核 Worker 在本次 flush
                // 中直接应用触摸升频地板，不等待下一个 160ms 负载决策 tick。
                // 窗口内的持续触摸会刷新截止时间（FastWriter 去重重复写频）。
                while touch_rx.try_recv().is_ok() {
                    if !halted && cpu_governor.is_active() {
                        cpu_governor.on_touch();
                        log::debug!("{}", t("touch-event-received"));
                    }
                }

                // 极速模式：每 5 秒重写一次硬件最高频，防止系统/厂商守护进程篡改；
                // 返回距下次重写的剩余时间，纳入动态超时计算
                // 停摆期不碰任何节点：tick 内部虽有 is_active 早退，这里显式断开，
                // 免得将来给 FastLock 加的开关绕过这道防线
                let fast_next = if halted { None } else { fast_lock.tick() };

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
                                        "tuned-watchdog-release",
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
                // 停摆期间事件照收但不下发：调度类动作全停，采集与遥测由上面的周期块
                // 负责（status.csv / devimp / 日志照常）。**屏幕事件例外**——只更新
                // is_screen_on 等本地状态（不做任何调度动作）：这些变量在停摆期若不
                // 吸收变化，退出停摆时 monitor 不会再补发（它自己那份 arc 已是最新），
                // 亲和/core_ctl 会按过期的屏幕状态应用（息屏却按亮屏口径调度）。
                if halted {
                    if let DaemonEvent::ScreenStateChange(on) = &msg {
                        is_screen_on = *on;
                        screen_off_at = if *on { None } else { Some(Instant::now()) };
                        scene_mode_active = false;
                    }
                    continue;
                }
                match msg {
                    // [evt_screen] 
                    // 屏幕状态事件：息屏深度睡眠
                    DaemonEvent::ScreenStateChange(screen_on) => {
                        // 双源事件去重：uevent 线程直推 + app_detect verify 自愈兜底
                        // 都可能上报同一次屏幕切换，状态未变化时只打点不处理
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
                        // 息屏/亮屏触发事件 info 打点：uevent 直推路径绕过了 app_detect 的
                        // info 级变更日志，这里统一保证触发点可见
                        if screen_on {
                            log::info!("{}", t("scheduler-screen-on"));
                        } else {
                            log::info!("{}", t("scheduler-screen-off"));
                        }
                        let current_mode = mode_clone.lock().unwrap().clone();

                        if !is_screen_on {
                            log::info!("{}", t("scheduler-doze-enable"));
                            // 息屏计时起点：超过 scene_mode_delay_secs 后切到 scenemode 低功耗
                            screen_off_at = Some(Instant::now());
                            scene_mode_active = false;

                            // 特调模式下息屏保持 akmode 接管，不切换到 CLG doze：
                            // akmode 已统一为 schedutil，息屏后 schedutil 随负载自然降频省电，
                            // 无需 CLG 介入；避免 release + 亮屏 re-init 的 governor 反复切换。
                            if crate::common::is_special_mode(&current_mode) {
                                // akmode 继续运行，CLG 保持释放状态
                                log::info!("{}", t("scheduler-doze-special-keep"));
                            } else if current_mode == "fas" && fas_mgr.is_active() {
                                // FAS 息屏省电已完全移除（2026-09）：FAS 息屏保持接管，
                                // 不释放实例、不切 CLG doze（与特调息屏保持接管同语义）。
                                // FAS 失效（看门狗释放/初始化冷却）时走下方分支由 doze 接管。
                            } else {
                                // 非特调模式：交回 CLG 处理深度睡眠。
                                // FAS 息屏不再释放（息屏省电已移除）：本分支仅在非 fas 模式
                                // 或 FAS 未活跃（看门狗释放/初始化冷却）时执行。
                                // 旧设计 enter_doze 写低频锁后依赖帧事件恢复——锁屏无帧
                                // 事件时全簇锁死最低频，亮屏 0.2fps，且永久阻塞 CLG 接管。
                                ak_governor.release();
                                fast_lock.release();

                                // 让 CLG 接管，动态生成一个低功耗配置
                                let config_lock = config_clone.read().unwrap();
                                let mut doze_cfg = get_clg_cfg(&config_lock, "reduce");
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
                        } else {
                            log::info!("{}", t("scheduler-doze-restore"));
                            // 亮屏：清空息屏计时与 scenemode 状态（恢复逻辑在下方重放原模式）
                            screen_off_at = None;
                            scene_mode_active = false;
                            scene_hold_logged = false;
                            let config_lock = config_clone.read().unwrap();

                            // 亲和/core_ctl 先恢复：若此前处于 scenemode，prime 仍离线，
                            // 其 cpufreq policy 目录不存在——必须先恢复全部核上线，
                            // 下方 CLG/akmode/fast_lock 的 reload/init 才能枚举到完整
                            // policy 列表（否则 prime 永久失去 worker，上线后残留
                            // 离线前的锁频状态、脱离调度控制）
                            apply_affinity_and_corectl(
                                &mut affinity_mgr,
                                &mut corectl_mgr,
                                &config_lock,
                                &current_mode,
                                true,
                                crate::monitor::app_detect::get_current_pid(),
                                &last_core_utils,
                                scene_mode_active,
                            );

                            if crate::common::is_special_mode(&current_mode) {
                                // 亮屏恢复特调：akmode 息屏期间通常保持接管（息屏分支不释放）；
                                // 但若息屏时负载事件停止触发看门狗释放过 akmode，这里必须重新接管，
                                // 否则特调限频失效、采样间隔也不会切回 40ms。
                                // 冷却期内跳过特调，直接走 CLG。
                                let in_cooldown = tuned_cooldown_until
                                    .map_or(false, |until| Instant::now() < until);
                                if !ak_governor.is_active() && !in_cooldown {
                                    cpu_governor.release();
                                    let ak_cfg = config_lock.get_tuned_profile(&current_mode);
                                    if !ak_governor.init_policies(&current_mode, &ak_cfg) {
                                        // init 失败（配置缺失/硬件不支持）：冷却 5 分钟，CLG 接管
                                        tuned_cooldown_until = Some(Instant::now() + TUNED_COOLDOWN);
                                        log::warn!("{}", t_with_args(
                                            "scheduler-tuned-cooldown",
                                            &fluent_args!("secs" => TUNED_COOLDOWN.as_secs().to_string())
                                        ));
                                        let clg_cfg = get_clg_cfg(&config_lock, &current_mode);
                                        if clg_cfg.enabled { cpu_governor.init_policies(&clg_cfg); }
                                    }
                                } else if !ak_governor.is_active() {
                                    // 冷却中：CLG 接管
                                    let clg_cfg = get_clg_cfg(&config_lock, &current_mode);
                                    if clg_cfg.enabled { cpu_governor.init_policies(&clg_cfg); }
                                }
                            } else if current_mode == "contingency" || current_mode == "babel" {
                                // 亮屏恢复 lab 静态模式：governor/GPU/极速锁频按模式重建。
                                // 不能走下面的通用 else——它会释放 fast_lock 且不给
                                // contingency 任何频率接管（CLG 未注册），锁频就空了
                                cpu_governor.release();
                                ak_governor.release();
                                fast_lock.release();
                                sync_lab_governor_gpu(&current_mode, &mut governor_guard, &mut gpu_guard, &mut fast_lock);
                            } else if current_mode == "vector" {
                                // 亮屏恢复极速模式：释放 doze CLG，由 fast_lock 接管
                                ak_governor.release();
                                cpu_governor.release();
                                fast_lock.init();
                            } else if current_mode == "fas" {
                                // FAS 活跃：保持接管，CLG 绝不能接管 FAS 已接管的 CPU，
                                // 亮屏不做任何恢复（FAS 息屏保持接管，2026-09）。
                                // FAS 失效（初始化冷却/异常）：激活 CLG default 接管——
                                // 冷却期内 1s 兜底被短路（fas_cooldown 门控），此处是
                                // 退出 doze/scenemode 低性能配置的唯一恢复路径；冷却结束
                                // 后由 1s 兜底重新激活 FAS（activate 前先 release CLG）。
                                if !fas_mgr.is_active() {
                                    let clg_cfg = get_clg_cfg(&config_lock, "default");
                                    if clg_cfg.enabled {
                                        if cpu_governor.is_active() { cpu_governor.reload_config(&clg_cfg); }
                                        else { cpu_governor.init_policies(&clg_cfg); }
                                    }
                                    else { cpu_governor.release(); }
                                }
                            } else {
                                ak_governor.release();
                                fast_lock.release();
                                let clg_cfg = get_clg_cfg(&config_lock, &current_mode);
                                if clg_cfg.enabled {
                                    // 息屏 doze 期间 CLG 仍持有 writer，热切换配置即可
                                    if cpu_governor.is_active() { cpu_governor.reload_config(&clg_cfg); }
                                    else { cpu_governor.init_policies(&clg_cfg); }
                                }
                                else { cpu_governor.release(); }
                            }
                        }
                        crate::logger::devimp_event("screen", "-", if is_screen_on { "on" } else { "off" });
                    },

                    // [evt_mode] 
                    // 前台模式切换事件
                    DaemonEvent::ModeChange { package_name, pid, mode, temperature } => {
                        let mut current_mode_lock = mode_clone.lock().unwrap();
                        let old_mode = current_mode_lock.clone();
                        log::debug!("{}", t_with_args("scheduler-event-mode-change", &fluent_args!(
                            "pkg" => package_name.as_str(),
                            "old" => old_mode.clone(),
                            "new" => mode.as_str(),
                            "temp" => temperature
                        )));
                        // 前台切换已改由 app_detect 直写 status.csv fg 行（含同模式切换），
                        // ModeChange 事件仅在模式变化时产生，此处不再重复记录

                        if old_mode != mode {
                            log::info!("{}", t_with_args("scheduler-mode-change-request", &fluent_args!(
                                "old" => old_mode.clone(), "new" => mode.as_str(), "pkg" => package_name.as_str(), "temp" => temperature
                            )));

                            // FAS 延迟退出（15s）：fas → 非 fas 的模式变化不立即切换——
                            // FAS 仍持有接管（mode 保持 fas、频率停在最后状态、governor=
                            // performance），延迟期内切回白名单应用由 activate 无缝续期；
                            // 到期由 1s tick 完成退出并按 pending_mode_after_fas 重新接管。
                            // mode_clone 不动，否则会出现「文件写 default、FAS 还持着
                            // 频率」的中间态。
                            if old_mode == "fas" && mode != "fas" && fas_mgr.is_active() {
                                fas_mgr.request_delayed_exit();
                                pending_mode_after_fas = Some(mode.clone());
                                drop(current_mode_lock);
                                continue;
                            }

                            *current_mode_lock = mode.clone();
                            drop(current_mode_lock);

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

                            // FAS 息屏省电已完全移除（2026-09）：原「息屏进入 fas 不激活
                            // FAS、交 doze 接管」的特殊分支已删除，屏幕状态不再影响 FAS 激活。
                            if mode == "fas" {
                                // 防御复查：determine_mode 已门控白名单，此处兜底（不应发生）
                                let fas_ready = crate::common::fas_whitelist_entry(&package_name)
                                    .and_then(|cfg| crate::common::fas_app_config(cfg))
                                    .is_some();
                                let in_cooldown = fas_cooldown_until.map_or(false, |until| Instant::now() < until);
                                // 防御性去激活（不应有活跃实例，无活跃时为无操作）
                                fas_mgr.deactivate_active();
                                if !fas_ready {
                                    log::warn!(
                                        "{}",
                                        t_with_args("scheduler-fas-init-failed", &fluent_args!("pkg" => package_name.as_str()))
                                    );
                                    // 可能从特调/极速模式切入：先释放独立 governor 再交给 CLG，避免双 governor 并存
                                    ak_governor.release();
                                    fast_lock.release();
                                    cpu_governor.release();
                                    let config_lock = config_clone.read().unwrap();
                                    let clg_cfg = get_clg_cfg(&config_lock, "default");
                                    if clg_cfg.enabled { cpu_governor.init_policies(&clg_cfg); }
                                } else if in_cooldown {
                                    log::warn!(
                                        "{}",
                                        t_with_args("scheduler-fas-cooldown", &fluent_args!("secs" => FAS_COOLDOWN.as_secs().to_string()))
                                    );
                                    ak_governor.release();
                                    fast_lock.release();
                                    cpu_governor.release();
                                    let config_lock = config_clone.read().unwrap();
                                    let clg_cfg = get_clg_cfg(&config_lock, "default");
                                    if clg_cfg.enabled { cpu_governor.init_policies(&clg_cfg); }
                                } else {
                                    // 三 governor 全停（互斥），再激活 FAS
                                    ak_governor.release();
                                    fast_lock.release();
                                    cpu_governor.release();
                                    if !fas_mgr.activate(&package_name, pid) {
                                        fas_mgr.deactivate_all();
                                        fas_cooldown_until = Some(Instant::now() + FAS_COOLDOWN);
                                        log::warn!("{}", t_with_args("scheduler-fas-init-failed", &fluent_args!("pkg" => package_name.as_str())));
                                        let config_lock = config_clone.read().unwrap();
                                        let clg_cfg = get_clg_cfg(&config_lock, "default");
                                        if clg_cfg.enabled { cpu_governor.init_policies(&clg_cfg); }
                                    } else {
                                        fas_affinity_hook(&mut affinity_mgr, &mut corectl_mgr, true, pid);
                                    }
                                }
                            } else {
                                // 退出 FAS：去激活（恢复频率）必须先于任何下一 governor 的 init，保证对方快照真实系统状态
                                fas_mgr.deactivate_active();
                                fas_affinity_hook(&mut affinity_mgr, &mut corectl_mgr, false, 0);

                                // 离开 lab 静态模式：先恢复组级 cpus 与 governor/GPU 快照——
                                // 后续接管方（boost/fas 的 ensure_snapshot、CLG init 等）才能
                                // 快照到真实状态，否则 lab 分组值会被当成「系统原值」固化
                                // （top-app 永久停留大核，重启前无自愈）。
                                if !matches!(mode.as_str(), "contingency" | "babel") {
                                    affinity_mgr.lab_static_deactivate();
                                    governor_guard.release();
                                    gpu_guard.release();
                                    // contingency 的极速锁频也要解除（息屏路径不会走到
                                    // 下面的亮屏接管块，漏了会把锁频残留到亮屏）
                                    fast_lock.release();
                                }

                                // 仅在亮屏时处理调度接管。如果息屏，Doze 配置仍在生效，这里不能覆盖它
                                if is_screen_on {
                                    let config_lock = config_clone.read().unwrap();
                                    if crate::common::is_special_mode(&mode) {
                                        // 进入特调模式：停止 CLG，改由 akmode 独立接管。
                                        // 冷却期内跳过特调，直接走 CLG。
                                        let in_cooldown = tuned_cooldown_until
                                            .map_or(false, |until| Instant::now() < until);
                                        if !in_cooldown {
                                            cpu_governor.release();
                                            let ak_cfg = config_lock.get_tuned_profile(&mode);
                                            if !ak_governor.init_policies(&mode, &ak_cfg) {
                                                // init 失败：冷却 5 分钟，CLG 接管
                                                tuned_cooldown_until = Some(Instant::now() + TUNED_COOLDOWN);
                                                log::warn!("{}", t_with_args(
                                                    "scheduler-tuned-cooldown",
                                                    &fluent_args!("secs" => TUNED_COOLDOWN.as_secs().to_string())
                                                ));
                                                let clg_cfg = get_clg_cfg(&config_lock, &mode);
                                                if clg_cfg.enabled { cpu_governor.init_policies(&clg_cfg); }
                                            }
                                        } else {
                                            // 冷却中：CLG 接管
                                            let clg_cfg = get_clg_cfg(&config_lock, &mode);
                                            if clg_cfg.enabled { cpu_governor.init_policies(&clg_cfg); }
                                        }
                                    } else if mode == "vector" {
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

                                // 进入 lab 静态模式：governor 写 performance / GPU 锁最高频 /
                                // contingency 极速锁频（sync 内做）。必须在旧 governor 释放
                                // 之后——CLG release 会恢复它 init 时快照的 schedutil、先写
                                // 会被覆盖，且 CLG 的 tick/防篡改会继续压 scaling_max_freq
                                // 与锁频打架。亮屏时上面的接管块已释放三者；息屏路径不走
                                // 那里，这里兜底（重复 release 幂等）。
                                if mode == "contingency" || mode == "babel" {
                                    cpu_governor.release();
                                    ak_governor.release();
                                    fast_lock.release();
                                }
                                sync_lab_governor_gpu(&mode, &mut governor_guard, &mut gpu_guard, &mut fast_lock);
                            }
                        }
                    },

                    // [evt_pkg_switch] 
                    // 同模式前台包切换（fas→fas 热切换通路，ChiRi 专属）
                    DaemonEvent::PackageSwitch { package_name, pid } => {
                        let current_mode = mode_clone.lock().unwrap().clone();
                        // 同包去重前先续期：延迟期内同包回前台即时取消退出
                        // （1s 巡检会兜一次，这里把窗口压到 0）
                        if fas_mgr.active_pkg().is_some_and(|a| a == package_name.as_str()) {
                            fas_mgr.renew_if_same_pkg(&package_name);
                        }
                        if current_mode == "fas" && fas_mgr.is_active() {
                            let switched = match fas_mgr.active_pkg() {
                                Some(active) if active != package_name.as_str() => {
                                    if crate::common::fas_whitelist_entry(&package_name)
                                        .and_then(|cfg| crate::common::fas_app_config(cfg))
                                        .is_some()
                                    {
                                        fas_mgr.deactivate_active();
                                        if fas_mgr.activate(&package_name, pid) {
                                            fas_affinity_hook(&mut affinity_mgr, &mut corectl_mgr, true, pid);
                                            true
                                        } else {
                                            false
                                        }
                                    } else {
                                        true // 非白名单包（不应出现，determine 门控保证）：视为已处理，忽略
                                    }
                                }
                                // 同包去重：延迟期内同包回前台也要续期（1s 巡检会兜一次，
                                // 这里即时取消，避免最多 1s 的窗口被 tick 拆掉重建）
                                _ => {
                                    fas_mgr.renew_if_same_pkg(&package_name);
                                    true
                                }
                            };
                            if !switched {
                                fas_mgr.deactivate_all();
                                fas_cooldown_until = Some(Instant::now() + FAS_COOLDOWN);
                                log::warn!("{}", t_with_args("scheduler-fas-init-failed", &fluent_args!("pkg" => package_name.as_str())));
                                let config_lock = config_clone.read().unwrap();
                                let clg_cfg = get_clg_cfg(&config_lock, "default");
                                if clg_cfg.enabled { cpu_governor.init_policies(&clg_cfg); }
                            }
                        }
                    },

                    // [evt_load] 
                    // CPU 负载事件 (eBPF 驱动)
                    DaemonEvent::SystemLoadUpdate { core_utils, foreground_max_util } => {
                        // 刷新看门狗心跳：只要有负载事件到达即视为负载源存活
                        last_load_event = Instant::now();
                        // 逐核 util 快照的赋值延后到下方负载投喂之后：投喂各分支
                        // 只读借用 core_utils，先移动会打断借用；延后即可用**移动**
                        // 替代克隆，省掉每 tick 一次 Vec 分配（40ms 特调下 25 次/s）
                        // 该事件常规 160ms / 特调 40ms 一次，仅在 DEBUG 时输出摘要便于排查。
                        // 字符串构造在宏外会被无条件求值（每 tick 分配一次），
                        // 用 log_enabled! 门控——INFO 级别下零分配。
                        if log::log_enabled!(log::Level::Debug) {
                            log::debug!("{}", t_with_args("scheduler-event-load", &fluent_args!(
                                "cores" => core_utils.iter().map(|u| format!("{:.0}", u * 100.0)).collect::<Vec<_>>().join(",")
                            )));
                        }
                        // 负载投喂优先级链：FAS 活跃时优先投喂（前台最重线程 util + 逐核 util）；
                        // 否则特调（akmode）白名单应用前台时投喂 akmode 做动态限频
                        // （无档位负载直拉：max 随组内负载在 [最低档, 硬件最高] 间连续变化）；
                        // 否则若 CLG 处于活动状态（日常模式或息屏 Doze），投喂 CLG。
                        if fas_mgr.is_active() {
                            fas_mgr.on_load_update(foreground_max_util, &core_utils);
                        } else if ak_governor.is_active() {
                            ak_governor.on_load_update(&core_utils);
                        } else if cpu_governor.is_active() {
                            // Worker 架构：on_load_update 广播给各核心组 Worker，
                            // Worker 线程内自主完成决策 + 写频，无需外部 flush
                            cpu_governor.on_load_update(&core_utils);
                        }

                        // 逐核 util 快照：按核亲和选核打分与 devimp tick 行的输入。
                        // 移动而非克隆（上方投喂只读借用，此后本 tick 不再用 core_utils）
                        last_core_utils = core_utils;

                        // scenemode：息屏超过 scene_mode_delay_secs 后把 CLG 切到低功耗配置
                        // （一次性）。特调模式由 akmode 独立接管不参与；亮屏后恢复原模式。
                        // FAS 优先（2026-09）：任意 FAS 实例存在（活跃或后台保留）即禁止
                        // 进入 scenemode——CLG 绝不能接管 FAS 已接管的 CPU，后台保留实例
                        // 也视为 FAS 存在；保留实例 60s TTL reap 后自动放行。前台不在 FAS
                        // 名单时模式非 fas，CLG 正常走 doze → scenemode。
                        // scenemode 未启用时释放 CLG 回系统默认。
                        // 饱和退出冷却期内不得重进（防止与后台负载反复拉锯）。
                        let scenemode_cooldown_ok = scenemode_cooldown_until
                            .map_or(true, |u| Instant::now() >= u);
                        // scenemode_enabled=false 热重载生效：退出已激活的 scenemode，
                        // 恢复 affinity/core_ctl 快照并交回 CLG doze 低功耗配置
                        if scene_mode_active && !crate::common::scenemode_enabled() {
                            scene_mode_active = false;
                            scene_hold_logged = false;
                            {
                                let cfg = config_clone.read().unwrap();
                                let cur_mode = mode_clone.lock().unwrap().clone();
                                apply_affinity_and_corectl(
                                    &mut affinity_mgr,
                                    &mut corectl_mgr,
                                    &cfg,
                                    &cur_mode,
                                    is_screen_on,
                                    crate::monitor::app_detect::get_current_pid(),
                                    &last_core_utils,
                                    false,
                                );
                            }
                            let config_lock = config_clone.read().unwrap();
                            let mut doze_cfg = get_clg_cfg(&config_lock, "reduce");
                            doze_cfg.enabled = true;
                            doze_cfg.perf_floor = 0.0;
                            doze_cfg.perf_ceil = doze_cfg.perf_ceil.min(0.30);
                            doze_cfg.smoothing_up = 0.10;
                            doze_cfg.touch_boost_enabled = false;
                            if cpu_governor.is_active() {
                                cpu_governor.reload_config(&doze_cfg);
                            } else {
                                cpu_governor.init_policies(&doze_cfg);
                            }
                            log::info!("{}", t("scheduler-scene-mode-exit-switch"));
                            crate::logger::devimp_event("scene_exit", "-", "switch_off");
                        }
                        if !is_screen_on
                            && crate::common::scenemode_enabled()
                            && !scene_mode_active
                            && scenemode_cooldown_ok
                        {
                            // 先用免锁的计时预判（最低 60s），避免息屏期间每个负载 tick 都抢锁
                            let delay_hit = screen_off_at
                                .map_or(false, |off| off.elapsed().as_secs() >= 60);
                            if delay_hit {
                                let current_mode = mode_clone.lock().unwrap().clone();
                                // 特调由 akmode 独立接管不参与；任意 FAS 实例存在（活跃或
                                // 后台保留）不参与——FAS 失效实例 reap 后才允许进入
                                if !crate::common::is_special_mode(&current_mode)
                                    && !fas_mgr.has_any_instance()
                                {
                                    let config_read = config_clone.read().unwrap();
                                    let delay = config_read.scene_mode_delay_secs.max(60);
                                    let scene_cfg =
                                        config_read.scenemode.cpu_load_governor.clone();
                                    drop(config_read);
                                    if screen_off_at
                                        .map_or(false, |off| off.elapsed().as_secs() >= delay)
                                    {
                                        // 负载门槛：与饱和退出共用 SCENEMODE_SAT_UTIL——常驻簇
                                        // 仍被后台负载顶满时进入 scenemode 必然 ~10s 后饱和退出，
                                        // 形成整夜「进→退→300s 冷却」拉锯。跳过本次（不动
                                        // screen_off_at，后续 tick 持续复评），负载回落后自动进入。
                                        let ranges = crate::common::chiri_core_ranges();
                                        let standby_max = ranges
                                            .little
                                            .clone()
                                            .chain(ranges.big.clone())
                                            .filter_map(|c| last_core_utils.get(c).copied())
                                            .fold(0.0_f32, f32::max);
                                        // 长息屏兜底：息屏时长远超进入延迟（≥4×）时不再受负载门槛
                                        // 限制。门槛防的是「进→10s 饱和退出→300s 冷却」拉锯，但
                                        // 实测后台常驻负载会让小核 util 长期停在 60-70%（峰值触顶
                                        // 阈值 0.75），门槛足可把整夜待机永久挡在 scenemode 之外
                                        // ——代价（大核/prime 整夜带电 + 小核上限全开）远大于偶发
                                        // 拉锯。短息屏仍按原门槛防抖。
                                        let long_off = screen_off_at.map_or(false, |off| {
                                            off.elapsed().as_secs() >= delay.saturating_mul(4)
                                        });
                                        if standby_max >= SCENEMODE_SAT_UTIL && !long_off {
                                            if !scene_hold_logged {
                                                scene_hold_logged = true;
                                                crate::logger::devimp_event(
                                                    "scene_hold",
                                                    "-",
                                                    &format!("util={:.0}", standby_max * 100.0),
                                                );
                                            }
                                        } else {
                                            scene_hold_logged = false;
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
                                            // 小核+大核常驻低频（频率上限由 scenemode
                                            // CLG 配置压制）+ prime 下线 + 专用小核独占
                                            // 自钉（cpuset 排除其他进程）
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
                        }

                        // scenemode 饱和退出：**常驻簇（小核 + 大核）** max_util
                        // 持续顶满性能上限 → 退回 reduce（恢复全部在线核）+
                        // 300s 冷却不得重进，防止后台负载压不死常驻核时反复
                        // 拉锯。util 是忙时占比（与频率无关），饱和即真饱和，
                        // 与 perf_ceil 数值无耦合。
                        if scene_mode_active && !is_screen_on {
                            let ranges = crate::common::chiri_core_ranges();
                            let standby_max = ranges
                                .little
                                .clone()
                                .chain(ranges.big.clone())
                                .filter_map(|c| last_core_utils.get(c).copied())
                                .fold(0.0_f32, f32::max);
                            if standby_max >= SCENEMODE_SAT_UTIL {
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
                                    scene_hold_logged = false;
                                    scenemode_cooldown_until =
                                        Some(Instant::now() + SCENEMODE_COOLDOWN);
                                    let current_mode = mode_clone.lock().unwrap().clone();
                                    // 立即恢复全部在线核 + 解除专用核钉定。
                                    // 必须先于 CLG reload：prime 离线期间其 cpufreq policy
                                    // 目录会消失，reload 枚举不到该 policy，prime 将永久
                                    // 失去 worker（上线后残留离线前的锁频状态、脱离调度控制）
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
                                    // 退回 reduce：给后台负载更大余量（核已全部上线，
                                    // 此时枚举 policy 才完整）。reduce CLG 未启用时释放，
                                    // 与 ModeChange 路径口径一致
                                    let ps_cfg = get_clg_cfg(&config_clone.read().unwrap(), "reduce");
                                    if ps_cfg.enabled {
                                        if cpu_governor.is_active() {
                                            cpu_governor.reload_config(&ps_cfg);
                                        } else {
                                            cpu_governor.init_policies(&ps_cfg);
                                        }
                                    } else {
                                        cpu_governor.release();
                                    }
                                    log::info!(
                                        "{}",
                                        t_with_args(
                                            "scheduler-scene-mode-saturation",
                                            &fluent_args!("util" => format!("{:.0}", standby_max * 100.0))
                                        )
                                    );
                                    crate::logger::devimp_event(
                                        "scene_exit",
                                        "-",
                                        "saturation->reduce+300s",
                                    );
                                }
                            } else {
                                scenemode_sat_since = None;
                            }
                        }
                    },

                    // [evt_frame] 
                    // 帧率事件 (eBPF 驱动)
                    DaemonEvent::FrameUpdate { frame_delta_ns } => {
                        // 帧事件喂给 FAS 活跃实例（内部含 3s 温度刷新）
                        // FAS 息屏省电已完全移除（2026-09）：原 `is_screen_on &&` 门控删除——
                        // FAS 息屏保持接管，帧事件照常投喂（息屏无帧，稳态为 no-op）
                        if fas_mgr.is_active() {
                            fas_mgr.on_frame(frame_delta_ns);
                        }
                        if log::log_enabled!(log::Level::Debug) {
                            log::debug!("{}", t_with_args("scheduler-event-frame", &fluent_args!(
                                "delta_ms" => format!("{:.2}", frame_delta_ns as f64 / 1_000_000.0)
                            )));
                        }
                    }

                    // [evt_reload] 
                    // 热重载配置事件
                    DaemonEvent::ConfigReload(_new_rules) => {
                        let current_mode = mode_clone.lock().unwrap().clone();
                        log::debug!("{}", t_with_args("scheduler-event-config-reload", &fluent_args!(
                            "mode" => current_mode.clone(),
                            "screen_on" => is_screen_on.to_string()
                        )));
                        // fas 模式：FAS 配置编译期嵌入静态，rules.yaml 重载不参与
                        if is_screen_on { // 息屏时不要用新配置覆盖 Doze
                            let config_lock = config_clone.read().unwrap();
                            if crate::common::is_special_mode(&current_mode) {
                                // 特调模式：重载 akmode 配置
                                let ak_cfg = config_lock.get_tuned_profile(&current_mode);
                                if ak_governor.is_active() {
                                    ak_governor.reload_config(&ak_cfg);
                                } else {
                                    let in_cooldown = tuned_cooldown_until
                                        .map_or(false, |until| Instant::now() < until);
                                    if !in_cooldown {
                                        if !ak_governor.init_policies(&current_mode, &ak_cfg) {
                                            tuned_cooldown_until = Some(Instant::now() + TUNED_COOLDOWN);
                                            log::warn!("{}", t_with_args(
                                                "scheduler-tuned-cooldown",
                                                &fluent_args!("secs" => TUNED_COOLDOWN.as_secs().to_string())
                                            ));
                                            let clg_cfg = get_clg_cfg(&config_lock, &current_mode);
                                            if clg_cfg.enabled { cpu_governor.init_policies(&clg_cfg); }
                                        }
                                    } else {
                                        let clg_cfg = get_clg_cfg(&config_lock, &current_mode);
                                        if clg_cfg.enabled { cpu_governor.init_policies(&clg_cfg); }
                                    }
                                }
                            } else if current_mode == "vector" {
                                // 极速模式不读 yaml 参数，ConfigReload 无需处理；
                                // fast_lock 保持活跃，仅确保 CLG 未意外启动
                                if cpu_governor.is_active() { cpu_governor.release(); }
                            } else if current_mode != "fas" {
                                // 非特调、非极速模式：确保 fast_lock 释放，CLG 接管
                                // （fas 模式下 CLG fallback 不参与热重载，FAS 配置编译期嵌入静态）
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

                    // [evt_bpf] 
                    // eBPF 扩展探针统计（ChiRi 专属遥测，2s 一次增量）
                    DaemonEvent::BpfStats { wakeups, migrations, freq_transitions } => {
                        // 仅缓存供遥测 CSV/摘要落盘，不参与调频决策，也不刷新 CLG 看门狗心跳
                        // （探针加载失败时增量为 0，不影响任何控制路径）
                        last_bpf_stats = (wakeups, migrations, freq_transitions);
                    }
                }
            }
            }));
                // [panic_recovery] 
                if loop_result.is_ok() {
                    // channel 关闭：正常退出
                    break;
                }
                log::error!("{}", t("scheduler-ipc-panic"));
                // 清理到安全态（release 幂等；收尾本身也包 catch_unwind，
                // release 路径再 panic 不能击穿重启循环）
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    cpu_governor.release();
                    ak_governor.release();
                    fast_lock.release();
                    fas_mgr.deactivate_all();
                    corectl_mgr.release();
                    affinity_mgr.release();
                }));
                ipc_restart_count += 1;
                if ipc_restart_count > SCHEDULER_IPC_RESTART_MAX {
                    log::error!(
                        "{}",
                        t_with_args(
                            "scheduler-ipc-restart-giveup",
                            &fluent_args!("count" => SCHEDULER_IPC_RESTART_MAX.to_string())
                        )
                    );
                    // 极端兜底：看门狗（service.sh）只监控**进程**存活（前台运行
                    // daemon、退出后 3s 拉起），线程级死亡它永远感知不到——上面
                    // 已把 CPU 控制权清理到安全态，此处退出整个进程交给看门狗
                    // 按进程级拉起全新 daemon（启动初始化含 force_online_all 全核上线）
                    std::process::exit(1);
                }
                // 重置状态机到亮屏安全态：真实屏幕状态由下一个 ScreenStateChange
                // 事件纠正（去重比较 is_screen_on，重置后的首个事件必然被处理）
                is_screen_on = true;
                screen_off_at = None;
                scene_mode_active = false;
                scene_hold_logged = false;
                scenemode_sat_since = None;
                last_core_utils.clear();
                std::thread::sleep(SCHEDULER_IPC_RESTART_BACKOFF);
                // 按当前模式重新接管（等价亮屏恢复语义；特调/fas 由后续事件重建）。
                // **停摆期间必须跳过**：这里重建的是 contingency/babel 分组、vector
                // 锁频与 CLG 接管，没有 `mode` 之外的门控——靠「down 恰好没有 CLG 段」
                // 才没出事，一旦模式停在 lab/vector 就会把刚释放的东西重新接回来
                if !halted {
                    let current_mode = mode_clone
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .clone();
                    if current_mode == "contingency" || current_mode == "babel" {
                        // lab 静态分组：governor/GPU 与分组都重建
                        sync_lab_governor_gpu(&current_mode, &mut governor_guard, &mut gpu_guard, &mut fast_lock);
                        let cfg = config_clone.read().unwrap();
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
                    } else if current_mode == "vector" {
                        // vector 档不能漏：它不注册 CLG（feature.yaml 无 vector 段、
                        // get_mode 返回 None → enabled=false），漏掉就是频率零接管
                        fast_lock.init();
                    } else if current_mode != "fas"
                        && !crate::common::is_special_mode(&current_mode)
                    {
                        let config_lock = config_clone.read().unwrap();
                        let clg_cfg = get_clg_cfg(&config_lock, &current_mode);
                        if clg_cfg.enabled {
                            cpu_governor.init_policies(&clg_cfg);
                        }
                    }
                }
                log::warn!(
                    "{}",
                    t_with_args(
                        "scheduler-ipc-restart",
                        &fluent_args!("count" => ipc_restart_count.to_string())
                    )
                );
            }
            log::warn!("{}", t("scheduler-channel-closed"));
            // 收尾：无论 channel 关闭还是 panic，都恢复 CPU 控制状态，避免频率/governor 残留
            cpu_governor.release();
            ak_governor.release();
            fast_lock.release();
            governor_guard.release();
            gpu_guard.release();
            affinity_mgr.lab_static_deactivate();
            // FAS 全部实例去激活（恢复频率）后清理
            fas_mgr.deactivate_all();
            // 亲和布局与 core_ctl 同步恢复系统原始状态
            corectl_mgr.release();
            affinity_mgr.release();
        })?;

    Ok(())
}
