//! FAS（帧感知调度）实例管理器 —— ChiRi 专属。
//!
//! 解耦多实例架构：每个 FAS 白名单应用对应一个独立 FasInstance（逻辑 FAS 进程），
//! 应用进入前台时立即创建/复用（C1），失去激活时立即恢复频率但保留实例 60 秒（C2/C3），
//! 超时注销。帧/负载/温度事件只喂当前活跃实例（C6）。
//!
//! 生命周期规则：
//! - C1 创建/复用：activate() —— 无实例则 FasController::new + load_policies（快照真实系统状态），
//!   有实例（60s 内切回）则复用：set_game 挂回包名 + apply_freqs 重写频率。
//! - C2 去激活：deactivate_active() —— 仅对活跃实例 reset_all_freqs + clear_game，
//!   必须先于任何其他 governor（CLG/akmode/fast）的 init，保证对方快照到真实状态；
//!   非活跃实例的频率已在各自去激活时恢复，绝不再写（避免踩坏后续 governor）。
//! - C3 注销：reap() —— 非活跃实例 last_fg 超过 FAS_INSTANCE_TTL 后移除（纯内存清理）。
//! - C4 息屏释放：息屏时调用方 deactivate_active() 恢复频率并交由 CLG doze /
//!   scenemode 全局接管，实例保留 60s；亮屏后由 ModeChange / 1s 兜底重新 activate()。
//!   （原设计为息屏 enter_doze 写低频锁、亮屏 exit_doze 恢复，但锁频恢复依赖帧事件
//!   驱动的 apply_freqs——锁屏前台无帧事件时全簇被锁死在最低频，亮屏后 0.2fps，
//!   且息屏期间无法进入 CLG doze / scenemode，已废弃）
//! - C5 收尾：deactivate_all() —— panic/线程退出时恢复全部频率。
//! - C6 事件路由：on_frame/on_load_update/温度刷新只作用于活跃实例。

use std::path::PathBuf;
use std::time::{Duration, Instant};

use log::info;

use crate::fluent_args;
use crate::i18n::t_with_args;
use crate::scheduler::fas::FasController;

/// 非活跃实例保留时长：丢失前台后 60s 内切回可复用（免重建 policies 快照）
const FAS_INSTANCE_TTL: Duration = Duration::from_secs(60);
/// 活跃实例 CPU 温度刷新周期（喂给 FAS 引擎内部限温逻辑，core_temp_threshold=0 时无效）
const FAS_TEMP_REFRESH: Duration = Duration::from_secs(3);

struct FasInstance {
    package: String,
    controller: FasController,
    last_fg: Instant,
}

pub struct FasManager {
    instances: Vec<FasInstance>,
    active_pkg: Option<String>,
    last_temp: f64,
    last_temp_read: Instant,
    temp_path: Option<PathBuf>,
}

impl FasManager {
    /// temp_path：FAS 专用 CPU 温度源（毫摄氏度），None = 无传感器（内部限温默认关闭，不影响其余功能）
    pub fn new(temp_path: Option<PathBuf>) -> Self {
        Self {
            instances: Vec::new(),
            active_pkg: None,
            last_temp: 0.0,
            last_temp_read: Instant::now(),
            temp_path,
        }
    }

