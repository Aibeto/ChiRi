/*
 * Copyright (C) 2026 ChiRi
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

/// CPU 亲和与线程迁移控制器（ChiRi 专属）。
///
/// boost 模式（performance/fast/特调 akmode）下：
/// - `/dev/cpuset/top-app`、`foreground` 的 cpus 收窄到大核+超大核（区间随命中 SoC 变化）；
/// - `background`/`system-background`/`restricted` 的 cpus 压到小核，防后台任务抢大核；
/// - 可选写 `/dev/cpuctl/top-app/cpu.uclamp.min` 抬前台调度利用率下限；
/// - 可选写 `/dev/cpuctl/top-app/cpu.uclamp.max` 钳前台任务级性能上限（EAS 原生感知，
///   按机型 yaml 配置；内核 < 5.3 / 节点缺失 / 写入回读无效时自动纠正关闭）；
/// - 可选把前台进程全部线程 `sched_setaffinity` 迁移到大核+超大核（线程迁移）。
///
/// normal/doze 布局：仅保留「后台压小核」的省电收益，top-app/foreground 恢复快照。
/// 所有写入前先快照原值，release 时恢复；节点缺失或写入失败 warn 一次并跳过。
/// 写入去重：布局类型未变化时不重复写 sysfs，boost 下前台 PID 变化时仅重新迁移线程。
use crate::chiri::config::AffinityConfig;
use crate::utils::SysPathExist;
use log::{debug, info, warn};
use std::sync::Arc;

use crate::fluent_args;
use crate::i18n::{t, t_with_args};

/// 未应用任何布局（初始/release 后）
const KIND_NONE: u8 = 0;
/// boost 布局：top-app/foreground 大核+超大核
const KIND_BOOST: u8 = 1;
/// normal/doze 布局：top-app 恢复快照，后台压小核
const KIND_NORMAL: u8 = 2;

const GROUP_TOP_APP: &str = "top-app";
const GROUP_FOREGROUND: &str = "foreground";
/// boost/normal 布局下都压小核的后台分组（restricted 兜底相机的受限分组）
const BACKGROUND_GROUPS: [&str; 3] = ["background", "system-background", "restricted"];

/// 把 CPU ID 列表格式化为 cpuset cpus 值（逗号分隔，如 "3,4,5,6,7"）
fn format_cpu_list(mut ids: Vec<usize>) -> String {
    ids.sort_unstable();
    ids.dedup();
    ids.iter()
        .map(|c| c.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

/// 读取 cpuset 分组的 cpus 当前值；节点不存在/读失败返回 None
fn read_cpuset_cpus(group: &str) -> Option<String> {
    let path = format!("/dev/cpuset/{}/cpus", group);
    std::fs::read_to_string(&path)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// 写 cpuset 分组的 cpus 值，失败打 warn
fn write_cpuset_cpus(group: &str, value: &str) {
    let path = format!("/dev/cpuset/{}/cpus", group);
    if crate::utils::try_write_file(&path, value).is_err() {
        warn!(
            "{}",
            t_with_args("affinity-write-failed", &fluent_args!("path" => path))
        );
    }
}

/// 枚举前台进程的全部线程 TID（/proc/<pid>/task）
fn thread_tids(pid: i32) -> Vec<i32> {
    let mut tids = Vec::new();
    if let Ok(entries) = std::fs::read_dir(format!("/proc/{}/task", pid)) {
        for entry in entries.flatten() {
            if let Some(name) = entry.file_name().to_str() {
                if let Ok(tid) = name.parse::<i32>() {
                    tids.push(tid);
                }
            }
        }
    }
    tids
}

/// 设置单个线程的 CPU 亲和掩码。
/// 用 zeroed 的 `cpu_set_t` 分配：保证对齐与布局合法（`vec![0u8]` 起始地址对齐仅 1，
/// 强转 `*const cpu_set_t` 是未对齐指针，依赖 libc 包装器不解引用的运气，不可靠）。
/// 经 `libc::CPU_SET` 置位（bionic 64 位下 cpu_set_t = [u64; 16]，支持 1024 CPU）；
/// CPU_SET 内部数组索引无越界检查，超出容量直接跳过（与原字节掩码防护等价）。
/// 成功返回 true；线程已退出（ESRCH）等错误返回 false。
fn set_tid_affinity(tid: i32, cpu_ids: &[usize]) -> bool {
    let mut mask: libc::cpu_set_t = unsafe { std::mem::zeroed() };
    let max_cpu = std::mem::size_of::<libc::cpu_set_t>() * 8;
    for &c in cpu_ids {
        if c < max_cpu {
            unsafe { libc::CPU_SET(c, &mut mask) };
        }
    }
    let ret =
        unsafe { libc::sched_setaffinity(tid, std::mem::size_of::<libc::cpu_set_t>(), &mask) };
    ret == 0
}

/// 读内核版本 (major, minor, patch)，解析 /proc/sys/kernel/osrelease
/// （如 "5.15.78-android13-8-g..."，失败返回 None）。
fn kernel_version() -> Option<(u32, u32, u32)> {
    let s = std::fs::read_to_string("/proc/sys/kernel/osrelease").ok()?;
    let head = s.trim().split(['-', '+', ' ']).next()?;
    let mut it = head.split('.');
    Some((
        it.next()?.parse().ok()?,
        it.next()?.parse().ok()?,
        it.next().and_then(|v| v.parse().ok()).unwrap_or(0),
    ))
}

/// cpu.uclamp.max 支持状态：Unknown = 未判定，Ok = 可用，Unsupported = 已纠正关闭。
/// uclamp 主线自 v5.3 引入，老内核（如 8998 的 4.4）无该节点；
/// 厂商 backport 的实现质量不一，故版本判定之外再做写入回读验证。
enum UclampSupport {
    Unknown,
    Ok,
    Unsupported,
}

/// top-app 的 cpu.uclamp.max 节点路径
const UCLAMP_MAX_PATH: &str = "/dev/cpuctl/top-app/cpu.uclamp.max";

/// CPU 亲和控制器
pub struct AffinityManager {
    /// sysfs 路径存在性缓存（复用 SysPathExist 已探测的 cpuset/cpuctl 能力位）
    sys: Arc<SysPathExist>,
    /// 首次写入前读取的快照：cpus 文件路径 → 原值；None = 尚未快照
    snapshot: Option<Vec<(String, String)>>,
    /// cpu.uclamp.min 快照值；None = 未快照或节点不存在
    uclamp_snapshot: Option<String>,
    /// cpu.uclamp.max 快照值；None = 未快照
    uclamp_max_snapshot: Option<String>,
    /// cpu.uclamp.max 支持状态（机型配置开启时惰性判定，识别后缓存避免重复探测）
    uclamp_max_support: UclampSupport,
    /// 当前生效布局类型（去重用）
    applied_kind: u8,
    /// 上次迁移线程的前台 PID（去重用）
    last_pinned_pid: i32,
}

impl AffinityManager {
    pub fn new(sys: Arc<SysPathExist>) -> Self {
        Self {
            sys,
            snapshot: None,
            uclamp_snapshot: None,
            uclamp_max_snapshot: None,
            uclamp_max_support: UclampSupport::Unknown,
            applied_kind: KIND_NONE,
            last_pinned_pid: 0,
        }
    }

    /// 快照所有将被修改的节点原值（只做一次；uclamp 节点不存在时快照为空串）
    fn ensure_snapshot(&mut self) {
        if self.snapshot.is_some() {
            return;
        }
        let mut snap = Vec::new();
        for group in BACKGROUND_GROUPS
            .iter()
            .copied()
            .chain([GROUP_TOP_APP, GROUP_FOREGROUND])
        {
            if let Some(v) = read_cpuset_cpus(group) {
                snap.push((format!("/dev/cpuset/{}/cpus", group), v));
            }
        }
        self.snapshot = Some(snap);

        let uclamp_path = "/dev/cpuctl/top-app/cpu.uclamp.min";
        self.uclamp_snapshot = Some(
            std::fs::read_to_string(uclamp_path)
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_default(),
        );
        self.uclamp_max_snapshot = Some(
            std::fs::read_to_string(UCLAMP_MAX_PATH)
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_default(),
        );
    }

    /// 应用亲和布局。`boost` 由调用方按模式判定（performance/fast/特调）。
    ///
    /// 去重规则：布局类型未变时跳过 cpuset 写入；boost 下前台 PID 变化时
    /// 仅重新迁移线程（App 同模式切换不产生 ModeChange 事件，靠周期刷新兜底）。
    pub fn apply(&mut self, screen_on: bool, fg_pid: i32, cfg: &AffinityConfig, boost: bool) {
        // 开关关闭：恢复系统原始布局（等同 release），不驻留任何写入
        if !cfg.enabled {
            self.release();
            return;
        }

        let ranges = crate::common::chiri_core_ranges();
        let boost_cpus: Vec<usize> = ranges.big.clone().chain(ranges.prime.clone()).collect();
        let little_cpus: Vec<usize> = ranges.little.clone().collect();
        let boost_list = format_cpu_list(boost_cpus.clone());
        let little_list = format_cpu_list(little_cpus.clone());
        // 全核列表：0..prime.end（8998 无 prime 时取 big.end）
        let all_cpus: Vec<usize> = (0..ranges.prime.end.max(ranges.big.end)).collect();

        let old_kind = self.applied_kind;
        if boost {
            self.ensure_snapshot();
            if old_kind != KIND_BOOST {
                // top-app / foreground 收窄到大核+超大核
                if self.sys.cpuset_top_app_exist {
                    write_cpuset_cpus(GROUP_TOP_APP, &boost_list);
                }
                if self.sys.cpuset_foreground_exist {
                    write_cpuset_cpus(GROUP_FOREGROUND, &boost_list);
                }
                self.apply_uclamp(cfg);
                self.pin_background(&little_list);
                info!(
                    "{}",
                    t_with_args(
                        "affinity-boost-applied",
                        &fluent_args!("big" => boost_list, "little" => little_list)
                    )
                );
            }
            self.applied_kind = KIND_BOOST;

            // 线程迁移：boost 下前台 PID 变化（或首次/亮屏）时迁移到大核+超大核
            if cfg.pin_foreground_threads
                && screen_on
                && fg_pid > 0
                && fg_pid != self.last_pinned_pid
            {
                self.pin_threads(fg_pid, &boost_cpus);
            }
        } else {
            // normal/doze 布局：top-app/foreground/uclamp 恢复快照，后台保持压小核
            if old_kind == KIND_BOOST {
                self.restore_foreground_groups();
                self.restore_uclamp();
                self.restore_uclamp_max();
                if cfg.pin_foreground_threads {
                    self.pin_threads(fg_pid, &all_cpus); // 恢复全核亲和
                }
                info!("{}", t("affinity-normal-restore"));
            }
            if old_kind != KIND_NORMAL {
                self.pin_background(&little_list);
            }
            self.applied_kind = KIND_NORMAL;
            self.last_pinned_pid = fg_pid;
        }
    }

    /// 后台分组压小核（仅布局切换时调用，避免高频重复写 sysfs）
    fn pin_background(&self, little_list: &str) {
        for group in BACKGROUND_GROUPS {
            let exist = match group {
                "background" => self.sys.cpuset_background_exist,
                "system-background" => self.sys.cpuset_system_background_exist,
                _ => self.sys.cpuset_restricted_exist,
            };
            if exist {
                write_cpuset_cpus(group, little_list);
            }
        }
    }

    /// 写 top-app 的 uclamp.min（配置为 0 时不启用，避免与 CLG 上限语义打架）
    fn apply_uclamp(&self, cfg: &AffinityConfig) {
        if cfg.top_app_uclamp_min_pct > 0 && self.sys.cpuctl_top_app_exist {
            crate::utils::try_write_file(
                "/dev/cpuctl/top-app/cpu.uclamp.min",
                &cfg.top_app_uclamp_min_pct.to_string(),
            );
        }
    }

    /// 恢复 top-app/foreground 的快照布局
    fn restore_foreground_groups(&self) {
        if let Some(snap) = &self.snapshot {
            for (path, val) in snap {
                if path.contains(GROUP_TOP_APP) || path.contains(GROUP_FOREGROUND) {
                    let _ = crate::utils::try_write_file(path, val);
                }
            }
        }
    }

    /// 恢复 uclamp.min 快照（空串 = 节点原本不存在或不可读，跳过）
    fn restore_uclamp(&self) {
        if let Some(v) = &self.uclamp_snapshot {
            if !v.is_empty() {
                let _ = crate::utils::try_write_file("/dev/cpuctl/top-app/cpu.uclamp.min", v);
            }
        }
    }

    /// boost 模式下写 top-app 的 cpu.uclamp.max（任务级性能上限钳制，EAS 原生感知）。
    /// 配置为 0 时不启用。三重兜底纠正（任一不满足即置 Unsupported 永久跳过，只打点一次）：
    /// 1. 内核版本识别：uclamp 主线 v5.3 引入，< 5.3 判定不支持（防老内核配了也白配）；
    /// 2. 节点存在性：/dev/cpuctl/top-app/cpu.uclamp.max 必须存在（防半成品 backport）；
    /// 3. 写入回读验证：写入 "NN.00" 后回读比对（防厂商内核静默忽略写入）。
    fn apply_uclamp_max(&mut self, cfg: &AffinityConfig) {
        let pct = cfg.top_app_uclamp_max_pct;
        if pct == 0 || self.uclamp_max_support == UclampSupport::Unsupported {
            return;
        }
        if self.uclamp_max_support == UclampSupport::Unknown {
            let ver = kernel_version();
            let ver_ok = ver.map_or(false, |(a, b, _)| a > 5 || (a == 5 && b >= 3));
            let node_ok = std::path::Path::new(UCLAMP_MAX_PATH).exists();
            if !ver_ok || !node_ok {
                self.uclamp_max_support = UclampSupport::Unsupported;
                log::warn!(
                    "{}",
                    t_with_args(
                        "affinity-uclamp-unavailable",
                        &fluent_args!(
                            "version" => ver
                                .map(|(a, b, _)| format!("{a}.{b}"))
                                .unwrap_or_else(|| "unknown".to_string()),
                            "reason" => if ver_ok { "node missing" } else { "kernel < 5.3" }
                        )
                    )
                );
                return;
            }
        }
        // 回读验证：uclamp.max 内核格式 "NN.NN"，解析为数值与配置比对
        let val = format!("{pct}.00");
        if crate::utils::try_write_file(UCLAMP_MAX_PATH, &val).is_ok() {
            let applied = std::fs::read_to_string(UCLAMP_MAX_PATH)
                .ok()
                .and_then(|s| s.trim().parse::<f32>().ok())
                .map(|v| (v - pct as f32).abs() < 0.01)
                .unwrap_or(false);
            if applied {
                self.uclamp_max_support = UclampSupport::Ok;
            } else {
                self.uclamp_max_support = UclampSupport::Unsupported;
                log::warn!(
                    "{}",
                    t_with_args(
                        "affinity-uclamp-unavailable",
                        &fluent_args!(
                            "version" => kernel_version()
                                .map(|(a, b, _)| format!("{a}.{b}"))
                                .unwrap_or_else(|| "unknown".to_string()),
                            "reason" => "write not applied"
                        )
                    )
                );
            }
        }
    }

    /// 恢复 uclamp.max 快照（空串 = 节点原本不存在或不可读，跳过）
    fn restore_uclamp_max(&self) {
        if let Some(v) = &self.uclamp_max_snapshot {
            if !v.is_empty() {
                let _ = crate::utils::try_write_file(UCLAMP_MAX_PATH, v);
            }
        }
    }

    /// 把前台进程全部线程迁移到指定 CPU 集合（sched_setaffinity），
    /// 迁移结果打 debug 摘要并记录 PID 去重。
    fn pin_threads(&mut self, pid: i32, cpu_ids: &[usize]) {
        let tids = thread_tids(pid);
        let total = tids.len();
        let mut pinned = 0;
        for tid in &tids {
            if set_tid_affinity(*tid, cpu_ids) {
                pinned += 1;
            }
        }
        self.last_pinned_pid = pid;
        if total > 0 {
            debug!(
                "{}",
                t_with_args(
                    "affinity-pin-threads",
                    &fluent_args!(
                        "pid" => pid.to_string(),
                        "pinned" => pinned.to_string(),
                        "total" => total.to_string()
                    )
                )
            );
        } else {
            debug!(
                "{}",
                t_with_args(
                    "affinity-pin-failed",
                    &fluent_args!("pid" => pid.to_string())
                )
            );
        }
    }

    /// 释放全部接管：恢复所有 cpuset/uclamp 快照，前台线程恢复全核亲和。
    /// 由调度线程收尾与开关关闭时调用。
    pub fn release(&mut self) {
        if self.applied_kind == KIND_NONE {
            return;
        }
        if let Some(snap) = self.snapshot.take() {
            for (path, val) in &snap {
                let _ = crate::utils::try_write_file(path, val);
            }
        }
        self.restore_uclamp();
        self.restore_uclamp_max();
        let ranges = crate::common::chiri_core_ranges();
        let all_cpus: Vec<usize> = (0..ranges.prime.end.max(ranges.big.end)).collect();
        let pid = self.last_pinned_pid;
        if pid > 0 {
            let tids = thread_tids(pid);
            for tid in &tids {
                set_tid_affinity(*tid, &all_cpus);
            }
            debug!(
                "{}",
                t_with_args(
                    "affinity-threads-restored",
                    &fluent_args!("pid" => pid.to_string(), "count" => tids.len().to_string())
                )
            );
        }
        self.applied_kind = KIND_NONE;
        self.last_pinned_pid = 0;
        info!("{}", t("affinity-released"));
    }
}
