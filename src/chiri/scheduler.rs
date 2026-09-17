//! scheduler.rs: [tweaks] [cpu_idle] [io] [touch_boost]

use super::config::Config;
use anyhow::Result;
use std::fs;
use std::sync::{Arc, RwLock};

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

    /// 应用所有一次性的、与模式无关的系统调整
    pub fn apply_system_tweaks(&self) -> Result<()> {
        self.apply_cpu_idle_governor()?;
        self.apply_io_settings()?;
        self.apply_disable_touch_boost()?;
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
                let _ = utils::try_write_file(
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
        let items: Vec<(String, String)> = [
            "/sys/module/cpu_boost/parameters/input_boost_enabled",
            "/sys/module/cpu_boost/parameters/sched_boost_on_input",
            "/sys/module/cpu_boost/parameters/input_boost_ms",
            "/sys/module/cpu_boost/parameters/boost_ms",
        ]
        .iter()
        .map(|path| (path.to_string(), "0".to_string()))
        .collect();
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
