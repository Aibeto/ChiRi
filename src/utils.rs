//! utils.rs: [io] [temp_probe] [batt_temp] [sys_path] [fast_writer] [misc]

use anyhow::Result;
use inotify::{Inotify, WatchMask};
use log;
use nix::unistd::{AccessFlags, access};
use serde::de::DeserializeOwned;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::fluent_args;
use crate::i18n::t_with_args;

// [io]
/// 向文件写入内容，并处理可能的错误
pub fn write_to_file<P: AsRef<Path>, C: AsRef<[u8]>>(path: P, content: C) -> Result<()> {
    let path = path.as_ref();

    // 尝试修改权限以便写入
    if path.exists() {
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o664));
    }

    fs::write(path, content)?;

    // 写完后设为只读
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o444));
    Ok(())
}

// 尝试写入内容 (不抛出错误，只记录警告)
pub fn try_write_file<P: AsRef<Path>, C: AsRef<[u8]>>(path: P, content: C) -> Result<()> {
    if let Err(e) = write_to_file(path.as_ref(), content) {
        log::warn!("Failed to write to {}: {}.", path.as_ref().display(), e);
    }
    Ok(())
}

pub fn enable_perm<P: AsRef<Path>>(path: P) -> Result<()> {
    let path = path.as_ref();
    if path.exists() {
        fs::set_permissions(path, fs::Permissions::from_mode(0o664))?;
    }
    Ok(())
}

/// 监控指定路径的文件/目录事件
pub fn watch_path<P: AsRef<Path>>(path_to_watch: P) -> Result<()> {
    let mut inotify = Inotify::init()?;
    // CLOSE_WRITE 覆盖直接写入；MOVED_TO 覆盖原子替换（WebUI 用临时文件 + mv 保存配置时
    // 是 rename 而非写打开，只有 MOVED_TO 能感知），两者任一触发即返回并触发重载
    inotify
        .watches()
        .add(path_to_watch, WatchMask::CLOSE_WRITE | WatchMask::MOVED_TO)?;

    let mut buffer = [0u8; 1024];
    inotify.read_events_blocking(&mut buffer)?;
    Ok(())
}

// 读取文件内容解析为 f64
pub fn read_f64_from_file(path: &str) -> Result<f64> {
    let mut content = String::new();
    File::open(path)?.read_to_string(&mut content)?;
    let val: f64 = content.trim().parse()?;
    Ok(val)
}

// 辅助函数：读取文件内容为 String
pub fn read_file_content(path: &str) -> Result<String> {
    let mut content = String::new();
    File::open(path)?.read_to_string(&mut content)?;
    Ok(content.trim().to_string())
}

// [temp_probe]
// 查找 CPU 温度传感器路径
pub fn find_cpu_temp_path() -> Result<String> {
    let thermal_path = "/sys/class/thermal";
    let thermal_dir = Path::new(thermal_path);

    if !thermal_dir.exists() {
        return Err(anyhow::anyhow!("Thermal directory not found"));
    }

    for entry in fs::read_dir(thermal_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            if let Some(dir_name) = path.file_name().and_then(|s| s.to_str()) {
                if dir_name.starts_with("thermal_zone") {
                    let type_path = path.join("type");
                    // 修复 E0532 模式匹配错误: 直接使用 if let Ok(...)
                    if let Ok(type_content) =
                        read_file_content(type_path.to_str().unwrap_or_default())
                    {
                        if type_content.contains("soc_max")
                            || type_content.contains("mtktscpu")
                            || type_content.contains("cpu-1-")
                            || type_content.contains("cpu-0-0-usr")
                        {
                            let temp_path = path.join("temp");
                            if temp_path.exists() {
                                return Ok(temp_path.to_str().unwrap().to_string());
                            }
                        }
                    }
                }
            }
        }
    }
    Err(anyhow::anyhow!("Valid CPU thermal zone not found"))
}

/// 电池温度节点路径。**原始刻度的单位因内核/厂商而异，不可硬编码换算**，
/// 读取一律先经 `battery_temp_scale()` 预识别（见下）。
/// None = 节点不存在（依赖它的功能自动失效，不影响其余功能）
pub fn find_battery_temp_path() -> Option<&'static str> {
    const BATT_TEMP: &str = "/sys/class/power_supply/battery/temp";
    std::path::Path::new(BATT_TEMP)
        .exists()
        .then_some(BATT_TEMP)
}

