//! touch_detect.rs: [consts] [monitor] [poll_wait]

use log::{debug, info, warn};
use std::fs;
use std::io::Read;
use std::os::unix::io::AsRawFd;
use std::sync::mpsc::SyncSender;
use std::time::Duration;

use crate::fluent_args;
use crate::i18n::{t, t_with_args};

// [consts]
// evdev 事件常量（linux/input-event-codes.h）
const EV_KEY: u16 = 0x01;
const EV_ABS: u16 = 0x03;
const BTN_TOUCH: u16 = 0x14a; // 330
const ABS_MT_TRACKING_ID: u16 = 0x39; // 57
/// 64 位 Android 下 struct input_event 长度：timeval(16) + type(2) + code(2) + value(4)
const INPUT_EVENT_SIZE: usize = 24;

// [monitor]
/// 触摸检测线程：读取全部 /dev/input/event* 输入设备，检测触摸按下事件，
/// 并把触摸事件通过 `tx` 发给 scheduler_ipc（事件驱动，即时触发 CLG 大核升频）。
/// 阻塞运行（poll + 阻塞 read），随守护进程退出消亡。
pub fn monitor_touch(tx: SyncSender<()>) {
    info!("{}", t("touch-detect-started"));

    // 外层循环：枚举设备；设备增删/读取异常时重新枚举
    loop {
        let mut devices: Vec<std::fs::File> = Vec::new();
        if let Ok(entries) = fs::read_dir("/dev/input") {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with("event") {
                    let path = format!("/dev/input/{name}");
                    if let Ok(f) = std::fs::OpenOptions::new().read(true).open(&path) {
                        devices.push(f);
                    }
                }
            }
        }

        if devices.is_empty() {
            warn!("{}", t("touch-detect-no-devices"));
            std::thread::sleep(Duration::from_secs(3));
            continue;
        }

        // 内层循环：poll 阻塞等待可读（超时 -1，无事件零唤醒）
        'poll: loop {
            let mut fds: Vec<libc::pollfd> = devices
                .iter()
                .map(|f| libc::pollfd {
                    fd: f.as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                })
                .collect();

            // [poll_wait] 超时 -1 = 无限期阻塞。旧 200ms 超时分支（ret==0 → continue）
            // 不改任何状态、只是每秒 5 次空唤醒，删掉与保留等价；事件处理与重枚举时机不变
            let ret = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, -1) };
            if ret < 0 {
                debug!("{}", t("touch-detect-poll-error"));
                std::thread::sleep(Duration::from_millis(500));
                break; // 重新枚举
            }

            let mut buf = [0u8; INPUT_EVENT_SIZE];
            for (i, pfd) in fds.iter().enumerate() {
                // 挂断/错误/无效描述符（部分内核在设备断开时上报）：与读错误同路
                // 重新枚举。阻塞等待后没有超时兜底，这类 revents 必须显式处理，
                // 否则 poll 立即返回却无事可做，会原地空转
                if pfd.revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0 {
                    // [poll_backoff] 异常 revents 多为电平触发，重枚举后 poll 会立即
                    // 返回；退避 500ms（与 poll 出错路径同款）防持续异常时紧循环空转。
                    std::thread::sleep(Duration::from_millis(500));
                    break 'poll;
                }
                if pfd.revents & libc::POLLIN == 0 {
                    continue;
                }
                match devices[i].read(&mut buf) {
                    // 读到 0 字节或读取失败 = 设备断开/异常，重新枚举
                    // [poll_backoff] 读失败多为驱动持续异常（POLLIN 电平触发但 read
                    // 恒败），与异常 revents 同款退避 500ms，防重枚举后立即再失败的紧循环
                    Ok(0) | Err(_) => {
                        std::thread::sleep(Duration::from_millis(500));
                        break 'poll;
                    }
                    Ok(n) => {
                        // 一次 read 可能包含多个 input_event，逐个解析
                        let mut off = 0;
                        while off + INPUT_EVENT_SIZE <= n {
                            let etype = u16::from_ne_bytes([buf[off + 16], buf[off + 17]]);
                            let code = u16::from_ne_bytes([buf[off + 18], buf[off + 19]]);
                            let value = i32::from_ne_bytes([
                                buf[off + 20],
                                buf[off + 21],
                                buf[off + 22],
                                buf[off + 23],
                            ]);
                            // 触摸按下：BTN_TOUCH 置位，或 ABS_MT_TRACKING_ID 出现有效触点（>= 0）
                            let touched = (etype == EV_KEY && code == BTN_TOUCH && value == 1)
                                || (etype == EV_ABS && code == ABS_MT_TRACKING_ID && value >= 0);
                            if touched {
                                // 事件驱动：向 scheduler_ipc 发送触摸事件（非阻塞，
                                // 通道满时丢弃，多余的触摸事件丢了不碍事）
                                let _ = tx.try_send(());
                                debug!(
                                    "{}",
                                    t_with_args(
                                        "touch-detect-down",
                                        &fluent_args!(
                                            "type" => etype.to_string(),
                                            "code" => code.to_string()
                                        )
                                    )
                                );
                            }
                            off += INPUT_EVENT_SIZE;
                        }
                    }
                }
            }
        }
    }
}
