//! FAS（帧感知调度）管理器 —— 单实例（2026-09-17 重构，原「一个进程绑一个 FAS 实例」
//! 的多实例架构已废弃）。
//! 区块索引: [types] [activate] [deactivate] [delayed_exit] [events]
//!
//! 白名单前台判断 + 延迟进出：
//! - 白名单应用进入前台 → activate（governor 切 performance + 接管频率）；
//! - 失去前台不立即退出：request_delayed_exit 进入 15s 延迟期（时长来自应用配置
//!   `deactivate_delay_secs`），期间 FAS 仍持有接管（mode 保持 fas、频率停在最后
//!   状态）；切回白名单应用（activate）即取消延迟无缝续期；
//! - 超时由 tick()（1s 周期调用）完成真正退出：恢复频率 + governor 快照，返回 true
//!   让调用方按延迟期记住的目标模式重新接管。
//!
//! governor（performance）归 GovernorGuard 管，频率归 FasController 管，职责分离：
//! 引擎的 load_policies 只快照 min/max/频点表，不碰 governor，两套快照互不踩踏。

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use log::info;

use crate::chiri::governor::GovernorGuard;
use crate::fluent_args;
use crate::i18n::t_with_args;
use crate::scheduler::fas::FasController;

/// 活跃实例温度刷新周期（喂给 FAS 引擎内部限温逻辑，core_temp_threshold=0 时无效）
const FAS_TEMP_REFRESH: Duration = Duration::from_secs(3);

// [types]
struct FasInstance {
    package: String,
    controller: FasController,
}

pub struct FasManager {
    /// 单实例：同一时刻至多一个白名单应用被接管
    instance: Option<FasInstance>,
    /// 延迟退出的截止时刻：失去白名单前台后 Some(截止)；在前台/未接管为 None
    exit_deadline: Option<Instant>,
    /// 延迟时长（activate 时从应用配置刷新；normalize 已夹在 1..=600s）
    exit_delay: Duration,
    /// performance 调速器接管（activate 时切，deactivate 时按快照恢复）
    governor: GovernorGuard,
    last_temp: f64,
    last_temp_read: Instant,
    /// FAS 温度源节点路径。原始读数刻度因内核而异，除数不在此固化——
    /// 每次刷新经 `utils::battery_temp_divisor()` 取全局预识别结论（CLG 热保护
    /// 同源），避免两处口径漂移；None = 无温度源（引擎侧护栏失效，不影响其余功能）
    temp_path: Option<PathBuf>,
    /// FAS 前台激活共享标志（monitor 层 fps_monitor 消费）：activate 置位、
    /// deactivate 清零——fps_monitor 据此推迟/摘除 eBPF uprobe（反偷跑门控）
    fas_active_flag: Arc<AtomicBool>,
}

impl FasManager {
    /// temp_path：FAS 专用温度源节点。温度看电池不看处理器——电池温度是
    /// 热安全边界（阈值按 ℃ 配置），处理器长期 95℃ 属正常工作区，不作为
    /// 降频依据。None = 无温度源（内部限温关闭，不影响其余功能）。
    /// fas_active_flag 由 main.rs 创建、monitor 与 chiri 两层共享。
    pub fn new(temp_path: Option<PathBuf>, fas_active_flag: Arc<AtomicBool>) -> Self {
        Self {
            instance: None,
            exit_deadline: None,
            exit_delay: Duration::from_secs(15),
            governor: GovernorGuard::new(),
            last_temp: 0.0,
            last_temp_read: Instant::now(),
            temp_path,
            fas_active_flag,
        }
    }

    /// C1：激活。返回 false = 白名单/配置不可用或 load_policies 后无可用 policy
    /// （调用方走冷却回退）。
    ///
    /// - 已是同一包 → 仅刷新 set_game（重复 activate 不重复打点）；
    /// - 另一白名单包 → fas→fas 热切换（scheduler-fas-switch 打点）；
    /// - 首次 → FasController::new + load_policies，policies 为空返回 false。
    ///
    /// 激活同时接管 governor（performance）。延迟退出请求在此被取消（无缝续期）。
    // [activate]
    pub fn activate(&mut self, pkg: &str, pid: i32) -> bool {
        // 白名单复查：包名 → 白名单配置名 → FAS 规则（'static，normalize 已在缓存时完成）
        let Some(rules) = crate::common::fas_whitelist_entry(pkg)
            .and_then(|cfg| crate::common::fas_app_config(cfg))
        else {
            return false;
        };
        self.exit_delay = Duration::from_secs(u64::from(rules.deactivate_delay_secs.max(1)));

        // 单实例不变量：另一包仍活跃，先按 C2 去激活（防御调用方未显式调用）
        let switch_from = self
            .instance
            .as_ref()
            .map(|i| i.package.clone())
            .filter(|a| a != pkg);
        if switch_from.is_some() {
            self.deactivate_active();
        }

        if let Some(inst) = self.instance.as_mut() {
            // 同包续期：刷新 set_game 并取消延迟退出
            inst.controller.set_game(pid, pkg);
            self.exit_deadline = None;
            self.fas_active_flag.store(true, Ordering::Release);
            return true;
        }

        let mut controller = FasController::new();
        controller.load_policies(rules);
        if controller.policies.is_empty() {
            return false;
        }
        // governor 先切 performance：本层快照在写入前完成，与引擎的频率快照互不干扰
        self.governor.activate();
        controller.set_game(pid, pkg);
        controller.set_temperature(self.last_temp);
        controller.set_temp_threshold(rules.core_temp_threshold);
        self.instance = Some(FasInstance { package: pkg.to_string(), controller });
        self.exit_deadline = None;
        self.fas_active_flag.store(true, Ordering::Release);
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
        true
    }