// 电池温度刻度预识别
//
// `/sys/class/power_supply/battery/temp` 的单位在不同内核/厂商上不一致：
// 内核 power_supply 标准是 0.1°C（raw 400 = 40.0°C），部分平台报毫摄氏度
// （raw 40000 = 40.0°C），少数厂商直接报 °C（raw 40 = 40.0°C）。硬编码除数
// 会带来 10×/100× 偏差：8550 的 0.1°C 节点曾被误按毫摄氏度除 1000，实测
// 恒读 0.4°C（真实约 20~50°C），电池软/硬限（41/45°C）永不触发、主参考
// 彻底失效。故改为运行时预识别一次并缓存，CLG 热保护与 FAS 温度护栏共用
// 同一结论，避免两处口径漂移。

// [batt_temp]
/// 电池温度节点的原始刻度
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BatteryTempScale {
    /// 0.1°C（内核 power_supply 标准）：raw 400 = 40.0°C
    TenthsCelsius,
    /// 毫摄氏度：raw 40000 = 40.0°C
    MilliCelsius,
    /// 摄氏度（少数厂商直读）：raw 40 = 40.0°C
    Celsius,
}

impl BatteryTempScale {
    /// 原始读数换算为 °C 的除数
    pub fn divisor(self) -> f64 {
        match self {
            BatteryTempScale::TenthsCelsius => 10.0,
            BatteryTempScale::MilliCelsius => 1000.0,
            BatteryTempScale::Celsius => 1.0,
        }
    }

    /// 日志用刻度名
    pub fn label(self) -> &'static str {
        match self {
            BatteryTempScale::TenthsCelsius => "0.1C",
            BatteryTempScale::MilliCelsius => "milliC",
            BatteryTempScale::Celsius => "C",
        }
    }
}

/// 合理电池温度窗口（°C）：用于在多种单位解释里挑出唯一合理者。
/// 下界取 5、上界取 80：直读 °C 的内核常给出 5..80，若下界取 0，tenths 解释
/// （raw/10）会把直读值 40 误判成 4.0°C 从而失去区分度；上界放宽到 80 是为了
/// 让偶发高温读数仍能参与定档（真正离谱的值由热保护 `TempFilter` 的物理范围门兜底）。
/// 该窗口下 tenths 与 milli 的解释不会互相碰撞（tenths raw 50..800 ↔ milli raw 5万..8万）
const BATT_TEMP_PLAUSIBLE_MIN_C: f64 = 5.0;
const BATT_TEMP_PLAUSIBLE_MAX_C: f64 = 80.0;

/// 预识别结果缓存：仅在探测得出确定结论时写入；节点未就绪/读数异常时留空，
/// 下次调用自动重试（避免把开机早期的 0/占位值固化下来）
static BATTERY_TEMP_SCALE: OnceLock<BatteryTempScale> = OnceLock::new();

/// 读一次电池温度原始值（不换算）
pub fn read_battery_temp_raw() -> Option<f64> {
    std::fs::read_to_string(find_battery_temp_path()?)
        .ok()?
        .trim()
        .parse::<f64>()
        .ok()
}

/// 预识别电池温度刻度：读一次原始值，按「唯一落进合理温度窗口」的解释定档；
/// 多个解释同时合理时按内核标准优先（tenths > milli > Celsius）。
/// 读数为 0（未初始化）或节点缺失返回 None（调用方下次重试）；
/// 非 0 但三种解释都不合理（传感器异常/极端低温）时回退内核标准 tenths——
/// 更离谱的值由热保护 `TempFilter` 的物理范围门丢弃，不会污染控制链。
pub fn detect_battery_temp_scale() -> Option<BatteryTempScale> {
    let raw = read_battery_temp_raw()?;
    if raw == 0.0 {
        return None;
    }
    [
        BatteryTempScale::TenthsCelsius,
        BatteryTempScale::MilliCelsius,
        BatteryTempScale::Celsius,
    ]
    .into_iter()
    .find(|s| {
        let c = raw / s.divisor();
        (BATT_TEMP_PLAUSIBLE_MIN_C..=BATT_TEMP_PLAUSIBLE_MAX_C).contains(&c)
    })
    .or(Some(BatteryTempScale::TenthsCelsius))
}

/// 取（必要时预识别并缓存）电池温度刻度；None = 无法确定（调用方退化为仅 CPU 温度）
pub fn battery_temp_scale() -> Option<BatteryTempScale> {
    if let Some(s) = BATTERY_TEMP_SCALE.get() {
        return Some(*s);
    }
    let detected = detect_battery_temp_scale()?;
    // 竞态下可能已被其它线程写入（值相同，忽略结果）
    let _ = BATTERY_TEMP_SCALE.set(detected);
    BATTERY_TEMP_SCALE.get().copied()
}

