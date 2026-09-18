//! logger.rs: [buffer] [level] [appender] [loglimit] [init] [status] [devimp_state] [devrow] [devimp_writer] [devimp_api] [archive]

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
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// 适配 `log4rs::encode::Write` 的内存写入器：`PatternEncoder` 编码时写入
/// 该缓冲，`append` 再把字节落盘（`set_style` 走默认空实现，无需着色）。
// [buffer]
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

// [level]
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
// [appender]
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
        // 锁内完成编码与落盘；记账（note_write）必须在锁释放后调用——
        // 它达到门限时会经 log::info! 打点再退出，若在持锁状态下重入
        // append 将在同一把非重入 Mutex 上死锁（进程卡死，看门狗无存活
        // 超时探测救不回）
        let written = {
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
            line.0.len() as u64
        };
        // 记账：写入即事件触发（无需遍历目录）
        note_write(&LOGS_BYTES_WRITTEN, "logs/", written);
        Ok(())
    }

    fn flush(&self) {}
}

// 日志打包门限（事件触发，零额外 syscall）：本会话写入 logs/ 与 devimp/ 的字节数
// 各自累计，任一目录累计 ≥ LOG_RESTART_THRESHOLD_BYTES（16MB）即退出进程——看门狗
// 3s 后拉起新进程，在 logger::init 之前由 archive_on_startup 把两个目录打包进
// logd/（打包只在启动路径发生，故「打包」只能靠重启调度触发）。
//
// 为什么按写入字节记账而不是定期遍历目录：daemon 写日志只有三条路径
// （daemon.log / status.csv / devimp 当前文件），写入即记账是纯事件驱动、不产生
// 任何 stat；而目录里也只有这几条路径在写（watchdog.pid 由 daemon 每 5s 自愈时写，
// 量级为字节）。轮转/清理发生前（daemon.log 50MB、status.csv 8MB+备份、devimp
// 单文件 128MB）累计写入量与目录实际大小一致，故累计值即目录增长量。
// 每进程从 0 计起：启动归档已把两目录清空重建，本会话写入量即目录大小。
//
// [loglimit]
/// logs/ 或 devimp/ 任一目录增长到该大小（16MB）即重启调度以打包日志
const LOG_RESTART_THRESHOLD_BYTES: u64 = 16 * 1024 * 1024;
/// logs/ 本会话累计写入字节（daemon.log + status.csv）
static LOGS_BYTES_WRITTEN: AtomicU64 = AtomicU64::new(0);
/// devimp/ 本会话累计写入字节（当前诊断文件）
static DEVIMP_BYTES_WRITTEN: AtomicU64 = AtomicU64::new(0);
/// 已进入重启流程：日志落盘走同一写路径，置位后不再记账/重入
static LOG_RESTARTING: AtomicBool = AtomicBool::new(false);

/// 记账并判定门限：`counter` 为本目录累计写入量，`dir` 为目录名（日志展示）。
/// 达到门限即退出进程，由看门狗 3s 后拉起走启动归档。
///
/// - 无看门狗（`watchdog_pid()` 为 None：调试直跑 / 孤儿态）**不退出**——退出后
///   无人拉起、调度会永久停止；此时计数清零，等下一个 16MB 再判（避免每行写
///   都去读 /proc）；
/// - 退出前打点的日志本身也走本函数（同一条写路径），以 `LOG_RESTARTING` 防重入；
/// - **调用方不得持有 appender 锁**：本函数可能经 `log::info!` 重入 append，
///   在持锁状态下调用会对同一把非重入 Mutex 死锁（status/devimp 路径各自的
///   锁在调用前释放，append 路径见其函数内注释）。
fn note_write(counter: &AtomicU64, dir: &str, bytes: u64) {
    // 仅 Chiri 调度启用（Yumi 侧调度逻辑冻结，不做自动重启；devimp 本就不产生）
    if !common::is_chiri_soc() {
        return;
    }
    if LOG_RESTARTING.load(Ordering::Relaxed) {
        return;
    }
    let total = counter.fetch_add(bytes, Ordering::Relaxed) + bytes;
    if total < LOG_RESTART_THRESHOLD_BYTES {
        return;
    }
    if watchdog_pid().is_none() {
        counter.store(0, Ordering::Relaxed);
        return;
    }
    LOG_RESTARTING.store(true, Ordering::Relaxed);
    log::info!(
        "{}",
        t_with_args(
            "logger-log-restart-for-archive",
            &fluent_args!(
                "dir" => dir,
                "mb" => (total / (1024 * 1024)).to_string()
            )
        )
    );
    std::process::exit(0);
}

// [init]
/// 行编码：格式与 `PatternEncoder`（`[{d(%Y-%m-%d %H:%M:%S)}] [{l}] [{M}] {m}{n}`，
/// 本地时间）完全一致，只把模块路径里**重复的 crate 名前缀剥掉**——包名与子模块
/// 同名（`src/chiri/`），`chiri::chiri::config` 的首个 `chiri` 属于包名，界面上白占
/// 一列宽度（2026-09-18 用户要求去掉 `chiri::chiri` 这种情况）。委托 PatternEncoder
/// 编码（时间/级别格式零改动），再裁剪行首第三个方括号段；非本 crate 的模块路径
/// （依赖库日志，如 `log4rs::`）原样保留。
#[derive(Debug)]
struct LineEncoder {
    inner: PatternEncoder,
}

/// crate 名：daemon.log 模块路径里冗余的前缀（crate 自身日志才有）
const CRATE_PREFIX: &str = "chiri::";

impl Encode for LineEncoder {
    fn encode(
        &self,
        w: &mut dyn log4rs::encode::Write,
        record: &log::Record,
    ) -> anyhow::Result<()> {
        let mut buf = BufferWriter(Vec::new());
        self.inner.encode(&mut buf, record)?;
        let line = String::from_utf8_lossy(&buf.0);
        // 前缀形如 `[时间] [级别] [模块] 消息`：跳过前两对方括号，第三段是模块。
        // 消息正文里出现 `[`/`]` 不影响——只裁剪前缀，其余字节原样透传。
        let mut cursor = 0usize;
        let mut module_at = None;
        for _ in 0..3 {
            let Some(open) = line[cursor..].find('[').map(|i| cursor + i) else {
                break;
            };
            let Some(close) = line[open..].find(']').map(|i| open + i) else {
                break;
            };
            cursor = close + 1;
            module_at = Some(open + 1);
        }
        match module_at {
            Some(at) if line[at..].starts_with(CRATE_PREFIX) => {
                std::io::Write::write_all(w, line[..at].as_bytes())?;
                std::io::Write::write_all(w, line[at + CRATE_PREFIX.len()..].as_bytes())?;
            }
            _ => std::io::Write::write_all(w, line.as_bytes())?,
        }
        Ok(())
    }
}

