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
use std::collections::HashMap;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

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
//  状态日志（logs/status.csv，CSV 宽表）：daemon.log 之外的唯一状态文件
// ════════════════════════════════════════════════════════════════
//
// 整合原 foreground / power / telemetry 三个独立 CSV。仅一种行类型：
// - snap 行（1s 一条，chiri 调度线程）：遥测 + 热保护 + 模式状态 + 前台包名
//   + 充放电状态——所有信息都在每秒汇总行内，稳定 1s 一行（前台包切换不
//   再单独写 fg 行，切换点由相邻行的 package 列变化体现）。
//
// 开销控制（对比旧 append_aux_log 每行 3 次 open + stat）：
// - Mutex 常驻 append 句柄：每次写入仅一次 write syscall；
// - 每 256 行巡检一次：轮转（8MB）+ 被删自愈（外部删除 status.csv 后自动重建）；
// - 写失败重开重试一次，全程不 unwrap、不阻塞调度线程。

/// 状态日志路径（CSV 宽表）
const STATUS_LOG_REL: &str = "logs/status.csv";
/// 单文件上限 8MB，保留 1 份备份；1s 一条约 250B，轮转周期约 6 小时
const STATUS_LOG_MAX_BYTES: u64 = 8 * 1024 * 1024;
/// 每 N 行巡检一次（轮转 + 被删自愈）；1s 一条时约 4 分钟巡检一次
const STATUS_CHECK_EVERY: u64 = 256;

/// CSV 表头（列序由 status_log_snapshot 保证对齐）：
/// ts,type,mode,package,charge,screen_on,batt_temp,cpu_temp,thermal_cap_pct,thermal_free_pct,
/// clg_active,psi_cpu_some,psi_io_some,psi_mem_some,gpu_busy_pct,batt_voltage_v,batt_current_ma,
/// batt_power_w,wakeups,migrations,freq_trans
const STATUS_HEADER: &str = "timestamp,type,mode,package,charge,screen_on,batt_temp,cpu_temp,thermal_cap_pct,thermal_free_pct,clg_active,psi_cpu_some,psi_io_some,psi_mem_some,gpu_busy_pct,batt_voltage_v,batt_current_ma,batt_power_w,wakeups,migrations,freq_trans";

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

