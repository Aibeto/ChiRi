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

use crate::monitor::config::RulesConfig;
use std::env;
use std::path::PathBuf;

/// 守护进程全局事件总线
/// FAS 暂禁用：`FrameUpdate` 变体暂无生产者、`ModeChange.pid` 与 `SystemLoadUpdate.foreground_max_util`
/// 暂无消费者，均为恢复 FAS 时保留，故允许 dead_code。
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub enum DaemonEvent {
    /// 低频事件：前台应用切换或环境温度变化引起的模式改变
    ModeChange {
        package_name: String,
        pid: i32,
        mode: String,
        temperature: f64,
    },
    /// 高频事件：eBPF 捕获到的底层渲染帧数据
    FrameUpdate {
        frame_delta_ns: u64, // 纳秒级帧间隔
    },
    /// eBPF 全局系统负载更新 (每 X 毫秒触发一次)
    SystemLoadUpdate {
        /// 每个 CPU 核心的真实利用率 (0.0 ~ 1.0)，数组索引即 cpu_id
        core_utils: Vec<f32>,
        /// 如果当前有前台应用，这是该应用最吃 CPU 的那 1 个线程的利用率
        foreground_max_util: f32,
    },

    ConfigReload(RulesConfig),

    ScreenStateChange(bool),
}

/// 获取模块根目录的绝对路径
pub fn get_module_root() -> PathBuf {
    // 获取当前执行文件的绝对路径
    let exe_path = env::current_exe().unwrap_or_else(|_| PathBuf::from("/"));

    // 回溯两级目录:
    // core/bin/yumi -> core/bin -> core -> yumi
    exe_path
        .parent()
        .unwrap_or(&exe_path) // .../core/bin
        .parent()
        .unwrap_or(&exe_path) // .../core
        .parent()
        .unwrap_or(&exe_path) // .../yumi (Root)
        .to_path_buf()
}

/// 读取文件首行并去除空白（用于 SoC 型号探测）
fn read_first_line(path: &str) -> String {
    std::fs::read_to_string(path)
        .map(|s| s.lines().next().unwrap_or("").trim().to_string())
        .unwrap_or_default()
}

/// 触发 Chiri 专用调度的特定处理器型号片段列表。
/// 探测到任一命中即启用 Chiri 调度器；新增机型只需在此追加片段，不要绑定单一型号。
/// 例：SM8550（骁龙 8 Gen 2）含 "8550"
const CHIRI_SOC_HINTS: &[&str] = &["8550"];

/// 型号片段是否命中任一特定处理器
fn soc_hint_matches(hints: &[&str]) -> bool {
    // 来源1：/sys/devices/soc0/machine（高通直接暴露型号，如 "SM8550"）
    let machine = read_first_line("/sys/devices/soc0/machine");
    if hints.iter().any(|h| machine.contains(h)) {
        return true;
    }
    // 来源2：/proc/cpuinfo（Hardware / model name 行可能含型号）
    let cpuinfo = std::fs::read_to_string("/proc/cpuinfo").unwrap_or_default();
    hints.iter().any(|h| cpuinfo.contains(h))
}

/// 是否应启用 Chiri 专用调度器（检测到列表中的特定处理器时为 true）
pub fn is_chiri_soc() -> bool {
    soc_hint_matches(CHIRI_SOC_HINTS)
}

/// 返回当前应加载的配置文件路径：
/// - 命中 Chiri 目标 SoC 且 config 目录存在 `config_{命中片段}.yaml` 时，使用该独立文件
/// - 否则回退到默认 `config.yaml`
///
/// 所有配置加载/热重载入口（main.rs 与两套调度器的 config_watcher）统一走这里，
/// 保证 8550 等目标机型使用独立配置，其余机型不受影响。
pub fn get_config_path() -> PathBuf {
    let config_dir = get_module_root().join("config");
    if is_chiri_soc() {
        for hint in CHIRI_SOC_HINTS {
            let candidate = config_dir.join(format!("config_{}.yaml", hint));
            if candidate.exists() {
                return candidate;
            }
        }
    }
    config_dir.join("config.yaml")
}
