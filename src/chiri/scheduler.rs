//! scheduler.rs: [tweaks] [snapshot] [sched] [cpu_idle] [io] [touch_boost]

use super::config::Config;
use anyhow::Result;
use std::collections::HashMap;
use std::fs;
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use crate::fluent_args;
use crate::i18n::{t, t_with_args};
use crate::utils;
use crate::utils::SysPathExist;

// [tweaks]
/// 与模式无关的一次性系统设置执行器（cpuidle / IO）。
/// 每次配置热重载后由 config_watcher 线程调用，用于把系统参数对齐到新配置。
pub struct CpuScheduler {
    /// 全局共享配置（main 启动解析一次，config_watcher 热重载时覆盖）
    config: Arc<RwLock<Config>>,
    /// sysfs 路径存在性缓存，避免对同一路径反复探测
    sys_path_exist: Arc<SysPathExist>,
}

impl CpuScheduler {
    pub fn new(config: Arc<RwLock<Config>>, sys_path_exist: Arc<SysPathExist>) -> Self {
        Self {
            config,
            sys_path_exist,
        }
    }

    /// 应用所有一次性的、与模式无关的系统调整。
    /// 停摆期一律跳过（含调用的竞争窗口）：本函数是唯一的入口，
    /// 闸内的那次复查才是权威判定。
    pub fn apply_system_tweaks(&self) -> Result<()> {
        let _gate = Self::tweaks_gate()
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if crate::down::is_down() {
            log::info!("{}", t("system-tweaks-skipped-down"));
            return Ok(());
        }
        self.apply_cpu_idle_governor()?;
        self.apply_io_settings()?;
        self.apply_disable_touch_boost()?;
        self.apply_sched_params()?;
        Ok(())
    }

