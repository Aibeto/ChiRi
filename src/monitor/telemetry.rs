//! telemetry.rs: [data] [options] [parse] [loop]

/// 遥测数据源（ChiRi 专属，1s 轮询）：
/// - PSI 压力信息（/proc/pressure/{cpu,io,memory} 的 some avg10，无 PSI 的设备恒为 0）
/// - GPU 利用率（高通 kgsl gpu_busy_percentage / MTK ged gpu_loading，缺失为 None）
/// - 电池电流/电压（默认 Android 标准节点 /sys/class/power_supply/battery/{current_now,voltage_now}；
///   meta `oplus_chg` 打开时优先 OPlus 私有节点 bcc_parms，读不到才回退标准节点；缺失为 None）
///
/// 数据写入进程级共享原子量（monitor 层写、chiri 调度层读），不占用事件通道容量；
/// 消费端为 chiri scheduler_ipc 的 2s 热循环：telemetry.log CSV 落盘 + 周期 debug 摘要。
/// 线程仅在 ChiRi SoC 上由 monitor/mod.rs 启动，Yumi 设备零开销。
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering};

// [data]
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
    /// 电池电流（**节点原始值**：私有节点 mV/mA 口径、标准节点 µA 口径；
    /// 负值常见于放电方向），UNAVAIL = 不可用
    batt_current_raw: AtomicI32,
    /// 电池电压（**节点原始值**：私有节点 mV 口径、标准节点 µV 口径），UNAVAIL = 不可用
    batt_voltage_raw: AtomicI32,
}

static TELEMETRY: Telemetry = Telemetry {
    psi_cpu_some: AtomicU32::new(0),
    psi_io_some: AtomicU32::new(0),
    psi_mem_some: AtomicU32::new(0),
    gpu_busy: AtomicU32::new(0x7FC00000), // f32::NAN.to_bits()
    batt_current_raw: AtomicI32::new(UNAVAIL),
    batt_voltage_raw: AtomicI32::new(UNAVAIL),
};

/// BCC 字段不可用告警去重（一次运行一条，避免 1s 轮询刷屏）
static BCC_UNUSABLE_WARNED: AtomicBool = AtomicBool::new(false);

/// 电池电流/电压候选节点（OPlus BCC + 标准节点）**全部**读不到时的告警去重：
/// 进入失效态报一条，恢复正常即重新武装（与 `utils::write_nodes` 同口径）
static BATT_UNAVAIL_WARNED: AtomicBool = AtomicBool::new(false);

/// GPU 利用率候选节点全部不存在时的告警去重（只报一次）
static GPU_UNAVAIL_WARNED: AtomicBool = AtomicBool::new(false);

/// OPlus 私有节点缺失告警去重（节点在运行期出现/重新可用时重新武装）
static BCC_MISSING_WARNED: AtomicBool = AtomicBool::new(false);

/// 私有节点原始值快照是否已打点（单位排查用，一次运行一条）
static BCC_RAW_LOGGED: AtomicBool = AtomicBool::new(false);

/// 私有节点存在性复查间隔：驱动加载晚于 daemon（或节点被 recreate）时，
/// 不会因为开机瞬间探到「不存在」就永远钉在回退标准节点的状态
const BCC_PROBE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60);

