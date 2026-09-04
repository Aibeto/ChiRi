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

use crate::common;
use crate::fluent_args;
use crate::i18n::t_with_args;
use anyhow::{Result, anyhow};
use log::{LevelFilter, Record};
use log4rs::Handle;
use log4rs::append::Append;
use log4rs::config::{Appender, Config, Root};
use log4rs::encode::Encode;
use log4rs::encode::pattern::PatternEncoder;
use once_cell::sync::OnceCell;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// 适配 `log4rs::encode::Write` 的内存写入器：`PatternEncoder` 编码时写入
/// 该缓冲，`append` 再把字节落盘（`set_style` 走默认空实现，无需着色）。
struct BufferWriter(Vec<u8>);

impl std::io::Write for BufferWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl log4rs::encode::Write for BufferWriter {}

static LOG_HANDLE: OnceCell<Mutex<Handle>> = OnceCell::new();

fn parse_level(level_str: &str) -> LevelFilter {
    match level_str.to_uppercase().as_str() {
        "OFF" => LevelFilter::Off,
        "ERROR" => LevelFilter::Error,
        "WARN" => LevelFilter::Warn,
        "INFO" => LevelFilter::Info,
        "DEBUG" => LevelFilter::Debug,
        "TRACE" => LevelFilter::Trace,
        _ => LevelFilter::Info,
    }
}

/// 本模块 `daemon.log` 的相对路径（`logs/daemon.log`）
const LOG_REL_PATH: &str = "logs/daemon.log";
/// 单文件大小上限：达到后把 `daemon.log` 循环后移成 `daemon.{1,2,...}.log`
const LOG_MAX_BYTES: u64 = 50 * 1024 * 1024;
/// 保留的备份数量（`daemon.1.log` ~ `daemon.N.log`，N = LOG_KEEP_BACKUPS）
const LOG_KEEP_BACKUPS: u32 = 3;

/// 日志追加器：文件被删除也能自愈、且日志路径上绝不 panic。
///
/// 背景：log4rs 的 `RollingFileAppender` 持有持久化 `Mutex<File>` 写句柄，
/// 一旦 `daemon.log` 被外部删除（日志清理/误删），句柄指向的是已被 unlink 的
/// inode，旧数据再也写不进去；且其写路径的 `lock().unwrap()` 在锁毒化或轮转失败
/// 时会直接 panic，把守护进程整个打崩（表现为“进程未运行”）。
///
/// 本实现针对以上问题做了三点处理：
///   1. **每次写入都按路径重新打开**（`create+append`），文件被删会自动重建，无需
///      持有长期文件句柄，天然不受删除影响；
///   2. **循环轮转全程无 panic**：所有重命名/删除都吞掉错误，杜绝 `unwrap`；
///   3. **锁毒化也不崩**：`Mutex` 上锁失败时剥除 poison 继续使用，日志线程不 panic。
/// 单项编码失败仅丢弃该条日志，不影响进程存活。
#[derive(Debug)]
struct SelfHealingAppender {
    path: PathBuf,
    max_bytes: u64,
    keep: u32,
    encoder: Box<dyn Encode + Send>,
    lock: Mutex<()>,
}

impl SelfHealingAppender {
    /// 备份名：`daemon.log` -> `daemon.1.log`（把扩展名前缀替换为 `.{n}.log`）
    fn archive_path(&self, n: u32) -> PathBuf {
        let stem = self
            .path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let parent = self.path.parent().unwrap_or(Path::new(""));
        parent.join(format!("{stem}.{n}.log"))
    }

    fn current_size(&self) -> u64 {
        fs::metadata(&self.path).map(|m| m.len()).unwrap_or(0)
    }

    /// 无 panic 的循环轮转：`daemon.log -> daemon.1.log -> ... -> daemon.keep.log`，
    /// 最旧备份被删除；任何一步失败（如文件恰好不存在）都直接忽略。
    fn rotate(&self) {
        let _ = fs::remove_file(self.archive_path(self.keep));
        for i in (1..self.keep).rev() {
            let from = self.archive_path(i);
            let to = self.archive_path(i + 1);
            let _ = fs::rename(&from, &to);
        }
        let _ = fs::rename(&self.path, self.archive_path(1));
    }
}