fn build_config(level: LevelFilter) -> Result<Config> {
    let root = common::get_module_root();
    let log_path = root.join(LOG_REL_PATH);

    let appender = SelfHealingAppender {
        path: log_path.clone(),
        max_bytes: LOG_MAX_BYTES,
        keep: LOG_KEEP_BACKUPS,
        encoder: Box::new(LineEncoder {
            inner: PatternEncoder::new("[{d(%Y-%m-%d %H:%M:%S)}] [{l}] [{M}] {m}{n}"),
        }),
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
    LOG_LEVEL.store(level as u8, Ordering::Release);
    LOG_HANDLE
        .set(Mutex::new(handle))
        .map_err(|_| anyhow!("Logger already initialized"))?;
    Ok(())
}

/// 当前生效的日志等级（`update_level` 判变化用；启动时由 init 写入）
static LOG_LEVEL: AtomicU8 = AtomicU8::new(LevelFilter::Info as u8);

/// 动态更新日志等级
pub fn update_level(level_str: &str) {
    let level = parse_level(level_str);
    let prev = LOG_LEVEL.swap(level as u8, Ordering::AcqRel);
    if let Some(mutex) = LOG_HANDLE.get() {
        if let Ok(handle) = mutex.lock() {
            match build_config(level) {
                Ok(cfg) => {
                    handle.set_config(cfg);
                    // 等级变化走 info：热重载后必须能在默认 INFO 级别下直接确认
                    // 「新等级是否已生效」——此前只有 debug 打点，用户在 INFO 下
                    // 改完配置看不到任何回执，只能凭副作用猜是否生效。
                    // 未变化则仅 debug，避免每次重载刷一行。
                    if prev != level as u8 {
                        log::info!(
                            "{}",
                            t_with_args(
                                "log-level-updated",
                                &fluent_args!("level" => level.to_string())
                            )
                        );
                    } else {
                        log::debug!(
                            "{}",
                            t_with_args(
                                "log-level-updated",
                                &fluent_args!("level" => level.to_string())
                            )
                        );
                    }
                }
                Err(e) => eprintln!("Failed to rebuild logger config: {}", e),
            }
        }
    }
}

// 状态日志（logs/status.csv，CSV 宽表）：daemon.log 之外的唯一状态文件
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

// [status]
/// 状态日志路径（CSV 宽表）
const STATUS_LOG_REL: &str = "logs/status.csv";
/// 单文件上限 8MB，保留 1 份备份；1s 一条约 250B，轮转周期约 6 小时
const STATUS_LOG_MAX_BYTES: u64 = 8 * 1024 * 1024;
/// 每 N 行巡检一次（轮转 + 被删自愈）。常驻句柄在文件被外部删除后指向
/// 孤儿 inode，写入不报错、只能靠巡检发现——间隔过长会长时间"无文件可读"
/// （logs/ 整目录被删后最长 256s 才重建 status.csv）。1s 一行时 16 行 ≈
/// 16s 自愈窗口，stat 开销可忽略。
const STATUS_CHECK_EVERY: u64 = 16;

/// CSV 表头（列序由 status_log_snapshot 保证对齐）：
/// ts,type,mode,package,charge,screen_on,batt_temp,cpu_temp,thermal_cap_pct,thermal_free_pct,
/// clg_active,psi_cpu_some,psi_io_some,psi_mem_some,gpu_busy_pct,batt_voltage_v,batt_current_ma,
/// batt_power_w,wakeups,migrations,freq_trans,fps
///
/// fps 为**预留列**，schema 恒定存在（1s 一行照常占位）：仅 FAS 激活且帧窗口
/// 有样本时为实测值，其余（FAS 未启动 / 无活跃实例 / 窗口尚无样本）一律 "-"。
/// 追加在末尾而非插入中间，避免打乱既有列的索引。
const STATUS_HEADER: &str = "timestamp,type,mode,package,charge,screen_on,batt_temp,cpu_temp,thermal_cap_pct,thermal_free_pct,clg_active,psi_cpu_some,psi_io_some,psi_mem_some,gpu_busy_pct,batt_voltage_v,batt_current_ma,batt_power_w,wakeups,migrations,freq_trans,fps";

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
    drop(w);
    // 记账（含换行）：写入即事件触发，logs/ 累计增长达 16MB 触发重启打包
    note_write(&LOGS_BYTES_WRITTEN, "logs/", line.len() as u64 + 1);
}

// [power_avg]
/// PowerAVG 输出文件（模块根，对外暴露、供 WebUI 只读展示）
pub const POWER_AVG_CHR: &str = "PowerAVG.chr";

/// PowerAVG 递推状态：（当前留存值, 留存次数）。进程级静态——调度线程 panic 重启
/// 不丢；daemon 进程重启从零开始（按契约设计：文件只是输出记录，不读回）。
static POWER_AVG: Mutex<(f32, u64)> = Mutex::new((0.0, 0));

/// 参考模式的权重：**上次留存值 : 新采样 = 10 : 1**（用户口径 2026-09-18）。
/// 比原先的 1:1 取半更钝——单次异常读数只把结果拉动 1/11，避免读数跟着尖峰跳。
const REFERENCE_WEIGHT: f32 = 10.0;

/// 启动清空 `PowerAVG.chr`（保留文件、内容置空）：文件是**本次运行**的输出记录、
/// 不读回，上次运行留下的值在本次取样前是过期数据，WebUI 会把它当有效读数展示
/// （数值 + 仪表盘）。置空后界面显示 — 并给出「该文件由调度运行期写入」的说明，
/// 直到本次第一个放电采样写回。属运行时输出，不受 `nofix` 约束（那管的是二进制
/// 内容对外部文件的覆盖：meta 自愈与 webui 资产还原）。
pub fn power_avg_reset() {
    let path = common::get_module_root().join(POWER_AVG_CHR);
    let _ = common::write_file_no_panic(&path, b"");
}