    // [snapshot]
    /// 一次性系统调整的**原值快照**（节点路径 → 接管前内容）。
    /// 与 governor 的 snapshot/restore 同语义：DOWN 停摆要把系统还原成「ChiRi 没改过」
    /// 的样子，否则被关掉的 cpu_boost、改过的 cpuidle/IO/sched_* 在停摆期依旧生效，
    /// 采集到的就不是「ChiRi 不工作时」的真实基线。**每次节点只记第一次的值**——
    /// 配置热重载会重复下发，覆盖快照会把上一次 tweak 的值当成系统原值。
    fn tweak_snapshot() -> &'static Mutex<HashMap<String, String>> {
        static SNAP: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
        SNAP.get_or_init(|| Mutex::new(HashMap::new()))
    }

    /// 下发与还原的互斥闸：两个调用方在不同线程（`config_watcher` 与调度循环），
    /// 「判完 is_down 就被另一线程切进/切出 DOWN」的窗口会让 tweak 写进停摆期、
    /// 或让还原覆盖掉刚补发的下发。双方都在闸内**再看一次**停摆标志即闭合该窗口。
    fn tweaks_gate() -> &'static Mutex<()> {
        static GATE: OnceLock<Mutex<()>> = OnceLock::new();
        GATE.get_or_init(|| Mutex::new(()))
    }

    /// 节点不存在/不可读时不记录（写也一定失败，没有可还原的东西）。
    /// IO 调度器节点（`*/queue/scheduler`）读出来是候选列表、当前值带方括号，
    /// 写回必须剥壳——原样写 "[mq-deadline] kyber" 是非法值。
    fn snapshot_node(path: &str) {
        let Ok(raw) = fs::read_to_string(path) else {
            return;
        };
        let raw = raw.trim();
        let value = match (raw.find('['), raw.find(']')) {
            (Some(a), Some(b)) if b > a => raw[a + 1..b].trim().to_string(),
            _ => raw.to_string(),
        };
        let mut snap = Self::tweak_snapshot()
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        snap.entry(path.to_string()).or_insert(value);
    }

    /// `write_nodes` 批量写不读原值，写前先补齐快照。
    /// 与 `utils::write_nodes` 同口径泛型：`Vec<(String, String)>` 与 `&[(&str, &str)]`
    /// 都能直接传（这里只取路径，值不参与快照）
    fn snapshot_nodes<P: AsRef<str>, V>(items: &[(P, V)]) {
        for (path, _) in items {
            Self::snapshot_node(path.as_ref());
        }
    }

    /// 单个节点：先快照再写（与 `utils::try_write_file` 同口径，节点缺失降 debug）。
    fn snapshot_write(path: &str, value: &str) -> Result<()> {
        Self::snapshot_node(path);
        utils::try_write_file(path, value)
    }

    /// DOWN 停摆进入时调用：把所有被 ChiRi 改过的节点写回接管前的值并清空快照
    /// （幂等；快照为空什么都不做）。清空后退出停摆的重新下发会再记一份新快照。
    pub fn restore_system_tweaks() {
        let _gate = Self::tweaks_gate()
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let mut snap = Self::tweak_snapshot()
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if snap.is_empty() {
            return;
        }
        let mut restored = 0usize;
        for (path, value) in snap.drain() {
            if utils::try_write_file(&path, &value).is_ok() {
                restored += 1;
            }
        }
        log::info!(
            "{}",
            t_with_args(
                "system-tweaks-restore",
                &fluent_args!("count" => restored.to_string())
            )
        );
    }

    // [sched]
    /// 内核调度器参数写白名单：只允许写这些 /proc/sys/kernel 节点。
    /// 借鉴 LittleYouran CTS 的 Scheduler 段（sched_energy_aware / 迁移成本等），
    /// 白名单外（含用户篡改 feature.yaml 注入）一律拒绝并 warn。
    const SCHED_ALLOWED_PARAMS: &'static [&'static str] = &[
        "sched_migration_cost_ns",
        "sched_nr_migrate",
        "sched_latency_ns",
        "sched_min_granularity_ns",
        "sched_wakeup_granularity_ns",
        "sched_energy_aware",
        "sched_schedstats",
    ];

    /// 写入 Sched 段配置的内核调度器参数：节点写入去重不是本层职责
    /// （FastWriter 面向高频路径，此处热重载频率低、直接写）。逐节点 debug、
    /// 结束 info 汇总实际写入数；空值与越白名单键跳过。
    fn apply_sched_params(&self) -> Result<()> {
        let config = self.config.read().unwrap();
        let sched = &config.sched;
        if !sched.enabled {
            return Ok(());
        }
        let mut applied = 0usize;
        for (key, value) in &sched.params {
            if !Self::SCHED_ALLOWED_PARAMS.contains(&key.as_str()) {
                log::warn!(
                    "{}",
                    t_with_args(
                        "sched-tuning-key-rejected",
                        &fluent_args!("key" => key.as_str())
                    )
                );
                continue;
            }
            if value.trim().is_empty() {
                continue;
            }
            let path = format!("/proc/sys/kernel/{key}");
            let _ = Self::snapshot_write(&path, value);
            applied += 1;
            log::debug!(
                "SchedTuning: wrote {} = {}",
                path,
                value
            );
        }
        log::info!(
            "{}",
            t_with_args(
                "sched-tuning-applied",
                &fluent_args!("count" => applied.to_string())
            )
        );
        Ok(())
    }

    // [cpu_idle]
    /// 写入 cpuidle current_governor。
    /// 仅在 `CpuIdleScalingGovernor` 开关开启、配置了目标 governor 且 sysfs 路径存在时写入。
    fn apply_cpu_idle_governor(&self) -> Result<()> {
        let config = self.config.read().unwrap();
        if config.function.cpu_idle_scaling_governor && !config.cpu_idle.current_governor.is_empty()
        {
            if self.sys_path_exist.cpuidle_governor_exist {
                let _ = Self::snapshot_write(
                    "/sys/devices/system/cpu/cpuidle/current_governor",
                    &config.cpu_idle.current_governor,
                );
                // 仅在真正发起写入时输出"已完成"，避免开关未开启时误报
                log::info!("{}", t("apply-cpu-idle-governor-start"));
            }
        }
        Ok(())
    }

    // [io]
    /// 遍历 /sys/block/*/queue，逐设备写入 IO 优化参数（调度器/预读/合并/统计）。
    /// 开关关闭或 /sys/block 不存在时直接返回；每个参数非空且路径存在才写。
    fn apply_io_settings(&self) -> Result<()> {
        let config = self.config.read().unwrap();
        // 开关关闭时直接返回，不打"已完成"日志，避免误报
        if !config.function.io_optimization {
            return Ok(());
        }

        let io = &config.io_settings;
        let block_dir = std::path::Path::new("/sys/block");
        if !block_dir.exists() {
            log::warn!("IOOptimization: /sys/block does not exist, skipping");
            return Ok(());
        }

        // 逐设备逐参数收集，最后一次批量写：单节点失败 debug、全部失败 warn
        // （见 utils::write_nodes）。不再逐节点预判 exists——块设备/参数节点各机型
        // 差异大，预判会让日志与真实写入结果脱节。
        let mut items: Vec<(String, String)> = Vec::new();
        if let Ok(entries) = fs::read_dir(block_dir) {
            for entry in entries.flatten() {
                let dev_path = entry.path();
                let queue_path = dev_path.join("queue");
                if !queue_path.exists() {
                    continue;
                }
                for (name, value) in [
                    ("scheduler", &io.scheduler),
                    ("read_ahead_kb", &io.read_ahead_kb),
                    ("nomerges", &io.nomerges),
                    ("iostats", &io.iostats),
                ] {
                    if !value.is_empty() {
                        items.push((
                            queue_path.join(name).to_string_lossy().into_owned(),
                            value.clone(),
                        ));
                    }
                }
                log::debug!(
                    "IOOptimization: device queued: {:?}",
                    dev_path.file_name().unwrap_or_default()
                );
            }
        }
        Self::snapshot_nodes(&items);
        let _ = utils::write_nodes(&items, "io-tuning");

        log::info!("{}", t("apply-io-settings-start"));
        Ok(())
    }

    // [touch_boost]
    /// 屏蔽 Android/内核自带的触摸升频（cpu_boost 驱动），改由 ChiRi 触摸升频统一接管：
    /// 关闭 input_boost 与 sched_boost_on_input，避免内核一上一下互抢导致频率抖动。
    /// 候选节点逐条尝试写（不再预判存在性）：单点失败 debug、**全部**失败 warn
    /// （见 utils::write_nodes）——非 ChiRi 通用内核可能一个节点都没有。
    fn apply_disable_touch_boost(&self) -> Result<()> {
        // 路径是字面量，直接用 `(&str, &str)` 数组，省掉逐个 `to_string()` 与 Vec 分配
        let items: [(&str, &str); 4] = [
            ("/sys/module/cpu_boost/parameters/input_boost_enabled", "0"),
            ("/sys/module/cpu_boost/parameters/sched_boost_on_input", "0"),
            ("/sys/module/cpu_boost/parameters/input_boost_ms", "0"),
            ("/sys/module/cpu_boost/parameters/boost_ms", "0"),
        ];
        Self::snapshot_nodes(&items);
        let written = utils::write_nodes(&items, "touch-boost-disable");
        for path in &written {
            log::debug!(
                "{}",
                t_with_args("touch-boost-disable-node", &fluent_args!("path" => path.clone()))
            );
        }
        if !written.is_empty() {
            log::info!("{}", t("touch-boost-disable-applied"));
        }
        Ok(())
    }
}
