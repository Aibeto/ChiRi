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

mod common;
mod logger;
mod monitor;
mod scheduler;
pub mod i18n;
pub mod utils;
pub mod fas_types;
use std::sync::mpsc;
use std::thread;
use anyhow::Result;
use log::{info, error, debug};
use crate::i18n::{t, t_with_args, load_language};
// 注意：fluent_args 由 i18n.rs 的 #[macro_export] 注入 crate 根宏命名空间，
// main.rs 即 root 模块，可直接使用，不能再用 use crate::fluent_args 重复导入（E0255）。
use crate::scheduler::config::Config;

fn main() -> Result<()> {
    // 1. 环境初始化
    let chdir_path = std::env::args().nth(1);
    if let Some(path) = &chdir_path {
        nix::unistd::chdir(path.as_str())?;
    }

    let root = common::get_module_root();
    let log_dir = root.join("logs");
    std::fs::create_dir_all(&log_dir)?;

    // 2. 提前读取配置
    let config_path: std::path::PathBuf = root.join("config/config.yaml");
    let config = Config::from_file(config_path.to_str().unwrap()).unwrap_or_default();

    // 3. 立即加载语言
    load_language(&config.meta.language);

    // 4. 初始化日志
    logger::init(&config.meta.loglevel)?;

    // 日志系统就绪后再输出调试信息（init 前的 log 会被静默丢弃）
    if let Some(path) = &chdir_path {
        debug!("{}", t_with_args("main-chdir", &fluent_args!("dir" => path.as_str())));
    }
    debug!("{}", t_with_args("main-module-root", &fluent_args!("path" => root.to_string_lossy().to_string())));
    debug!("{}", t_with_args("main-config-loaded", &fluent_args!(
        "path" => config_path.to_string_lossy().to_string(),
        "loglevel" => config.meta.loglevel.clone(),
        "language" => config.meta.language.clone()
    )));
    info!("{}", t("yumi-module-starting"));

    // 3. 创建通信通道
    let (tx, rx) = mpsc::channel::<common::DaemonEvent>();

    // 4. 启动 Scheduler
    if let Err(e) = scheduler::start_scheduler_thread(rx) {
        error!("{}", t_with_args("scheduler-module-start-failed", &fluent_args!("error" => e.to_string())));
        return Err(e);
    }
    info!("{}", t("scheduler-module-started"));

    // 5. 启动 Monitor
    let monitor_thread = thread::Builder::new()
        .name("monitor_core".to_string())
        .spawn(move || {
            if let Err(e) = monitor::start_monitor(tx) {
                error!("{}", t_with_args("monitor-module-crashed", &fluent_args!("error" => e.to_string())));
            }
        })?;
    
    info!("{}", t("monitor-module-started"));

    // 6. 挂起
    monitor_thread.join().unwrap();

    Ok(())
}