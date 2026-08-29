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

use anyhow::{anyhow, Result};
use log::{LevelFilter, Record};
use log4rs::append::Append;
use log4rs::config::{Appender, Config, Root};
use log4rs::encode::pattern::PatternEncoder;
use log4rs::encode::Encode;
use log4rs::Handle;
use once_cell::sync::OnceCell;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use crate::common;
use crate::i18n::t_with_args;
use crate::fluent_args;

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
const LOG_MAX_BYTES: u64 = 5 * 1024 * 1024;
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

        // 确保日志目录存在（目录被删也能重建）
        if let Some(parent) = self.path.parent() {
            let _ = fs::create_dir_all(parent);
        }

        // 尺寸达到上限先轮转
        if self.current_size() >= self.max_bytes {
            self.rotate();
        }

        // 按路径追加：文件被删会自动重建，写失败仅吞掉不影响进程
        if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(&self.path) {
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
        encoder: Box::new(PatternEncoder::new("[{d(%Y-%m-%d %H:%M:%S)}] [{l}] [{M}] {m}{n}")),
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
    LOG_HANDLE.set(Mutex::new(handle))
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
                    log::debug!("{}", t_with_args("log-level-updated", &fluent_args!("level" => level.to_string())));
                }
                Err(e) => eprintln!("Failed to rebuild logger config: {}", e),
            }
        }
    }
}