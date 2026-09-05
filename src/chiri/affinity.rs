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

/// CPU 亲和与线程迁移控制器（ChiRi 专属，按核心粒度放置，低开销版）。
///
/// 分层：
/// - cgroup 层：boost（performance/fast/特调）下收窄 top-app/foreground cpuset
///   到大核+超大核、后台分组压小核；可选 uclamp.min/max；normal/doze 恢复快照。
/// - 线程层：
///   * 前台（fg_pid 由 app_detect 提供）：关键线程（主线程/RenderThread 等）
///     核组绑定——boost 下 top-app/foreground cpuset 已收窄为 prime∪big，内核
///     在组内自调度，cpuset 不可用时写一次组掩码兜底；普通线程单核钉定，
///     优先 big、钉满溢出 prime。
///   * 后台忙线程动态 promote 到 big（避免全压小核时的能效灾难），回落 demote。
///   * 单核过载重钉带两道闸：分数滞回（OVERLOAD_MARGIN）+ 反跳回冷却
///     （RETURN_COOLDOWN）。此前只有时间防抖，重负载线程会以 8s 节拍在
///     prime↔big 或两颗大核间乒乓，每次迁移 cache/TLB 全冷，表现为周期性卡顿。
///
/// 开销控制（相对逐线程每轮全读 stat 约省 >50% 文件 I/O）：
/// - 前台：每轮 1 次 read_dir 前台 task 目录；仅**新增**线程读 1 次 stat 判定
///   关键/建档；存量线程的 home 合法性只用缓存的逐核 util 与在线位图判断，
///   不再逐线程 open stat。已消失线程在下一轮 read_dir 后即清理（释放钉核计数）。
/// - 后台候选：每 BG_LIST_EVERY_ROUNDS(2) 轮读一次三个后台 cpuset 的 tasks，
///   候选按游标分片，每轮只深扫 BG_SCAN_WINDOW(64) 个；对已建档未 promote 的
///   候选复用状态做窗口 util（首窗为 0），**两窗防抖**（上次采样忙且本次仍忙，
///   期间采到低负载即清除标记）即 promote——不依赖两次采样间隔，分片稀疏
///   采样下仍有效；已 promote 线程数量少，每 2 轮复查 demote。
/// - 在线核位图缓存每 4 轮刷新（热插拔不频繁），devimp core 日志每 2 轮一次。
/// - 稳态（前台线程集不变、后台空闲）单轮 ≈ 1 read_dir + 0~64 stat + 低频辅助读。
///
/// 选核：score = 逐核 util(最近 SystemLoadUpdate) + 本核钉线程数×0.2，取核池 ∩
/// 在线核中的最低分核（离线核 util 恒 0，钉到离线核会冻结，必须排除）。
/// 关键线程不走 score 选核——组绑定交给内核调度；score 只服务单核钉定的
/// 普通线程与后台 promoted 线程。
///
/// 黑名单：affinity_blacklist.txt 编译嵌入 + 空 cmdline/`/` 开头内置兜底；
/// 线程 comm 命中不迁移；后台 promote 前读一次进程 cmdline 校验并缓存。
use crate::chiri::config::AffinityConfig;
use crate::utils::SysPathExist;
use log::{debug, info, warn};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::fluent_args;
use crate::i18n::{t, t_with_args};

const KIND_NONE: u8 = 0;
const KIND_BOOST: u8 = 1;
const KIND_NORMAL: u8 = 2;

const GROUP_TOP_APP: &str = "top-app";
const GROUP_FOREGROUND: &str = "foreground";
const BACKGROUND_GROUPS: [&str; 3] = ["background", "system-background", "restricted"];

const REBALANCE_INTERVAL: Duration = Duration::from_secs(2);
/// 单轮最多深扫的后台候选线程数
const BG_SCAN_WINDOW: usize = 64;
const BG_LIST_EVERY_ROUNDS: u64 = 2;
const PROMOTED_REVIEW_EVERY_ROUNDS: u64 = 2;
const ONLINE_EVERY_ROUNDS: u64 = 4;
/// devimp 低频行（place/core）输出周期：2s/轮 × 4 = 8s 一轮。
/// place 为前台线程全量快照（游戏可达数十线程），周期过长会丢放置变化
/// 细节，过短则写入量过大——8s 是快照完整度与 IO 量的折衷
const DEVL_ROW_EVERY_ROUNDS: u64 = 4;

/// 线程迁移最小间隔（promote/demote 流程）：过于频繁的迁移带来 cache
/// 冷却与调度抖动，负载窗口本身已做两窗/3 连防抖，这里只挡边界抖动
const MIN_MIGRATE_INTERVAL: Duration = Duration::from_secs(4);
/// 重钉分数滞回：目标核 score 需比 home 核低至少该值才值得迁移。
/// 没有滞回时，0.70 阈值边缘的 util 快照噪声就能驱动无收益搬动——
/// 两个核互相「看起来略好一点」正是两核乒乓的直接来源
const OVERLOAD_MARGIN: f32 = 0.15;
/// 反跳回冷却：迁离某核后该时长内禁止迁回，打破 A→B→A 振荡。
/// 乒乓周期与 REPIN_DEBOUNCE 同拍（8s），取两倍时长足以错开节奏
const RETURN_COOLDOWN: Duration = Duration::from_secs(16);
/// 过载重钉防抖：重钉打断线程本地性（cache/TLB 冷却）比 promote 更明显，
/// 拉长间隔减少迁移次数；首次分配分散到位后应极少触发
const REPIN_DEBOUNCE: Duration = Duration::from_secs(8);
/// 核心过载阈值：钉定线程所在核心 util 超过该值时重钉到低占用核。
/// 动机：单核持续高占用会把 schedutil 顶进高频区、能效变差，把忙线程
/// 分散到低占用核压制峰值频率（前台与 bg promoted 线程共用同一阈值）。
const CORE_OVERLOAD_UTIL: f32 = 0.70;
/// 已钉线程对核心 score 的保守负载基准：util 快照滞后（线程刚钉上、核心
/// util 尚未反映其负载）时仍能把后续线程推向其他核，避免多线程挤同核
/// 时间片轮转造成卡顿；权重过弱会让第二个重线程挤入"看似空闲"的核
const PINNED_WEIGHT: f32 = 0.4;
const PROMOTE_UTIL_PCT: f32 = 25.0;
const LITTLE_HIGH_WATER: f32 = 0.70;
const LITTLE_PROMOTE_UTIL_PCT: f32 = 10.0;
const BIG_HIGH_WATER: f32 = 0.90;
const DEMOTE_UTIL_PCT: f32 = 5.0;
const DEMOTE_STREAK: u32 = 3;
const THREAD_STALE: Duration = Duration::from_secs(30);