    /// C2：立即去激活（reset_all_freqs + clear_game + governor 按快照恢复），并清零
    /// fas_active 共享标志（fps_monitor 摘除 uprobe 回到零开销待机）。
    /// 无活跃实例时为 no-op。延迟退出请求一并取消。
    /// info 打点 scheduler-fas-deactivate（pkg）+ devimp event("fas", pkg, "deactivate")。
    // [deactivate]
    pub fn deactivate_active(&mut self) {
        let Some(mut inst) = self.instance.take() else {
            return;
        };
        self.exit_deadline = None;
        self.fas_active_flag.store(false, Ordering::Release);
        // 先恢复频率再清状态：调用方随后 init 其他 governor（CLG/akmode/fast）时，
        // 对方才能快照到真实的系统状态；governor 快照与频率互不依赖，最后恢复
        inst.controller.reset_all_freqs();
        inst.controller.clear_game();
        self.governor.release();
        let pkg = inst.package;
        info!(
            "{}",
            t_with_args(
                "scheduler-fas-deactivate",
                &fluent_args!("pkg" => pkg.as_str())
            )
        );
        crate::logger::devimp_event("fas", &pkg, "deactivate");
    }

    /// C5：收尾/失败路径（panic 自愈、DOWN、fas_enabled=false 热重载）——立即全退。
    pub fn deactivate_all(&mut self) {
        self.deactivate_active();
    }

    pub fn is_active(&self) -> bool {
        self.instance.is_some()
    }

    pub fn active_pkg(&self) -> Option<&str> {
        self.instance.as_ref().map(|i| i.package.as_str())
    }

    /// 是否存在 FAS 接管（含延迟退出期）。scenemode 入口门控依据：接管期间
    /// scenemode 禁止进入；延迟到期退出后自动放行。
    pub fn has_any_instance(&self) -> bool {
        self.instance.is_some()
    }

    // [delayed_exit]
    /// 失去白名单前台：进入延迟退出期。FAS 仍持有接管（mode 保持 fas），
    /// 期间 activate（切回白名单）会取消延迟。未活跃时无操作。
    pub fn request_delayed_exit(&mut self) {
        if self.instance.is_some() {
            self.exit_deadline = Some(Instant::now() + self.exit_delay);
        }
    }

    /// 同包回到前台：取消延迟退出。延迟期内切回**同一个**白名单应用不走 activate
    /// （PackageSwitch 同包去重 / 1s 巡检同包 no-op），必须由调用方显式续期，
    /// 否则到期会把正在前台的游戏拆掉重建（局内卡顿）。
    pub fn renew_if_same_pkg(&mut self, pkg: &str) {
        if self.exit_deadline.is_some()
            && self.instance.as_ref().is_some_and(|i| i.package == pkg)
        {
            self.exit_deadline = None;
        }
    }

    /// 1s 周期调用：延迟退出到期则完成退出（恢复频率 + governor 快照）。
    /// 返回 true = 刚完成退出，调用方需按延迟期记住的目标模式重新接管。
    pub fn tick(&mut self) -> bool {
        match self.exit_deadline {
            Some(deadline) if Instant::now() >= deadline => {
                self.deactivate_active();
                true
            }
            _ => false,
        }
    }

    /// 帧事件（仅活跃实例）。内部每 FAS_TEMP_REFRESH 读一次 temp_path
    /// （按全局预识别刻度换算后存 last_temp 并 set_temperature）。
    // [events]
    pub fn on_frame(&mut self, delta_ns: u64) {
        if !self.is_active() {
            return;
        }
        self.refresh_temperature();
        if let Some(inst) = self.instance.as_mut() {
            inst.controller.update_frame(delta_ns);
        }
    }

    /// 负载事件（仅活跃实例）update_cpu_util(fg_util) + update_core_utils(core_utils)。
    pub fn on_load_update(&mut self, fg_util: f32, core_utils: &[f32]) {
        if let Some(inst) = self.instance.as_mut() {
            inst.controller.update_cpu_util(fg_util);
            inst.controller.update_core_utils(core_utils);
        }
    }

    /// 当前活跃实例的帧率（fps）：FAS 未启动或窗口尚无样本时返回 None——
    /// 调用方据此在 status.csv 的 fps 列写 "-"。只读快照，不推进引擎状态。
    pub fn current_fps(&self) -> Option<f32> {
        self.instance.as_ref()?.controller.current_fps()
    }

    // [helpers]
    /// 每 FAS_TEMP_REFRESH 读一次温度源（按全局预识别的刻度换算为 ℃），
    /// 缓存 last_temp 并喂给引擎内部限温逻辑（core_temp_threshold=0 时无效，此处照常喂）。
    fn refresh_temperature(&mut self) {
        let Some(path) = self.temp_path.as_ref() else {
            return;
        };
        if self.last_temp_read.elapsed() < FAS_TEMP_REFRESH {
            return;
        }
        self.last_temp_read = Instant::now();
        let Ok(raw) = crate::utils::read_f64_from_file(&path.to_string_lossy()) else {
            return;
        };
        // 刻度取全局预识别结论（与 CLG 热保护同源）；无法确定时按内核标准 0.1°C 兜底
        let divisor = crate::utils::battery_temp_divisor().unwrap_or(10.0);
        let temp = raw / divisor;
        self.last_temp = temp;
        if let Some(inst) = self.instance.as_mut() {
            inst.controller.set_temperature(temp);
        }
    }
}
