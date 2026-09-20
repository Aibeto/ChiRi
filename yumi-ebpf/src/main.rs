//! yumi-ebpf/src/main.rs: [fps-probe] [cpu-probe] [telemetry-probes] [helpers]

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
#![no_std]
#![no_main]

use aya_ebpf::{
    helpers::{bpf_get_current_pid_tgid, bpf_ktime_get_ns},
    macros::{map, tracepoint, uprobe},
    maps::{Array, HashMap, PerCpuArray, RingBuf},
    programs::{ProbeContext, TracePointContext},
};

// [fps-probe]
// FPS Probe — uprobe on Surface::queueBuffer

#[repr(C)]
pub struct FrameTimestampEvent {
    pub pid: u32,
    pub ktime_ns: u64,
}

#[map]
static RING_BUF: RingBuf = RingBuf::with_byte_size(0x8000, 0);

#[uprobe]
pub fn handle_frame(ctx: ProbeContext) -> u32 {
    match try_handle_frame(ctx) {
        Ok(ret) => ret,
        Err(ret) => ret,
    }
}

fn try_handle_frame(_ctx: ProbeContext) -> Result<u32, u32> {
    let pid_tgid = bpf_get_current_pid_tgid();
    let pid = (pid_tgid >> 32) as u32;
    let ktime_ns = unsafe { bpf_ktime_get_ns() };

    if let Some(mut entry) = RING_BUF.reserve::<FrameTimestampEvent>(0) {
        entry.write(FrameTimestampEvent { pid, ktime_ns });
        entry.submit(0);
    }

    Ok(0)
}

// [cpu-probe]
// CPU Probe — tracepoint on sched/sched_switch

// sched_switch 参数布局 (offset → field)
//  0: pad            u64
//  8: prev_comm     [u8; 16]
// 24: prev_pid       i32
// 28: prev_prio      i32
// 32: prev_state     i64
// 40: next_comm     [u8; 16]
// 56: next_pid       i32
// 60: next_prio      i32
const OFF_PREV_PID: usize = 24;
const OFF_NEXT_PID: usize = 56;

/// 每核心运行时状态：原先是五个独立 PerCpuArray（last_time / idle / busy /
/// cur_tid / cur_tgid），合并后一次查找即可读写全部字段，把每次 sched_switch
/// 的 map 查找从 5 次降到 1 次。
/// 布局必须与用户态 `src/monitor/cpu_monitor.rs` 的同名结构逐字段一致：
/// `#[repr(C)]`、u64 在前 u32 在后 → 32 字节、无 padding。两侧必须同批发布。
#[repr(C)]
#[derive(Clone, Copy)]
pub struct CoreState {
    /// 上次切换时间戳 (ns)
    pub last_time: u64,
    /// 累计 Idle 时间 (ns)
    pub idle: u64,
    /// 累计 Busy 时间 (ns)
    pub busy: u64,
    /// 当前运行的 TID
    pub cur_tid: u32,
    /// 当前运行任务的 TGID
    pub cur_tgid: u32,
}

#[map]
static CORE_STATE: PerCpuArray<CoreState> = PerCpuArray::with_max_entries(1, 0);

/// 线程级运行时间 (TID → ns)
#[map]
static THREAD_RUN_TIME: HashMap<u32, u64> = HashMap::with_max_entries(32768, 0);

/// TGID 级聚合运行时间 (TGID → ns)
#[map]
static TGID_RUN_TIME: HashMap<u32, u64> = HashMap::with_max_entries(1024, 0);

/// 线程级记账开关（0 = 关，非 0 = 开）：由用户态写，置位条件与
/// cpu_monitor 的 FAS_FG_UTIL_ENABLED 相同（ChiRi SoC 且 FAS 配置可用）。
/// 关闭时 sched_switch 不再做 THREAD_RUN_TIME 的 hash 查找/插入——该 map
/// 只被用户态「TGID 主路径失败」时的降级路径消费。
#[map]
static THREAD_ACCT: Array<u32> = Array::with_max_entries(1, 0);

const ZERO_KEY: u32 = 0;
const NS_10_SEC: u64 = 10_000_000_000;

// [telemetry-probes]
// Telemetry Probes — 唤醒 / 线程迁移 / 频率切换计数（ChiRi 专属遥测）
// userspace 每 2s 读取累计值取增量；探针挂载失败（内核缺 tracepoint）不影响主探针