/// 更新 PowerAVG 并写模块根 `PowerAVG.chr`。**在 status.csv 写入流程里顺序调用**
/// （每个 1s 采样一次），不另起并行处理。
///
/// - 参考模式（`use_average=false`，默认）：value = (旧值 × 10 + 新值) / 11——上次
///   留存值**先乘 10** 再参与（用户口径 2026-09-18，原为 1:1 取半），偏历史、读数稳：
///   单次异常采样只占 1/11，仅作参考；
/// - 平均模式（true）：value = (旧值 × 次数 + 新值) / (次数 + 1)——累计平均、
///   等权全史（用户口径：「上次留存平均值 × 留存次数 + 当前值，除以（留存次数 + 1）」）。
///
/// `留存次数` 两种模式都累计：切到平均模式时即有历史次数可用。
/// 功耗缺测（None/非法）时跳过本次（没读到就不算），不写文件。
/// **取样口径由调用方保证**（`chiri/mod.rs`）：仅电池放电；且**平均模式排除息屏样本**
/// （息屏功耗低但占时长大，等权全史会把亮屏读数整体拉低），**参考值保留息屏**。
/// 被跳过的样本不推进留存次数、不改动文件——文件里始终是本次运行的有效样本结果。
///
/// 返回**当前留存值**（W）：本次接受则为新值，跳过则沿用上次值，从未采到过为 None
/// ——调用方（常驻通知）据此展示，不必回读文件。
pub fn power_avg_update(power_w: Option<f32>, use_average: bool) -> Option<f32> {
    let current = || {
        let state = POWER_AVG.lock().unwrap_or_else(|e| e.into_inner());
        let (value, count) = *state;
        (count > 0).then_some(value)
    };
    let Some(p) = power_w.filter(|v| v.is_finite() && *v >= 0.0) else {
        return current(); // 本次跳过（缺测/非放电/息屏进均值）：沿用上次留存值
    };
    let mut state = POWER_AVG.lock().unwrap_or_else(|e| e.into_inner());
    let (value, count) = *state;
    let next = if count == 0 {
        p
    } else if use_average {
        (value * count as f32 + p) / (count as f32 + 1.0)
    } else {
        (value * REFERENCE_WEIGHT + p) / (REFERENCE_WEIGHT + 1.0)
    };
    *state = (next, count + 1);
    drop(state);
    // 原子写（tmp+rename）：WebUI 每秒读它展示，绝不能读到半截内容
    let path = common::get_module_root().join(POWER_AVG_CHR);
    let _ = common::write_file_no_panic(&path, format!("{:.2}\n", next).as_bytes());
    Some(next)
}

// [live_time]
/// 心跳文件（模块根，对外暴露、供 WebUI 只读）：每 [`LIVE_TIME_INTERVAL_SECS`]
/// 秒写一次当前**本地时间** `MM:SS`。WebUI 每次刷新读它并与本机时间比对，差值超过
/// 容差（20s）即判定调度已关闭（文件缺失/内容非法同判）。
///
/// 只写分秒（不含小时/日期）是用户口径：差值按 1 小时取模即可判定新鲜度——心跳
/// 间隔 15s + 容差 20s 远小于半小时，取模不会把「超过半小时前的旧值」误判为新鲜
/// （见 WebUI `data/live-time.ts`）。写文件走原子替换（tmp+rename），避免 WebUI
/// 读到半截内容。
pub const LIVE_TIME_CHR: &str = "LiveTime.chr";

/// 心跳间隔（秒）：与 WebUI 侧容差常量配套（容差必须 > 它，留满一轮余量）
pub const LIVE_TIME_INTERVAL_SECS: u64 = 15;

/// 写一次心跳（`MM:SS` + 换行，本地时间）。失败静默：模块根不可写时 WebUI
/// 自然判定为已关闭，不需要额外告警刷日志。
pub fn write_live_time() {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let (_, m, s) = local_hms(now.as_secs() as i64).unwrap_or_else(|| {
        let total = now.as_secs() % 86400;
        (
            (total / 3600) as u32,
            ((total % 3600) / 60) as u32,
            (total % 60) as u32,
        )
    });
    let path = common::get_module_root().join(LIVE_TIME_CHR);
    let _ = common::write_file_no_panic(&path, format!("{m:02}:{s:02}\n").as_bytes());
}

/// 缺失数值的占位
const NA: &str = "-";

/// 数值格式化（None → "-"）。**全精度写入**（2026-09-18 用户口径）：CSV 保留
/// 全部小数位（f32 的最短往返表示），取整是显示层（WebUI 状态快照 toFixed(1)）的职责
fn fmt_num(v: Option<f32>) -> String {
    v.map(|x| x.to_string()).unwrap_or_else(|| NA.to_string())
}

/// 写 snapshot 行（1s 一条，chiri 调度线程）：
/// 遥测（PSI/GPU/电池，含 OPlus bcc 实时数据）+ 热保护（温度/压制/豁免）
/// + 模式状态 + 前台包名（每行实时快照，切换点由相邻行变化体现）
/// + 充放电状态（charging/discharging/full/not_charging，未知为 "-"）
/// + FAS 实测帧率（`fps` 预留列，FAS 未启动/窗口无样本为 "-"，见 STATUS_HEADER）。
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
    fps: Option<f32>,
) {
    status_write_line(&[
        &format_now(),
        "snap",
        mode,
        package,
        charge,
        if screen_on { "1" } else { "0" },
        &fmt_num(batt_temp),
        &fmt_num(cpu_temp),
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
        &fmt_num(fps),
    ]);
}

/// HH:MM:SS.mmm 格式**设备本地时间**（避免引入 chrono 依赖）。
/// 时区统一（2026-09）：status.csv / devimp / daemon.log 全部为本地时间——
/// 此前本函数按 `as_secs() % 86400` 输出 UTC，而 daemon.log 与 devimp 文件名
/// 是本地时间，两路日志相差时区，离线对齐必须人工换算（8550 整夜功耗分析踩坑）。
/// 经 libc localtime_r 走系统时区；不可用时回退 UTC 原口径（仅丢时区正确性）。
fn format_now() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let (h, m, s) = local_hms(now.as_secs() as i64).unwrap_or_else(|| {
        let total = now.as_secs() % 86400;
        (
            (total / 3600) as u32,
            ((total % 3600) / 60) as u32,
            (total % 60) as u32,
        )
    });
    format!("{:02}:{:02}:{:02}.{:03}", h, m, s, now.subsec_millis())
}

