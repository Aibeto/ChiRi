//! notify.rs: [content] [dispatch] [worker] [post]
// 常驻状态通知（daemon → 系统通知栏）：通过 shell 工具 `cmd notification post` 投递，
// 内容为调度状态快照：前台包名（标题）/ 模式 / 家族 / 子模式 / 温度 / 功耗。
// 只有 daemon 能持续更新（WebUI 只在打开时存在），故由 1s 调度循环按周期调用；
// **投递在 notify 自己的线程上串行执行**（容量 1 的通道，落后就丢本次）：`cmd` 是新建
// 进程，快慢不可控，不能让调度循环等它——内容去重、失败冷却、进程创建都在该线程里。
// 「关闭调度」时 daemon 是被信号杀死的（没有清理时机），由 WebUI 调用
// `cmd notification post -d chiri-status` 取消。
//
// 内容口径（用户要求 2026-09-18）：
// - 标题 = 当前前台进程/包名（取不到时用模块名兜底）；
// - 正文**单行** = 模式 · 家族 · 子模式 · 温度 · 功耗，各参数**只出值、不带字段标签**
//   （用户口径 2026-09-18），由 ` · ` 分隔；家族与子模式按信息量去重（同下）；
// - 功耗随 meta.yaml `power_avg`（高级设置里的开关）取参考值或平均值——**值本身就是该
//   口径下的结果，通知只出数值、不标注是哪一种**（用户口径）。

use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::sync::{Mutex, OnceLock};

use crate::fluent_args;
use crate::i18n::{t, t_with_args};

/// 通知 tag：更新与取消都靠它定位同一条通知
pub const TAG: &str = "chiri-status";
/// 通知渠道 id（Android 8+ 需要；渠道不存在时由系统按 id 建默认渠道）
const CHANNEL: &str = "chiri-status";
/// `/system/bin/cmd`（Android 8+ 自带；不走 `sh -c`，参数直接传避免任何引号问题）
const CMD: &str = "/system/bin/cmd";

/// 更新周期（秒）：包名/模式变化要跟手，功耗读数变化慢——5s 是响应与进程创建开销的折中
pub const INTERVAL_SECS: u32 = 5;

/// 上次成功投递的（标题, 正文）：内容没变就跳过，省掉一次进程创建
static LAST_POSTED: Mutex<Option<(String, String)>> = Mutex::new(None);
/// 所有候选命令行都失败后的告警去重（本进程只报一次）
static POST_WARNED: AtomicBool = AtomicBool::new(false);
/// 上次全部候选失败的时刻：失败后进入冷却，避免在通知不可用的机型上每 5s 反复起进程
static LAST_FAIL: Mutex<Option<std::time::Instant>> = Mutex::new(None);
/// 失败冷却时长（成功投递会清除冷却记录，内容变化不受此限）
const FAIL_COOLDOWN: std::time::Duration = std::time::Duration::from_secs(60);

/// 通知快照：1s 循环里现成的数据
pub struct Snapshot<'a> {
    /// 前台包名（标题；空串则回退模块名）
    pub pkg: &'a str,
    /// current_mode 的原始值（如 default / akmode / down）
    pub mode: &'a str,
    /// 电池温度（°C）
    pub batt_temp: Option<f32>,
    /// CPU 温度（°C）
    pub cpu_temp: Option<f32>,
    /// PowerAVG 当前留存值（W）。口径（参考/平均）由 meta.power_avg 决定，值本身就是
    /// 该口径下的结果，通知不再标注是哪种
    pub power_w: Option<f32>,
}

// [content]
/// 模式家族（与 WebUI `data/mode.ts` 的 ModeKind 对齐）。名字用拉丁字面量，与 zh locale
/// 的 `mode.family.*` 保持一致，不另做本地化。
fn family(mode: &str) -> &'static str {
    match mode {
        "down" => "DOWN",
        "scenemode" => "Stardust",
        "fas" => "FAS",
        "reduce" | "default" | "boost" => "CLG",
        "vector" | "contingency" | "babel" => "RHINE",
        _ if is_special(mode) => "Special",
        _ => "UNKNOWN",
    }
}

/// 特调模式集合：编译期嵌入的 `special_tuned.yaml` 里 modes 的并集（首次调用构建一次）。
/// 正则条目无法按包名精确匹配，但模式名本身照收——模式展示只需要名字。
fn is_special(mode: &str) -> bool {
    static SPECIALS: OnceLock<std::collections::HashSet<String>> = OnceLock::new();
    let set = SPECIALS.get_or_init(|| {
        crate::common::special_tuned_entries()
            .iter()
            .flat_map(|entry| entry.modes.iter().cloned())
            .collect()
    });
    set.contains(mode)
}