/// 电池温度原始读数的换算除数（供持有 (路径, 除数) 的调用方复用同一结论）
pub fn battery_temp_divisor() -> Option<f64> {
    battery_temp_scale().map(|s| s.divisor())
}

/// 读电池温度并按预识别刻度换算为 °C；节点缺失/读数异常返回 None
pub fn read_battery_temp_celsius() -> Option<f32> {
    let scale = battery_temp_scale()?;
    Some((read_battery_temp_raw()? / scale.divisor()) as f32)
}

// [sys_path]
pub struct SysPathExist {
    pub qcom_feas_exist: bool,
    pub mtk_feas_exist: bool,
    pub walt_exist: bool,
    pub stune_exist: bool,
    pub hi6220_ufs_exist: bool,
    pub cpuctl_top_app_exist: bool,
    pub cpuctl_foreground_exist: bool,
    pub cpuctl_background_exist: bool,
    pub cpuset_top_app_exist: bool,
    pub cpuset_foreground_exist: bool,
    pub cpuset_background_exist: bool,
    pub cpuset_system_background_exist: bool,
    pub cpuset_restricted_exist: bool,
    pub cpuset_root_exist: bool,
    pub cpuidle_governor_exist: bool,
    pub sda_scheduler_exist: bool,
}

impl SysPathExist {
    pub fn new() -> Self {
        Self {
            qcom_feas_exist: Self::path_exists("/sys/module/perfmgr/parameters/perfmgr_enable"),
            mtk_feas_exist: Self::path_exists("/sys/module/mtk_fpsgo/parameters/perfmgr_enable"),
            walt_exist: Self::path_exists("/proc/sys/walt"),
            stune_exist: Self::path_exists("/dev/stune"),
            hi6220_ufs_exist: Self::path_exists(
                "/sys/bus/platform/devices/hi6220-ufs/ufs_clk_gate_disable",
            ),
            cpuctl_top_app_exist: Self::path_exists("/dev/cpuctl/top-app"),
            cpuctl_foreground_exist: Self::path_exists("/dev/cpuctl/foreground"),
            cpuctl_background_exist: Self::path_exists("/dev/cpuctl/background"),
            cpuset_top_app_exist: Self::path_exists("/dev/cpuset/top-app"),
            cpuset_foreground_exist: Self::path_exists("/dev/cpuset/foreground"),
            cpuset_background_exist: Self::path_exists("/dev/cpuset/background"),
            cpuset_system_background_exist: Self::path_exists("/dev/cpuset/system-background"),
            cpuset_restricted_exist: Self::path_exists("/dev/cpuset/restricted"),
            cpuset_root_exist: Self::path_exists("/dev/cpuset"),
            cpuidle_governor_exist: Self::path_exists(
                "/sys/devices/system/cpu/cpuidle/current_governor",
            ),
            sda_scheduler_exist: Self::path_exists("/sys/block/sda/queue/scheduler"),
        }
    }

    fn path_exists(path: &str) -> bool {
        access(path, AccessFlags::F_OK).is_ok()
    }
}

// [fast_writer]
// FastWriter — 带去重 + unmount 的 sysfs 写入器

pub struct FastWriter {
    file: Option<File>,
    buf: [u8; 20],
    path: PathBuf,
    /// 本次生命周期内是否已尝试过 umount（最多一次，避免反复 detach 合法挂载）
    unmounted: bool,
}

impl FastWriter {
    pub fn new<P: AsRef<Path>>(path: P) -> Self {
        let path_ref = path.as_ref();
        let _ = enable_perm(path_ref);
        let mut w = Self {
            file: Self::open_file(path_ref),
            buf: [0u8; 20],
            path: path_ref.to_path_buf(),
            unmounted: false,
        };
        // 惰性卸载：仅当直接打开失败（厂商 bind mount 保护 / 权限封装）时才尝试 umount 重开，
        // 避免对正常设备上每个节点无条件 detach（可能拆掉合法挂载）。写入被拒（EACCES/EROFS）
        // 时也会走一次卸载重试（见 do_write）。
        if w.file.is_none() {
            w.unmount_and_reopen();
        }
        w
    }

    fn open_file(path: &Path) -> Option<File> {
        OpenOptions::new()
            .write(true)
            .open(path)
            .map_err(|e| {
                log::error!(
                    "{}",
                    t_with_args(
                        "sysfs-open-failed",
                        &fluent_args!("path" => path.display().to_string(), "error" => e.to_string())
                    )
                )
            })
            .ok()
    }

    fn unmount_and_reopen(&mut self) {
        Self::try_unmount(&self.path);
        self.unmounted = true;
        // 卸载后强制重新打开：旧 fd 可能仍指向被卸载的挂载背衬，需换到真实节点
        self.file = Self::open_file(&self.path);
    }

