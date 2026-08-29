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

mod chiri;
mod common;
pub mod fas_types;
pub mod i18n;
mod logger;
mod monitor;
mod scheduler;
pub mod utils;
use crate::i18n::{load_language, t, t_with_args};
use anyhow::Result;
use log::{debug, error, info};
use std::sync::mpsc;
use std::sync::{Arc, RwLock};
use std::thread;
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

    // 2. 判断是否启用 Chiri 专用调度器（检测到列表中的特定处理器时启用）
    let chiri_active = common::is_chiri_soc();

    // 3. 读取配置（两套调度共用同一份 config.yaml，各自按自己的 Config 结构解析）
    let config_path: std::path::PathBuf = root.join("config/config.yaml");

    // 4. 立即加载语言与日志（两套 Config 的 meta 结构一致，先用它初始化）
    let (language, loglevel) = if chiri_active {
        let cfg =
            chiri::config::Config::from_file(config_path.to_str().unwrap()).unwrap_or_default();
        (cfg.meta.language, cfg.meta.loglevel)
    } else {
        let cfg = Config::from_file(config_path.to_str().unwrap()).unwrap_or_default();
        (cfg.meta.language, cfg.meta.loglevel)
    };
    load_language(&language);
    logger::init(&loglevel)?;

    // 日志系统就绪后再输出调试信息（init 前的 log 会被静默丢弃）
    if let Some(path) = &chdir_path {
        debug!(
            "{}",
            t_with_args("main-chdir", &fluent_args!("dir" => path.as_str()))
        );
    }
    debug!(
        "{}",
        t_with_args(
            "main-module-root",
            &fluent_args!("path" => root.to_string_lossy().to_string())
        )
    );
    debug!(
        "{}",
        t_with_args(
            "main-config-loaded",
            &fluent_args!(
                "path" => config_path.to_string_lossy().to_string(),
                "loglevel" => loglevel,
                "language" => language
            )
        )
    );
    info!("{}", t("yumi-module-starting"));

    // 5. 创建通信通道（有界：防止高频事件在调度线程繁忙时无限积压占用内存；
    //    容量 64 足够承载 200ms 负载事件与低频状态事件，满时 send 阻塞形成背压）
    let (tx, rx) = mpsc::sync_channel::<common::DaemonEvent>(64);

    // 6. 按 SoC 启动对应的调度器（两套互斥，同一事件通道只被其中一个消费）
    let start_result = if chiri_active {
        log::info!("{}", t("main-chiri-scheduler-selected"));
        let cfg =
            chiri::config::Config::from_file(config_path.to_str().unwrap()).unwrap_or_default();
        chiri::start_scheduler_thread(rx, Arc::new(RwLock::new(cfg)))
    } else {
        let cfg = Config::from_file(config_path.to_str().unwrap()).unwrap_or_default();
        scheduler::start_scheduler_thread(rx, Arc::new(RwLock::new(cfg)))
    };
    if let Err(e) = start_result {
        error!(
            "{}",
            t_with_args(
                "scheduler-module-start-failed",
                &fluent_args!("error" => e.to_string())
            )
        );
        return Err(e);
    }
    info!("{}", t("scheduler-module-started"));

    // 7. 启动 Monitor
    let monitor_thread = thread::Builder::new()
        .name("monitor_core".to_string())
        .spawn(move || {
            if let Err(e) = monitor::start_monitor(tx) {
                error!(
                    "{}",
                    t_with_args(
                        "monitor-module-crashed",
                        &fluent_args!("error" => e.to_string())
                    )
                );
            }
        })?;

    info!("{}", t("monitor-module-started"));

    // 8. 挂起
    monitor_thread.join().unwrap();

    Ok(())
}