    /// C1：创建或复用实例并激活。返回 false = 白名单/配置不可用或 load_policies 后无可用 policy
    /// （调用方走冷却回退）。
    ///
    /// - 已是活跃实例 → 仅刷新 last_fg 与 set_game（重复 activate 不重复打点）；
    /// - 已有实例（60s 内切回）→ 复用：set_game 挂回包名 + apply_freqs 按 perf_index 重写频率。
    ///   引擎状态复位由引擎自带的应用切换语义完成：去激活时已 clear_game，复活后首个
    ///   真实帧的帧间隔必然跨越失去前台的整段时长、超过 `app_switch_gap_ms`，
    ///   `handle_early_exit` 会内部 reset_runtime 并落 `app_switch_resume_perf`；
    /// - 新建 → FasController::new + load_policies，policies 为空返回 false。
    ///
    /// 随后 set_game(pid, pkg) + set_temperature(last_temp) + set_temp_threshold(rules.core_temp_threshold)，
    /// active_pkg = Some(pkg)，info 打点 scheduler-fas-activate（fluent 参数 pkg、pid）
    /// + devimp event("fas", pkg, "activate")。
    pub fn activate(&mut self, pkg: &str, pid: i32) -> bool {
        // 白名单复查：包名 → 白名单配置名 → FAS 规则（'static，normalize 已在缓存时完成）
        let Some(rules) = crate::common::fas_whitelist_entry(pkg)
            .and_then(|cfg| crate::common::fas_app_config(cfg))
        else {
            return false;
        };

        // fas→fas 热切换来源包（在防御性去激活之前捕获，否则 active_pkg 已被清空）
        let switch_from = self
            .active_pkg
            .as_deref()
            .filter(|a| *a != pkg)
            .map(str::to_string);

        // 单活跃不变量：若另一实例仍活跃，先按 C2 去激活（防御调用方未显式调用）
        if switch_from.is_some() {
            self.deactivate_active();
        }

        // 已是活跃实例：仅刷新 last_fg 与 set_game
        if self.active_pkg.as_deref() == Some(pkg) {
            if let Some(inst) = self.active_instance_mut() {
                inst.last_fg = Instant::now();
                inst.controller.set_game(pid, pkg);
            }
            return true;
        }

        // 已有实例（60s 内切回）：复用，免重建 policies 快照
        if let Some(pos) = self.instances.iter().position(|i| i.package == pkg) {
            let inst = &mut self.instances[pos];
            inst.last_fg = Instant::now();
            inst.controller.set_game(pid, pkg);
            inst.controller.set_temperature(self.last_temp);
            inst.controller
                .set_temp_threshold(rules.core_temp_threshold);
            inst.controller.apply_freqs();
            self.active_pkg = Some(pkg.to_string());
            // fas→fas 切换与首次激活区分打点
            match switch_from.as_deref() {
                Some(old) => info!(
                    "{}",
                    t_with_args(
                        "scheduler-fas-switch",
                        &fluent_args!("old" => old, "new" => pkg)
                    )
                ),
                None => info!(
                    "{}",
                    t_with_args(
                        "scheduler-fas-activate",
                        &fluent_args!("pkg" => pkg, "pid" => pid.to_string())
                    )
                ),
            }
            crate::logger::devimp_event("fas", pkg, "activate");
            return true;
        }

        // 新建：FasController::new + load_policies（快照真实系统状态：governor/min/max/频点表）
        let mut controller = FasController::new();
        controller.load_policies(rules);
        if controller.policies.is_empty() {
            return false;
        }
        controller.set_game(pid, pkg);
        controller.set_temperature(self.last_temp);
        controller.set_temp_threshold(rules.core_temp_threshold);
        self.instances.push(FasInstance {
            package: pkg.to_string(),
            controller,
            last_fg: Instant::now(),
        });
        self.active_pkg = Some(pkg.to_string());
        info!(
            "{}",
            t_with_args(
                "scheduler-fas-activate",
                &fluent_args!(
                    "pkg" => pkg,
                    "pid" => pid.to_string()
                )
            )
        );
        crate::logger::devimp_event("fas", pkg, "activate");
        true
    }

    /// C2：去激活当前活跃实例（reset_all_freqs + clear_game）。无活跃实例时为无操作。
    /// info 打点 scheduler-fas-deactivate（pkg）+ devimp event("fas", pkg, "deactivate")。
    pub fn deactivate_active(&mut self) {
        let Some(pkg) = self.active_pkg.take() else {
            return;
        };
        if let Some(inst) = self.instances.iter_mut().find(|i| i.package == pkg) {
            // 先恢复频率再清状态：调用方随后 init 其他 governor（CLG/akmode/fast）时，
            // 对方才能快照到真实的系统状态
            inst.controller.reset_all_freqs();
            inst.controller.clear_game();
        }
        info!(
            "{}",
            t_with_args(
                "scheduler-fas-deactivate",
                &fluent_args!("pkg" => pkg.as_str())
            )
        );
        crate::logger::devimp_event("fas", &pkg, "deactivate");
    }