/// 写一行到 status.csv（调用方保证 fields 与 STATUS_HEADER 列数对齐）
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
/// 遥测（PSI/GPU/电池，含 OPlus bcc 实时数据）+ 热保护（温度/压制/豁免）
/// + 模式状态 + 前台包名（每行实时快照，切换点由相邻行变化体现）
/// + 充放电状态（charging/discharging/full/not_charging，未知为 "-"）。
#[allow(clippy::too_many_arguments)]
pub fn status_log_snapshot(
    mode: &str,
    package: &str,
    charge: &str,
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
        package,
        charge,
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
//  开发诊断日志（devimp/devimp_<前台包名>_<毫秒时间戳>.log）
// ════════════════════════════════════════════════════════════════
//
// 供离线分析改善调度的按核诊断数据，与 status.csv 分离：
// - 独立目录 `devimp/`（模块根，与 logs/ 平级），**不受启动日志归档影响**
//   （归档只 rename 整个 logs/ 目录，devimp/ 自行管理）；
// - **按前台包名分组**：文件名 `devimp_<包名>_<unix 毫秒时间戳>.log`，首次
//   写入惰性创建（整轮未开启 DEV 则不产生文件）；scheduler_ipc 每秒经
//   set_devimp_package 同步前台包名，包名变化即关闭当前文件、下次写入以
//   新包名 + 当前时间戳开新文件（同应用的分析数据聚合在同文件）；
// - 无包名场景（启动初期尚未检测到前台应用）不触发切文件：继续写当前
//   文件；尚无任何包名时文件名包名段为 `nopkg`（避免空段产生 `devimp__`）；
// - 单文件软上限 128MB：触顶自动换新时间戳文件继续写（修复旧版触顶后
//   静默停写直到进程重启的问题），不丢数据不 panic；
// - 启动时 devimp_prepare() 清理旧文件；写入巡检（每 256 行）在换新文件
//   时同样清理，仅保留最近 DEVIMP_KEEP_FILES 份（文件名含包名段，字典序
//   不再等于时间序，按文件 mtime 排序，当前活跃文件不清理）；
// - 总开关 DEVIMP_ACTIVE 由 scheduler_ipc 按 Config.meta.dev_record 同步
//   （meta 段允许外部修改的字段之一，WebUI 开关 + config_watcher 热重载），
//   写入点（CLG/akmode Worker、亲和再平衡、事件分支）各自检查该标志。
//
// CSV 宽表 + `type` 列，行类型与关键列：
// - tick（每决策 tick × 每核心组，**决策签名变化才写 + 2s 心跳**）：
//   cluster/max_util/over/under/cur_perf/tgt_perf/cur_freq/max_freq/decision/
//   deb_up/deb_down —— 调频决策轨迹；package 列自动填充前台包名
// - snap（1s）：环境上下文（PSI/GPU/电池/温度/热压制），关联决策与功耗
// - place（每再平衡轮）：pid/package/tid/comm/core/util_pct —— 线程放置与占用率
// - aff（事件驱动）：from_core/to_core/decision(动作)/reason —— 亲和迁移动作
// - core（每再平衡轮 × 每核）：core/max_util/pinned —— 逐核负载与钉核计数
// - event：decision(事件名)/reason —— 模式/屏幕/热/配置/触摸等状态变化

/// 开发记录总开关（scheduler_ipc 按 Config.meta.dev_record 同步）
static DEVIMP_ACTIVE: AtomicBool = AtomicBool::new(false);

/// 当前生效模式名（scheduler_ipc 在启动/模式切换/周期刷新时同步），
/// devimp 各行 mode 列自动填充，写入点无需感知模式
static DEVIMP_MODE: Mutex<String> = Mutex::new(String::new());

/// 当前前台包名（set_devimp_package 每秒同步，原始未清洗值），
/// devimp 各行 package 列自动填充；行主体有更精确包名时（place/aff/event）
/// 由调用方覆盖。空 = 尚未检测到前台应用，package 列保持 "-"
static DEVIMP_FG_PKG: Mutex<String> = Mutex::new(String::new());

/// 设置开发记录总开关
pub fn set_devimp_active(on: bool) {
    DEVIMP_ACTIVE.store(on, Ordering::Relaxed);
}

/// 同步当前模式名（devimp 行 mode 列填充用）
pub fn set_devimp_mode(mode: &str) {
    if let Ok(mut m) = DEVIMP_MODE.lock() {
        *m = mode.to_string();
    }
}

/// 同步前台包名并按需切换 devimp 文件（scheduler_ipc 每秒调用，内部去重）。
///
/// - 包名与当前一致（对比写入器归属的包名段）：仅更新 DEVIMP_FG_PKG；
/// - 包名变化：关闭当前文件，下次写入以「新包名 + 当前毫秒时间戳」开新文件
///   （同应用诊断数据聚合在同一文件，切换应用即分文件）；
/// - **空包名（无包名特殊场景，如启动初期尚未检测到前台应用）不触发切换**，
///   继续写当前文件；包名从空变非空 / 非空变化才会开新文件；
/// - 顺序敏感：先切文件（WRITER）后更新 DEVIMP_FG_PKG——FG_PKG 是行
///   package 列数据源，晚于文件切换更新可保证切换边界处
///   「行的 package 列」与「所在文件」永不错位（详见函数体注释）。
pub fn set_devimp_package(pkg: &str) {
    let p = pkg.trim();
    if p.is_empty() {
        return;
    }
    // 包名 → 文件名安全段：只保留字母数字与 . _ -（Android 包名合法字符），
    // 异常字符替换丢弃，超长截断，全被过滤时退化为 nopkg
    let seg: String = p
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        .take(64)
        .collect();
    let seg = if seg.is_empty() {
        "nopkg"
    } else {
        seg.as_str()
    };
    // ① 先切换文件（对比写入器当前归属的包名段）。WRITER 临界区内只做
    // writer 自身状态修改（文件 IO 与无锁操作），不获取任何其他锁——
    // 锁序约定见 DEVIMP_WRITER 定义处
    let pkg_switched = {
        let mut w = DEVIMP_WRITER.lock().unwrap_or_else(|p| p.into_inner());
        if w.pkg_seg == seg {
            false
        } else {
            w.pkg_seg = seg.to_string();
            // 关闭当前文件（cur_name 一并清空）：下次写入按新包名 + 新时间戳开新文件
            w.file = None;
            w.cur_name = None;
            true
        }
    };
    // tick 节流状态清零在 WRITER 锁释放后执行（新文件首 tick 即记录，不留心跳空窗）
    if pkg_switched {
        devimp_tick_state_clear();
    }
    // ② 最后更新 DEVIMP_FG_PKG（各行的 package 列数据源，DevRow::new 读取）。
    // 顺序敏感：写线程构造行读 FG_PKG 与写行拿 WRITER 之间无原子性——
    // 若先更新 FG_PKG 再切文件，切换边界处写线程会读到新包名却写进旧文件，
    // 产生 1~2 行归属错位；先切文件后更新 FG_PKG 则任何时刻
    // 「行的 package 列」与「所在文件」一致（旧行旧文件 / 新行新文件）。
    if let Ok(mut g) = DEVIMP_FG_PKG.lock() {
        *g = p.to_string();
    }
}

/// 开发记录是否开启（各写入点检查；关闭时不产生任何 IO）
pub fn devimp_active() -> bool {
    DEVIMP_ACTIVE.load(Ordering::Relaxed)
}

/// devimp 目录相对模块根的路径
const DEVIMP_DIR_REL: &str = "devimp";
/// 保留的历史文件数（按文件 mtime 从旧到新删除，当前活跃文件除外）；
/// 按包名分组后单轮会话可能产生多份文件，较旧版（一进程一文件）放宽
const DEVIMP_KEEP_FILES: usize = 20;
/// 单文件软上限：触顶换新时间戳文件继续写（不静默停写）
const DEVIMP_MAX_BYTES: u64 = 128 * 1024 * 1024;
/// 每 N 行巡检一次（触顶换文件 + 被删自愈）
const DEVIMP_CHECK_EVERY: u64 = 256;
/// tick 行无变化时的心跳间隔（决策签名不变时每 2s 仍写一条，保证时间轴连续）
const DEVIMP_TICK_HEARTBEAT: Duration = Duration::from_secs(2);

/// tick 行节流状态：cluster 名 → (上次写入的决策签名, 上次写入时刻)。
/// 签名只含决策结果字段（decision/tgt_perf/cur_freq/max_freq/thermal/touch/
/// 防抖进度），util/over/under 等每 tick 抖动的观测值不参与——稳态不写，
/// 防抖与升降过渡期逐 tick 记录。写入量：CLG ~6 行/s、akmode 25 行/s/组
/// → 稳态每组 0.5 行/s
static DEVIMP_TICK_STATE: OnceCell<Mutex<HashMap<String, (String, Instant)>>> = OnceCell::new();

fn devimp_tick_state() -> &'static Mutex<HashMap<String, (String, Instant)>> {
    DEVIMP_TICK_STATE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 清空 tick 节流状态（包名切换/触顶换新文件时调用：新文件首 tick 即记录，
/// 不留心跳空窗）
fn devimp_tick_state_clear() {
    devimp_tick_state()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .clear();
}

/// CSV 表头（列序由 devimp_tick / devimp_snap / devimp_place / devimp_aff /
/// devimp_core / devimp_event 的写入保证对齐，共 40 列）
const DEVIMP_HEADER: &str = "ts,type,mode,screen_on,pid,package,tid,comm,cluster,core,from_core,to_core,util_pct,max_util,over_cores,under_cores,cur_perf,tgt_perf,cur_freq_khz,max_freq_khz,decision,deb_up,deb_down,reason,pinned,thermal_cap_pct,touch,psi_cpu,psi_io,psi_mem,gpu_busy,batt_v,batt_i,batt_p,wakeups,migrations,freq_trans,batt_temp,cpu_temp,clg_active";

// 列索引常量（DevRow.set 用，调用方按列语义取用）
const D_TS: usize = 0;
const D_TYPE: usize = 1;
const D_MODE: usize = 2;
const D_SCREEN: usize = 3;
const D_PID: usize = 4;
const D_PKG: usize = 5;
const D_TID: usize = 6;
const D_COMM: usize = 7;
const D_CLUSTER: usize = 8;
const D_CORE: usize = 9;
const D_FROM: usize = 10;
const D_TO: usize = 11;
const D_UTIL: usize = 12;
const D_MAXUTIL: usize = 13;
const D_OVER: usize = 14;
const D_UNDER: usize = 15;
const D_CURPERF: usize = 16;
const D_TGTPERF: usize = 17;
const D_CURFREQ: usize = 18;
const D_MAXFREQ: usize = 19;
const D_DECISION: usize = 20;
const D_DEBUP: usize = 21;
const D_DEBDOWN: usize = 22;
const D_REASON: usize = 23;
const D_PINNED: usize = 24;
const D_THERMAL: usize = 25;
const D_TOUCH: usize = 26;
const D_PSICPU: usize = 27;
const D_PSIIIO: usize = 28;
const D_PSIMEM: usize = 29;
const D_GPU: usize = 30;
const D_BATTV: usize = 31;
const D_BATTI: usize = 32;
const D_BATTP: usize = 33;
const D_WAKEUPS: usize = 34;
const D_MIGR: usize = 35;
const D_FREQT: usize = 36;
const D_BATTTEMP: usize = 37;
const D_CPUTEMP: usize = 38;
const D_CLGACT: usize = 39;

/// 一行诊断记录（固定 40 列，未用列填 "-"），由各 devimp_* 函数填充
struct DevRow([String; 40]);

impl DevRow {
    /// 新建一行：ts/type/mode/package 已填（mode 读全局 DEVIMP_MODE，package
    /// 读全局 DEVIMP_FG_PKG——当前前台包名，tick/core 等不感知包名的行类型
    /// 由这里补齐），其余置 "-"
    fn new(kind: &str) -> Self {
        let mut row = DevRow(std::array::from_fn(|_| NA.to_string()));
        row.0[D_TS] = format_now();
        row.0[D_TYPE] = kind.to_string();
        if let Ok(m) = DEVIMP_MODE.lock() {
            row.0[D_MODE] = m.clone();
        }
        if let Ok(g) = DEVIMP_FG_PKG.lock() {
            if !g.is_empty() {
                row.0[D_PKG] = g.clone();
            }
        }
        row
    }

    fn set(&mut self, idx: usize, v: impl Into<String>) -> &mut Self {
        self.0[idx] = v.into();
        self
    }

    /// 覆盖 package 列：仅当调用方携带有效包名（非空且非 "-"）时覆盖自动
    /// 填充值；空/"-" 视为未提供，保留前台包名（如 place 行 fg_cmdline 尚未
    /// 缓存、event 行与具体包名无关）
    fn set_pkg(&mut self, pkg: &str) -> &mut Self {
        if !pkg.is_empty() && pkg != NA {
            self.0[D_PKG] = pkg.to_string();
        }
        self
    }
}

/// 常驻写入器：append 句柄 + 当前文件名 + 当前包名段 + 巡检计数。
/// `cur_name` 为 None 时，下次写入按 `pkg_seg` + 当前毫秒时间戳确定新文件名
/// （包名切换与 128MB 触顶续写都走这条路径）。
///
/// 锁序约定（防死锁）：devimp 共四把锁 —— `DEVIMP_MODE` / `DEVIMP_FG_PKG` /
/// `DEVIMP_TICK_STATE` / `DEVIMP_WRITER`。约定：
/// 1. 各锁均为短临界区，**不嵌套持有**（获取下一个锁前必须释放上一个）；
///    历史上曾在 WRITER 临界区内清 TICK_STATE（包名切换/触顶），已移出；
/// 2. WRITER 临界区内只允许文件 IO 与 writer 自身状态修改，不获取任何
///    其他锁——它是写路径的汇合点（scheduler_ipc / CLG Worker / 亲和线程
///    都会写入），嵌套获取最易构成环；
/// 3. `DevRow::new`（MODE、FG_PKG 短持有）先于 `devimp_write_line`（WRITER）
///    完成，两段之间无重叠。
/// 违反上述任一条都会引入死锁风险（如未来某线程反向先取 WRITER 再取
/// FG_PKG）。
struct DevimpWriter {
    file: Option<fs::File>,
    /// 当前文件名（含包名段与文件创建时间戳）
    cur_name: Option<String>,
    /// 当前文件名的包名段（空串 = 尚无前台包名，命名退化为 nopkg）
    pkg_seg: String,
    since_check: u64,
}

static DEVIMP_WRITER: Mutex<DevimpWriter> = Mutex::new(DevimpWriter {
    file: None,
    cur_name: None,
    pkg_seg: String::new(),
    since_check: 0,
});

/// 生成新文件名：`devimp_<包名段>_<当前毫秒时间戳>.log`（包名段空 → nopkg）。
/// 时间戳在每次开新文件时取当前时刻，同一包名触顶续写也会得到新文件名。
fn devimp_new_name(pkg_seg: &str) -> String {
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    if pkg_seg.is_empty() {
        format!("devimp_nopkg_{ms}.log")
    } else {
        format!("devimp_{pkg_seg}_{ms}.log")
    }
}

/// 打开（或重建）诊断日志：create+append；空文件补表头。
/// `cur_name` 为空则按当前包名段 + 当前时间戳确定新文件名并记入写入器。
/// 返回 None 表示打开失败（调用方下次写入时再试）。
fn devimp_open(w: &mut DevimpWriter) -> Option<fs::File> {
    let dir = common::get_module_root().join(DEVIMP_DIR_REL);
    let _ = fs::create_dir_all(&dir);
    let name = match w.cur_name.clone() {
        Some(n) => n,
        None => {
            let n = devimp_new_name(&w.pkg_seg);
            w.cur_name = Some(n.clone());
            n
        }
    };
    let mut f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join(&name))
        .ok()?;
    if f.metadata().map(|m| m.len()).unwrap_or(1) == 0 {
        let _ = f.write_all(DEVIMP_HEADER.as_bytes());
        let _ = f.write_all(b"\n");
    }
    Some(f)
}