/// epoch 秒 → 本地 (时, 分, 秒)。仅 unix（bionic/glibc 均有 localtime_r）；
/// 非 unix 主机恒 None（回退 UTC），不影响 Android 目标。
#[cfg(unix)]
fn local_hms(epoch: i64) -> Option<(u32, u32, u32)> {
    unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        let t: libc::time_t = epoch as libc::time_t;
        if libc::localtime_r(&t, &mut tm).is_null() {
            return None;
        }
        Some((tm.tm_hour as u32, tm.tm_min as u32, tm.tm_sec as u32))
    }
}

#[cfg(not(unix))]
fn local_hms(_epoch: i64) -> Option<(u32, u32, u32)> {
    None
}

// 开发诊断日志（devimp/devimp_<前台包名>_<毫秒时间戳>.log）
//
// 供离线分析改善调度的按核诊断数据，与 status.csv 分离：
// - 独立目录 `devimp/`（模块根，与 logs/ 平级），**启动时随 logs/ 一起归档**到
//   logd/devimp_<MMDD-HHmmss>.zip（子线程异步打包），归档后新建空目录接住
//   本进程写入；
// - **按前台包名分组**：文件名 `devimp_<包名>_<MMDD-HHmmss>.log`（本地时间，
//   人眼可辨），首次写入惰性创建（整轮未开启 DEV 则不产生文件）；scheduler_ipc
//   每秒经 set_devimp_package 同步前台包名，包名变化即关闭当前文件、下次写入
//   以新包名 + 当前时间戳开新文件（同应用的分析数据聚合在同文件）；同秒重开
//   以 -N 后缀去重；
// - **文件头元信息**：新文件在 CSV 表头后写 `#` 注释行——模块名/版本、SoC、
//   机型、Android 版本、内核版本，方便多设备/多版本日志离线辨别；
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
// [devimp_state]
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

/// CSV 表头（列 schema，40 列定长，行类型无关——tgtop/place/aff 等只是 type
/// 列的取值，各自写既有列的子集、其余留 "-"，禁止按行类型增删列）。
/// 列序由 devimp_tick / devimp_snap / devimp_place / devimp_aff /
/// devimp_core / devimp_tgtop / devimp_event 的写入保证对齐
// [devrow]
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
// [devimp_writer]
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

/// 本地时间文件名时间戳：`MMDD-HHmmss`（人眼可辨，如 0906-163045）。
/// 经 libc::localtime_r 取设备本地时区（bionic 在 TZ 未设置时读
/// persist.sys.timezone）；localtime_r 失败回退 epoch 秒的十六进制。
fn filename_ts() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let ok = unsafe { libc::localtime_r(&secs, &mut tm) };
    if ok.is_null() {
        return format!("t{secs:x}");
    }
    format!(
        "{:02}{:02}-{:02}{:02}{:02}",
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec
    )
}

/// 生成新文件名：`devimp_<包名段>_<MMDD-HHmmss>.log`（包名段空 → nopkg）。
/// 时间戳在每次开新文件时取当前本地时刻，同一包名触顶续写也会得到新文件名。
/// 同秒内重开（极端：包名快速抖动）由 devimp_open 的存在性检测补 -N 后缀去重。
fn devimp_new_name(pkg_seg: &str) -> String {
    if pkg_seg.is_empty() {
        format!("devimp_nopkg_{}.log", filename_ts())
    } else {
        format!("devimp_{pkg_seg}_{}.log", filename_ts())
    }
}

/// devimp 文件头元信息（`#` 注释行，CSV 解析跳过）：处理器型号、系统版本、
/// 模块版本等，方便多设备/多版本日志离线比对。进程内只收集一次。
fn devimp_meta() -> &'static String {
    static META: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    META.get_or_init(|| {
        let root = common::get_module_root();
        // module.prop：name / version / versionCode
        let (mut m_name, mut m_ver, mut m_code) =
            (String::from("-"), String::from("-"), String::from("-"));
        if let Ok(text) = fs::read_to_string(root.join("module.prop")) {
            for line in text.lines() {
                if let Some(v) = line.strip_prefix("name=") {
                    m_name = v.trim().to_string();
                } else if let Some(v) = line.strip_prefix("version=") {
                    m_ver = v.trim().to_string();
                } else if let Some(v) = line.strip_prefix("versionCode=") {
                    m_code = v.trim().to_string();
                }
            }
        }
        // /system/build.prop：机型 / 平台 / Android 版本
        let (mut model, mut board, mut release, mut sdk) = (
            String::from("-"),
            String::from("-"),
            String::from("-"),
            String::from("-"),
        );
        if let Ok(text) = fs::read_to_string("/system/build.prop") {
            for line in text.lines() {
                if let Some(v) = line.strip_prefix("ro.product.model=") {
                    model = v.trim().to_string();
                } else if let Some(v) = line.strip_prefix("ro.board.platform=") {
                    board = v.trim().to_string();
                } else if let Some(v) = line.strip_prefix("ro.build.version.release=") {
                    release = v.trim().to_string();
                } else if let Some(v) = line.strip_prefix("ro.build.version.sdk=") {
                    sdk = v.trim().to_string();
                }
            }
        }
        // getprop 拿跨分区属性的合并视图：ro.product.model 等常不在
        // /system/build.prop（Android 10+ 属 /product、/vendor 分区），直接读文件
        // 会显示 "-"（devimp 头 model 曾因此读空）。getprop 为空时保留上面的
        // build.prop 解析值兜底。
        let gp = |k: &str| crate::common::getprop(k);
        let m = gp("ro.product.model");
        if !m.is_empty() {
            model = m;
        }
        let b = gp("ro.board.platform");
        if !b.is_empty() {
            board = b;
        }
        let r = gp("ro.build.version.release");
        if !r.is_empty() {
            release = r;
        }
        let s = gp("ro.build.version.sdk");
        if !s.is_empty() {
            sdk = s;
        }
        let soc = common::matched_soc_hint().unwrap_or("-");
        let kernel = fs::read_to_string("/proc/sys/kernel/osrelease")
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|_| "-".to_string());
        format!(
            "# module={m_name} {m_ver} (versionCode {m_code})\n\
             # soc={soc} board={board} model={model}\n\
             # android={release} (sdk {sdk}) kernel={kernel}\n\
             # ts-column=local format_now\n"
        )
    })
}

