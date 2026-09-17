//! scheduler.rs: [sched]

use super::config::Config;
use anyhow::Result;
use std::fs;
use std::sync::{Arc, RwLock};

use crate::i18n::t;
use crate::utils;
use crate::utils::SysPathExist;

// [sched]
// CPU 调度器：一次性系统级调优（cpuidle governor / IO 设置）
pub struct CpuScheduler {
    config: Arc<RwLock<Config>>,
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
        Ok(())
    }

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
        // （见 utils::write_nodes）——块设备/参数节点各机型差异大，不预判 exists，
        // 让真实写入结果说话
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
}