/// 巡检：触顶换新时间戳文件继续写（修复旧版触顶静默停写）、被删自愈
/// （丢弃句柄后同名重建）、触顶换新文件后做容量清理。
/// 返回是否发生触顶换文件（调用方在 WRITER 锁释放后据此清 tick 节流状态，
/// 避免 WRITER 临界区内嵌套获取 TICK_STATE 锁）。
fn devimp_check(w: &mut DevimpWriter) -> bool {
    let mut rotated = false;
    if let Some(name) = w.cur_name.clone() {
        let path = common::get_module_root().join(DEVIMP_DIR_REL).join(&name);
        match fs::metadata(&path) {
            // 触顶：关闭当前文件，换新时间戳文件继续写（不丢数据）
            Ok(m) if m.len() >= DEVIMP_MAX_BYTES => {
                w.file = None;
                w.cur_name = None;
                rotated = true;
            }
            Ok(_) => {}
            // 文件被删：丢弃旧句柄（fd 指向孤儿 inode），下方同名重建
            Err(_) => w.file = None,
        }
    }
    if w.file.is_none() {
        let f = devimp_open(w);
        w.file = f;
        if rotated {
            devimp_prune(w.cur_name.as_deref());
        }
    }
    rotated
}

/// 容量清理：仅保留最近 DEVIMP_KEEP_FILES 份 devimp_*.log，从旧到新删除；
/// `current` 为当前活跃文件名，不参与清理。文件名含包名段，字典序不再等于
/// 时间序，因此按文件 mtime 排序。
fn devimp_prune(current: Option<&str>) {
    let dir = common::get_module_root().join(DEVIMP_DIR_REL);
    let mut files: Vec<(std::time::SystemTime, String)> = fs::read_dir(&dir)
        .map(|rd| {
            rd.flatten()
                .filter_map(|e| {
                    let name = e.file_name().to_string_lossy().into_owned();
                    if !name.starts_with("devimp_") || !name.ends_with(".log") {
                        return None;
                    }
                    if Some(name.as_str()) == current {
                        return None;
                    }
                    let mtime = e.metadata().ok()?.modified().ok()?;
                    Some((mtime, name))
                })
                .collect()
        })
        .unwrap_or_default();
    files.sort();
    while files.len() > DEVIMP_KEEP_FILES {
        let (_, old) = files.remove(0);
        let _ = fs::remove_file(dir.join(old));
    }
}