/// devimp 文件头（CSV 表头 + `#` 元信息注释行）的**内存预拼接缓存**：
/// 进程内只收集/拼接一次，每个新文件创建时直接一次 `write_all` 整块写入。
fn devimp_file_head() -> &'static [u8] {
    static HEAD: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    HEAD.get_or_init(|| {
        let mut head = Vec::with_capacity(DEVIMP_HEADER.len() + devimp_meta().len());
        head.extend_from_slice(DEVIMP_HEADER.as_bytes());
        head.push(b'\n');
        head.extend_from_slice(devimp_meta().as_bytes());
        head
    })
}

/// 打开（或重建）诊断日志：create+append；空文件整块写入内存缓存的文件头
/// （CSV 表头 + `#` 元信息注释行——处理器/系统/模块版本等，方便离线辨别
/// 日志来源，拼接结果进程内复用）。
/// `cur_name` 为空则按当前包名段 + 当前本地时间戳确定新文件名并记入写入器；
/// MMDD-HHmmss 秒级精度存在同秒重开的理论碰撞（包名快速抖动），以 -N 后缀去重。
/// 返回 None 表示打开失败（调用方下次写入时再试）。
fn devimp_open(w: &mut DevimpWriter) -> Option<fs::File> {
    let dir = common::get_module_root().join(DEVIMP_DIR_REL);
    let _ = fs::create_dir_all(&dir);
    let name = match w.cur_name.clone() {
        Some(n) => n,
        None => {
            let base = devimp_new_name(&w.pkg_seg);
            let stem = base.strip_suffix(".log").unwrap_or(&base);
            let mut name = base.clone();
            for n in 1..100u32 {
                let candidate = if n == 1 {
                    base.clone()
                } else {
                    format!("{stem}-{n}.log")
                };
                if !dir.join(&candidate).exists() {
                    name = candidate;
                    break;
                }
                name = candidate;
            }
            w.cur_name = Some(name.clone());
            name
        }
    };
    let mut f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join(&name))
        .ok()?;
    if f.metadata().map(|m| m.len()).unwrap_or(1) == 0 {
        let _ = f.write_all(devimp_file_head());
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
        // 触顶即新增了一个 128MB 级文件：顺手执行目录预算清理
        // （logd/ 与 devimp/ 各自独立计量，各自超 128MB 才从本目录最旧文件删到 <96MB）
        enforce_dir_limits(&common::get_module_root());
    }
    // 记账（含换行）：写入即事件触发，devimp/ 累计增长达 16MB 触发重启打包
    // （在 WRITER 锁外调用，遵守锁序约定）
    note_write(&DEVIMP_BYTES_WRITTEN, "devimp/", line.len() as u64 + 1);
}

/// 启动兜底清理：仅保留最近 DEVIMP_KEEP_FILES 份历史诊断文件（按文件 mtime
/// 排序，超出从旧到新删除）。main.rs 启动时调用一次；正常路径下 devimp/ 已被
/// 启动归档整体 rename 走并新建为空目录，此函数仅作为归档 rename 失败时的
/// 兜底（旧文件保留在原目录时防止无限堆积）。
// [devimp_api]
pub fn devimp_prepare() {
    let dir = common::get_module_root().join(DEVIMP_DIR_REL);
    let _ = fs::create_dir_all(&dir);
    devimp_prune(None);
}

/// 看门狗 PID（daemon 由看门狗 sh 前台拉起，`getppid()` 即其 PID）。
/// ppid <= 1：daemon 非看门狗前台子进程（调试直跑 / 看门狗已被杀后的孤儿态，
/// 由 init 收养），返回 None；再校验父进程 comm 为 sh/mksh，进一步防 pid
/// 复用/调试直跑误判。
fn watchdog_pid() -> Option<i32> {
    let ppid = unsafe { libc::getppid() };
    if ppid <= 1 {
        return None;
    }
    let comm = fs::read_to_string(format!("/proc/{ppid}/comm")).unwrap_or_default();
    let comm = comm.trim();
    if comm != "sh" && comm != "mksh" {
        return None;
    }
    Some(ppid)
}

