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
//  状态日志（logs/status.log，CSV 宽表）：daemon.log 之外的唯一状态文件
// ════════════════════════════════════════════════════════════════
//
// 整合原 foreground / power / telemetry 三个独立 CSV，用 `type` 列区分行类型：
// - snap 行（1s 一条，chiri 调度线程）：遥测 + 热保护 + 模式状态
// - fg 行（事件驱动，app_detect）：前台包切换——**含同模式切换**。
//   修复原 log_foreground 依赖 ModeChange 事件（仅模式变化才发送）导致
//   同模式 App 切换无记录的失效问题。
//
// 开销控制（对比旧 append_aux_log 每行 3 次 open + stat）：
// - Mutex 常驻 append 句柄：每次写入仅一次 write syscall；
// - 每 256 行巡检一次：轮转（8MB）+ 被删自愈（外部删除 status.log 后自动重建）；
// - 写失败重开重试一次，全程不 unwrap、不阻塞调度线程。

/// 状态日志路径（CSV 宽表）
const STATUS_LOG_REL: &str = "logs/status.log";
/// 单文件上限 8MB，保留 1 份备份；1s 一条约 250B，轮转周期约 6 小时
const STATUS_LOG_MAX_BYTES: u64 = 8 * 1024 * 1024;
/// 每 N 行巡检一次（轮转 + 被删自愈）；1s 一条时约 4 分钟巡检一次
const STATUS_CHECK_EVERY: u64 = 256;

/// CSV 表头（列序由 status_log_snapshot / status_log_fg 保证对齐）：
/// ts,type,mode,screen_on,batt_temp,cpu_temp,thermal_cap_pct,thermal_free_pct,clg_active,
/// psi_cpu_some,psi_io_some,psi_mem_some,gpu_busy_pct,batt_voltage_v,batt_current_ma,
/// batt_power_w,wakeups,migrations,freq_trans,fg_temp,package,old_mode,new_mode
const STATUS_HEADER: &str = "timestamp,type,mode,screen_on,batt_temp,cpu_temp,thermal_cap_pct,thermal_free_pct,clg_active,psi_cpu_some,psi_io_some,psi_mem_some,gpu_busy_pct,batt_voltage_v,batt_current_ma,batt_power_w,wakeups,migrations,freq_trans,fg_temp,package,old_mode,new_mode";

/// 常驻写入器：append 句柄 + 巡检计数
struct StatusWriter {
    file: Option<fs::File>,
    since_check: u64,
}

/// 进程级单例（Mutex::new 为 const，可静态初始化）
static STATUS_WRITER: Mutex<StatusWriter> = Mutex::new(StatusWriter {
    file: None,
    since_check: 0,
});

/// 打开（或重建）状态日志：create+append；空文件补表头。
/// 返回 None 表示打开失败（调用方下次写入时再试）。
fn status_open() -> Option<fs::File> {
    let path = common::get_module_root().join(STATUS_LOG_REL);
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let mut f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .ok()?;
    if f.metadata().map(|m| m.len()).unwrap_or(1) == 0 {
        let _ = f.write_all(STATUS_HEADER.as_bytes());
        let _ = f.write_all(b"\n");
    }
    Some(f)
}

/// 巡检：轮转（超 8MB）与被删自愈（外部删除后 metadata 报错 → 重开重建）
fn status_check(w: &mut StatusWriter) {
    let path = common::get_module_root().join(STATUS_LOG_REL);
    match fs::metadata(&path) {
        Ok(m) if m.len() < STATUS_LOG_MAX_BYTES => return,
        Ok(_) => {
            // 轮转：path -> path.1（旧备份覆盖删除）
            let bak = common::get_module_root().join(format!("{}.1", STATUS_LOG_REL));
            let _ = fs::remove_file(&bak);
            let _ = fs::rename(&path, &bak);
        }
        Err(_) => {} // 文件被删：丢弃旧句柄（fd 仍指向孤儿 inode），下方重开重建
    }
    w.file = status_open();
}

/// 写一行到 status.log（调用方保证 fields 与 STATUS_HEADER 列数对齐）
fn status_write_line(fields: &[&str]) {
    let line = fields.join(",");
    let mut w = STATUS_WRITER.lock().unwrap_or_else(|p| p.into_inner());
    if w.file.is_none() {
        w.file = status_open();
    }
    let write_ok = match w.file.as_mut() {
        Some(f) => f
            .write_all(line.as_bytes())
            .and_then(|_| f.write_all(b"\n"))
            .is_ok(),
        None => false,
    };
    if !write_ok {
        // 写失败（磁盘/句柄异常）：重开重试一次，仍失败则丢弃本行
        w.file = status_open();
        if let Some(f) = w.file.as_mut() {
            let _ = f.write_all(line.as_bytes());
            let _ = f.write_all(b"\n");
        }
    }
    w.since_check += 1;
    if w.since_check >= STATUS_CHECK_EVERY {
        w.since_check = 0;
        status_check(&mut w);
    }
}

/// 缺失数值的占位
const NA: &str = "-";

