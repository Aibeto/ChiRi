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
use std::sync::atomic::AtomicBool;
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

    // 单实例锁：flock 互斥（进程存活期间持有，崩溃/被杀由内核自动释放）。
    // 必须先于日志归档执行：否则第二个实例会把首个实例的 logs/ 整体归档改名。
    // 背景：service.sh/action.sh/WebUI 的看门狗清理依赖 pidfile，logs/watchdog.pid
    // 丢失时旧看门狗无法被终止，killall 后 3s 把 daemon 再拉起，与新实例并行——
    // devimp_<pkg>_<毫秒时间戳>.log 按进程各自命名，同包名内容写进两份不同文件，
    // status/daemon 日志同理。flock 在进程层兜底一切 shell 侧竞态：
    // 拿锁失败即退出（看门狗 3s 后重试，旧实例退出后即可接管），全程至多一个实例。
    // 锁文件放模块根而非 logs/（logs/ 每次启动被整体归档，不可作为锁锚点）。
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        if let Ok(lock_file) = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(root.join("daemon.lock"))
        {
            if unsafe { libc::flock(lock_file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
                eprintln!("yumi: another daemon instance holds daemon.lock, exiting");
                std::process::exit(0);
            }
            // 故意泄漏 fd 持锁至进程退出；fd 关闭即释放锁
            std::mem::forget(lock_file);
        }
    }

    let log_dir = root.join("logs");
    // 启动归档：把上一轮整个 logs/ 与 devimp/ 分别重命名为临时目录并交单个
    // 子线程异步打包为 logd/ziped_<ts>.zip 与 logd/devimp_<ts>.zip（watchdog.pid
    // 复制回新建的 logs/ 供 stopScheduler 定位看门狗）；打包完成后执行
    // logd+devimp 预算清理（总大小 >128MB 时从最旧文件删到 <96MB）。
    // 本进程日志全部写入新建的 logs/、devimp/，互不干扰。
    // 必须在 create_dir_all(log_dir)/logger::init 之前执行，保证新旧文件分离。
    let (archived_zip, archived_devimp) = logger::archive_on_startup(&root);
    std::fs::create_dir_all(&log_dir)?;
    // devimp 目录已随归档新建；此处仅做容量清理兜底（归档失败时旧文件仍在）
    logger::devimp_prepare();

    // 2. 判断是否启用 Chiri 专用调度器（检测到列表中的特定处理器时启用）
    let chiri_active = common::is_chiri_soc();

    // 3. 解析配置文件路径：8550 等 Chiri 目标 SoC 优先加载处理器子目录 config/{soc}/config.yaml，
    //    其余机型回退到默认 config/config.yaml（两套调度仍共用同一份选中的文件）
    let config_path = common::get_config_path();
    // 写生效配置相对 config 目录的路径（处理器子目录时为 "8550/config.yaml"，默认时为 "config.yaml"），
    // WebUI 拼接 `config/{相对路径}` 读取同一份文件，避免改错文件
    let config_rel = config_path
        .strip_prefix(root.join("config"))
        .unwrap_or(&config_path);
    // as_encoded_bytes 未稳定化（各 toolchain 均不可用），用 to_string_lossy 兼容
    let _ = utils::try_write_file(
        root.join("active_config.txt"),
        config_rel.to_string_lossy().as_bytes(),
    );

    // 配置快照自愈：调优配置编译期嵌入二进制（common::embedded_config_str），
    // 磁盘文件只是「嵌入内容 + meta 覆盖」的快照。启动时把嵌入内容写回生效路径：
    // 文件被篡改 → 还原调优参数（meta 保留用户选择）；文件缺失 → 重建。
    common::sync_config_snapshot(&config_path);

    // 导出内部特调白名单（编译期嵌入 src/chiri/special_tuned.txt）供 WebUI 展示
    // “特调”标签与专属选项：每行一条 `包名:特调模式列表(逗号分隔):优先回退模式`。
    // 只导出精确包名条目（正则条目无法按包名精确查找）；WebUI 只读该文件，不提供修改入口。
    // 仅在 Chiri 专属调度激活时导出——非 Chiri（Yumi）设备不生成该文件，WebUI 据此隐藏特调功能。
    // 日志延后到 logger::init 之后输出（init 前的 log 会被静默丢弃），见下方「日志系统就绪」块。
    let mut exported_special: Option<usize> = None;
    let mut exported_fas: Option<usize> = None;
    if chiri_active {
        let exact: Vec<&common::SpecialTunedEntry> = common::special_tuned_entries()
            .iter()
            .filter(|e| e.regex.is_none())
            .collect();
        let special_tuned_content: String = exact
            .iter()
            .map(|e| format!("{}:{}:{}\n", e.package, e.modes.join(","), e.fallback))
            .collect();
        let _ = utils::try_write_file(
            root.join("special_tuned.txt"),
            special_tuned_content.as_bytes(),
        );
        exported_special = Some(exact.len());

        // 导出 FAS 白名单（编译期嵌入，WebUI 只读展示用；WebUI 不可修改，daemon 不读磁盘）
        let fas_content: String = common::fas_whitelist()
            .iter()
            .map(|(pkg, cfg)| format!("{pkg}:{cfg}\n"))
            .collect();
        if utils::try_write_file(root.join("fas_whitelist.txt"), fas_content.as_bytes()).is_ok() {
            exported_fas = Some(common::fas_whitelist().len());
        }
    }

    // 4. 立即加载语言与日志（两套 Config 的 meta 结构一致，先用它初始化）
    let (language, loglevel) = if chiri_active {
        let cfg = chiri::config::Config::load(config_path.to_str().unwrap()).unwrap_or_default();
        (cfg.meta.language, cfg.meta.loglevel)
    } else {
        let cfg = Config::load(config_path.to_str().unwrap()).unwrap_or_default();
        (cfg.meta.language, cfg.meta.loglevel)
    };
    load_language(&language);
    logger::init(&loglevel)?;

    // 日志系统就绪后输出归档结果（归档线程在 logger::init 之前已启动，不阻塞）
    if let Some(zip) = &archived_zip {
        info!(
            "{}",
            t_with_args(
                "main-log-archive-submitted",
                &fluent_args!("zip" => zip.clone())
            )
        );
    }
    if let Some(zip) = &archived_devimp {
        info!(
            "{}",
            t_with_args(
                "main-devimp-archive-submitted",
                &fluent_args!("zip" => zip.clone())
            )
        );
    }

    // 白名单导出结果（文件写入在 logger::init 之前完成，日志延后到此处才可见）
    if let Some(count) = exported_special {
        info!(
            "{}",
            t_with_args(
                "main-special-tuned-exported",
                &fluent_args!("count" => count.to_string())
            )
        );
    }
    if let Some(count) = exported_fas {
        info!(
            "{}",
            t_with_args(
                "main-fas-whitelist-exported",
                &fluent_args!("count" => count.to_string())
            )
        );
    }

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

    // 5. 创建通信通道（有界：容量 64，满时 send 阻塞形成背压，防止事件无限积压；
    //    足够承载 160ms（特调 40ms）负载事件与低频状态事件）
    let (tx, rx) = mpsc::sync_channel::<common::DaemonEvent>(64);

    // 特调（akmode）激活共享标志：AkmodeGovernor 接管/释放时置位，
    // cpu_monitor 据此在 120ms 与 40ms 采样间隔间切换
    let ak_active = Arc::new(AtomicBool::new(false));

    // 6. 按 SoC 启动对应的调度器（两套互斥，同一事件通道只被其中一个消费）
    let start_result = if chiri_active {
        log::info!("{}", t("main-chiri-scheduler-selected"));
        let cfg = chiri::config::Config::load(config_path.to_str().unwrap()).unwrap_or_default();
        chiri::start_scheduler_thread(rx, Arc::new(RwLock::new(cfg)), ak_active.clone())
    } else {
        let cfg = Config::load(config_path.to_str().unwrap()).unwrap_or_default();
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
    // 常规采样间隔按 SoC 参数化：Chiri 160ms，Yumi 保持原有 200ms
    let sample_ms_normal: u64 = if chiri_active { 160 } else { 200 };
    let monitor_thread = thread::Builder::new()
        .name("monitor_core".to_string())
        .spawn(move || {
            if let Err(e) = monitor::start_monitor(tx, ak_active, sample_ms_normal) {
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