/// logs/watchdog.pid 运行时自愈：logs/ 目录被外部删除时该文件随目录一起
/// 消失，WebUI「关闭调度」靠它定位并终止看门狗——缺失时 stopScheduler 只能
/// killall chiri 杀不死看门狗，看门狗 3s 后把 daemon 再拉起；用户此时手动
/// 重启（action.sh）又清不掉无 pid 可寻的旧看门狗，最终新旧两个 daemon
/// 实例并行，devimp/status/daemon 日志各写两份。
/// 看门狗 sh 以 `echo $$ > pidfile` 记录自身 PID，而 daemon 正是该 sh 的
/// 前台子进程（service.sh/action.sh 的 "$DAEMON"），`getppid()` 即看门狗 PID。
/// 由两套 scheduler_ipc 的 5s 周期块调用；文件存在时零开销直返。
pub fn ensure_watchdog_pid_file() {
    let root = common::get_module_root();
    let pid_path = root.join("logs/watchdog.pid");
    if pid_path.exists() {
        return;
    }
    if fs::create_dir_all(root.join("logs")).is_err() {
        return;
    }
    // 非看门狗拉起（调试直跑/孤儿态）不写，防 stopScheduler 误杀无关进程
    let Some(ppid) = watchdog_pid() else {
        return;
    };
    let _ = fs::write(&pid_path, ppid.to_string());
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

/// tgtop 行：全系统 top 消耗者快照（30s 一轮 × 每进程一行，最多 5 行/轮）。
/// 用途：定位待机期「小核 util 长期 60%+」的元凶进程（8550 整夜功耗分析遗留
/// 盲区——place 行只记前台应用线程，后台消耗者完全不可见）。
/// 列语义复用：pid/tid = TGID，comm = 进程名（cmdline 首段优先），
/// util_pct = 该窗口运行时间占比（多核并行可 >100%，如 320% ≈ 3.2 核满载），
/// max_util = 窗口内运行时长增量（ms）。
pub fn devimp_tgtop(tgid: u32, comm: &str, util_pct: &str, delta_ms: &str) {
    let mut r = DevRow::new("tgtop");
    r.set(D_PID, tgid.to_string())
        .set(D_TID, tgid.to_string())
        .set(D_COMM, comm)
        .set(D_UTIL, util_pct)
        .set(D_MAXUTIL, delta_ms);
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

// 启动归档：logs/ → logd/<ts>.tar、devimp/ → logd/devimp_<ts>.tar（一次性子线程
// 串行打包），打包完成后执行 logd/ 与 devimp/ 的**各自独立**预算清理
//
// 流程（由 main.rs 在 logger::init 之前调用，保证新旧日志文件分离）：
// 1. 把整个 logs/ 与 devimp/ 分别原子重命名为同级 `ziped_<ts>` / `ziped_devimp_<ts>`
//    临时目录（同分区 rename；`ziped_` 是历史命名、遗留扫描按它认目录，保留不动），
//    并新建空目录接住本进程的新写入；
// 2. **复制回 watchdog.pid** 到新建的 logs/——看门狗先于本进程启动、WebUI
//    stopScheduler 靠 logs/watchdog.pid 定位并终止看门狗，归档不能带走它；
// 3. 单个一次性子线程把两个临时目录串行打包为**无压缩 tar**，打包本身由外部
//    脚本 scripts/pack.sh 完成（对外暴露的稳定接口、构建流程不得修改；优先
//    模块自带 core/bin/tar，回退系统 tar。2026-09-17 起不再由 Rust 手写 ZIP），
//    成功后删除临时目录并自然退出（无常驻线程）；失败保留对应临时目录并写入
//    ARCHIVE_FAILED.txt 供事后排查（此时 logger 尚未 init，无法打点）；
// 4. 打包完成后执行目录预算清理：logd/ 与 devimp/ 各自超过
//    LOGD_MAX_BYTES / DEVIMP_DIR_MAX_BYTES 时，只删本目录内最旧文件到低于对应 target；
// 5. rename 失败或原目录为空时跳过对应归档，不影响启动。

// [archive]
/// logd/ 归档目录自身预算：超过后只清本目录内最旧归档（128MB）
const LOGD_MAX_BYTES: u64 = 128 * 1024 * 1024;
/// logd/ 清理目标：从最旧归档删到低于该值（96MB，滞回防每次归档都触发清理）
const LOGD_TARGET_BYTES: u64 = 96 * 1024 * 1024;
/// devimp/ 目录自身预算，与 logd/ **独立计量**（128MB）
const DEVIMP_DIR_MAX_BYTES: u64 = 128 * 1024 * 1024;
/// devimp/ 清理目标（96MB）
const DEVIMP_DIR_TARGET_BYTES: u64 = 96 * 1024 * 1024;

/// 归档 staging 目录名前缀（logs/ → `ziped_<ts>`，devimp/ → `ziped_devimp_<ts>`）
const STAGING_PREFIX: &str = "ziped_";

/// 唯一化目标路径：`stem.ext` 已存在则依次试 `stem-1.ext` / `stem-2.ext` …
fn unique_path(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    let base = dir.join(format!("{stem}.{ext}"));
    if !base.exists() {
        return base;
    }
    for i in 1..100 {
        let p = dir.join(format!("{stem}-{i}.{ext}"));
        if !p.exists() {
            return p;
        }
    }
    base
}

/// 唯一化 staging 目录名（同目录 rename 目标必须不存在，否则 ENOTEMPTY 会让
/// **本次完全不归档**——旧实现用秒级 ts 命名，同秒两次启动或上一轮遗留同名
/// 目录都会命中，日志就此滞留在模块根）
fn unique_staging(root: &Path, base: &str) -> PathBuf {
    let p = root.join(base);
    if !p.exists() {
        return p;
    }
    for i in 1..100 {
        let q = root.join(format!("{base}-{i}"));
        if !q.exists() {
            return q;
        }
    }
    p
}

/// 回收上一轮遗留的 staging 目录。
///
/// 进程在打包完成前退出（崩溃 / 日志门限 `exit(0)` 时 archiver 线程被连带杀掉 /
/// 手动 kill）时，被 rename 出去的 `ziped_*` 会永久留在模块根：它既不在 `logd/`
/// 也不在 `devimp/`，`enforce_dir_limits` 扫不到、后续启动也不再认领——日志
/// **永远进不了 logd**，且此时 `logs/`/`devimp/` 是上一轮重建的空目录，本次
/// 启动「看起来没有东西可归档」。这就是「特定条件下启动调度不打包」的根因。
/// 在 rename 本轮目录**之前**扫描（避免把本轮刚建目录也算成遗留）。
fn collect_staging_dirs(root: &Path) -> Vec<PathBuf> {
    let Ok(rd) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.is_dir()
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.starts_with(STAGING_PREFIX))
                    .unwrap_or(false)
        })
        .collect();
    out.sort();
    out
}

/// 短会话判定阈值：上一轮 daemon 启动距本次启动不足该秒数 → 丢弃其日志、不打包。
/// 崩溃循环（启动即崩，看门狗 3→10→30→60s 退避重拉）每轮只产生几行日志，
/// 逐轮打包会把 logd/ 塞满几 KB 的空壳归档，且把真正的崩溃现场淹掉。
const SHORT_SESSION_SECS: i64 = 30;

/// 本轮是否因「上一轮为短会话」而丢弃了日志（供 main 在 logger::init 之后打点，
/// 归档发生在 init 之前，当时无日志可打；不打点则日志凭空消失无从解释）
static SHORT_SESSION_DISCARDED: AtomicBool = AtomicBool::new(false);

/// 本轮是否丢弃了上一轮短会话日志（main 在 logger::init 后查询打点）
pub fn short_session_discarded() -> bool {
    SHORT_SESSION_DISCARDED.load(Ordering::Acquire)
}