/// 数值格式化（None → "-"）
fn fmt_num(v: Option<f32>, digits: usize) -> String {
    v.map(|x| format!("{:.*}", digits, x))
        .unwrap_or_else(|| NA.to_string())
}

/// 写 snapshot 行（1s 一条，chiri 调度线程）：
/// 遥测（PSI/GPU/电池，含 OPlus bcc 实时数据）+ 热保护（温度/压制/豁免）+ 模式状态。
/// 替代原 power.log（2s）+ telemetry.log（1s）两条流，精度与信息量不变、文件合一。
#[allow(clippy::too_many_arguments)]
pub fn status_log_snapshot(
    mode: &str,
    screen_on: bool,
    batt_temp: Option<f32>,
    cpu_temp: Option<f32>,
    thermal_cap: &str,
    thermal_free: &str,
    clg_active: bool,
    psi_cpu: &str,
    psi_io: &str,
    psi_mem: &str,
    gpu_busy: &str,
    batt_v: &str,
    batt_i: &str,
    batt_p: &str,
    wakeups: u32,
    migrations: u32,
    freq_trans: u32,
) {
    status_write_line(&[
        &format_now(),
        "snap",
        mode,
        if screen_on { "1" } else { "0" },
        &fmt_num(batt_temp, 1),
        &fmt_num(cpu_temp, 1),
        thermal_cap,
        thermal_free,
        if clg_active { "1" } else { "0" },
        psi_cpu,
        psi_io,
        psi_mem,
        gpu_busy,
        batt_v,
        batt_i,
        batt_p,
        &wakeups.to_string(),
        &migrations.to_string(),
        &freq_trans.to_string(),
        NA,
        NA,
        NA,
        NA,
    ]);
}

/// 写 fg 行（事件驱动，app_detect 包切换确认时调用，含同模式切换）：
/// 记录前台包名与切换前后的模式（old_mode 为前一前台包的生效模式）。
pub fn status_log_fg(pkg: &str, screen_on: bool, temp: &str, old_mode: &str, new_mode: &str) {
    status_write_line(&[
        &format_now(),
        "fg",
        NA,
        if screen_on { "1" } else { "0" },
        NA,
        NA,
        NA,
        NA,
        NA,
        NA,
        NA,
        NA,
        NA,
        NA,
        NA,
        NA,
        NA,
        NA,
        NA,
        temp,
        pkg,
        old_mode,
        new_mode,
    ]);
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

// ════════════════════════════════════════════════════════════════
//  启动日志归档：logs/ → logd/ziped_<毫秒时间戳>.zip（子线程异步打包）
// ════════════════════════════════════════════════════════════════
//
// 流程（由 main.rs 在 logger::init 之前调用，保证新旧日志文件分离）：
// 1. 把整个 logs/ 原子重命名为 logs 同级的 `ziped_<ts>` 临时目录（同分区 rename）；
// 2. **复制回 watchdog.pid** 到新建的 logs/——看门狗先于本进程启动、WebUI
//    stopScheduler 靠 logs/watchdog.pid 定位并终止看门狗，归档不能带走它；
// 3. 子线程把临时目录打包为 logd/ziped_<ts>.zip（stored ZIP，无压缩、零新依赖），
//    成功后删除临时目录并自然退出（无常驻线程）；失败保留临时目录并写入
//    ARCHIVE_FAILED.txt 供事后排查（此时 logger 尚未 init，无法打点）；
// 4. rename 失败或原目录为空时跳过归档，日志继续写入原 logs/，不影响启动。

/// 启动归档入口。返回归档 zip 文件名（logger::init 后供 main info 打点）；
/// 未归档（首次安装 / 空目录 / rename 失败）返回 None。
pub fn archive_logs_on_startup(root: &Path) -> Option<String> {
    let src = root.join("logs");
    // 空目录或不存在：无归档价值（首次安装），main 随后新建 logs/
    let has_entries = std::fs::read_dir(&src)
        .map(|mut d| d.next().is_some())
        .unwrap_or(false);
    if !has_entries {
        return None;
    }
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let tmp_name = format!("ziped_{}", ts);
    let tmp = root.join(&tmp_name);
    std::fs::rename(&src, &tmp).ok()?;

    // 新建 logs/ 并保留看门狗 pid（stopScheduler 的定位依据）
    let new_logs = root.join("logs");
    let _ = fs::create_dir_all(&new_logs);
    let pid_src = tmp.join("watchdog.pid");
    if pid_src.exists() {
        let _ = fs::copy(&pid_src, new_logs.join("watchdog.pid"));
    }

    let zip_name = format!("ziped_{}.zip", ts);
    let logd = root.join("logd");
    let _ = fs::create_dir_all(&logd);
    let zip_path = logd.join(&zip_name);
    let dir_for_thread = tmp;
    // 一次性子线程：打包完成（或失败落标记）即退出，无常驻
    let _ = std::thread::Builder::new()
        .name("log_archiver".to_string())
        .spawn(
            move || match pack_dir_stored_zip(&dir_for_thread, &zip_path) {
                Ok(()) => {
                    let _ = fs::remove_dir_all(&dir_for_thread);
                }
                Err(_) => {
                    let _ = fs::write(
                        dir_for_thread.join("ARCHIVE_FAILED.txt"),
                        "zip packing failed; this directory was kept for inspection\n",
                    );
                }
            },
        );
    Some(zip_name)
}

/// 递归收集目录下全部普通文件（logs/ 实际为平铺结构，递归仅作防御）
fn collect_files(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let p = entry?.path();
        if p.is_dir() {
            collect_files(&p, out)?;
        } else {
            out.push(p);
        }
    }
    Ok(())
}