/// 写一行到 devimp 日志（未开启开关时不产生任何 IO）
fn devimp_write_line(row: DevRow) {
    if !devimp_active() {
        return;
    }
    let line = row.0.join(",");
    // WRITER 临界区内只做写入与巡检（文件 IO），不获取任何其他锁；
    // 触顶换文件后的 tick 节流清零移到锁释放之后（锁序约定见 DEVIMP_WRITER）
    let rotated = {
        let mut w = DEVIMP_WRITER.lock().unwrap_or_else(|p| p.into_inner());
        if w.file.is_none() {
            let f = devimp_open(&mut w);
            w.file = f;
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
            let f = devimp_open(&mut w);
            w.file = f;
            if let Some(f) = w.file.as_mut() {
                let _ = f.write_all(line.as_bytes());
                let _ = f.write_all(b"\n");
            }
        }
        w.since_check += 1;
        if w.since_check >= DEVIMP_CHECK_EVERY {
            w.since_check = 0;
            devimp_check(&mut w)
        } else {
            false
        }
    };
    if rotated {
        // 触顶换新文件：清 tick 节流状态（新文件首 tick 即记录，不留心跳空窗）
        devimp_tick_state_clear();
    }
}

/// 启动清理：仅保留最近 DEVIMP_KEEP_FILES 份历史诊断文件（按文件 mtime
/// 排序，超出从旧到新删除）。main.rs 启动时调用一次，与日志归档互不影响。
pub fn devimp_prepare() {
    let dir = common::get_module_root().join(DEVIMP_DIR_REL);
    let _ = fs::create_dir_all(&dir);
    devimp_prune(None);
}