/// 当前 epoch 秒（与 mktime 结果同基准）
fn now_epoch_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 解析 daemon.log 行首的 `[YYYY-MM-DD HH:MM:SS]` → epoch 秒。
///
/// **时区口径**：log4rs 1.4 的 `{d(...)}` 走 chrono，默认**设备本地时间**
/// （仅显式传第二个参数 `utc` 才是 UTC），本项目的 pattern 未指定 → 本地时间。
/// 因此这里必须用 `mktime`（按本地时区解释）而不是 `timegm`（按 UTC 解释），
/// 否则解析结果会偏一个时区（东八区即 -8h），30s 判定彻底失效。
/// `tm_isdst = -1`：交给 libc 按当前 DST 规则判定，避免夏令时切换期差一小时。
#[cfg(unix)]
fn parse_log_ts_secs(line: &str) -> Option<i64> {
    let s = line.trim_start().strip_prefix('[')?;
    let ts = &s[..s.find(']')?];
    let (date, time) = ts.split_once(' ')?;
    let (y, rest) = date.split_once('-')?;
    let (mo, d) = rest.split_once('-')?;
    let (h, rest) = time.split_once(':')?;
    let (mi, se) = rest.split_once(':')?;
    let (y, mo, d, h, mi, se): (i32, i32, i32, i32, i32, i32) = (
        y.parse().ok()?,
        mo.parse().ok()?,
        d.parse().ok()?,
        h.parse().ok()?,
        mi.parse().ok()?,
        se.parse().ok()?,
    );
    unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        tm.tm_year = y - 1900;
        tm.tm_mon = mo - 1;
        tm.tm_mday = d;
        tm.tm_hour = h;
        tm.tm_min = mi;
        tm.tm_sec = se;
        tm.tm_isdst = -1;
        let t = libc::mktime(&mut tm);
        if t < 0 { None } else { Some(t) }
    }
}

#[cfg(not(unix))]
fn parse_log_ts_secs(_line: &str) -> Option<i64> {
    None
}

/// 读 `logs/daemon.log` 首行时间戳（epoch 秒）。daemon.log 单文件可达 50MB，
/// **只读前 256 字节**（首行必然完整），绝不整读。
fn first_log_line_secs(path: &Path) -> Option<i64> {
    let mut f = fs::File::open(path).ok()?;
    let mut buf = [0u8; 256];
    let n = f.read(&mut buf).ok()?;
    if n == 0 {
        return None;
    }
    let text = String::from_utf8_lossy(&buf[..n]);
    parse_log_ts_secs(text.lines().next()?)
}

/// 上一轮是否为短会话（存活 < SHORT_SESSION_SECS）。
/// 读不到/解析失败时返回 false —— 保守走正常归档，绝不因判定失败而丢日志。
fn is_short_session(daemon_log: &Path) -> bool {
    let Some(start) = first_log_line_secs(daemon_log) else {
        return false;
    };
    // 时钟回拨（用户改时间/NTP 校正）时差值为负：不当作短会话，避免误删
    (0..SHORT_SESSION_SECS).contains(&now_epoch_secs().saturating_sub(start))
}

/// 清空目录内容（**保留 `keep` 里的文件名**，目录本身不删）。
fn clear_dir_keep(dir: &Path, keep: &[&str]) {
    let Ok(rd) = fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if keep.contains(&name) {
            continue;
        }
        if p.is_dir() {
            let _ = fs::remove_dir_all(&p);
        } else {
            let _ = fs::remove_file(&p);
        }
    }
}

/// 串行打包一批 staging 目录：每个目录打成 `logd/<去 ziped_ 前缀名>.tar`
/// （沿用其原始时间戳），成功删目录、失败留痕（pack_or_keep）；空目录直接丢弃。
fn pack_staging_dirs(root: &Path, logd: &Path, dirs: Vec<PathBuf>) {
    for dir in dirs {
        let mut files = Vec::new();
        if collect_files(&dir, &mut files).is_ok() && files.is_empty() {
            let _ = fs::remove_dir_all(&dir);
            continue;
        }
        let stem = dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| STAGING_PREFIX.to_string());
        let stem = stem.strip_prefix(STAGING_PREFIX).unwrap_or(&stem).to_string();
        let tar_path = unique_path(logd, &stem, "tar");
        pack_or_keep(root, &dir, &tar_path);
    }
}

/// 启动归档入口。返回 (logs 归档 tar 名, devimp 归档 tar 名)（logger::init 后供
/// main info 打点）；未归档（首次安装 / 空目录 / rename 失败 / 短会话丢弃）
/// 对应项为 None。
pub fn archive_on_startup(root: &Path) -> (Option<String>, Option<String>) {
    // 归档命名用本地时间 MMDD-HHmmss（人眼可辨）；同秒内两次启动会重名，
    // 由 unique_staging 补 -N 后缀（重名会让 rename 失败、本次整个不归档）
    let ts = filename_ts();

    // 先把上一轮遗留的 staging 目录收进本轮打包队列（必须在 rename 之前扫描）
    let orphans = collect_staging_dirs(root);

    let src_logs = root.join("logs");
    let src_devimp = root.join("devimp");

    // ── 短会话丢弃：上一轮存活不足 30s 直接清空两目录、不打包 ──
    // 判据取自 logs/daemon.log 首行（上一轮进程的首条日志 ≈ 其启动时刻，本地时间）。
    // 必须在 rename 之前判定：rename 后 logs/ 已被换成空目录，就读不到上一轮日志了。
    if is_short_session(&src_logs.join("daemon.log")) {
        // watchdog.pid 必须保留：看门狗先于 daemon 启动、WebUI stopScheduler 靠它
        // 终止看门狗，清掉会导致「关闭调度」失效（与归档路径同口径）
        clear_dir_keep(&src_logs, &["watchdog.pid"]);
        clear_dir_keep(&src_devimp, &[]);
        SHORT_SESSION_DISCARDED.store(true, Ordering::Release);
        // 遗留 staging 目录仍照常回收打包——它们属于更早的会话，不是本轮垃圾，
        // 且在崩溃循环里正是最有价值的那份现场
        if !orphans.is_empty() {
            let logd = root.join("logd");
            let _ = fs::create_dir_all(&logd);
            let root = root.to_path_buf();
            let _ = std::thread::Builder::new()
                .name("log_archiver".to_string())
                .spawn(move || {
                    pack_staging_dirs(&root, &logd, orphans);
                    enforce_dir_limits(&root);
                });
        }
        return (None, None);
    }

    // ── logs/：rename → 新建 logs/ → 复制回 watchdog.pid ──
    let mut logs_tmp: Option<PathBuf> = None;
    let logs_has_entries = fs::read_dir(&src_logs)
        .map(|mut d| d.next().is_some())
        .unwrap_or(false);
    if logs_has_entries {
        let tmp = unique_staging(root, &format!("ziped_{}", ts));
        if fs::rename(&src_logs, &tmp).is_ok() {
            let new_logs = root.join("logs");
            let _ = fs::create_dir_all(&new_logs);
            let pid_src = tmp.join("watchdog.pid");
            if pid_src.exists() {
                let _ = fs::copy(&pid_src, new_logs.join("watchdog.pid"));
            }
            logs_tmp = Some(tmp);
        }
    }

    // ── devimp/：rename → 新建空目录接住本进程新写入 ──
    let mut devimp_tmp: Option<PathBuf> = None;
    let devimp_has_entries = fs::read_dir(&src_devimp)
        .map(|mut d| d.next().is_some())
        .unwrap_or(false);
    if devimp_has_entries {
        let tmp = unique_staging(root, &format!("ziped_devimp_{}", ts));
        if fs::rename(&src_devimp, &tmp).is_ok() {
            let _ = fs::create_dir_all(&src_devimp);
            devimp_tmp = Some(tmp);
        }
    }

    // ── 单个一次性子线程：串行打包（遗留优先）+ 预算清理 ──
    // tar 名沿用各自 staging 目录名的时间戳段（保留原始时间戳），不再统一用本次 ts；
    // `ziped_` 前缀是历史命名（遗留扫描按它认目录，保留不动），产物名剥掉它
    fn tar_name_of(dir: &Path) -> String {
        let stem = dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| STAGING_PREFIX.to_string());
        let stem = stem.strip_prefix(STAGING_PREFIX).unwrap_or(&stem).to_string();
        format!("{stem}.tar")
    }
    let logs_tar = logs_tmp.as_deref().map(tar_name_of);
    let devimp_tar = devimp_tmp.as_deref().map(tar_name_of);
    if !orphans.is_empty() || logs_tmp.is_some() || devimp_tmp.is_some() {
        let logd = root.join("logd");
        let _ = fs::create_dir_all(&logd);
        let root = root.to_path_buf();
        let logs_tar_name = logs_tar.clone().unwrap_or_default();
        let devimp_tar_name = devimp_tar.clone().unwrap_or_default();
        let _ = std::thread::Builder::new()
            .name("log_archiver".to_string())
            .spawn(move || {
                // 遗留目录先打：本轮数据即便再次被打断，下次启动还能回收
                pack_staging_dirs(&root, &logd, orphans);
                if let Some(dir) = logs_tmp {
                    pack_or_keep(&root, &dir, &logd.join(&logs_tar_name));
                }
                if let Some(dir) = devimp_tmp {
                    pack_or_keep(&root, &dir, &logd.join(&devimp_tar_name));
                }
                // 两个归档均已落盘后才清点预算：此时总大小才包含新归档
                enforce_dir_limits(&root);
            });
    }

    (logs_tar, devimp_tar)
}

