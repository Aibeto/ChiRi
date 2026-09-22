//! main.rs: [daemonize] [env_init] [soc_check] [config_path] [lang_logger] [channels] [live_time] [scheduler_start] [monitor_start] [suspend]

mod chiri;
mod common;
mod down;
pub mod fas_types;
pub mod i18n;
mod logger;
mod monitor;
mod notify;
mod rhine;
mod scheduler;
pub mod utils;
mod webui_asset;
use crate::i18n::{load_language, t, t_with_args};
use anyhow::Result;
use log::{debug, error, info};
use std::sync::atomic::AtomicBool;
use std::sync::mpsc;
use std::sync::{Arc, RwLock};
use std::thread;
// 注意：fluent_args 由 i18n.rs 的 #[macro_export] 注入 crate 根宏命名空间，
// main.rs 即 root 模块，可直接使用，不能再用 use crate::fluent_args 重复导入（E0255）。

fn main() -> Result<()> {
    // [daemonize]
    // 0. 进程自保：脱离派生方的会话与管道。
    //    背景：模块热更新 / Action 按钮重启调度时，看门狗与 daemon 由管理器
    //    app（KSU 等）的 su 会话派生——管理器被关闭（从最近任务划掉/被系统
    //    杀后台）时，su 会话清理按会话/进程组连带终止调度服务，且看门狗同
    //    死、无人拉起（boot 时 service.sh 由 ksud 系统执行不受影响，故表现
    //    为「热更新场景下关闭管理器 app，调度就停」）。shell 层的 setsid 在
    //    busybox 缺失时退化为 nohup（只忽略 SIGHUP、不换会话）无法兜底，
    //    此处在 daemon 侧原生处理，覆盖全部启动路径：
    //    - prctl(PR_SET_PDEATHSIG, 0)：显式归零父死信号，父进程死亡不牵连；
    //    - setsid()：自成新会话、脱离原进程组（已是会话首进程时 EPERM，
    //      忽略——说明上层 shell 已 setsid 成功）；
    //    - stdio 重定向到 /dev/null：su 会话的管道在管理器死后断裂，重定向
    //      后 daemon 与派生方再无任何 fd 关联（日志全走 daemon.log/devimp 落盘，
    //      panic 由全局钩子写 daemon.log，不依赖 stderr）。
    #[cfg(unix)]
    unsafe {
        libc::prctl(libc::PR_SET_PDEATHSIG, 0, 0, 0, 0);
        libc::setsid();
        // 经典 daemonize 手法：先关 stdin，open /dev/null 必然复用 fd 0，
        // 再 dup2 覆盖 1/2——三个标准 fd 全部指向 /dev/null 且与派生方管道
        // 解除关联（断裂管道的写入从「EPIPE/panic/潜在 SIGPIPE」变为普通
        // 写 /dev/null 恒成功）。
        libc::close(0);
        let null_fd = libc::open(b"/dev/null\0".as_ptr() as *const libc::c_char, libc::O_RDWR);
        if null_fd >= 0 {
            libc::dup2(null_fd, 0);
            libc::dup2(null_fd, 1);
            libc::dup2(null_fd, 2);
            if null_fd > 2 {
                libc::close(null_fd);
            }
        } else {
            // /dev/null 打不开（fd 耗尽等极端场景）：退而求其次关闭 stdout/
            // stderr，与派生方管道解除关联——后续写入返回 EBADF，同样无
            // SIGPIPE 风险、无阻塞写；fd 0 已在上面关闭，不留管道残留。
            libc::close(1);
            libc::close(2);
        }
    }

    // [env_init]
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
    // main_<pkg>_<MMDD-HHmmss>.log 按进程各自命名（秒级时间戳，同秒重开补 -N 去重），
    // 同包名内容写进两份不同文件，
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
                eprintln!("chiri: another daemon instance holds daemon.lock, exiting");
                std::process::exit(0);
            }
            // 故意泄漏 fd 持锁至进程退出；fd 关闭即释放锁
            std::mem::forget(lock_file);
        }
    }

    let log_dir = root.join("logs");
    // 启动归档：把上一轮整个 logs/ 与 devimp/ 分别重命名为临时目录并交单个
    // 子线程异步打包为 logd/<ts>.tar 与 logd/devimp_<ts>.tar（tar 无压缩；
    // watchdog.pid 复制回新建的 logs/ 供 stopScheduler 定位看门狗）；打包完成后执行
    // logd/ 与 devimp/ 各自独立的预算清理（各自 >128MB 时删本目录最旧文件到 <96MB）。
    // 本进程日志全部写入新建的 logs/、devimp/，互不干扰；devimp/ 为双诊断文件——
    // main_<pkg>_<MMDD-HHmmss>.log（主诊断，tick/snap/event）+ aff_<MMDD-HHmmss>.log
    // （线程流，@A 动作帧 + @S 每秒快照帧），归档时一并进 devimp_<ts>.tar。
    // 必须在 create_dir_all(log_dir)/logger::init 之前执行，保证新旧文件分离。
    let (archived_zip, archived_devimp) = logger::archive_on_startup(&root);
    std::fs::create_dir_all(&log_dir)?;
    // devimp 目录已随归档新建；此处仅做容量清理兜底（归档失败时旧文件仍在）
    logger::diag_prepare();

    // [soc_check]
    // 2. 判断是否启用 Chiri 专用调度器（检测到列表中的特定处理器时启用）
    let chiri_active = common::is_chiri_soc();

    // [config_path]
    // 3. 解析配置路径：8550 等 Chiri 目标 SoC 优先使用处理器子目录 config/{soc}/meta.yaml，
    //    其余机型回退到默认 config/meta.yaml（可修改抬头；调优段 feature.yaml 仅存于二进制）
    let config_path = common::get_config_path();
    // 写生效 meta.yaml 相对 config 目录的路径（处理器子目录时为 "8550/meta.yaml"，
    // 默认时为 "meta.yaml"），WebUI 拼接 `config/{相对路径}` 读取同一份文件，避免改错文件
    let config_rel = config_path
        .strip_prefix(root.join("config"))
        .unwrap_or(&config_path);
    // as_encoded_bytes 未稳定化（各 toolchain 均不可用），用 to_string_lossy 兼容
    let _ = utils::try_write_file(
        root.join("active_config.chr"),
        config_rel.to_string_lossy().as_bytes(),
    );

    // meta.yaml 快照自愈：可修改字段的基准是编译期嵌入的 meta.yaml。启动时校验磁盘副本：
    // 字段非法 → 用嵌入默认整体覆盖并在文件尾追加警告注释；文件缺失 → 原子重建（不加注释）。
    // 「不改」开关（meta.yaml 可选字段 nofix）：true = 跳过所有「二进制内容对外部
    // 文件的覆盖类操作」（meta 快照自愈 + 下面的 webui 资产还原）。必须在自愈之前读，
    // 晚了文件可能已被内嵌默认覆盖、开关本身先丢了。日志等 logger 初始化后再打。
    let nofix = common::read_nofix_flag(&config_path);
    // 置进程级标志：热重载路径的 meta/rules 自愈与 webui 资产还原都读它，
    // 避免各入口重复读磁盘（高频路径只读原子量，与 FAS_ENABLED 同范式）
    common::set_nofix(nofix);
    if !nofix {
        common::sync_meta_snapshot(&config_path);
    }

    // 清理拆分前的遗留 config.yaml（内容已并入二进制 feature 段与 meta.yaml，磁盘无读取方）
    let _ = std::fs::remove_file(root.join("config").join("config.yaml"));
    if let Ok(rd) = std::fs::read_dir(root.join("config")) {
        for e in rd.flatten() {
            if e.path().is_dir() {
                let _ = std::fs::remove_file(e.path().join("config.yaml"));
            }
        }
    }

    // rules.yaml 快照复制：rules.yaml 同样编译期嵌入二进制（只读，运行时一律读嵌入值），
    // 启动时把嵌入内容复制到模块根作对外展示副本（被篡改不影响调度行为，下次启动还原）。
    // 写失败经重写兜底后直接跳过，绝不 panic（见 common::sync_rules_snapshot）。
    // 这同样属于「二进制内容对外部文件的覆盖类操作」，nofix 开启时交还用户。
    if !nofix {
        common::sync_rules_snapshot(&root.join("rules.yaml"));
    }

    // 导出内部特调白名单（编译期嵌入 src/chiri/special_tuned.yaml）供 WebUI 展示
    // “特调”标签与专属选项：每行一条 `包名:特调模式列表(逗号分隔):优先回退模式`。
    // 只导出精确包名条目（正则条目无法按包名精确查找）；WebUI 只读该文件，不提供修改入口。
    // 仅在 Chiri 专属调度激活时导出——非 Chiri 机型不生成该文件，WebUI 据此隐藏特调功能。
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
            root.join("special_tuned.yaml"),
            special_tuned_content.as_bytes(),
        );
        exported_special = Some(exact.len());

        // 导出 FAS 白名单（编译期嵌入，WebUI 只读展示用；WebUI 不可修改，daemon 不读磁盘）
        let fas_content: String = common::fas_whitelist()
            .iter()
            .map(|(pkg, cfg)| format!("{pkg}:{cfg}\n"))
            .collect();
        if utils::try_write_file(root.join("fas_whitelist.yaml"), fas_content.as_bytes()).is_ok() {
            exported_fas = Some(common::fas_whitelist().len());
        }
    }

    // [rhine]
    // 实验室（rhine）启动期处理：先按 rhine-back.chr 还原上次的改动，再按 rhine.chr
    // 决定要不要重新套用。必须排在下面首次 Config::load 之前——还原写的就是 meta.yaml，
    // 晚了本轮就读到旧值（要等下一次热重载才纠正）。
    // 开机路径的意义：service.sh 已经把 rhine.chr 清空，光看它看不出要不要还原，
    // 唯一信号是残留的 rhine-back.chr。
    // 只有 Chiri 走这条（实验室是 Chiri 专属，非 Chiri 连文件都不生成）。
    let rhine_report = if chiri_active {
        rhine::on_startup(&root, &config_path)
    } else {
        rhine::StartupReport::default()
    };

    // [lang_logger]
    // 4. 立即加载语言与日志（meta 读取统一走 chiri::config，非 ChiRi 机型同样适用）
    let (language, loglevel) = {
        let cfg = chiri::config::Config::load(config_path.to_str().unwrap()).unwrap_or_default();
        (cfg.meta.language, cfg.meta.loglevel)
    };
    load_language(&language);
    logger::init(&loglevel)?;

    // [webui_restore]
    // 内嵌 WebUI 资产还原：把编译期嵌入的 webui/dist 覆盖回模块 webroot/，界面文件被
    // 改动/删除的部分每次开机回到出厂内容（与 meta 自愈同属「二进制对外部文件的覆盖
    // 类操作」，nofix: true 时一并跳过，用户自担风险）。
    if nofix {
        log::info!("{}", t("main-nofix-skip"));
    } else if !webui_asset::EMBEDDED {
        log::warn!("{}", t("main-webui-not-embedded"));
    } else {
        let restored = webui_asset::restore_webroot(&root);
        if restored > 0 {
            log::info!(
                "{}",
                t_with_args(
                    "main-webui-restored",
                    &fluent_args!("count" => restored.to_string())
                )
            );
        }
    }

    // [power_avg]
    // 启动清空 PowerAVG.chr：它是「本次运行」的输出记录（不读回），上次运行的值在
    // 本次取样前是过期数据，WebUI 会当有效读数展示（旧值配上新会话的仪表盘）。
    // 置空后界面回到 — +「该文件由调度运行期写入」的说明，直到首个放电采样写回。
    logger::power_avg_reset();

    // 全局 panic 钩子：任何线程的 panic 都落盘到 daemon.log。
    // 此前 panic 消息只写 stderr（守护进程的 stderr 无人接收），调度线程
    // 崩溃后日志里零痕迹——status.csv 只能反推死亡时间线，无法定位原因。
    // 钩子内**不得调用 i18n**：钩子在 unwind 开始前于 panic 线程执行，若
    // panic 发生在持有 BUNDLE 锁的代码段（load_language 写锁 / fluent 格式化
    // 持有的读锁），此刻锁尚未释放，钩子再请求同一把不可重入的锁会死锁；
    // 且 fluent 自身可能就是 panic 源，钩子内复用会二次 panic 直接 abort。
    // 这里只用纯格式化 + log::error!（SelfHealingAppender 全路径无 panic、
    // 不依赖任何业务锁），保证钩子自身零崩溃风险。消息用英文格式（panic
    // payload 本身即英文，检索一致性优先于界面语言）。
    {
        std::panic::set_hook(Box::new(|info| {
            let payload = info.payload();
            let msg = if let Some(s) = payload.downcast_ref::<&'static str>() {
                (*s).to_string()
            } else if let Some(s) = payload.downcast_ref::<String>() {
                s.clone()
            } else {
                "<non-string panic payload>".to_string()
            };
            let loc = info
                .location()
                .map(|l| format!("{}:{}", l.file(), l.line()))
                .unwrap_or_else(|| "<unknown>".to_string());
            let thread_name = std::thread::current()
                .name()
                .unwrap_or("<unnamed>")
                .to_string();
            log::error!("[PANIC] thread \"{thread_name}\" panicked: {msg} (at {loc})");
        }));
    }

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
    // 上一轮存活不足 30s（崩溃循环）：其日志已被直接丢弃、未打包。归档发生在
    // logger::init 之前，当时无日志可打，这里补一条说明，否则日志凭空消失无从解释
    if logger::short_session_discarded() {
        info!("{}", t("main-log-short-session-discarded"));
    }
    // 实验室启动期结果：同样发生在 init 之前，这里补打
    rhine::report_startup(&rhine_report);

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
    info!("{}", t("chiri-module-starting"));

    // [live_time]
    // 心跳线程：每 15s 把当前本地时间（MM:SS）写模块根 LiveTime.chr，WebUI 刷新时
    // 比对差值（容差 20s）判定调度是否在跑——取代此前的 flock 探测：不再依赖
    // toybox 是否带 flock applet，也不会在探测命令不可用时退化成「无法判定」。
    // 语义仍是**进程级存活**（与 flock 判据一致）：独立线程而非搭调度循环——
    // 进程被信号杀死/看门狗没拉起都会停写。
    // 循环永不 panic、写失败静默（logger::write_live_time 内部吞错）。
    thread::Builder::new()
        .name("live_time".to_string())
        .spawn(|| {
            loop {
                logger::write_live_time();
                thread::sleep(std::time::Duration::from_secs(
                    logger::LIVE_TIME_INTERVAL_SECS,
                ));
            }
        })?;

    // [channels]
    // 5. 创建通信通道（有界：容量 64，满时 send 阻塞形成背压，防止事件无限积压；
    //    足够承载 160ms（特调 40ms）负载事件与低频状态事件）
    let (tx, rx) = mpsc::sync_channel::<common::DaemonEvent>(64);

    // 特调激活共享标志：TunedGovernor 接管/释放时置位，
    // cpu_monitor 据此在 120ms 与 40ms 采样间隔间切换
    let ak_active = Arc::new(AtomicBool::new(false));

    // FAS 前台激活共享标志：FasManager 激活/去激活时置位，fps_monitor 据此
    // 门控 eBPF 探针——FAS 未激活时不建 tokio runtime、不加载 eBPF、不挂
    // uprobe（此前 daemon 启动即对前台应用挂 queueBuffer uprobe，非 FAS
    // 会话每帧白付一次探针开销）。由 main.rs 创建，monitor 与 chiri 各持克隆。
    let fas_active = Arc::new(AtomicBool::new(false));

    // [scheduler_start]
    // 6. 启动 ChiRi 调度器（Yumi 兜底已移除：非 ChiRi SoC 不接管 CPU，
    //    只跑监控/WebUI/日志，事件通道无消费者直接丢弃）
    let start_result = if chiri_active {
        log::info!("{}", t("main-chiri-scheduler-selected"));
        let cfg = chiri::config::Config::load(config_path.to_str().unwrap()).unwrap_or_default();
        Some(chiri::start_scheduler_thread(
            rx,
            Arc::new(RwLock::new(cfg)),
            ak_active.clone(),
            fas_active.clone(),
            // 启动期收敛后的实验室模式：监听线程据此判断「文件没变就不重复套用」
            rhine_report.enabled.clone(),
        ))
    } else {
        log::warn!("{}", t("main-no-chiri-scheduler"));
        drop(rx);
        None
    };
    if let Some(Err(e)) = start_result {
        error!(
            "{}",
            t_with_args(
                "scheduler-module-start-failed",
                &fluent_args!("error" => e.to_string())
            )
        );
        return Err(e);
    }
    if chiri_active {
        info!("{}", t("scheduler-module-started"));
    }

    // [monitor_start]
    // 7. 启动 Monitor
    // 常规采样间隔按 SoC 参数化：Chiri 160ms，非 ChiRi（仅监控）沿用 200ms
    let sample_ms_normal: u64 = if chiri_active { 160 } else { 200 };
    let monitor_thread = thread::Builder::new()
        .name("monitor_core".to_string())
        .spawn(move || {
            if let Err(e) = monitor::start_monitor(tx, ak_active, sample_ms_normal, fas_active) {
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

    // [suspend]
    // 8. 挂起
    monitor_thread.join().unwrap();

    Ok(())
}
