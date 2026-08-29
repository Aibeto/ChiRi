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

use super::config::Config;
use anyhow::Result;
use std::fs;
use std::sync::{Arc, RwLock};

use crate::i18n::t;
use crate::utils;
use crate::utils::SysPathExist;

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
        Ok(())
    }

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

    /// 遍历 /sys/block/*/queue，逐设备写入 IO 优化参数（调度器/预读/合并/统计）。
    /// 开关关闭或 /sys/block 不存在时直接返回；每个参数非空且路径存在才写。
    fn apply_io_settings(&self) -> Result<()> {
        let config = self.config.read().unwrap();
        if !config.function.io_optimization {
            log::info!("{}", t("apply-io-settings-start"));
            return Ok(());
        }

        let io = &config.io_settings;
        let block_dir = std::path::Path::new("/sys/block");
        if !block_dir.exists() {
            log::warn!("IOOptimization: /sys/block does not exist, skipping");
            return Ok(());
        }

        if let Ok(entries) = fs::read_dir(block_dir) {
            for entry in entries.flatten() {
                let dev_path = entry.path();
                let queue_path = dev_path.join("queue");
                if !queue_path.exists() {
                    continue;
                }

                if !io.scheduler.is_empty() {
                    let p = queue_path.join("scheduler");
                    if p.exists() {
                        let _ = utils::try_write_file(&p, &io.scheduler);
                    }
                }
                if !io.read_ahead_kb.is_empty() {
                    let p = queue_path.join("read_ahead_kb");
                    if p.exists() {
                        let _ = utils::try_write_file(&p, &io.read_ahead_kb);
                    }
                }
                if !io.nomerges.is_empty() {
                    let p = queue_path.join("nomerges");
                    if p.exists() {
                        let _ = utils::try_write_file(&p, &io.nomerges);
                    }
                }
                if !io.iostats.is_empty() {
                    let p = queue_path.join("iostats");
                    if p.exists() {
                        let _ = utils::try_write_file(&p, &io.iostats);
                    }
                }
                log::debug!(
                    "IOOptimization: applied to {:?}",
                    dev_path.file_name().unwrap_or_default()
                );
            }
        }

        log::info!("{}", t("apply-io-settings-start"));
        Ok(())
    }
}