// [options]
/// 电池读数的机型选项（meta.yaml 五个字段：oplus_chg / oplus_dual_cell /
/// voltage_double / current_double / unit_divisor）。由 chiri `Config::load` 写入
/// （含热重载），读点在 1s 遥测线程与各消费点——用原子量，不每轮解析 YAML。
///
/// - `OPLUS_CHG`：读 OPlus 私有节点（bcc_parms，随采样刷新）。**存在且读到有效值时
///   只认它**：同机型的标准通用节点不可信（OPlus 上约 10s 才刷新、单位也不保证）。
///   存在但无效（字段缺失/解析失败/全零）或节点不存在时才走标准节点；
/// - `OPLUS_DUAL_CELL`：私有节点按两节**并联**读——电压取两节平均（下标 6 与 11），
///   电流 ×2（下标 8 是单节支路电流，整包 = 两节之和）；下标 11 非正视为单电芯
///   布局（电压只用下标 6、电流不翻倍），详见 `read_oplus_bcc`；
/// - `VOLTAGE_DOUBLE` / `CURRENT_DOUBLE`：标准节点路径的倍电压/倍电流（双电芯机型上
///   标准节点可能只报单节/单芯值）。**与私有节点互斥**：私有开关打开时 UI 强制关闭
///   这两个开关，这里再判一次，手改 meta 也挡得住；
/// - `VOLT_DIVISOR` / `CURR_DIVISOR`：单位校准（**电压、电流各一个**）。**全链没有内置
///   换算**——`输出 = 节点原始值 ÷ 校准值`：电压得到 V、电流得到**安培**
///   （`batt_power_w` = 电流值 × 电压 = W）。校准值就是「原始单位 → 输出单位」的除数：
///     电压：节点报 mV 填 1000、报 µV 填 1000000（**缺省 1000000**，即标准 ABI 口径）；
///     电流：节点报 mA 填 1000、报 µA 填 1000000（缺省同上；私有节点 bcc_parms 报 mV/mA，
///           安装脚本检测到该节点时会把两个值都写成 1000）。
///   分开的理由：节点的电压与电流单位未必同时错（常见只错一个），一个共用值会让
///   功率按平方变化，改了也说不清是谁的锅。
static OPLUS_CHG: AtomicBool = AtomicBool::new(false);
static OPLUS_DUAL_CELL: AtomicBool = AtomicBool::new(false);
static VOLTAGE_DOUBLE: AtomicBool = AtomicBool::new(false);
static CURRENT_DOUBLE: AtomicBool = AtomicBool::new(false);
static VOLT_DIVISOR_BITS: AtomicU32 = AtomicU32::new(crate::utils::DEFAULT_UNIT_DIVISOR.to_bits());
static CURR_DIVISOR_BITS: AtomicU32 = AtomicU32::new(crate::utils::DEFAULT_UNIT_DIVISOR.to_bits());

/// 写入电池读数选项。校准值非有限/非正时退回默认值——单项笔误不牵连其它选项。
pub fn set_battery_options(
    oplus_chg: bool,
    oplus_dual_cell: bool,
    voltage_double: bool,
    current_double: bool,
    voltage_divisor: f32,
    current_divisor: f32,
) {
    OPLUS_CHG.store(oplus_chg, Ordering::Relaxed);
    OPLUS_DUAL_CELL.store(oplus_dual_cell, Ordering::Relaxed);
    VOLTAGE_DOUBLE.store(voltage_double, Ordering::Relaxed);
    CURRENT_DOUBLE.store(current_double, Ordering::Relaxed);
    VOLT_DIVISOR_BITS.store(sane_divisor(voltage_divisor).to_bits(), Ordering::Relaxed);
    CURR_DIVISOR_BITS.store(sane_divisor(current_divisor).to_bits(), Ordering::Relaxed);
}

/// 校准值兜底：非有限/非正一律按默认值处理
fn sane_divisor(v: f32) -> f32 {
    if v.is_finite() && v > 0.0 {
        v
    } else {
        crate::utils::DEFAULT_UNIT_DIVISOR
    }
}

/// 电压校准除数（节点原始单位 → V 的除数），恒 > 0
fn voltage_divisor() -> f32 {
    f32::from_bits(VOLT_DIVISOR_BITS.load(Ordering::Relaxed))
}

/// 电流校准除数（1 个 A 对应多少毫单位），恒 > 0
fn current_divisor() -> f32 {
    f32::from_bits(CURR_DIVISOR_BITS.load(Ordering::Relaxed))
}

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
    /// 电池电流（**单位是安培**，保留方向符号；函数名里的 ma 是历史遗留命名）；None = 不可用。
    /// `节点原始值 ÷ current_divisor`：标准节点报 µA 填 1000000（缺省值）、
    /// OPlus 私有节点报 mA 填 1000（安装脚本检测到该节点时自动写入）。
    /// `batt_power_w` 按安培使用本值（× 电压得瓦），status.csv 的 `batt_current_ma`
    /// 列写的也是这个值（列名同样遗留）。
    pub fn batt_current_ma(&self) -> Option<f32> {
        let v = self.batt_current_raw.load(Ordering::Relaxed);
        (v != UNAVAIL).then(|| v as f32 / current_divisor())
    }
    /// 电池电压（V）；None = 不可用。
    /// `节点原始值 ÷ voltage_divisor`（口径同电流，见 [`Self::batt_current_ma`]）
    pub fn batt_voltage_v(&self) -> Option<f32> {
        let v = self.batt_voltage_raw.load(Ordering::Relaxed);
        (v != UNAVAIL).then(|| v as f32 / voltage_divisor())
    }
    /// 电池瞬时功率（W，电流取绝对值）；电流或电压缺失返回 None
    pub fn batt_power_w(&self) -> Option<f32> {
        Some(self.batt_current_ma()?.abs() * self.batt_voltage_v()?)
    }
}

// [parse]
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