    fn try_unmount(path: &Path) {
        if let Some(path_str) = path.to_str() {
            if let Ok(cpath) = std::ffi::CString::new(path_str) {
                let ret = unsafe { libc::umount2(cpath.as_ptr(), libc::MNT_DETACH) };
                if ret != 0 {
                    let errno = std::io::Error::last_os_error();
                    if errno.raw_os_error() != Some(libc::EINVAL)
                        && errno.raw_os_error() != Some(libc::ENOENT)
                    {
                        log::debug!(
                            "{}",
                            t_with_args(
                                "sysfs-umount2-failed",
                                &fluent_args!("path" => path_str, "error" => errno.to_string())
                            )
                        );
                    }
                }
            }
        }
    }

    pub fn re_unmount(&self) {
        Self::try_unmount(&self.path);
    }

    pub fn write_value_force(&mut self, value: u32) -> bool {
        self.do_write(value)
    }

    pub fn is_valid(&self) -> bool {
        self.file.is_some()
    }

    fn do_write(&mut self, value: u32) -> bool {
        let len = Self::u32_to_buf(value, &mut self.buf);
        let write_result = if let Some(file) = &mut self.file {
            let _ = file.seek(SeekFrom::Start(0));
            file.write_all(&self.buf[..len])
        } else {
            return false;
        };
        match write_result {
            Ok(()) => true,
            Err(e) => {
                match e.raw_os_error() {
                    // EINVAL(22): 内核拒绝该频率 (热限频 / 范围收窄)
                    // EBUSY(16): sysfs 节点短暂被占用
                    // 两者均为预期内的瞬态错误，降级为 debug 并且不缓存，下次 tick 自动重试
                    Some(libc::EINVAL) | Some(libc::EBUSY) => {
                        log::debug!("write freq {} to {:?} skipped: {}", value, self.path, e);
                    }
                    // EACCES/EROFS: 多为厂商挂载写保护，卸载后重试一次
                    Some(libc::EACCES) | Some(libc::EROFS) if !self.unmounted => {
                        log::warn!(
                            "{}",
                            t_with_args(
                                "sysfs-write-freq-failed",
                                &fluent_args!("freq" => value.to_string(), "error" => e.to_string())
                            )
                        );
                        self.unmount_and_reopen();
                        return self.do_write(value);
                    }
                    _ => {
                        log::warn!(
                            "{}",
                            t_with_args(
                                "sysfs-write-freq-failed",
                                &fluent_args!("freq" => value.to_string(), "error" => e.to_string())
                            )
                        );
                    }
                }
                // 写入失败不更新 last_value，保证下次 tick 会重试
                false
            }
        }
    }

    fn u32_to_buf(mut v: u32, buf: &mut [u8; 20]) -> usize {
        if v == 0 {
            buf[0] = b'0';
            buf[1] = b'\n';
            return 2;
        }
        let mut pos = 18;
        while v > 0 {
            buf[pos] = b'0' + (v % 10) as u8;
            v /= 10;
            pos -= 1;
        }
        let start = pos + 1;
        let digit_len = 19 - start;
        buf.copy_within(start..19, 0);
        buf[digit_len] = b'\n';
        digit_len + 1
    }
}

// [misc]
// 通用跨模块工具函数
/// Serde 默认值辅助函数：始终返回 true
pub fn default_true() -> bool {
    true
}

/// 读取文件内容并解析为 i32
pub fn read_i32_from_file(path: &str) -> Result<i32> {
    let mut content = String::new();
    File::open(path)?.read_to_string(&mut content)?;
    Ok(content.trim().parse()?)
}

/// 获取与 BPF ktime_get_ns() 绝对对齐的单调时钟时间 (纳秒)
pub fn get_ktime_ns() -> u64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    (ts.tv_sec as u64) * 1_000_000_000 + (ts.tv_nsec as u64)
}

/// Serde 反序列化辅助：从 YAML 文件读取配置，解析失败时返回 Default
pub fn read_config<T, P>(path: P) -> Result<T>
where
    T: DeserializeOwned + Default,
    P: AsRef<Path>,
{
    let path_ref = path.as_ref();
    match File::open(path_ref) {
        Ok(mut file) => {
            let mut s = String::new();
            file.read_to_string(&mut s)?;
            serde_yaml::from_str(&s).or_else(|e| {
                log::warn!(
                    "[Config] Parse error {}: {}. Default.",
                    path_ref.display(),
                    e
                );
                Ok(T::default())
            })
        }
        Err(_) => {
            log::warn!("[Config] Not found: {}. Default.", path_ref.display());
            Ok(T::default())
        }
    }
}