/// tick 行：CLG/akmode 调频决策轨迹（每决策 tick × 每核心组一行）。
///
/// 写入量控制：按 cluster 节流——决策签名（decision/tgt_perf/cur_freq/
/// max_freq/thermal/touch/防抖进度）变化即写，无变化时每 2s 心跳一条；
/// util/over/under 等逐 tick 抖动的观测值不触发写入。稳态从 CLG ~6 行/s、
/// akmode 25 行/s/组 降到每组 0.5 行/s，防抖与升降过渡期仍逐 tick 记录。
#[allow(clippy::too_many_arguments)]
pub fn devimp_tick(
    cluster: &str,
    max_util: &str,
    over: u32,
    under: u32,
    cur_perf: &str,
    tgt_perf: &str,
    cur_freq_khz: &str,
    max_freq_khz: &str,
    decision: &str,
    deb_up: u32,
    deb_down: u32,
    thermal_cap_pct: &str,
    touch_active: bool,
) {
    if !devimp_active() {
        return;
    }
    let sig = format!(
        "{decision}|{tgt_perf}|{cur_freq_khz}|{max_freq_khz}|{thermal_cap_pct}|{touch_active}|{deb_up}|{deb_down}"
    );
    let now = Instant::now();
    let should_write = {
        let mut st = devimp_tick_state()
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        match st.get_mut(cluster) {
            Some((last_sig, last_t)) => {
                if *last_sig == sig && now.duration_since(*last_t) < DEVIMP_TICK_HEARTBEAT {
                    false
                } else {
                    *last_sig = sig;
                    *last_t = now;
                    true
                }
            }
            None => {
                st.insert(cluster.to_string(), (sig, now));
                true
            }
        }
    };
    if !should_write {
        return;
    }
    let mut r = DevRow::new("tick");
    r.set(D_CLUSTER, cluster)
        .set(D_MAXUTIL, max_util)
        .set(D_OVER, over.to_string())
        .set(D_UNDER, under.to_string())
        .set(D_CURPERF, cur_perf)
        .set(D_TGTPERF, tgt_perf)
        .set(D_CURFREQ, cur_freq_khz)
        .set(D_MAXFREQ, max_freq_khz)
        .set(D_DECISION, decision)
        .set(D_DEBUP, deb_up.to_string())
        .set(D_DEBDOWN, deb_down.to_string())
        .set(D_THERMAL, thermal_cap_pct)
        .set(D_TOUCH, if touch_active { "1" } else { "0" });
    devimp_write_line(r);
}