/// sched_wakeup 唤醒次数（全核累计）
#[map]
static WAKEUP_COUNT: PerCpuArray<u64> = PerCpuArray::with_max_entries(1, 0);

/// sched_migrate_task 线程跨核迁移次数（亲和策略的实际迁移观测）
#[map]
static MIGRATE_COUNT: PerCpuArray<u64> = PerCpuArray::with_max_entries(1, 0);

/// cpufreq_transition 频率切换次数（调频活跃度，含热限频切换）
#[map]
static FREQ_TRANS_COUNT: PerCpuArray<u64> = PerCpuArray::with_max_entries(1, 0);

/// 计数器自增（PerCpuArray key 0 槽位，per-cpu 天然免锁）
fn bump_counter(map: &PerCpuArray<u64>) -> u32 {
    if let Some(ptr) = map.get_ptr_mut(ZERO_KEY) {
        unsafe { *ptr += 1 };
    }
    0
}

#[tracepoint]
pub fn handle_sched_wakeup(_ctx: TracePointContext) -> u32 {
    bump_counter(&WAKEUP_COUNT)
}

#[tracepoint]
pub fn handle_sched_migrate_task(_ctx: TracePointContext) -> u32 {
    bump_counter(&MIGRATE_COUNT)
}

#[tracepoint]
pub fn handle_cpufreq_transition(_ctx: TracePointContext) -> u32 {
    bump_counter(&FREQ_TRANS_COUNT)
}

#[tracepoint]
pub fn handle_sched_switch(ctx: TracePointContext) -> u32 {
    match try_handle_sched_switch(&ctx) {
        Ok(ret) => ret,
        Err(ret) => ret as u32,
    }
}

fn try_handle_sched_switch(ctx: &TracePointContext) -> Result<u32, i64> {
    let now = unsafe { bpf_ktime_get_ns() };

    let prev_tid: i32 = unsafe { ctx.read_at(OFF_PREV_PID)? };
    let next_tid: i32 = unsafe { ctx.read_at(OFF_NEXT_PID)? };

    // bpf_get_current_pid_tgid() 在 sched_switch 中返回 **next** 任务的 pid_tgid
    let pid_tgid = bpf_get_current_pid_tgid();
    let next_tgid = (pid_tgid >> 32) as u32;

    // 单次查找拿到本核全部状态（原先是 5 个独立 PerCpuArray 各查一次）
    let state_ptr = match CORE_STATE.get_ptr_mut(ZERO_KEY) {
        Some(p) => p,
        None => return Ok(0),
    };
    let state = unsafe { &mut *state_ptr };

    // ── 计算上一个任务的耗时并累加 ──
    let last_ts = state.last_time;
    let delta = now.saturating_sub(last_ts);

    if delta > 0 && delta < NS_10_SEC {
        if prev_tid == 0 {
            // Idle 时间
            state.idle += delta;
        } else {
            // Busy 时间
            state.busy += delta;

            // 线程级累计：按用户态开关执行（见 THREAD_ACCT 说明）
            if thread_acct_enabled() {
                add_to_hash(&THREAD_RUN_TIME, prev_tid as u32, delta);
            }

            // TGID 级聚合累计：prev 任务的 TGID 取本核当前记录
            if state.cur_tgid > 0 {
                add_to_hash(&TGID_RUN_TIME, state.cur_tgid, delta);
            }
        }
    }

    // ── 更新当前核心状态 ──
    state.last_time = now;
    state.cur_tid = next_tid as u32;
    state.cur_tgid = next_tgid;

    Ok(0)
}

// [helpers]

/// 向 HashMap 累加 delta（查找然后 +=，不存在则 insert）
fn add_to_hash(map: &HashMap<u32, u64>, key: u32, delta: u64) {
    if let Some(ptr) = map.get_ptr_mut(&key) {
        unsafe {
            *ptr += delta;
        }
    } else {
        let _ = map.insert(&key, &delta, 0);
    }
}

/// 线程级记账开关是否打开（THREAD_ACCT 由用户态写：非 0 = 记账）。
/// 关闭时 sched_switch 跳过 THREAD_RUN_TIME 的 hash 查找/插入。
fn thread_acct_enabled() -> bool {
    THREAD_ACCT.get(ZERO_KEY).is_some_and(|v| *v != 0)
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    unsafe { core::hint::unreachable_unchecked() }
}
