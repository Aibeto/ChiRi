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

/// 遥测数据源（ChiRi 专属，1s 轮询）：
/// - PSI 压力信息（/proc/pressure/{cpu,io,memory} 的 some avg10，无 PSI 的设备恒为 0）
/// - GPU 利用率（高通 kgsl gpu_busy_percentage / MTK ged gpu_loading，缺失为 None）
/// - 电池电流/电压（/sys/class/power_supply/battery/{current_now,voltage_now}，缺失为 None）
///
/// 数据写入进程级共享原子量（monitor 层写、chiri 调度层读），不占用事件通道容量；
/// 消费端为 chiri scheduler_ipc 的 2s 热循环：telemetry.log CSV 落盘 + 周期 debug 摘要。
/// 线程仅在 ChiRi SoC 上由 monitor/mod.rs 启动，Yumi 设备零开销。
use std::sync::atomic::{AtomicI32, AtomicU32, Ordering};

/// 电池电流/电压的「不可用」哨兵值
const UNAVAIL: i32 = i32::MIN;

/// 遥测共享快照（f32 以 bit pattern 存 AtomicU32，与热保护/触摸状态同口径）
pub struct Telemetry {
    /// PSI cpu some avg10（%）
    psi_cpu_some: AtomicU32,
    /// PSI io some avg10（%）
    psi_io_some: AtomicU32,
    /// PSI memory some avg10（%）
    psi_mem_some: AtomicU32,
    /// GPU 利用率（%），NaN = 节点不可用
    gpu_busy: AtomicU32,
    /// 电池电流（µA，负值常见于放电方向），UNAVAIL = 不可用
    batt_current_ua: AtomicI32,
    /// 电池电压（µV），UNAVAIL = 不可用
    batt_voltage_uv: AtomicI32,
}

static TELEMETRY: Telemetry = Telemetry {
    psi_cpu_some: AtomicU32::new(0),
    psi_io_some: AtomicU32::new(0),
    psi_mem_some: AtomicU32::new(0),
    gpu_busy: AtomicU32::new(0x7FC00000), // f32::NAN.to_bits()
    batt_current_ua: AtomicI32::new(UNAVAIL),
    batt_voltage_uv: AtomicI32::new(UNAVAIL),
};

/// 取进程级遥测快照
pub fn telemetry() -> &'static Telemetry {
    &TELEMETRY
}

impl Telemetry {
    /// PSI some avg10（0.0~1.0 小数；文件缺失按 0 处理）
    pub fn psi_cpu_some(&self) -> f32 {
        f32::from_bits(self.psi_cpu_some.load(Ordering::Relaxed))
    }
    pub fn psi_io_some(&self) -> f32 {
        f32::from_bits(self.psi_io_some.load(Ordering::Relaxed))
    }
    pub fn psi_mem_some(&self) -> f32 {
        f32::from_bits(self.psi_mem_some.load(Ordering::Relaxed))
    }
    /// GPU 利用率（%）；None = 节点不可用
    pub fn gpu_busy(&self) -> Option<f32> {
        let v = f32::from_bits(self.gpu_busy.load(Ordering::Relaxed));
        if v.is_nan() { None } else { Some(v) }
    }
    /// 电池电流（mA，保留方向符号）；None = 不可用
    pub fn batt_current_ma(&self) -> Option<f32> {
        let v = self.batt_current_ua.load(Ordering::Relaxed);
        if v == UNAVAIL {
            None
        } else {
            Some(v as f32 / 1000.0)
        }
    }
    /// 电池电压（V）；None = 不可用
    pub fn batt_voltage_v(&self) -> Option<f32> {
        let v = self.batt_voltage_uv.load(Ordering::Relaxed);
        if v == UNAVAIL {
            None
        } else {
            Some(v as f32 / 1_000_000.0)
        }
    }
    /// 电池瞬时功率（W，电流取绝对值）；电流或电压缺失返回 None
    pub fn batt_power_w(&self) -> Option<f32> {
        Some(self.batt_current_ma()?.abs() * self.batt_voltage_v()?)
    }
}

/// 解析 PSI 文本的 some avg10（如 "some avg10=12.34 avg60=..."），无 some 行返回 0
fn psi_some_avg10(text: &str) -> f32 {
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("some") {
            if let Some(idx) = rest.find("avg10=") {
                let val = &rest[idx + 6..];
                let end = val.find(' ').unwrap_or(val.len());
                return val[..end].trim().parse::<f32>().unwrap_or(0.0);
            }
        }
    }
    0.0
}

/// 读 sysfs/procfs 整数（容许负号），失败返回 None
fn read_i32(path: &str) -> Option<i32> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| s.trim().parse::<i32>().ok())
}