/// 模式展示名：CLG/实验室/FAS/down 以及**特调**都直接用 id 本身（特调类名「特调」
/// 不含新模式信息，用户要求删掉——`akmode`、`playback` 这些 id 才是具体模式），
/// 只有息屏场景与未注册值有本地化名。
fn mode_label(mode: &str) -> String {
    if mode == "scenemode" {
        return t("notify-mode-scenemode");
    }
    if family(mode) == "UNKNOWN" {
        return t("notify-mode-unknown");
    }
    mode.to_string()
}

/// 家族是否提供「模式展示名」之外的信息：clg / lab / stardust 的成员名（default /
/// vector / 息屏场景）看不出所属家族，需要单列；special / unknown / fas / down 则是
/// 「类名 = 家族」（特调与 Special、未知与 UNKNOWN 是同一个词），再列一遍等于重复
/// —— 与 WebUI 模式卡片的 `FAMILY_KINDS` 同一口径。
fn family_adds_info(family: &str) -> bool {
    matches!(family, "CLG" | "RHINE" | "Stardust")
}

/// 温度格式化（缺测写 -，与 status.csv 同口径）
fn fmt_temp(v: Option<f32>) -> String {
    v.map(|x| format!("{x:.1}")).unwrap_or_else(|| "-".to_string())
}

/// 功耗格式化（缺测写 -）
fn fmt_power(v: Option<f32>) -> String {
    v.map(|x| format!("{x:.2}")).unwrap_or_else(|| "-".to_string())
}

/// 正文各参数之间的分隔符（单行展示，用户口径 2026-09-18）
const SEPARATOR: &str = " · ";

/// 组装正文：**单行**列出各参数（模式 · 家族 · 子模式 · 温度 · 功耗），用 [`SEPARATOR`]
/// 分隔——各参数只出值、不带「模式：」这类字段标签（用户口径），靠顺序与单位自解释；
/// 折起时系统按需截断，展开（bigtext）看全。
/// 去重（与 WebUI 模式卡片同口径）：
/// - 「家族」只在它提供模式名之外的信息时出现（clg/lab/stardust；特调与 Special、
///   未知与 UNKNOWN 是同一个词，同时出现等于重复两次）；
/// - 「子模式」只在它与展示名不是同一个词时出现（CLG 与实验室档的展示名就是 id）。
fn body(snap: &Snapshot) -> String {
    let label = mode_label(snap.mode);
    let mut lines = vec![t_with_args(
        "notify-line-mode",
        &fluent_args!("mode" => label.clone()),
    )];
    if family_adds_info(family(snap.mode)) {
        lines.push(t_with_args(
            "notify-line-family",
            &fluent_args!("family" => family(snap.mode).to_string()),
        ));
    }
    if !snap.mode.is_empty() && snap.mode != label {
        lines.push(t_with_args(
            "notify-line-submode",
            &fluent_args!("sub" => snap.mode.to_string()),
        ));
    }
    lines.push(t_with_args(
        "notify-line-temp",
        &fluent_args!(
            "batt" => fmt_temp(snap.batt_temp),
            "cpu" => fmt_temp(snap.cpu_temp)
        ),
    ));
    lines.push(t_with_args(
        "notify-line-power",
        &fluent_args!("watt" => fmt_power(snap.power_w)),
    ));
    lines.join(SEPARATOR)
}

// [dispatch]
/// 请求更新常驻通知：标题 = 前台包名（空则模块名兜底），正文见 [`body`]。
/// 组装（纯格式化）留在调用线程，投递交给 [`worker`] 线程——调度循环不碰进程创建，
/// 也不会被 `cmd` 的启动与等待拖住。
pub fn update(snap: &Snapshot) {
    let title = if snap.pkg.trim().is_empty() {
        t("notify-title-fallback")
    } else {
        snap.pkg.trim().to_string()
    };
    send(Msg::Post {
        title,
        text: body(snap),
    });
}

/// 请求撤销常驻通知（meta.yaml `notify: false` 时由调度循环调用；daemon 自己还活着，
/// 有能力清理，不必等 WebUI 的 stopScheduler）。
/// `force` = 无条件投一条 `-d`（用于启动首轮：可能残留上一次运行投递的通知）；
/// 否则只在本次确实投递过时才动手（每轮调用都幂等、不反复起进程）。同样非阻塞。
pub fn cancel(force: bool) {
    send(Msg::Cancel { force });
}

/// 投递线程的待办：`Post` 带的是**已格式化**的标题与正文（组装在调用方，这里只管投）
enum Msg {
    Post { title: String, text: String },
    Cancel { force: bool },
}