/// 标准 Android 节点（ABI：µV / µA），存**节点原始值**、读取层不做任何换算——
/// 由 meta 的校准除数（缺省 1000000，正是 ABI 口径）换算为 V / A。
/// 不可用返回哨兵 [`UNAVAIL`]。
/// OPlus 标准节点「单位也不保证」（部分版本 10s 才刷新）：回退读数本就只作兜底，
/// 且 OPlus 机型由安装脚本把校准值写成 1000（见 customize.sh 的 [battery-detect]），
/// 两路口径不一致时以私有节点为准（`oplus_chg` 打开且读到有效值就不回退）。
fn read_standard_battery() -> (i32, i32) {
    (
        read_i32("/sys/class/power_supply/battery/current_now").unwrap_or(UNAVAIL),
        read_i32("/sys/class/power_supply/battery/voltage_now").unwrap_or(UNAVAIL),
    )
}

/// OPlus 私有节点（bcc_parms）：逗号分隔字段，**0 基下标**（首项是下标 0）——
/// 下标 6 = 电芯0电压（第 7 项）、8 = 电流（第 9 项）、11 = 电芯1电压（第 12 项，
/// 双电芯机型；单电芯机型为 0）。该节点由 BCC 硬件直出、随采样刷新，
/// 而 OPlus 内核标准 power_supply 节点（current_now/voltage_now）约每 10s
/// 才刷新一次——1s 精度的功耗统计必须优先走该节点，否则读到的是重复旧值。
const OPLUS_BCC_PARMS: &str = "/sys/class/oplus_chg/battery/bcc_parms";

/// 读 OPlus bcc_parms，存**节点原始值**（不做任何换算）供共享快照使用。
/// 该节点报 mV / mA，配套校准除数填 **1000**（安装脚本检测到该节点时会写入；
/// 缺省 1000000 是标准 Android ABI µV/µA 的口径）。
/// 不做量级启发式猜测（旧的 mV/V、mA/A 自动识别 + 2–6V/±30A 物理范围门已删除）。
/// 字段缺失/越界返回 None（调用方回退标准 power_supply 节点）。
fn read_oplus_bcc(dual_cell: bool) -> Option<(i32, i32)> {
    let text = std::fs::read_to_string(OPLUS_BCC_PARMS).ok()?;
    let f: Vec<&str> = text.split(',').map(str::trim).collect();
    let v0: i64 = match f.get(6).and_then(|s| s.parse().ok()) {
        Some(v) => v,
        None => return bcc_unusable(),
    };
    let cur: i64 = match f.get(8).and_then(|s| s.parse().ok()) {
        Some(v) => v,
        None => return bcc_unusable(),
    };
    if v0 == 0 && cur == 0 {
        return None;
    }
    // 双电芯（`oplus_dual_cell` 打开时）= 两节**并联**（2P）：电压取两节**平均**
    // （样本 3841 / 3835 mV，仅差 6mV，同压域），电流 **×2**——下标 8 是**单节支路**
    // 电流，整包电流 = 两节之和（2026-09-18 用户补充）。不取电压和：那是 2S 串压
    // 口径，同机型标准节点只报 ~4V 时功率会翻倍。下标 11 缺失或非正视为单电芯
    // 布局的该字段无意义：电压退回下标 6，电流也不翻倍。
    // 自检：私有节点算出的 P 应与「标准节点 V × I」同量级（±20%）。
    let (v_raw, i_raw) = if dual_cell {
        let v1: i64 = f.get(11).and_then(|s| s.parse().ok()).unwrap_or(0);
        if v1 > 0 {
            ((v0 + v1) / 2, cur.checked_mul(2)?)
        } else {
            (v0, cur)
        }
    } else {
        (v0, cur)
    };
    // 原样存：越界（超出 i32）视为不可用，回退标准节点
    Some((i32::try_from(v_raw).ok()?, i32::try_from(i_raw).ok()?))
}

/// 电压/电流字段不可用：告警一次并返回 None（调用方回退标准 power_supply 节点）。
/// 无此打点时「BCC 其实一直没生效、功耗列一直来自 10s 缓存节点」完全不可见。
fn bcc_unusable() -> Option<(i32, i32)> {
    if !BCC_UNUSABLE_WARNED.swap(true, Ordering::Relaxed) {
        log::warn!("{}", crate::i18n::t("telemetry-bcc-unusable"));
    }
    None
}