/// snap 行：1s 环境上下文（遥测 + 热保护；前台包名由 DevRow 自动填充，
/// scheduler_ipc 每秒 set_devimp_package 与此处同值）
#[allow(clippy::too_many_arguments)]
pub fn devimp_snap(
    screen_on: bool,
    batt_temp: &str,
    cpu_temp: &str,
    thermal_cap_pct: &str,
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
    let mut r = DevRow::new("snap");
    r.set(D_SCREEN, if screen_on { "1" } else { "0" })
        .set(D_BATTTEMP, batt_temp)
        .set(D_CPUTEMP, cpu_temp)
        .set(D_THERMAL, thermal_cap_pct)
        .set(D_CLGACT, if clg_active { "1" } else { "0" })
        .set(D_PSICPU, psi_cpu)
        .set(D_PSIIIO, psi_io)
        .set(D_PSIMEM, psi_mem)
        .set(D_GPU, gpu_busy)
        .set(D_BATTV, batt_v)
        .set(D_BATTI, batt_i)
        .set(D_BATTP, batt_p)
        .set(D_WAKEUPS, wakeups.to_string())
        .set(D_MIGR, migrations.to_string())
        .set(D_FREQT, freq_trans.to_string());
    devimp_write_line(r);
}

/// place 行：线程放置快照（低频，affinity 缓存数据输出：包名/线程名/落点核；
/// fg_cmdline 未缓存时保留自动填充的前台包名）
#[allow(clippy::too_many_arguments)]
pub fn devimp_place(pid: i32, pkg: &str, tid: i32, comm: &str, core: i32, util_pct: &str) {
    let mut r = DevRow::new("place");
    r.set(D_PID, pid.to_string())
        .set_pkg(pkg)
        .set(D_TID, tid.to_string())
        .set(D_COMM, comm)
        .set(D_CORE, core.to_string())
        .set(D_UTIL, util_pct);
    devimp_write_line(r);
}