/// OPlus 私有节点（bcc_parms）：逗号分隔字段，下标 6 = 电芯1电压、8 = 电流、
/// 11 = 电芯2电压（双电芯机型，0 = 无）。该节点由 BCC 硬件直出、随采样刷新，
/// 而 OPlus 内核标准 power_supply 节点（current_now/voltage_now）约每 10s
/// 才刷新一次——1s 精度的功耗统计必须优先走该节点，否则读到的是重复旧值。
const OPLUS_BCC_PARMS: &str = "/sys/class/oplus_chg/battery/bcc_parms";

/// 读 OPlus bcc_parms，归一化为 (电压 µV, 电流 µA)。
/// 字段单位随机型可能是 µV/µA 或 mV/mA，按量级启发式归一；
/// 归一后超出物理合理范围（电压 2–6V、电流 ±30A）视为脏数据返回 None，
/// 由调用方回退标准 power_supply 节点。
fn read_oplus_bcc() -> Option<(i32, i32)> {
    let text = std::fs::read_to_string(OPLUS_BCC_PARMS).ok()?;
    let f: Vec<&str> = text.split(',').map(str::trim).collect();
    let v0: i64 = f.get(6)?.parse().ok()?;
    let cur: i64 = f.get(8)?.parse().ok()?;
    if v0 == 0 && cur == 0 {
        return None;
    }
    // 双电芯串联：下标 11 非零时电压取两节之和
    let v1: i64 = f.get(11).and_then(|s| s.parse().ok()).unwrap_or(0);
    let v_raw = if v1 != 0 { v0 + v1 } else { v0 };

    // 量级归一：电压 → µV
    let av = v_raw.abs();
    let v_uv = if av > 100_000 {
        v_raw
    } else if av > 100 {
        v_raw * 1_000 // mV
    } else {
        v_raw * 1_000_000 // V
    };
    // 量级归一：电流 → µA
    let ai = cur.abs();
    let i_ua = if ai > 100_000 {
        cur
    } else if ai > 100 {
        cur * 1_000 // mA
    } else {
        cur * 1_000_000 // A
    };

    if !(2_000_000..=6_000_000).contains(&v_uv) || i_ua.abs() > 30_000_000 {
        return None;
    }
    Some((v_uv as i32, i_ua as i32))
}

/// 遥测线程主循环：1s 轮询刷新共享快照。GPU 路径探测成功后缓存，避免每轮扫描。
pub fn telemetry_loop() {
    // GPU 利用率候选节点：高通 Adreno → MTK GED（按存在性取首个可读者）
    let gpu_candidates = [
        "/sys/class/kgsl/kgsl-3d0/gpu_busy_percentage",
        "/sys/kernel/ged/hal/gpu_loading",
    ];
    let mut gpu_path: Option<&str> = None;

    // OPlus 私有节点探测（一次性）：存在则功耗读取走 BCC 实时数据
    let bcc_available = std::path::Path::new(OPLUS_BCC_PARMS).exists();
    if bcc_available {
        log::info!("{}", crate::i18n::t("telemetry-oplus-bcc"));
    }

    loop {
        // --- PSI ---
        for (path, cell) in [
            ("/proc/pressure/cpu", &TELEMETRY.psi_cpu_some),
            ("/proc/pressure/io", &TELEMETRY.psi_io_some),
            ("/proc/pressure/memory", &TELEMETRY.psi_mem_some),
        ] {
            let v = std::fs::read_to_string(path)
                .map(|t| psi_some_avg10(&t))
                .unwrap_or(0.0);
            cell.store(v.to_bits(), Ordering::Relaxed);
        }

        // --- GPU busy% ---
        if gpu_path.is_none() {
            gpu_path = gpu_candidates
                .iter()
                .copied()
                .find(|p| std::path::Path::new(p).exists());
        }
        if let Some(p) = gpu_path {
            let busy = std::fs::read_to_string(p)
                .ok()
                .and_then(|s| s.trim().trim_end_matches('%').trim().parse::<f32>().ok())
                .map(|v| v.clamp(0.0, 100.0));
            let bits = busy.map(|v| v.to_bits()).unwrap_or(0x7FC00000);
            TELEMETRY.gpu_busy.store(bits, Ordering::Relaxed);
        }

        // --- 电池电流/电压：OPlus bcc_parms 优先（规避标准节点 10s 缓存），失败回退标准节点 ---
        let (current, voltage) = match bcc_available.then(read_oplus_bcc) {
            Some(Some((v, i))) => (i, v),
            _ => (
                read_i32("/sys/class/power_supply/battery/current_now").unwrap_or(UNAVAIL),
                read_i32("/sys/class/power_supply/battery/voltage_now").unwrap_or(UNAVAIL),
            ),
        };
        TELEMETRY.batt_current_ua.store(current, Ordering::Relaxed);
        TELEMETRY.batt_voltage_uv.store(voltage, Ordering::Relaxed);

        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}