/// 把目录打包为 stored（不压缩）ZIP。零依赖手写 ZIP 结构：
/// 每文件 [local file header + 文件名 + 原始数据] + central directory + EOCD。
/// 日志文本压缩可省 ~80%，但引入 zip/flate2 依赖违背体积优先约定；stored 任何
/// 解压器均可打开，体积换零依赖。只读文件、流式写出，失败返回 Err 交调用方处理。
fn pack_dir_stored_zip(dir: &Path, zip_path: &Path) -> std::io::Result<()> {
    let mut files: Vec<PathBuf> = Vec::new();
    collect_files(dir, &mut files)?;
    files.sort();
    let mut w = std::io::BufWriter::new(fs::File::create(zip_path)?);
    let mut central: Vec<u8> = Vec::new();
    let mut offset: u32 = 0;
    let mut count: u16 = 0;
    // 固定时间戳 1980-01-01（DOS 日期最小合法值），归档不依赖原 mtime
    const DOS_DATE: u16 = 0x21;
    for f in &files {
        let name_rel = f
            .strip_prefix(dir)
            .unwrap_or(f)
            .to_string_lossy()
            .replace('\\', "/");
        let data = fs::read(f)?;
        let crc = crc32_ieee(&data);
        let n = name_rel.len() as u16;
        let sz = data.len() as u32;
        // local file header
        w.write_all(&0x04034b50u32.to_le_bytes())?;
        w.write_all(&20u16.to_le_bytes())?; // version needed: 2.0
        w.write_all(&0u16.to_le_bytes())?; // flags
        w.write_all(&0u16.to_le_bytes())?; // method: stored
        w.write_all(&0u16.to_le_bytes())?; // mod time
        w.write_all(&DOS_DATE.to_le_bytes())?;
        w.write_all(&crc.to_le_bytes())?;
        w.write_all(&sz.to_le_bytes())?; // compressed size
        w.write_all(&sz.to_le_bytes())?; // uncompressed size
        w.write_all(&n.to_le_bytes())?;
        w.write_all(&0u16.to_le_bytes())?; // extra len
        w.write_all(name_rel.as_bytes())?;
        w.write_all(&data)?;
        // central directory entry
        central.extend_from_slice(&0x02014b50u32.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes()); // version made by
        central.extend_from_slice(&20u16.to_le_bytes()); // version needed
        central.extend_from_slice(&0u16.to_le_bytes()); // flags
        central.extend_from_slice(&0u16.to_le_bytes()); // method
        central.extend_from_slice(&0u16.to_le_bytes()); // mod time
        central.extend_from_slice(&DOS_DATE.to_le_bytes());
        central.extend_from_slice(&crc.to_le_bytes());
        central.extend_from_slice(&sz.to_le_bytes());
        central.extend_from_slice(&sz.to_le_bytes());
        central.extend_from_slice(&n.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes()); // extra len
        central.extend_from_slice(&0u16.to_le_bytes()); // comment len
        central.extend_from_slice(&0u16.to_le_bytes()); // disk number
        central.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
        central.extend_from_slice(&0u32.to_le_bytes()); // external attrs
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name_rel.as_bytes());
        offset = offset.wrapping_add(30 + n as u32 + sz);
        count = count.wrapping_add(1);
    }
    let cd_offset = offset;
    let cd_size = central.len() as u32;
    w.write_all(&central)?;
    // EOCD
    w.write_all(&0x06054b50u32.to_le_bytes())?;
    w.write_all(&0u16.to_le_bytes())?; // this disk
    w.write_all(&0u16.to_le_bytes())?; // cd start disk
    w.write_all(&count.to_le_bytes())?;
    w.write_all(&count.to_le_bytes())?;
    w.write_all(&cd_size.to_le_bytes())?;
    w.write_all(&cd_offset.to_le_bytes())?;
    w.write_all(&0u16.to_le_bytes())?; // comment len
    w.flush()?;
    Ok(())
}

/// CRC32（IEEE 反射多项式 0xEDB88320），表驱动、首次调用时构建 256 项表
fn crc32_ieee(data: &[u8]) -> u32 {
    static TABLE: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let mut t = [0u32; 256];
        let mut i = 0usize;
        while i < 256 {
            let mut c = i as u32;
            let mut k = 0;
            while k < 8 {
                c = if c & 1 != 0 {
                    0xEDB88320 ^ (c >> 1)
                } else {
                    c >> 1
                };
                k += 1;
            }
            t[i] = c;
            i += 1;
        }
        t
    });
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc = table[((crc ^ b as u32) & 0xFF) as usize] ^ (crc >> 8);
    }
    !crc
}