/// aff 行：亲和迁移动作（decision 列记动作：pin/promote/demote/restore/
/// blacklist_skip/rebalance；reason 记触发原因）。pkg 无条件覆盖自动填充：
/// 后台迁移行传 "-" 是刻意不归属前台包（避免把后台线程算进前台应用的
/// 线程集合），与 place/event 的 set_pkg 语义不同
#[allow(clippy::too_many_arguments)]
pub fn devimp_aff(
    action: &str,
    pid: i32,
    pkg: &str,
    tid: i32,
    comm: &str,
    from_core: &str,
    to_core: &str,
    util_pct: &str,
    reason: &str,
) {
    let mut r = DevRow::new("aff");
    r.set(D_DECISION, action)
        .set(D_PID, pid.to_string())
        .set(D_PKG, pkg)
        .set(D_TID, tid.to_string())
        .set(D_COMM, comm)
        .set(D_FROM, from_core)
        .set(D_TO, to_core)
        .set(D_UTIL, util_pct)
        .set(D_REASON, reason);
    devimp_write_line(r);
}

/// core 行：逐核负载与钉核计数（每再平衡轮 × 每核一行）
pub fn devimp_core(cluster: &str, core: usize, util: &str, pinned: u32) {
    let mut r = DevRow::new("core");
    r.set(D_CLUSTER, cluster)
        .set(D_CORE, core.to_string())
        .set(D_MAXUTIL, util)
        .set(D_PINNED, pinned.to_string());
    devimp_write_line(r);
}

/// event 行：状态变化（decision 列记事件名：mode_change/screen/thermal_change/
/// config_reload/touch/ak_cooldown；reason 记详情）。pkg 有效时覆盖自动填充
/// （mode_change 携带触发包名；screen/thermal 等系统事件保留前台包名上下文）
pub fn devimp_event(kind: &str, pkg: &str, reason: &str) {
    let mut r = DevRow::new("event");
    r.set(D_DECISION, kind).set_pkg(pkg).set(D_REASON, reason);
    devimp_write_line(r);
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
