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

use crate::common;
pub use crate::fas_types::FasRulesConfig;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

pub fn get_rules_path() -> PathBuf {
    common::get_module_root().join("rules.yaml")
}

/// global_mode 缺省时使用与模块模板一致的均衡（balance）模式
fn default_global_mode() -> String {
    "balance".to_string()
}

// ════════════════════════════════════════════════════════════════
//  Rules 配置
// ════════════════════════════════════════════════════════════════

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct RulesConfig {
    #[serde(default = "crate::utils::default_true")]
    pub yumi_scheduler: bool,
    // 注意：以下字段缺省时必须以 null/省略 安全反序列化。
    // 若不加 #[serde(default)]，用户精简 rules.yaml（删除任一字段）会导致
    // serde 报 missing field，read_config 回退 Default（dynamic_enabled=false）
    // 进而 dynamic 模式失效、CLG 无法按规则启动。
    // 缺省值需与模块随附 rules.yaml 模板保持一致：
    //   dynamic_enabled 缺省 true、global_mode 缺省 "balance"，
    // 否则删除字段后仍会进入空模式导致 CLG 不接管 CPU。
    #[serde(default = "crate::utils::default_true")]
    pub dynamic_enabled: bool,
    #[serde(default = "default_global_mode")]
    pub global_mode: String,
    #[serde(default)]
    pub app_modes: HashMap<String, String>,
    #[serde(default)]
    pub ignored_apps: Vec<String>,
    #[serde(default)]
    pub fas_rules: FasRulesConfig,
}