/// logd/ 与 devimp/ 目录预算清理（**各自独立**）：任一目录超过自己的 MAX 时，
/// 只删本目录内最旧文件，直到本目录低于自己的 TARGET。
///
/// 历史实现把两目录合并成一个 128MB 总预算并跨目录按 mtime 删除：devimp 的
/// 膨胀（单文件软上限 128MB、按数量保留最多 20 份）会被算到 logd 头上，而
/// logd 内的归档包 mtime 恒旧于本会话正在写的 devimp 文件 → 归档包被优先
/// 删光（含启动时刚打好的那个），即「logd/ 被全清」。改为独立计量后，devimp
/// 再大也动不到 logd，反之亦然。活跃写入文件（status/daemon/devimp 当前文件）
/// mtime 最新天然最后才轮到，devimp 写入器对被删文件另有自愈（写入巡检发现
/// metadata Err 后重开重建）。
///
/// 调用时机：启动归档打包完成后 + devimp 触顶换文件时（每次轮转最多增加
/// 一个 128MB 文件，运行中触发频率极低，全目录 metadata 扫描开销可忽略）。
fn enforce_dir_limits(root: &Path) {
    enforce_dir_limit(&root.join("logd"), LOGD_MAX_BYTES, LOGD_TARGET_BYTES);
    enforce_dir_limit(
        &root.join(DEVIMP_DIR_REL),
        DEVIMP_DIR_MAX_BYTES,
        DEVIMP_DIR_TARGET_BYTES,
    );
}

/// 单目录预算清理：`dir` 总大小超过 `max` 时按 mtime 从最旧文件逐个删除，
/// 直到低于 `target`。**最新的一个文件永不删除**——单个文件本身超过 target
/// 时（如一次归档 >96MB），删完其余文件后仍会留下它，避免目录被清空
/// （清理目标不可达时以「至少留一份」为准）。
fn enforce_dir_limit(dir: &Path, max: u64, target: u64) {
    let mut paths: Vec<PathBuf> = Vec::new();
    if collect_files(dir, &mut paths).is_err() {
        return;
    }
    let mut files: Vec<(std::time::SystemTime, u64, PathBuf)> = Vec::new();
    let mut total: u64 = 0;
    for p in paths {
        let Ok(meta) = fs::metadata(&p) else {
            continue;
        };
        let len = meta.len();
        total = total.saturating_add(len);
        let mtime = meta.modified().unwrap_or(std::time::UNIX_EPOCH);
        files.push((mtime, len, p));
    }
    if total <= max {
        return;
    }
    files.sort();
    files.pop(); // 保留最新一份
    for (_, len, path) in &files {
        if total < target {
            break;
        }
        if fs::remove_file(path).is_ok() {
            total = total.saturating_sub(*len);
        }
    }
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

/// 调用外部打包脚本（`module/scripts/pack.sh`，对外暴露的稳定接口，构建流程
/// 不得修改）把 staging 目录打成无压缩 tar。脚本优先用模块自带 `core/bin/tar`
/// （外部引入的二进制），回退系统 tar。成功判据：退出码 0 且目标文件存在。
/// 脚本缺失 / tar 全部不可用 → false，调用方保留 staging 并留痕。
fn pack_dir_tar(root: &Path, dir: &Path, out_tar: &Path) -> bool {
    let script = root.join("scripts/pack.sh");
    let ok = std::process::Command::new("/system/bin/sh")
        .arg(&script)
        .arg("archive")
        .arg(dir)
        .arg(out_tar)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    ok && out_tar.exists()
}

/// 打包单个 staging 目录：成功删除目录，失败写 ARCHIVE_FAILED.txt 保留待查
/// （此时 logger 尚未 init，无法打点）。
fn pack_or_keep(root: &Path, dir: &Path, out_tar: &Path) {
    if pack_dir_tar(root, dir, out_tar) {
        let _ = fs::remove_dir_all(dir);
    } else {
        let _ = fs::write(
            dir.join("ARCHIVE_FAILED.txt"),
            "tar packing failed; this directory was kept for inspection\n",
        );
    }
}