impl Append for SelfHealingAppender {
    fn append(&self, record: &Record) -> anyhow::Result<()> {
        // 锁毒化也剥除继续用，避免日志线程被 panic 波及崩溃
        let _guard = match self.lock.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };

        // 编码失败仅丢弃这条日志（写入内存缓冲，不直接碰文件）
        let mut line = BufferWriter(Vec::new());
        if self.encoder.encode(&mut line, record).is_err() {
            return Ok(());
        }

        // 保证日志目录存在（目录被删也能重建）
        if let Some(parent) = self.path.parent() {
            let _ = fs::create_dir_all(parent);
        }

        // 尺寸达到上限先轮转
        if self.current_size() >= self.max_bytes {
            self.rotate();
        }

        // 按路径追加：文件被删会自动重建，写失败仅吞掉不影响进程
        if let Ok(mut f) = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        {
            let _ = f.write_all(&line.0);
            let _ = f.flush();
        }
        Ok(())
    }

    fn flush(&self) {}
}

fn build_config(level: LevelFilter) -> Result<Config> {
    let root = common::get_module_root();
    let log_path = root.join(LOG_REL_PATH);

    let appender = SelfHealingAppender {
        path: log_path.clone(),
        max_bytes: LOG_MAX_BYTES,
        keep: LOG_KEEP_BACKUPS,
        encoder: Box::new(PatternEncoder::new(
            "[{d(%Y-%m-%d %H:%M:%S)}] [{l}] [{M}] {m}{n}",
        )),
        lock: Mutex::new(()),
    };

    let config = Config::builder()
        .appender(Appender::builder().build("logfile", Box::new(appender)))
        .build(Root::builder().appender("logfile").build(level))?;

    Ok(config)
}

/// 初始化日志系统，启动时调用一次
pub fn init(level_str: &str) -> Result<()> {
    let level = parse_level(level_str);
    let config = build_config(level)?;
    let handle = log4rs::init_config(config)?;
    LOG_HANDLE
        .set(Mutex::new(handle))
        .map_err(|_| anyhow!("Logger already initialized"))?;
    Ok(())
}

/// 动态更新日志等级
pub fn update_level(level_str: &str) {
    let level = parse_level(level_str);
    if let Some(mutex) = LOG_HANDLE.get() {
        if let Ok(handle) = mutex.lock() {
            match build_config(level) {
                Ok(cfg) => {
                    handle.set_config(cfg);
                    log::debug!(
                        "{}",
                        t_with_args(
                            "log-level-updated",
                            &fluent_args!("level" => level.to_string())
                        )
                    );
                }
                Err(e) => eprintln!("Failed to rebuild logger config: {}", e),
            }
        }
    }
}

// ════════════════════════════════════════════════════════════════
//  辅助文件日志：功耗监控 / 前台监控独立日志
// ════════════════════════════════════════════════════════════════

/// 功耗监控日志路径（CSV 格式，供 WebUI/脚本分析）
const POWER_LOG_REL: &str = "logs/power.log";
/// 前台监控日志路径
const FG_LOG_REL: &str = "logs/foreground.log";
/// 辅助日志单文件上限：1MB，保留 1 份备份（够用，不浪费磁盘）
const AUX_LOG_MAX_BYTES: u64 = 1024 * 1024;

/// 向辅助日志文件追加一行（CSV 格式），文件不存在自动创建含表头。
/// 写失败静默跳过，不阻塞主流程。
pub fn append_aux_log(rel_path: &str, header: &str, line: &str) {
    let root = common::get_module_root();
    let path = root.join(rel_path);
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    // 首次写入：文件不存在就先写表头
    if !path.exists() {
        if let Ok(mut f) = fs::OpenOptions::new().create(true).write(true).open(&path) {
            let _ = f.write_all(header.as_bytes());
            let _ = f.write_all(b"\n");
            let _ = f.flush();
        }
    }
    // 超限时简单轮转：path -> path.1
    if fs::metadata(&path).map(|m| m.len()).unwrap_or(0) >= AUX_LOG_MAX_BYTES {
        let bak = root.join(format!("{}.1", rel_path));
        let _ = fs::remove_file(&bak);
        let _ = fs::rename(&path, &bak);
        if let Ok(mut f) = fs::OpenOptions::new().create(true).write(true).open(&path) {
            let _ = f.write_all(header.as_bytes());
            let _ = f.write_all(b"\n");
            let _ = f.flush();
        }
    }
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = f.write_all(line.as_bytes());
        let _ = f.write_all(b"\n");
        let _ = f.flush();
    }
}