// [loop]
/// 遥测线程主循环：1s 轮询刷新共享快照。GPU 路径探测成功后缓存，避免每轮扫描。
pub fn telemetry_loop() {
    // GPU 利用率候选节点：高通 Adreno → MTK GED（按存在性取首个可读者）
    let gpu_candidates = [
        "/sys/class/kgsl/kgsl-3d0/gpu_busy_percentage",
        "/sys/kernel/ged/hal/gpu_loading",
    ];
    let mut gpu_path: Option<&str> = None;

    // OPlus 私有节点：开关打开期间探测（None = 尚未探测）；开关关着时不碰该节点
    let mut bcc_available: Option<bool> = None;
    let mut bcc_probed_at: Option<std::time::Instant> = None;

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
        // 候选节点（Adreno kgsl → MTK GED）全部不存在 = 该机型没有可读节点：
        // 一条 warn 说明「GPU 列恒为 -」是机型限制而非读取故障（1s 轮询只报一次）
        if gpu_path.is_none() {
            gpu_path = gpu_candidates
                .iter()
                .copied()
                .find(|p| std::path::Path::new(p).exists());
            if gpu_path.is_none() && !GPU_UNAVAIL_WARNED.swap(true, Ordering::Relaxed) {
                log::warn!("{}", crate::i18n::t("telemetry-gpu-unavailable"));
            }
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
        let use_oplus = OPLUS_CHG.load(Ordering::Relaxed);
        if use_oplus
            && bcc_probed_at.map_or(true, |t: std::time::Instant| {
                t.elapsed() >= BCC_PROBE_INTERVAL
            })
        {
            let now = std::path::Path::new(OPLUS_BCC_PARMS).exists();
            // 只在状态变化时打点：可用 = info；不可用 = warn（一次，恢复后重新武装）
            if bcc_available != Some(now) {
                if now {
                    log::info!("{}", crate::i18n::t("telemetry-oplus-bcc"));
                    BCC_MISSING_WARNED.store(false, Ordering::Relaxed);
                } else if !BCC_MISSING_WARNED.swap(true, Ordering::Relaxed) {
                    log::warn!("{}", crate::i18n::t("telemetry-oplus-bcc-missing"));
                }
            }
            bcc_available = Some(now);
            bcc_probed_at = Some(std::time::Instant::now());
        }
        let (mut current, mut voltage) = if use_oplus && bcc_available == Some(true) {
            // 私有节点**存在且有效**（字段可解析、非全零）时只认它，不回退标准节点——
            // 同机型的标准通用节点单位与刷新周期都不是给外部读的。存在却无效
            // （字段缺失、解析失败、全零）时才回退：此刻它是唯一还能给数的来源，
            // 回退好过整段读数变空（无效原因由 read_oplus_bcc 内的 warn 打点）
            match read_oplus_bcc(OPLUS_DUAL_CELL.load(Ordering::Relaxed)) {
                Some((v, i)) => {
                    // 首次读到私有节点：把原始值与当前校准值打一条 info，单位对不对看这几个数
                    if !BCC_RAW_LOGGED.swap(true, Ordering::Relaxed) {
                        log::info!(
                            "{}",
                            crate::i18n::t_with_args(
                                "telemetry-raw-snapshot",
                                &crate::fluent_args!(
                                    "v" => v.to_string(),
                                    "i" => i.to_string(),
                                    "vd" => format!("{:.0}", voltage_divisor()),
                                    "cd" => format!("{:.0}", current_divisor())
                                )
                            )
                        );
                    }
                    (i, v)
                }
                None => read_standard_battery(),
            }
        } else {
            read_standard_battery()
        };
        // 倍电压/倍电流：只作用于标准节点路径（私有开关打开时 UI 已强制关闭这两个开关，
        // 这里再判一次，手改 meta 也挡得住）
        if !use_oplus {
            if voltage != UNAVAIL && VOLTAGE_DOUBLE.load(Ordering::Relaxed) {
                voltage = voltage.saturating_mul(2);
            }
            if current != UNAVAIL && CURRENT_DOUBLE.load(Ordering::Relaxed) {
                current = current.saturating_mul(2);
            }
        }
        // 候选全失效（BCC 与标准节点都读不到）报一条 warn，恢复后重新武装：
        // 逐候选失败不单独打点——1s 轮询下那是刷屏，读不到的价值由这条汇总体现
        if current == UNAVAIL || voltage == UNAVAIL {
            if !BATT_UNAVAIL_WARNED.swap(true, Ordering::Relaxed) {
                log::warn!("{}", crate::i18n::t("telemetry-battery-unavailable"));
            }
        } else {
            BATT_UNAVAIL_WARNED.store(false, Ordering::Relaxed);
        }
        TELEMETRY.batt_current_raw.store(current, Ordering::Relaxed);
        TELEMETRY.batt_voltage_raw.store(voltage, Ordering::Relaxed);

        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}
