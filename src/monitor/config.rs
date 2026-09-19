//! config.rs: [helpers] [rules]

use crate::common;
pub use crate::fas_types::FasRulesConfig;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

// [helpers] 
pub fn get_rules_path() -> PathBuf {
    common::get_module_root().join("rules.yaml")
}

/// global_mode 缺省时使用模块模板中的默认（default）模式
fn default_global_mode() -> String {
    "default".to_string()
}

/// app_modes 缺失或为 null 时按空表处理：WebUI 旧版本会把空 app_modes 写成
/// "app_modes: null"，而 serde_yaml 无法把 null 反序列化为 HashMap
/// （#[serde(default)] 只对缺失字段生效），会导致 rules.yaml 解析失败并告警。
/// 这里显式兼容 null，保证守护进程读取/热重载不出错。
fn deserialize_app_modes<'de, D>(deserializer: D) -> Result<HashMap<String, String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum MapOrNull {
        Map(HashMap<String, String>),
        Null,
    }
    Ok(match MapOrNull::deserialize(deserializer)? {
        // **预处理**：规则键允许写带子进程后缀的进程名（`com.xx:push`），加载时统一
        // 归一到主包名（`com.xx`）——子进程本质上还是那个包，调度以包为单位，
        // 归一后前台名（同样在 `set_current_package` 归一）与规则键口径一致，
        // 查表就是原本的精确匹配。冲突（两个键归一到同一主包名）后者保留并告警，
        // 避免静默丢规则。
        MapOrNull::Map(m) => {
            let mut out: HashMap<String, String> = HashMap::with_capacity(m.len());
            for (k, v) in m {
                match k.split_once(':') {
                    Some((base, _suffix)) => {
                        log::warn!(
                            "{}",
                            crate::i18n::t_with_args(
                                "rule-key-normalized",
                                &crate::fluent_args!("key" => k.as_str(), "base" => base)
                            )
                        );
                        if out.insert(base.to_string(), v).is_some() {
                            log::warn!(
                                "{}",
                                crate::i18n::t_with_args(
                                    "rule-key-conflict",
                                    &crate::fluent_args!("key" => k.as_str(), "base" => base)
                                )
                            );
                        }
                    }
                    None => {
                        out.insert(k, v);
                    }
                }
            }
            out
        }
        MapOrNull::Null => HashMap::new(),
    })
}

// [rules] 

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct RulesConfig {
    #[serde(default = "crate::utils::default_true")]
    pub yumi_scheduler: bool,
    // 注意：以下字段缺省时必须以 null/省略 安全反序列化。
    // 若不加 #[serde(default)]，用户精简 rules.yaml（删除任一字段）会导致
    // serde 报 missing field，read_config 回退 Default（dynamic_enabled=false）
    // 进而 dynamic 模式失效、CLG 无法按规则启动。
    // 缺省值需与模块随附 rules.yaml 模板保持一致：
    //   dynamic_enabled 缺省 true、global_mode 缺省 "default"，
    // 否则删除字段后仍会进入空模式导致 CLG 不接管 CPU。
    #[serde(default = "crate::utils::default_true")]
    pub dynamic_enabled: bool,
    #[serde(default = "default_global_mode")]
    pub global_mode: String,
    #[serde(default, deserialize_with = "deserialize_app_modes")]
    pub app_modes: HashMap<String, String>,
    #[serde(default)]
    pub ignored_apps: Vec<String>,
    #[serde(default)]
    pub fas_rules: FasRulesConfig,
}