/// 写功耗监控日志（CSV 一行）。
/// 字段：timestamp,batt_temp,cpu_temp,thermal_cap_pct,thermal_free_pct,mode,screen_on,clg_active
pub fn log_power(
    batt_temp: &str,
    cpu_temp: &str,
    thermal_cap: &str,
    thermal_free: &str,
    mode: &str,
    screen_on: bool,
    clg_active: bool,
) {
    let ts = format_now();
    let header =
        "timestamp,batt_temp,cpu_temp,thermal_cap_pct,thermal_free_pct,mode,screen_on,clg_active";
    let line = format!(
        "{},{},{},{},{},{},{},{}",
        ts,
        batt_temp,
        cpu_temp,
        thermal_cap,
        thermal_free,
        mode,
        if screen_on { "1" } else { "0" },
        if clg_active { "1" } else { "0" },
    );
    append_aux_log(POWER_LOG_REL, header, &line);
}

/// 写前台监控日志（CSV 一行）。
/// 字段：timestamp,package,old_mode,new_mode,temperature,screen_on,active_governor
pub fn log_foreground(
    package: &str,
    old_mode: &str,
    new_mode: &str,
    temperature: f64,
    screen_on: bool,
    active_governor: &str,
) {
    let ts = format_now();
    let header = "timestamp,package,old_mode,new_mode,temperature,screen_on,active_governor";
    let line = format!(
        "{},{},{},{},{:.1},{},{}",
        ts,
        package,
        old_mode,
        new_mode,
        temperature,
        if screen_on { "1" } else { "0" },
        active_governor,
    );
    append_aux_log(FG_LOG_REL, header, &line);
}

/// 遥测日志路径（CSV 格式：PSI / GPU / 电池 / eBPF 扩展计数）
const TELEMETRY_LOG_REL: &str = "logs/telemetry.log";

/// 写遥测日志（CSV 一行，2s 周期）。
/// 字段：timestamp,psi_cpu_some,psi_io_some,psi_mem_some,gpu_busy_pct,
///       wakeups_2s,migrations_2s,freq_trans_2s,batt_current_ma,batt_voltage_v,batt_power_w,mode
/// PSI 为 some avg10 百分比；缺失指标写 "-"。
pub fn log_telemetry(
    psi_cpu: &str,
    psi_io: &str,
    psi_mem: &str,
    gpu_busy: &str,
    wakeups: u32,
    migrations: u32,
    freq_transitions: u32,
    batt_current: &str,
    batt_voltage: &str,
    batt_power: &str,
    mode: &str,
) {
    let ts = format_now();
    let header = "timestamp,psi_cpu_some,psi_io_some,psi_mem_some,gpu_busy_pct,\
wakeups_2s,migrations_2s,freq_trans_2s,batt_current_ma,batt_voltage_v,batt_power_w,mode";
    let line = format!(
        "{},{},{},{},{},{},{},{},{},{},{},{}",
        ts,
        psi_cpu,
        psi_io,
        psi_mem,
        gpu_busy,
        wakeups,
        migrations,
        freq_transitions,
        batt_current,
        batt_voltage,
        batt_power,
        mode,
    );
    append_aux_log(TELEMETRY_LOG_REL, header, &line);
}

/// HH:MM:SS.mmm 格式本地时间（避免引入 chrono 依赖）
fn format_now() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let total_sec = now.as_secs() % 86400;
    let h = total_sec / 3600;
    let m = (total_sec % 3600) / 60;
    let s = total_sec % 60;
    format!("{:02}:{:02}:{:02}.{:03}", h, m, s, now.subsec_millis())
}