    /// C5：收尾/失败路径——先 deactivate_active 再清空全部实例（非活跃实例零频率写入）。
    pub fn deactivate_all(&mut self) {
        self.deactivate_active();
        self.instances.clear();
    }

    pub fn is_active(&self) -> bool {
        self.active_pkg.is_some()
    }

    pub fn active_pkg(&self) -> Option<&str> {
        self.active_pkg.as_deref()
    }

    /// C6：帧事件（仅活跃实例）。内部每 FAS_TEMP_REFRESH 读一次 temp_path
    /// （毫摄氏度 /1000.0 存 last_temp 并 set_temperature）。
    pub fn on_frame(&mut self, delta_ns: u64) {
        if !self.is_active() {
            return;
        }
        if let Some(inst) = self.active_instance_mut() {
            inst.last_fg = Instant::now();
        }
        self.refresh_temperature();
        if let Some(inst) = self.active_instance_mut() {
            inst.controller.update_frame(delta_ns);
        }
    }

    /// C6：负载事件（仅活跃实例）update_cpu_util(fg_util) + update_core_utils(core_utils)。
    pub fn on_load_update(&mut self, fg_util: f32, core_utils: &[f32]) {
        if let Some(inst) = self.active_instance_mut() {
            inst.controller.update_cpu_util(fg_util);
            inst.controller.update_core_utils(core_utils);
        }
    }

    /// C3：注销超时非活跃实例（活跃实例豁免——长前台也可能超 60s，last_fg 不作注销依据）。
    /// 1s 周期调用，先收集被删包名再 retain。
    pub fn reap(&mut self) {
        let active = self.active_pkg.clone();
        let expired: Vec<String> = self
            .instances
            .iter()
            .filter(|i| {
                Some(&i.package) != active.as_ref() && i.last_fg.elapsed() >= FAS_INSTANCE_TTL
            })
            .map(|i| i.package.clone())
            .collect();
        if expired.is_empty() {
            return;
        }
        self.instances.retain(|i| {
            Some(&i.package) == active.as_ref() || i.last_fg.elapsed() < FAS_INSTANCE_TTL
        });
        for pkg in expired {
            info!(
                "{}",
                t_with_args(
                    "scheduler-fas-destroy",
                    &fluent_args!("pkg" => pkg.as_str())
                )
            );
            crate::logger::devimp_event("fas", &pkg, "destroy");
        }
    }

    fn active_instance_mut(&mut self) -> Option<&mut FasInstance> {
        let pkg = self.active_pkg.as_ref()?;
        self.instances.iter_mut().find(|i| &i.package == pkg)
    }

    /// 每 FAS_TEMP_REFRESH 读一次温度源（毫摄氏度 → ℃），缓存 last_temp 并喂给活跃实例
    /// 的引擎内部限温逻辑（core_temp_threshold=0 时引擎侧无效，此处照常喂）。
    fn refresh_temperature(&mut self) {
        if self.temp_path.is_none() {
            return;
        }
        if self.last_temp_read.elapsed() < FAS_TEMP_REFRESH {
            return;
        }
        self.last_temp_read = Instant::now();
        let Some(path) = self.temp_path.as_ref() else {
            return;
        };
        let Ok(raw) = crate::utils::read_f64_from_file(&path.to_string_lossy()) else {
            return;
        };
        let temp = raw / 1000.0;
        self.last_temp = temp;
        if let Some(inst) = self.active_instance_mut() {
            inst.controller.set_temperature(temp);
        }
    }
}