/// 投递线程发送端（懒启动，进程内一条）。与通知渠道常量 `CHANNEL` 是两回事
static WORKER_TX: OnceLock<SyncSender<Msg>> = OnceLock::new();

/// 取发送端，首次调用时起线程。线程起不来就只剩发送端（`try_send` 得到
/// Disconnected）→ 退化成「不投递」，调度线程照旧跑。
fn sender() -> &'static SyncSender<Msg> {
    WORKER_TX.get_or_init(|| {
        let (tx, rx) = sync_channel::<Msg>(1);
        if let Err(e) = std::thread::Builder::new()
            .name("notify".to_string())
            .spawn(move || worker(rx))
        {
            log::warn!("[notify] worker thread spawn failed: {e}");
        }
        tx
    })
}

/// 非阻塞投递：容量 1，投递线程还在忙上一条就丢本次（下个 5s 周期会再送）。
/// 这里是调度线程的路径，**绝不能阻塞**——`cmd` 是新建进程，快慢不可控。
fn send(msg: Msg) {
    match sender().try_send(msg) {
        Ok(()) => {}
        Err(TrySendError::Full(_)) => log::debug!("[notify] worker busy, update dropped"),
        Err(TrySendError::Disconnected(_)) => {
            log::debug!("[notify] worker unavailable, update dropped")
        }
    }
}

// [worker]
/// 投递线程主体：串行处理消息——内容去重、失败冷却、进程创建全在调度线程之外
fn worker(rx: Receiver<Msg>) {
    while let Ok(msg) = rx.recv() {
        match msg {
            Msg::Post { title, text } => handle_post(&title, &text),
            Msg::Cancel { force } => handle_cancel(force),
        }
    }
}

/// 投递：内容与上次成功投递的完全一致时跳过（省一次进程创建）；上一次全军覆没后
/// 60s 内不再尝试（这条通道多半整机不可用）
fn handle_post(title: &str, text: &str) {
    {
        let last = LAST_POSTED.lock().unwrap_or_else(|e| e.into_inner());
        let same = last
            .as_ref()
            .map(|(t, x)| t == title && x == text)
            .unwrap_or(false);
        if same {
            return;
        }
    }
    {
        let fail = LAST_FAIL.lock().unwrap_or_else(|e| e.into_inner());
        if (*fail).map(|at| at.elapsed() < FAIL_COOLDOWN).unwrap_or(false) {
            return;
        }
    }
    let ok = post(title, text);
    *LAST_FAIL.lock().unwrap_or_else(|e| e.into_inner()) = if ok {
        None
    } else {
        Some(std::time::Instant::now())
    };
    if ok {
        let mut last = LAST_POSTED.lock().unwrap_or_else(|e| e.into_inner());
        *last = Some((title.to_string(), text.to_string()));
    }
}

/// 撤销：`force` 无条件投一条 `-d`（启动首轮清上一次运行的残留），否则只在本次确实
/// 投递过时才动手
fn handle_cancel(force: bool) {
    {
        let mut last = LAST_POSTED.lock().unwrap_or_else(|e| e.into_inner());
        if !force && last.is_none() {
            return;
        }
        *last = None;
    }
    let args = ["notification", "post", "-d", TAG];
    let ok = Command::new(CMD)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !ok {
        log::debug!("[notify] cancel attempt failed: {}", args.join(" "));
    }
}

/// 按候选命令行顺序尝试投递（各 ROM 对 `cmd notification` 的旗标支持不同）：
/// ① 渠道 + 常驻（`-c`/`-o`）→ ② 去渠道 → ③ 最简形式。
/// 每条失败记 debug（属「候选节点」口径），**全部失败**才打一条 warn（本进程一次）。
fn post(title: &str, text: &str) -> bool {
    let attempts: [Vec<&str>; 3] = [
        vec![
            "notification", "post", "-S", "bigtext", "-t", title, "-c", CHANNEL, "-o", TAG, text,
        ],
        vec!["notification", "post", "-S", "bigtext", "-t", title, "-o", TAG, text],
        vec!["notification", "post", "-t", title, TAG, text],
    ];
    for args in attempts {
        // 丢弃子进程输出：`cmd` 在旗标不支持时会把用法/错误文本打到 stderr，
        // 默认继承会直接灌进 daemon.log（候选失败属预期路径，只需 debug 打点）
        let ok = Command::new(CMD)
            .args(&args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if ok {
            return true;
        }
        log::debug!("[notify] post attempt failed: {}", args.join(" "));
    }
    if !POST_WARNED.swap(true, Ordering::Relaxed) {
        log::warn!("{}", t("notify-post-failed"));
    }
    false
}