const KEY_THREAD_COMMS: [&str; 5] = [
    "RenderThread",
    "GLThread",
    "GameThread",
    "UnityMain",
    "UnityGfxDeviceW",
];

fn is_key_thread(tid: i32, main_tid: i32, comm: &str) -> bool {
    tid == main_tid || KEY_THREAD_COMMS.iter().any(|k| *k == comm)
}

fn format_cpu_list(mut ids: Vec<usize>) -> String {
    ids.sort_unstable();
    ids.dedup();
    ids.iter()
        .map(|c| c.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

fn fmt_home(home: i16) -> String {
    if home >= 0 {
        home.to_string()
    } else {
        "-".to_string()
    }
}

fn read_cpuset_cpus(group: &str) -> Option<String> {
    std::fs::read_to_string(format!("/dev/cpuset/{}/cpus", group))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn write_cpuset_cpus(group: &str, value: &str) {
    let path = format!("/dev/cpuset/{}/cpus", group);
    if crate::utils::try_write_file(&path, value).is_err() {
        warn!(
            "{}",
            t_with_args("affinity-write-failed", &fluent_args!("path" => path))
        );
    }
}

/// 读 cpuset 组 tasks（返回组内全部 TID），替代 /proc 全量枚举
fn read_cpuset_tasks(group: &str) -> Vec<i32> {
    let Ok(text) = std::fs::read_to_string(format!("/dev/cpuset/{}/tasks", group)) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|l| l.trim().parse::<i32>().ok())
        .collect()
}

/// 设置单个线程的 CPU 亲和掩码。
/// 用 zeroed 的 `cpu_set_t` 分配：保证对齐与布局合法（`vec![0u8]` 起始地址对齐仅 1，
/// 强转 `*const cpu_set_t` 是未对齐指针，依赖 libc 包装器不解引用的运气，不可靠）。
/// 经 `libc::CPU_SET` 置位（bionic 64 位下 cpu_set_t = [u64; 16]，支持 1024 CPU）；
/// CPU_SET 内部数组索引无越界检查，超出容量直接跳过（与原字节掩码防护等价）。
/// 成功返回 true；线程已退出（ESRCH）等错误返回 false。
/// 供 core_ctl 的 scenemode 专用核自钉复用。
pub(crate) fn set_tid_affinity(tid: i32, cpu_ids: &[usize]) -> bool {
    let mut mask: libc::cpu_set_t = unsafe { std::mem::zeroed() };
    let max_cpu = std::mem::size_of::<libc::cpu_set_t>() * 8;
    for &c in cpu_ids {
        if c < max_cpu {
            unsafe { libc::CPU_SET(c, &mut mask) };
        }
    }
    // AppOptR 式短路：当前掩码与期望一致时跳过 sched_setaffinity，重复钉定
    // 变为一次无副作用的读（周期 rebalance 与 core_ctl 复用点都会高频命中）
    // SAFETY: curr 为 zeroed 的合法 cpu_set_t，sched_getaffinity 仅写入该缓冲；
    // 两个 cpu_set_t 按整块内存逐字节比较，不依赖内部字段布局
    unsafe {
        let mut curr: libc::cpu_set_t = std::mem::zeroed();
        if libc::sched_getaffinity(tid, std::mem::size_of::<libc::cpu_set_t>(), &mut curr) == 0 {
            let size = std::mem::size_of::<libc::cpu_set_t>();
            let curr_bytes =
                std::slice::from_raw_parts(&curr as *const libc::cpu_set_t as *const u8, size);
            let mask_bytes =
                std::slice::from_raw_parts(&mask as *const libc::cpu_set_t as *const u8, size);
            if curr_bytes == mask_bytes {
                return true;
            }
        }
    }
    let ret =
        unsafe { libc::sched_setaffinity(tid, std::mem::size_of::<libc::cpu_set_t>(), &mask) };
    ret == 0
}

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

#[derive(PartialEq)]
enum UclampSupport {
    Unknown,
    Ok,
    Unsupported,
}

const UCLAMP_MAX_PATH: &str = "/dev/cpuctl/top-app/cpu.uclamp.max";

fn read_cmdline(pid: i32) -> String {
    std::fs::read_to_string(format!("/proc/{pid}/cmdline"))
        .unwrap_or_default()
        .split('\0')
        .next()
        .unwrap_or("")
        .trim()
        .to_string()
}

/// 解析 cpuset 风格 CPU 列表为位图
fn parse_cpu_bitmap(s: &str, max_cpu: usize) -> Vec<bool> {
    let mut bits = vec![false; max_cpu];
    for tok in s.split(',') {
        let tok = tok.trim();
        if tok.is_empty() {
            continue;
        }
        if let Some((a, b)) = tok.split_once('-') {
            let (Ok(lo), Ok(hi)) = (a.trim().parse::<usize>(), b.trim().parse::<usize>()) else {
                continue;
            };
            for c in lo..=hi.min(max_cpu.saturating_sub(1)) {
                bits[c] = true;
            }
        } else if let Ok(c) = tok.parse::<usize>() {
            if c < max_cpu {
                bits[c] = true;
            }
        }
    }
    bits
}

/// 在线核位图：解析异常/缺失回退全在线（宁可保守不钉也不让线程冻结）
fn online_bitmap(max_cpu: usize) -> Vec<bool> {
    let s = std::fs::read_to_string("/sys/devices/system/cpu/online").unwrap_or_default();
    if s.trim().is_empty() {
        return vec![true; max_cpu];
    }
    let bits = parse_cpu_bitmap(&s, max_cpu);
    if bits.iter().all(|&b| !b) {
        vec![true; max_cpu]
    } else {
        bits
    }
}

/// 单线程 stat 采样
struct ThreadSample {
    comm: String,
    ticks: u64,
}

fn sample_one_tid(tid: i32) -> Option<ThreadSample> {
    let text = std::fs::read_to_string(format!("/proc/{tid}/stat")).ok()?;
    let close = text.rfind(')')?;
    let rest = &text[close + 1..];
    let tokens: Vec<&str> = rest.split_whitespace().collect();
    if tokens.len() < 37 {
        return None;
    }
    let ticks: u64 = tokens[11].parse().unwrap_or(0) + tokens[12].parse::<u64>().unwrap_or(0);
    Some(ThreadSample {
        comm: text[1..close].to_string(),
        ticks,
    })
}

fn clk_tck() -> f32 {
    let v = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    if v > 0 { v as f32 } else { 100.0 }
}

/// 线程放置状态
struct ThreadState {
    /// 归属进程 PID（后台候选建档时为 0）
    pid: i32,
    /// 线程名（首见 stat 采样缓存，devimp place 行复用，避免重复读 stat）
    comm: String,
    /// 前台关键线程（prime 池）；后台为 false
    is_key: bool,
    /// 当前钉核号；-1 = 未钉
    home: i16,
    /// 已 promote（后台，含 cpuset 组迁移）
    promoted: bool,
    /// 来源 cpuset 组（demote/清理迁回）
    orig_group: String,
    /// 是否迁移过 cpuset 组
    moved_group: bool,
    /// 最近一次忙采样时刻
    last_busy: Option<Instant>,
    /// 连续低 util 复查计数（demote）
    low_streak: u32,
    /// 上次采样 ticks 与时刻（窗口 util）
    last_ticks: u64,
    last_sample: Instant,
    /// 上次迁移时刻（防抖）
    last_move: Instant,
    /// 最近一次被看到时刻（失联清理）
    last_seen: Instant,
    /// boost 下已写入 prime∪big 组掩码（cpuset 不可用时的关键线程兜底，
    /// 不占单核钉定计数，恢复时直接回全核掩码）
    group_pinned: bool,
    /// 最近一次迁离的核心（反跳回冷却用）；-1 = 无迁离记录
    prev_home: i16,
    /// 迁离时刻（配合 RETURN_COOLDOWN）
    prev_home_at: Instant,
}

pub struct AffinityManager {
    sys: Arc<SysPathExist>,
    snapshot: Option<Vec<(String, String)>>,
    uclamp_snapshot: Option<String>,
    uclamp_max_snapshot: Option<String>,
    uclamp_max_support: UclampSupport,
    applied_kind: u8,
    threads: HashMap<i32, ThreadState>,
    core_pinned: Vec<u32>,
    core_utils: Vec<f32>,
    /// 在线核位图缓存（每 ONLINE_EVERY_ROUNDS 刷新）
    online: Vec<bool>,
    bg_cursor: usize,
    tick: u64,
    bg_checked: HashSet<i32>,
    last_rebalance: Instant,
    last_fg_pid: i32,
    last_boost: bool,
    self_pid: u32,
    /// 前台进程 cmdline 缓存（每轮 rebalance 刷新一次，aff/place 行共用，
    /// 避免每次钉核重读 /proc/<pid>/cmdline）
    fg_cmdline: String,
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
            threads: HashMap::new(),
            core_pinned: Vec::new(),
            core_utils: Vec::new(),
            online: Vec::new(),
            bg_cursor: 0,
            tick: 0,
            bg_checked: HashSet::new(),
            last_rebalance: Instant::now(),
            last_fg_pid: 0,
            last_boost: false,
            self_pid: std::process::id(),
            fg_cmdline: String::new(),
        }
    }

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

    pub fn apply(
        &mut self,
        screen_on: bool,
        fg_pid: i32,
        cfg: &AffinityConfig,
        boost: bool,
        core_utils: &[f32],
    ) {
        if !cfg.enabled {
            self.release();
            return;
        }
        if !core_utils.is_empty() {
            self.core_utils = core_utils.to_vec();
        }

        let ranges = crate::common::chiri_core_ranges();
        let boost_list = format_cpu_list(ranges.big.clone().chain(ranges.prime.clone()).collect());
        let little_list = format_cpu_list(ranges.little.clone().collect());

        let old_kind = self.applied_kind;
        if boost {
            self.ensure_snapshot();
            if old_kind != KIND_BOOST {
                if self.sys.cpuset_top_app_exist {
                    write_cpuset_cpus(GROUP_TOP_APP, &boost_list);
                }
                if self.sys.cpuset_foreground_exist {
                    write_cpuset_cpus(GROUP_FOREGROUND, &boost_list);
                }
                self.apply_uclamp(cfg);
                self.apply_uclamp_max(cfg);
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
        } else {
            if old_kind == KIND_BOOST {
                self.restore_foreground_groups();
                self.restore_uclamp();
                self.restore_uclamp_max();
                info!("{}", t("affinity-normal-restore"));
            }
            if old_kind != KIND_NORMAL {
                self.pin_background(&little_list);
            }
            self.applied_kind = KIND_NORMAL;
        }

        let force = fg_pid != self.last_fg_pid || boost != self.last_boost;
        if cfg.pin_foreground_threads
            && (force || self.last_rebalance.elapsed() >= REBALANCE_INTERVAL)
        {
            self.last_rebalance = Instant::now();
            self.last_fg_pid = fg_pid;
            self.last_boost = boost;
            self.rebalance(screen_on, fg_pid, boost);
        }
    }

    // ==================== 工具 ====================

    fn cluster_of(&self, cpu: usize) -> &'static str {
        let ranges = crate::common::chiri_core_ranges();
        if ranges.prime.contains(&cpu) {
            "prime"
        } else if ranges.big.contains(&cpu) {
            "big"
        } else {
            "little"
        }
    }

    /// 选核：核池 ∩ 在线核，取 score=逐核 util+钉核数×0.2 最低者（含当前占用）
    fn pick_core(&self, pool: &[usize]) -> Option<usize> {
        pool.iter()
            .copied()
            .filter(|&c| self.online.get(c).copied().unwrap_or(false))
            .min_by(|&a, &b| {
                let score = |c: usize| {
                    self.core_utils.get(c).copied().unwrap_or(0.0)
                        + self.pinned_of(c) as f32 * PINNED_WEIGHT
                };
                score(a)
                    .partial_cmp(&score(b))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    }

    /// 核心打分（与 pick_core 同口径）：util + 钉核数×权重。
    /// 过载重钉的滞回校验用它对比 home 核与候选核
    fn core_score(&self, core: usize) -> f32 {
        self.core_utils.get(core).copied().unwrap_or(0.0)
            + self.pinned_of(core) as f32 * PINNED_WEIGHT
    }

    /// 主池优先选核（普通线程专用）：main（big 池）内存在未钉核心时只在
    /// main 内选；main 全部已钉才把 overflow（prime 池）中未钉核并入候选
    /// 按 score 竞争——大核挤满后负载自然溢出到超大核。关键线程已改核组
    /// 绑定，不再走本函数的单核选择。
    fn pick_core_pref(&self, main: &[usize], overflow: &[usize]) -> Option<usize> {
        let main_free = main.iter().any(|&c| self.pinned_of(c) == 0);
        if main_free {
            return self.pick_core(main);
        }
        let mut cands: Vec<usize> = main.to_vec();
        cands.extend(overflow.iter().copied().filter(|&c| self.pinned_of(c) == 0));
        self.pick_core(&cands)
    }

    fn pinned_of(&self, core: usize) -> u32 {
        self.core_pinned.get(core).copied().unwrap_or(0)
    }

    fn add_pinned(&mut self, core: usize, delta: i32) {
        if self.core_pinned.len() <= core {
            self.core_pinned.resize(core + 1, 0);
        }
        let cur = &mut self.core_pinned[core];
        if delta >= 0 {
            *cur += delta as u32;
        } else {
            *cur = cur.saturating_sub((-delta) as u32);
        }
    }

    /// 钉线程到单核。`pkg` 由调用方传入（前台为缓存的 fg_cmdline，后台为 "-"），
    /// 避免每次钉核都重读 /proc cmdline。
    fn pin_core(
        &mut self,
        tid: i32,
        core: usize,
        prev_home: i16,
        pid: i32,
        pkg: &str,
        reason: &str,
    ) {
        if !set_tid_affinity(tid, &[core]) {
            return;
        }
        if prev_home >= 0 {
            self.add_pinned(prev_home as usize, -1);
        }
        self.add_pinned(core, 1);
        if let Some(st) = self.threads.get_mut(&tid) {
            if prev_home >= 0 {
                // 记录迁离核与时刻，供重钉校验的反跳回冷却禁回
                st.prev_home = prev_home;
                st.prev_home_at = Instant::now();
            }
            st.home = core as i16;
            st.last_move = Instant::now();
        }
        crate::logger::devimp_aff(
            if pid == 0 { "promote" } else { "pin" },
            pid,
            pkg,
            tid,
            "-",
            &fmt_home(prev_home),
            &core.to_string(),
            "-",
            reason,
        );
    }

    /// 解除单核钉定：恢复全核掩码。`pkg` 由调用方传入（当前前台线程传缓存
    /// fg_cmdline；cleanup/demote 场景 pid 可能已不属于当前前台，传 "-" 以免
    /// 日志错误归属）。
    fn unpin_core(&mut self, tid: i32, prev_home: i16, pid: i32, pkg: &str) {
        let ranges = crate::common::chiri_core_ranges();
        let all: Vec<usize> = (0..ranges.prime.end.max(ranges.big.end)).collect();
        if prev_home >= 0 {
            self.add_pinned(prev_home as usize, -1);
        }
        if let Some(st) = self.threads.get_mut(&tid) {
            st.home = -1;
            st.promoted = false;
            st.low_streak = 0;
            st.last_busy = None;
        }
        if set_tid_affinity(tid, &all) {
            crate::logger::devimp_aff("restore", pid, pkg, tid, "-", "-", "full", "-", "reset");
        }
    }

    fn move_tid_group(&self, tid: i32, group: &str) -> bool {
        crate::utils::try_write_file(&format!("/dev/cpuset/{group}/tasks"), &tid.to_string())
            .is_ok()
    }

    /// 关键线程组掩码兜底的还原：恢复全核掩码并清 group_pinned。
    /// 不触碰单核钉定计数（组绑定从未占用）。线程未做组绑定时为无操作
    fn restore_group_mask(&mut self, tid: i32, pid: i32, pkg: &str) {
        let pinned = self
            .threads
            .get(&tid)
            .map(|st| st.group_pinned)
            .unwrap_or(false);
        if !pinned {
            return;
        }
        let max_cpu = {
            let ranges = crate::common::chiri_core_ranges();
            ranges.prime.end.max(ranges.big.end)
        };
        let all: Vec<usize> = (0..max_cpu).collect();
        if set_tid_affinity(tid, &all) {
            if let Some(st) = self.threads.get_mut(&tid) {
                st.group_pinned = false;
            }
            crate::logger::devimp_aff(
                "restore",
                pid,
                pkg,
                tid,
                "-",
                "-",
                "full",
                "-",
                "fg_group_reset",
            );
        }
    }

    /// 清理线程：迁回原组 + 恢复全核 + 移除状态
    fn cleanup_thread(&mut self, tid: i32) {
        let (moved, orig, home, pid) = match self.threads.get(&tid) {
            Some(st) => (st.moved_group, st.orig_group.clone(), st.home, st.pid),
            None => return,
        };
        if moved && !orig.is_empty() {
            let _ = self.move_tid_group(tid, &orig);
        }
        if home >= 0 {
            self.unpin_core(tid, home, pid, "-");
        } else {
            // 组掩码兜底的关键线程在此恢复全核（内部无绑定时为无操作）
            self.restore_group_mask(tid, pid, "-");
        }
        self.threads.remove(&tid);
    }

    // ==================== 再平衡 ====================

    fn rebalance(&mut self, screen_on: bool, fg_pid: i32, boost: bool) {
        let now = Instant::now();
        self.tick = self.tick.wrapping_add(1);
        let t = self.tick;
        let ranges = crate::common::chiri_core_ranges();
        let max_cpu = ranges.prime.end.max(ranges.big.end);
        let prime_pool: Vec<usize> = if ranges.prime.is_empty() {
            ranges.big.clone().collect()
        } else {
            ranges.prime.clone().collect()
        };
        let big_pool: Vec<usize> = ranges.big.clone().collect();
        // 前台线程合法落点 = 全部性能核（prime ∪ big；无 prime SoC 退化为 big）
        let mut perf_pool: Vec<usize> = ranges.big.clone().collect();
        perf_pool.extend(ranges.prime.clone());

        // 在线核位图低频刷新
        if t % ONLINE_EVERY_ROUNDS == 0 || self.online.len() != max_cpu {
            self.online = online_bitmap(max_cpu);
        }

        // —— 快速切换：立即解除上一个前台应用的线程钉定 ——
        // 同模式切换（无 ModeChange 事件）只靠 2s 周期发现；若等 30s 失联清理，
        // 旧前台应用已转后台但线程仍持有大核单核掩码——Android 会把上一个应用
        // 短暂留在 top-app/foreground cpuset，掩码与大核相交则继续生效：
        // 8550 只有一颗 prime，新前台关键线程会被选到同核互踩；连续快速切换
        // 还会让 core_pinned 计数漂移累积。按 pid 归属立即清理（pid>0 且
        // ≠ 当前前台 = 旧前台线程；后台 promote 线程 pid==0 不受影响）。
        // 幂等：稳态下该过滤器为空集，仅遍历小状态表，零文件 IO。
        let departed: Vec<i32> = self
            .threads
            .iter()
            .filter(|(_, st)| st.pid > 0 && st.pid != fg_pid)
            .map(|(tid, _)| *tid)
            .collect();
        for tid in departed {
            self.cleanup_thread(tid);
        }

        // —— 前台：每轮 1 次 read_dir，新增线程才读 stat ——
        if fg_pid > 0 {
            let fg_cmdline = read_cmdline(fg_pid);
            // cmdline 缓存：aff/place 行共用，钉核不再重读 /proc
            self.fg_cmdline = fg_cmdline.clone();
            let fg_ok =
                !fg_cmdline.is_empty() && !crate::common::is_affinity_blacklisted(&fg_cmdline);
            let pin_fg = fg_ok && boost && screen_on;

            if fg_ok {
                let task_dir = format!("/proc/{fg_pid}/task");
                // 每轮克隆一次供两处 pin_core 复用（避免分支内重复克隆）
                let pkg = self.fg_cmdline.clone();
                match std::fs::read_dir(&task_dir) {
                    Ok(rd) => {
                        let mut seen = HashSet::new();
                        for entry in rd.flatten() {
                            let tid: i32 =
                                match entry.file_name().to_str().and_then(|s| s.parse().ok()) {
                                    Some(x) => x,
                                    None => continue,
                                };
                            seen.insert(tid);
                            let fresh = !self.threads.contains_key(&tid);
                            let st = self.threads.entry(tid).or_insert_with(|| ThreadState {
                                pid: fg_pid,
                                comm: String::new(),
                                is_key: false,
                                home: -1,
                                promoted: false,
                                orig_group: GROUP_FOREGROUND.to_string(),
                                moved_group: false,
                                last_busy: None,
                                low_streak: 0,
                                last_ticks: 0,
                                last_sample: now,
                                last_move: now - MIN_MIGRATE_INTERVAL,
                                last_seen: now,
                                group_pinned: false,
                                prev_home: -1,
                                prev_home_at: now,
                            });
                            // tid 归属变化（PID 复用）视作新线程
                            if st.pid != fg_pid {
                                st.pid = fg_pid;
                                st.comm.clear();
                                st.home = -1;
                                st.promoted = false;
                                st.moved_group = false;
                                st.group_pinned = false;
                                st.prev_home = -1;
                                st.last_move = now - MIN_MIGRATE_INTERVAL;
                                // ticks 基准一并作废：否则下方「fresh 或 last_ticks==0
                                // 才重采样」不成立，is_key 沿用旧身份（后台建档恒为
                                // false），该线程转前台后会误走普通单核路径
                                st.last_ticks = 0;
                                st.last_busy = None;
                                st.low_streak = 0;
                            }
                            st.last_seen = now;
                            // 新增线程才读 stat（判关键线程 / 建档 ticks + comm 缓存）
                            if fresh || st.last_ticks == 0 {
                                if let Some(s) = sample_one_tid(tid) {
                                    st.is_key = is_key_thread(tid, fg_pid, &s.comm);
                                    st.comm = s.comm;
                                    st.last_ticks = s.ticks;
                                    st.last_sample = now;
                                }
                            }
                            let (home, is_key, group_pinned) =
                                (st.home, st.is_key, st.group_pinned);
                            if pin_fg {
                                if is_key {
                                    // 关键线程核组绑定（AppOptR 式）：不钉单核。
                                    // boost 下 top-app/foreground cpuset 已收窄为
                                    // prime∪big，内核在组内自调度——重负载主线程
                                    // 不会被过载重钉挤到单颗大核，也不存在
                                    // prime↔big 迁移。cpuset 不可用时写一次组掩码
                                    // 兜底（getaffinity 短路去重，不占钉核计数）
                                    if !group_pinned && !self.sys.cpuset_top_app_exist {
                                        let perf = perf_pool.clone();
                                        if set_tid_affinity(tid, &perf) {
                                            if let Some(st) = self.threads.get_mut(&tid) {
                                                st.group_pinned = true;
                                            }
                                            crate::logger::devimp_aff(
                                                "pin", fg_pid, &pkg, tid, "-", "-", "group", "-",
                                                "fg_group",
                                            );
                                        }
                                    }
                                } else {
                                    // 合法范围 = 全部性能核（含溢出落点：普通线程
                                    // 可溢出 prime），避免溢出核被误判非法
                                    let core_ok = home >= 0
                                        && (home as usize) < max_cpu
                                        && self.online.get(home as usize).copied().unwrap_or(false)
                                        && perf_pool.contains(&(home as usize));
                                    if !core_ok {
                                        // 普通线程优先大核，大核钉满后溢出超大核
                                        if let Some(core) =
                                            self.pick_core_pref(&big_pool, &prime_pool)
                                        {
                                            self.pin_core(tid, core, home, fg_pid, &pkg, "fg_pin");
                                        }
                                    } else if now.duration_since(st.last_move) >= REPIN_DEBOUNCE
                                        && self
                                            .core_utils
                                            .get(home as usize)
                                            .copied()
                                            .unwrap_or(0.0)
                                            > CORE_OVERLOAD_UTIL
                                    {
                                        // home 核过载（util > 70%）：重钉到低占用核
                                        // 分散负载压制峰值频率。目标核必须同时通过
                                        // 三道校验：不过载、分数显著更低（滞回）、
                                        // 不在反跳回冷却内（刚迁离的核禁回）——否则
                                        // 静止，防止边缘 util 噪声驱动 A↔B 乒乓
                                        if let Some(core) =
                                            self.pick_core_pref(&big_pool, &prime_pool)
                                        {
                                            let (left_home, left_at) = self
                                                .threads
                                                .get(&tid)
                                                .map(|st| (st.prev_home, st.prev_home_at))
                                                .unwrap_or((-1, now));
                                            let home_score = self.core_score(home as usize);
                                            let cand_score = self.core_score(core);
                                            let banned = left_home == core as i16
                                                && now.duration_since(left_at) < RETURN_COOLDOWN;
                                            if core != home as usize
                                                && self.core_utils.get(core).copied().unwrap_or(0.0)
                                                    <= CORE_OVERLOAD_UTIL
                                                && cand_score <= home_score - OVERLOAD_MARGIN
                                                && !banned
                                            {
                                                self.pin_core(
                                                    tid,
                                                    core,
                                                    home,
                                                    fg_pid,
                                                    &pkg,
                                                    "home_overload",
                                                );
                                            } else {
                                                crate::logger::devimp_event(
                                                    "overload_hold",
                                                    &pkg,
                                                    &format!(
                                                        "tid={} home={} cand={} score_home={:.2} score_cand={:.2}",
                                                        tid, home, core, home_score, cand_score
                                                    ),
                                                );
                                            }
                                        }
                                    }
                                }
                            } else if home >= 0 {
                                self.unpin_core(tid, home, fg_pid, &pkg);
                            } else if group_pinned {
                                // 退出 boost 后组掩码兜底的关键线程恢复全核
                                self.restore_group_mask(tid, fg_pid, &pkg);
                            }
                        }
                        // 已消失线程：立即清理释放钉核计数
                        let gone: Vec<i32> = self
                            .threads
                            .iter()
                            .filter(|(tid, st)| st.pid == fg_pid && !seen.contains(tid))
                            .map(|(tid, _)| *tid)
                            .collect();
                        for tid in gone {
                            self.cleanup_thread(tid);
                        }
                    }
                    Err(_) => {
                        // 前台进程已退出：清理其全部线程
                        let gone: Vec<i32> = self
                            .threads
                            .iter()
                            .filter(|(_, st)| st.pid == fg_pid)
                            .map(|(tid, _)| *tid)
                            .collect();
                        for tid in gone {
                            self.cleanup_thread(tid);
                        }
                    }
                }
            }
        }

        // —— 后台：仅亮屏；已 promote 复查 + 分片候选 ——
        if screen_on {
            // 已 promote 线程 demote 复查（集合小，每 2 轮）
            if t % PROMOTED_REVIEW_EVERY_ROUNDS == 0 {
                let clk = clk_tck();
                let promoted: Vec<i32> = self
                    .threads
                    .iter()
                    .filter(|(_, st)| st.promoted && st.home >= 0)
                    .map(|(tid, _)| *tid)
                    .collect();
                for tid in promoted {
                    if let Some(s) = sample_one_tid(tid) {
                        let util = self.window_util(tid, s.ticks, now, clk).unwrap_or(0.0);
                        // tid 来自上方 threads.iter() 收集、循环内无删除，正常
                        // 不可达；调度核心路径防御性跳过（不 panic、不中止整轮）
                        let low = match self.threads.get_mut(&tid) {
                            Some(st) => {
                                if util < DEMOTE_UTIL_PCT {
                                    st.low_streak += 1;
                                } else {
                                    st.low_streak = 0;
                                }
                                st.low_streak
                            }
                            None => continue,
                        };
                        if low >= DEMOTE_STREAK {
                            self.demote(tid);
                        } else if util >= DEMOTE_UTIL_PCT {
                            // 线程仍忙且所在核心过载（util > 70%）：换到低占用
                            // big 核分散负载、压制峰值频率。promoted 线程虽被移入
                            // top-app 组但不属于前台——保持 big 池与迁移防抖，不
                            // 走前台路径（前台=fg_pid 的线程，与顶层 cpuset 组区分）
                            let (home, last_move) = match self.threads.get(&tid) {
                                Some(st) => (st.home, st.last_move),
                                None => continue,
                            };
                            if home >= 0
                                && now.duration_since(last_move) >= REPIN_DEBOUNCE
                                && self.core_utils.get(home as usize).copied().unwrap_or(0.0)
                                    > CORE_OVERLOAD_UTIL
                            {
                                if let Some(core) = self.pick_core(&big_pool) {
                                    // 与前台同口径的三道校验：目标核不过载、分数
                                    // 显著更低（OVERLOAD_MARGIN 滞回）、不在反跳回
                                    // 冷却内。全不满足则静止，防止 promoted 线程
                                    // 在大核间乒乓
                                    let (left_home, left_at) = match self.threads.get(&tid) {
                                        Some(st) => (st.prev_home, st.prev_home_at),
                                        None => continue,
                                    };
                                    let home_score = self.core_score(home as usize);
                                    let cand_score = self.core_score(core);
                                    let banned = left_home == core as i16
                                        && now.duration_since(left_at) < RETURN_COOLDOWN;
                                    if core != home as usize
                                        && self.core_utils.get(core).copied().unwrap_or(0.0)
                                            <= CORE_OVERLOAD_UTIL
                                        && cand_score <= home_score - OVERLOAD_MARGIN
                                        && !banned
                                    {
                                        self.pin_core(tid, core, home, 0, "-", "bg_overload");
                                    } else {
                                        crate::logger::devimp_event(
                                            "overload_hold",
                                            "-",
                                            &format!(
                                                "tid={} home={} cand={} score_home={:.2} score_cand={:.2}",
                                                tid, home, core, home_score, cand_score
                                            ),
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // 候选刷新（低频）+ 分片深扫
            if t % BG_LIST_EVERY_ROUNDS == 0 || self.bg_cursor == 0 {
                let mut bg: Vec<(i32, String)> = Vec::new();
                for group in BACKGROUND_GROUPS {
                    let exist = match group {
                        "background" => self.sys.cpuset_background_exist,
                        "system-background" => self.sys.cpuset_system_background_exist,
                        _ => self.sys.cpuset_restricted_exist,
                    };
                    if exist {
                        for tid in read_cpuset_tasks(group) {
                            if tid as u32 != self.self_pid {
                                bg.push((tid, group.to_string()));
                            }
                        }
                    }
                }
                // bg 为空只跳过候选扫描：不得 return——否则会跳过 rebalance 尾部的
                // 失联清理（promoted 线程将滞留大核无法 demote）与 devimp 输出
                if !bg.is_empty() {
                    let little_max = ranges
                        .little
                        .clone()
                        .filter_map(|c| self.core_utils.get(c).copied())
                        .fold(0.0_f32, f32::max);
                    let big_max = ranges
                        .big
                        .clone()
                        .filter_map(|c| self.core_utils.get(c).copied())
                        .fold(0.0_f32, f32::max);
                    let promote_thresh = if little_max > LITTLE_HIGH_WATER {
                        LITTLE_PROMOTE_UTIL_PCT
                    } else {
                        PROMOTE_UTIL_PCT
                    };
                    let big_pressure = boost && big_max > BIG_HIGH_WATER;
                    let clk = clk_tck();
                    let n = bg.len();
                    self.bg_cursor %= n;
                    let end = (self.bg_cursor + BG_SCAN_WINDOW).min(n);
                    for (tid, group) in &bg[self.bg_cursor..end] {
                        // 已 promote / 前台线程跳过（前者走复查，后者走前台路径）
                        if let Some(st) = self.threads.get(tid) {
                            if st.promoted || st.pid > 0 {
                                continue;
                            }
                        }
                        let Some(s) = sample_one_tid(*tid) else {
                            continue;
                        };
                        if crate::common::is_affinity_blacklisted(&s.comm) {
                            continue;
                        }
                        // 进程级黑名单：首见读一次并缓存
                        if !self.bg_checked.contains(tid) {
                            if crate::common::is_affinity_blacklisted(&read_cmdline(*tid)) {
                                self.bg_checked.insert(*tid);
                                continue;
                            }
                            self.bg_checked.insert(*tid);
                        }
                        let busy = match self.threads.get_mut(tid) {
                            // 首见：建档并记基准，本轮无 util 不判 promote
                            None => {
                                self.threads.insert(
                                    *tid,
                                    ThreadState {
                                        pid: 0,
                                        comm: s.comm,
                                        is_key: false,
                                        home: -1,
                                        promoted: false,
                                        orig_group: group.clone(),
                                        moved_group: false,
                                        last_busy: None,
                                        low_streak: 0,
                                        last_ticks: s.ticks,
                                        last_sample: now,
                                        last_move: now - MIN_MIGRATE_INTERVAL,
                                        last_seen: now,
                                        group_pinned: false,
                                        prev_home: -1,
                                        prev_home_at: now,
                                    },
                                );
                                continue;
                            }
                            // 再次命中：窗口 util = ticks 增量/间隔秒/CLK_TCK×100
                            Some(st) => {
                                st.last_seen = now;
                                let dt =
                                    now.duration_since(st.last_sample).as_secs_f32().max(0.001);
                                let util =
                                    ((s.ticks.saturating_sub(st.last_ticks)) as f32 / clk / dt
                                        * 100.0)
                                        .min(100.0);
                                st.last_ticks = s.ticks;
                                st.last_sample = now;
                                // 两窗防抖（不依赖采样间隔——分片下同线程两次被采到可能
                                // 相隔很久）：本次忙且上次采样忙 → promote；期间采到低负载
                                // 则清除忙标记，防止瞬时忙/抖动被 promote。
                                let was_busy = st.last_busy.is_some();
                                if util >= promote_thresh {
                                    st.last_busy = Some(now);
                                } else if util < DEMOTE_UTIL_PCT {
                                    st.last_busy = None;
                                }
                                // 滞回带（demote..promote 之间）保持上次忙标记不变
                                if was_busy && util >= promote_thresh && !big_pressure {
                                    Some(util)
                                } else {
                                    None
                                }
                            }
                        };
                        if let Some(util) = busy {
                            // 移入 top-app 使 big 核可见，再按当前核心占用选核钉定
                            let moved = self.move_tid_group(*tid, GROUP_TOP_APP);
                            if let Some(st) = self.threads.get_mut(tid) {
                                st.moved_group = moved;
                                st.promoted = true;
                            }
                            if let Some(core) = self.pick_core(&big_pool) {
                                self.pin_core(*tid, core, -1, 0, "-", "bg_busy");
                            }
                            debug!(
                                "{}",
                                t_with_args(
                                    "affinity-promoted",
                                    &fluent_args!(
                                        "tid" => tid.to_string(),
                                        "util" => format!("{:.1}", util)
                                    )
                                )
                            );
                        }
                    }
                    self.bg_cursor = if end < n { end } else { 0 };
                    if self.bg_checked.len() > 512 {
                        self.bg_checked.clear();
                    }
                }
            }
        }

        // —— 失联清理 ——
        let stale: Vec<i32> = self
            .threads
            .iter()
            .filter(|(_, st)| now.duration_since(st.last_seen) > THREAD_STALE)
            .map(|(tid, _)| *tid)
            .collect();
        for tid in stale {
            self.cleanup_thread(tid);
        }

        // —— devimp 低频输出（每 DEVL_ROW_EVERY_ROUNDS 轮，全部复用缓存数据，
        // 零新增文件读）：place 行 = 前台线程放置快照（包名/线程名/落点核均
        // 来自缓存），core 行 = 逐核 util + 钉核计数 ——
        if t % DEVL_ROW_EVERY_ROUNDS == 0 {
            if fg_pid > 0 {
                for (tid, st) in &self.threads {
                    if st.pid != fg_pid {
                        continue;
                    }
                    let comm = if st.comm.is_empty() {
                        "-"
                    } else {
                        st.comm.as_str()
                    };
                    crate::logger::devimp_place(
                        fg_pid,
                        &self.fg_cmdline,
                        *tid,
                        comm,
                        st.home as i32,
                        "-",
                    );
                }
            }
            for cpu in 0..max_cpu {
                let util = self
                    .core_utils
                    .get(cpu)
                    .map(|u| format!("{:.0}", u * 100.0))
                    .unwrap_or_else(|| "-".to_string());
                crate::logger::devimp_core(self.cluster_of(cpu), cpu, &util, self.pinned_of(cpu));
            }
        }
    }

    /// 窗口 util%（ticks 增量 / 上次采样至今秒 / CLK_TCK × 100）；首见返回 None
    fn window_util(&mut self, tid: i32, ticks: u64, now: Instant, clk: f32) -> Option<f32> {
        self.threads.get_mut(&tid).map(|st| {
            let dt = now.duration_since(st.last_sample).as_secs_f32().max(0.001);
            let d = ticks.saturating_sub(st.last_ticks);
            st.last_ticks = ticks;
            st.last_sample = now;
            (d as f32 / clk / dt * 100.0).min(100.0)
        })
    }

    /// demote：迁回原 cpuset 组 + 恢复全核
    fn demote(&mut self, tid: i32) {
        let (moved, orig, home) = match self.threads.get(&tid) {
            Some(st) => (st.moved_group, st.orig_group.clone(), st.home),
            None => return,
        };
        if moved && !orig.is_empty() {
            let _ = self.move_tid_group(tid, &orig);
            if let Some(st) = self.threads.get_mut(&tid) {
                st.moved_group = false;
            }
        }
        if home >= 0 {
            self.unpin_core(tid, home, 0, "-");
        }
        debug!(
            "{}",
            t_with_args("affinity-demoted", &fluent_args!("tid" => tid.to_string()))
        );
    }

    // ==================== cgroup 布局 / uclamp / 释放 ====================

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

    fn apply_uclamp(&self, cfg: &AffinityConfig) {
        if cfg.top_app_uclamp_min_pct > 0 && self.sys.cpuctl_top_app_exist {
            let _ = crate::utils::try_write_file(
                "/dev/cpuctl/top-app/cpu.uclamp.min",
                &cfg.top_app_uclamp_min_pct.to_string(),
            );
        }
    }

    fn restore_foreground_groups(&self) {
        if let Some(snap) = &self.snapshot {
            for (path, val) in snap {
                if path.contains(GROUP_TOP_APP) || path.contains(GROUP_FOREGROUND) {
                    let _ = crate::utils::try_write_file(path, val);
                }
            }
        }
    }

    fn restore_uclamp(&self) {
        if let Some(v) = &self.uclamp_snapshot {
            if !v.is_empty() {
                let _ = crate::utils::try_write_file("/dev/cpuctl/top-app/cpu.uclamp.min", v);
            }
        }
    }

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

    fn restore_uclamp_max(&self) {
        if let Some(v) = &self.uclamp_max_snapshot {
            if !v.is_empty() {
                let _ = crate::utils::try_write_file(UCLAMP_MAX_PATH, v);
            }
        }
    }

    pub fn release(&mut self) {
        let tids: Vec<i32> = self.threads.keys().copied().collect();
        for tid in tids {
            self.cleanup_thread(tid);
        }
        if let Some(snap) = self.snapshot.take() {
            for (path, val) in &snap {
                let _ = crate::utils::try_write_file(path, val);
            }
        }
        self.restore_uclamp();
        self.restore_uclamp_max();
        self.applied_kind = KIND_NONE;
        self.last_fg_pid = 0;
        self.last_boost = false;
        self.bg_cursor = 0;
        self.bg_checked.clear();
        info!("{}", t("affinity-released"));
    }
}
