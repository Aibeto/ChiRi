//! down.rs: [consts] [state] [flag] [watch]
//!
//! DOWN 模式（调度停摆）：模块根 `down.chr` 写着保留字 `down` 时，调度关停**全部**
//! 调度功能，只留下采集与日志——eBPF、status.csv、daemon.log、devimp 照常，
//! 记录的就是「ChiRi 不工作时」的设备调度情况，便于和接管时做对比。
//!
//! 与 `rhine.chr` 同款：对外暴露、可手改、内容即状态（缺失或只有注释 = 正常调度）。
//! 因为文件在磁盘上，**重启设备后仍保持停摆**，只能改文件解除。
//!
//! 判据只认 `down.chr`；`current_mode.chr` 里的 `down` 是它的对外投影（给外部工具看
//! 当前是不是停摆），调度线程在停摆期间不再覆盖那个文件——否则一覆盖就等于退出停摆。
//!
//! 释放/恢复动作放在调度循环里（而不是这里），因为 governor 对象是那个线程独占的，
//! 这里只提供「当前该不该停摆」这一个事实。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use inotify::WatchMask;
use log::{info, warn};

use crate::common;
use crate::fluent_args;
use crate::i18n::{t, t_with_args};
use crate::utils;

// [consts]
/// 对外暴露的停摆状态文件（模块根）
pub const DOWN_CHR: &str = "down.chr";
/// 停摆期间写进 `current_mode.chr` 的模式值
pub const DOWN_MODE: &str = "down";
/// 随模块下发的内容模板，同时是「文件缺失」时补建的内容（只有注释 = 正常调度）
pub const DOWN_DEFAULT_CHR: &str = include_str!("../module/down.chr");
/// 监听异常后的重建间隔（与另外两套 watcher 同口径）
const WATCH_RETRY_BACKOFF: Duration = Duration::from_secs(2);

// [state]
/// 解析 `down.chr` 并顺带兜底：文件缺失就地补建模板。
/// 写了保留字 `down`（忽略大小写、允许成对引号）= 停摆；空文件/只有注释/其它内容 = 正常。
fn read_down(root: &Path) -> bool {
    let path = root.join(DOWN_CHR);
    let Ok(text) = fs::read_to_string(&path) else {
        // 缺失/不可读：补建模板，让用户看得到该文件与写法（与 rhine.chr 同款）。
        // 读不到按「不停摆」处理——绝不因为一个状态文件异常就让调度自己关掉。
        let _ = common::write_file_no_panic(&path, DOWN_DEFAULT_CHR.as_bytes());
        return false;
    };
    let body: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect();
    match body.first() {
        Some(raw) => raw
            .trim_matches(|c| c == '"' || c == '\'')
            .eq_ignore_ascii_case(DOWN_MODE),
        None => false,
    }
}

// [flag]
/// 停摆中。调度循环高频读它，那里不许再去读文件。
static DOWN_ACTIVE: AtomicBool = AtomicBool::new(false);

/// 当前是否处于 DOWN 停摆（进程级，读原子量）
pub fn is_down() -> bool {
    DOWN_ACTIVE.load(Ordering::Acquire)
}

// [watch]
/// 启动期读取一次。必须在调度线程起循环之前调用（ChiRi 专属）。
pub fn on_startup(root: &Path) -> bool {
    let v = read_down(root);
    DOWN_ACTIVE.store(v, Ordering::Release);
    v
}

/// 盯模块根的 `down.chr`：写入或删除即生效，与 meta.yaml、rhine.chr 同语义，不用重启调度。
pub fn watch_loop(root: PathBuf) {
    let mut watcher: Option<utils::DirWatcher> = None;
    loop {
        if watcher.is_none() {
            // 比默认多一个 DELETE：这个文件的**删除**同样表示解除停摆，而默认掩码只认
            // CLOSE_WRITE / MOVED_TO，删掉时不会醒（配置与实验室那两条链路没这需求）
            match utils::DirWatcher::new_with_mask(
                &root,
                WatchMask::CLOSE_WRITE | WatchMask::MOVED_TO | WatchMask::DELETE,
            ) {
                Ok(w) => watcher = Some(w),
                Err(e) => {
                    warn!(
                        "{}",
                        t_with_args("down-watch-error", &fluent_args!("error" => e.to_string()))
                    );
                    std::thread::sleep(WATCH_RETRY_BACKOFF);
                    continue;
                }
            }
        }
        if let Err(e) = watcher.as_mut().unwrap().wait_change(DOWN_CHR) {
            warn!(
                "{}",
                t_with_args("down-watch-error", &fluent_args!("error" => e.to_string()))
            );
            watcher = None;
            std::thread::sleep(WATCH_RETRY_BACKOFF);
            continue;
        }
        let next = read_down(&root);
        let prev = DOWN_ACTIVE.swap(next, Ordering::AcqRel);
        if next != prev {
            info!(
                "{}",
                t(if next {
                    "down-enabled"
                } else {
                    "down-disabled"
                })
            );
        }
    }
}
