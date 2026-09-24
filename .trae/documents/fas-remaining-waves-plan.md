# FAS 恢复实施计划（剩余波次：波 2-4，全并行执行）

## 执行策略（全并行，用户要求）

| 并行任务 | 文件范围（互不重叠） | 依赖 |
|---|---|---|
| 子代理 E：monitor 层恢复 | src/monitor/{cpu_monitor,fps_monitor,mod,app_detect}.rs | common.rs API（波 1 已落地） |
| 子代理 F：FasManager 新文件 | src/chiri/fas_manager.rs（新建）+ 必要时 policy_mgmt.rs `apply_freqs` 改 pub | common.rs API（已落地）+ 引擎 pub API（已核实） |
| 子代理 G：chiri/mod.rs 接线 | src/chiri/mod.rs（单文件） | FasManager **API 契约**（本计划「波 2F」已完全定型，G 按契约编码）+ common.rs API |

三路同时启动。波 4 主线程统一 `cargo check` + `npm run type-check`，错误/告警按文件归属并行分派修复子代理，主线程复核后重复检查，**反复直至走查清单全过**。最后由子代理更新 AGENTS.md（需波 4 全过后按最终实现描述撰写）。

## Summary

FAS 恢复的总体计划已获批（见 `fas-decoupled-plan.md`），波 1 已全部落地并核实。本计划覆盖**剩余实施工作**：波 2（monitor 层恢复 + FasManager 新文件）、波 3（chiri/mod.rs 接线）、波 4（编译检查 + 逐条逻辑走查修复）与 AGENTS.md 维护更新。

## Current State Analysis（波 1 已核实完成）

| 波次 | 内容 | 状态 |
|---|---|---|
| 1A | `module/config/normal/fas.yaml`（白名单）+ `module/config/normal/fas/endfield.yaml`（每应用配置）+ zh/en.ftl 各 12 个新 key | ✅ 已核实存在 |
| 1B | `src/fas_types.rs` doze_perf 字段（L241-242/L362/L451-454/L539）+ `src/common.rs`：`PackageSwitch` 变体、`fas_whitelist/fas_whitelist_entry/fas_app_config/fas_available/is_fas_mode/embedded_fas_app_str`（include_str! 两个新 yaml）、枚举级 `#[allow(dead_code)]` 移除 | ✅ 已核实 |
| 1C | WebUI 4 文件（bridge/mock/store/AppRulesView）+ `HomeView.vue` L33-34 fas 分支 | ✅ grep 核实已改（子代理结果丢失但改动在盘） |
| 1D | `src/scheduler/mod.rs` L36 `pub mod fas;` + PackageSwitch 空 arm；`src/main.rs` L102-115 `fas_whitelist.txt` 导出（chiri_active 门控内，try_write_file 返回 Result 用 is_ok） | ✅ 已核实 |

**当前编译断点**（波 3 完成前必然报错，属预期）：
1. `src/chiri/mod.rs` 的 DaemonEvent match 穷尽无通配 → `PackageSwitch` 变体使其非穷尽（编译错误），波 3 补 arm。
2. `include_str!` 目标文件波 1A 已创建，此项已解除。

## Proposed Changes（剩余）

### 波 2E：monitor 层恢复（子代理执行）

1. **`src/monitor/cpu_monitor.rs`**：
   - L39 `const FAS_FG_UTIL_ENABLED: bool = false` → 改为 `static FAS_FG_UTIL_ENABLED: AtomicBool = AtomicBool::new(false)`；在 `start_cpu_loop` 入口处：`if crate::common::is_chiri_soc() && crate::common::fas_available() { FAS_FG_UTIL_ENABLED.store(true, Ordering::Relaxed); }`（Yumi 设备保持 false，运行时行为零变化）；所有使用点改 `.load(Ordering::Relaxed)`（先 grep 全部使用点）。
   - 移除 L54-55 / L472-473 / L548-549 三处 `#[allow(dead_code)]`（get_thread_tids/compute_tgid_util/compute_thread_level_util，恢复 FAS 时启用）。
2. **`src/monitor/fps_monitor.rs`**：
   - L18-20 整模块 `#![allow(dead_code)]` 移除（全部条目恢复被使用）。
   - `start_fps_loop` 签名追加 `mut rx_pid: tokio::sync::watch::Receiver<u32>`；删除内部重复 watch channel（约 L245）与 500ms PID 轮询 task（约 L249-269）；替换为：入口先用 `*rx_pid.borrow()` 对初始 pid `switch_pid`，再 `while rx_pid.changed().await.is_ok() { let pid = *rx_pid.borrow(); if pid > 0 { switch_pid } }`（monitor/mod.rs L129 注释要求的共享广播源）。
3. **`src/monitor/mod.rs`**：
   - 取消 L156-174 fps_monitor 启动注释块（注释改写为准确表述），置于 cpu_monitor 启动**之前**（L185-188 move `rx_pid_cpu` 之前先 `let rx_pid_fps = rx_pid_cpu.clone();`）；启动门控 `if crate::common::is_chiri_soc() && crate::common::fas_available() { ... }`；调用 `fps_monitor::start_fps_loop(tx_fps, rx_pid_fps).await`。
4. **`src/monitor/app_detect.rs`**：
   - `determine_mode`（L195-284）：在特调判定之前插入 FAS 判定（`chiri && fas_available()` 门控）：`fas_whitelist_entry(pkg)` 且 `fas_app_config(配置名).is_some()` → debug `app-detect-fas-fallback`（fluent pkg 参数）→ 返回 `"fas".to_string()`。
   - L224 app_modes 特调非法判定改为 `if crate::common::is_special_mode(mode) || crate::common::is_fas_mode(mode)`，并在分支体开头加 fas 专用路径：`if crate::common::is_fas_mode(mode) { warn!(app-detect-fas-rejected, pkg, mode); return config.global_mode.clone(); }`（保持既有 special 逻辑原样跟随）。
   - L271 global_mode 检查同理：fas 全局映射 → warn `app-detect-fas-global-rejected`（pkg, mode）→ 返回 `"balance"`。
   - 主循环（约 L481-503）事件发射三分支：局部新增 `last_package: String` 缓存——模式变化 → `ModeChange`（现状，同步刷新 last_package）；模式不变且 `is_chiri_soc()` 且 last_package 非空且包名变化 → 发 `PackageSwitch { package_name, pid }`（pid 用 `get_current_pid()`）；否则只刷新缓存。Yumi 设备不发 PackageSwitch（共享层行为零变化）。

### 波 2F：新建 `src/chiri/fas_manager.rs`（主线程直接写，保证接口精确）

按已批计划「核心设计」实现，要点：

- `FasInstance { package, controller: FasController, rules: &'static FasRulesConfig, last_fg: Instant }`；`FasManager { instances: Vec<FasInstance>, active_pkg: Option<String>, doze: bool, last_temp: f64, last_temp_read: Instant, temp_path: Option<PathBuf> }`。
- `const FAS_INSTANCE_TTL: Duration = 60s`。
- API（全 pub）：
  - `new(temp_path: Option<PathBuf>)`
  - `activate(&mut self, pkg: &str, pid: i32) -> bool`：白名单复查（`fas_whitelist_entry` → `fas_app_config`）；已是活跃实例 → 仅刷 last_fg + set_game；有实例 → 复用（`reset_runtime()` + `apply_freqs()` 重写频率，不用 force_reapply——doze 后 current_freq 陈旧）；无实例 → `FasController::new()` + `load_policies(rules)`，policies 为空返回 false（C5，调用方冷却回退）；随后 `set_game` + `set_temperature(last_temp)` + `set_temp_threshold(rules.core_temp_threshold)`；info `scheduler-fas-activate` + devimp event。
  - `deactivate_active(&mut self)`：C2——仅对**活跃**实例 `reset_all_freqs()` + `clear_game()`（非活跃实例频率已恢复，重置会踩坏后续 governor 状态）；doze=false；info `scheduler-fas-deactivate` + devimp。
  - `deactivate_all(&mut self)`：`deactivate_active()` 后 `instances.clear()`（收尾/失败路径）。
  - `enter_doze/exit_doze/doze()`：C4——enter：活跃实例逐 policy `find_nearest_freq(rules.doze_perf)` + `apply_freq_locked`；exit：活跃实例 `apply_freqs()`（注意：**force_reapply 会重写 doze 后的陈旧 current_freq，必须用 apply_freqs**；若 `apply_freqs` 非 pub 则在 `src/scheduler/fas/policy_mgmt.rs` 将其改为 `pub fn`——引擎唯一可见性改动）；info `scheduler-fas-doze-enable/restore`。
  - `on_frame(&mut self, delta_ns: u64)`：仅活跃实例；内部 3s 温度刷新（`temp_path` 读数 /1000.0 存 last_temp 并 `set_temperature`）+ `update_frame`。
  - `on_load_update(&mut self, fg_util: f32, core_utils: &[f32])`：仅活跃实例 `update_cpu_util` + `update_core_utils`。
  - `reap(&mut self)`：C3——`retain(|i| Some(&i.package) == active.as_ref() || i.last_fg.elapsed() < FAS_INSTANCE_TTL)`；被移除实例打 info `scheduler-fas-destroy` + devimp（先收集被删包名再 retain）。
  - `is_active() / active_pkg() -> Option<&str>`。
- devimp：`logger::devimp_event`（先读 `src/logger.rs` 确认签名，参照 chiri/mod.rs L1017 `devimp_event("screen", "-", "off")` 用法），kind="fas"，action=activate/switch/deactivate/destroy/doze。
- 引擎 API 依赖（已核实）：`FasController::new()/load_policies(&FasRulesConfig)/set_game(i32,&str)/clear_game()/set_temperature(f64)/set_temp_threshold(f64)/update_cpu_util(f32)/update_core_utils(&[f32])/update_frame(u64)/reset_runtime()/reset_all_freqs()`，`policies: Vec<PolicyController>` pub；`PolicyController::find_nearest_freq(f32)->u32 / apply_freq_locked(u32) / force_reapply()`。

### 波 3：`src/chiri/mod.rs` 接线（主线程，16 处）

1. 模块区（L194-204）：加 `pub mod fas_manager;`；L196-198 旧注释改写为指向说明（引擎 scheduler::fas，管理器 fas_manager）。
2. L465 区域：新增 `fas_temp_path` 探测（`crate::utils::find_cpu_temp_path().ok()`，独立于 L512 thermal 的 `temp_sensor_path`）+ `let mut fas_mgr = fas_manager::FasManager::new(fas_temp_path);`。
3. L486-493 旧挂起变量注释块**删除**；L501-502 附近加 `const FAS_COOLDOWN: Duration = Duration::from_secs(300); let mut fas_cooldown_until: Option<Instant> = None;`。
4. ModeChange 进 fas（L1143-1159 整块重写）：白名单防御复查（失败 → warn `scheduler-fas-init-failed` + CLG balance init 回退）→ 冷却检查（`scheduler-fas-cooldown` warn + CLG balance 回退）→ `ak_governor.release()` + `fast_lock.release()` + `cpu_governor.release()` → `fas_mgr.activate(&package_name, pid)`（失败 → `deactivate_all()` + 冷却 + CLG balance 回退）→ `fas_affinity_hook(true, pid)` → `if !is_screen_on { fas_mgr.enter_doze(); }`（息屏期间进 fas 的竞态保险）。
5. ModeChange 退 fas（else 分支，替换 L1162-1177 挂起注释块）：`if fas_mgr.is_active() { fas_mgr.deactivate_active(); fas_affinity_hook(false, 0); }` 放在 `if is_screen_on` **之前**；后接既有特调/fast/CLG 分支不变。
6. ModeChange 新增 `PackageSwitch` arm（match 补全，否则编译失败）：`current_mode == "fas" && fas_mgr.is_active()` 且新包 ≠ active_pkg 且白名单命中 → `deactivate_active()` → `activate(新, pid)` → `if doze { enter_doze() }` → `fas_affinity_hook(true, pid)`；activate 失败同冷却回退。
7. ScreenStateChange 息屏（L970-984）：特调保持分支后插入 `} else if current_mode == "fas" && fas_mgr.is_active() { fas_mgr.enter_doze(); }`（C4，不 release）；后续 `apply_affinity_and_corectl` 调用不变。
8. ScreenStateChange 亮屏（L1066-1070 fas 空壳分支）：`if fas_mgr.is_active() { fas_mgr.exit_doze(); } else { /* FAS 未激活（冷却回退）时 CLG 从 doze 恢复：`get_clg_cfg("balance")` reload/init（镜像同分支其他模式的恢复写法） */ }`。
9. SystemLoadUpdate（L1234-1262）：负载投喂优先级链最前插 `fas_mgr.is_active()` → `fas_mgr.on_load_update(foreground_max_util, &core_utils)`；解构处 `foreground_max_util: _` 改消费。
10. scenemode 进入判定（L1276）：追加 `&& current_mode != "fas"`。
11. FrameUpdate（L1382-1399 重写）：`if is_screen_on && fas_mgr.is_active() { fas_mgr.on_frame(frame_delta_ns); }`；既有 debug 周期打点改 `log::log_enabled!` 门控（AGENTS 低开销原则 ④——帧事件恢复后逐帧 format 生效）。
12. 热保护块（L780-868）：块首 `current_mode == "fas"` 时整体跳过（温控移除）。
13. 1s 遥测块（L685-778）：追加 `fas_mgr.reap()`；PackageSwitch 兜底——`current_mode == "fas"` 时：活跃且 `get_current_package() != active_pkg` 且白名单命中 → 同 #6 热切换；**不活跃**且冷却过期且前台白名单命中 → 先 `ak/fast/cpu_governor.release()` 再 `activate`（启动即 fas / 事件丢失自愈）；不活跃且前台非白名单且三 governor 均 `!is_active()` → CLG balance init（启动残留 fas 模式自愈）。
14. config_dirty（L617-664）与 ConfigReload（L1402-1479）「其他模式」分支补 `&& current_mode != "fas"` 守卫；ConfigReload 内旧 fas_rules 重载注释块（L1411-1418）删除（配置编译期嵌入静态）。
15. panic/线程收尾（L1501-1515）：`fast_lock.release()` 之后、corectl 之前插入 `fas_mgr.deactivate_all();`。
16. 删除其余过时注释：L528-530（FAS 温度变量）、L1227-1230（同模式温度刷新——由 FasManager 内部 3s 刷新取代）、L1382-1394 旧注释内容、L1489-1498（挂起超时检查块，被 reap 取代）。

`fas_affinity_hook`（预留接口，主线程加在 mod.rs 辅助函数区）：

```rust
/// FAS 线程亲和预留接入点（当前无操作）。
/// 预留：FAS 模式下前台关键线程钉核/大核亲和扩展时在此实现；
/// 激活(true)/去激活(false)时由事件循环调用，后续扩展无需再改接线。
fn fas_affinity_hook(
    affinity_mgr: &mut affinity::AffinityManager,
    corectl_mgr: &mut core_ctl::CoreCtlManager,
    active: bool,
    fg_pid: i32,
) {
    let _ = (affinity_mgr, corectl_mgr, active, fg_pid);
}
```

### 波 4：验证与反复走查（用户要求：反复审查、逐个逻辑检测）

1. `cargo +nightly check -p yumi --target aarch64-linux-android`（Windows 走 ebpf stub 回退）；新告警逐个处理（重点：移除 allow 后的 dead_code、非穷尽 match）。
2. `cd webui && npm run type-check`（若无 node_modules 先 `npm install`）。
3. 走查清单（逐条核对，发现问题修复后**重复运行检查直至全过**）：
   - determine_mode 优先级：FAS 白名单 > 特调 > app_modes（fas 非法映射拒绝）> global_mode；
   - 生命周期：进 fas 前 governor release；退出先 deactivate 再下一 governor init（快照无污染）；init 失败冷却 + 回退；收尾 deactivate_all；60s 注销仅内存清理；60s 内切回复用；
   - fas→fas：PackageSwitch 即时热切换 + 1s 兜底 + 息屏中切换后 doze 锁恢复；
   - 事件路由：帧/负载/温度只喂活跃实例；reap 不触碰活跃实例；非活跃实例去激活后零频率写入；
   - doze：enter/exit 配对、亮屏恢复、scenemode/config_dirty/ConfigReload/热保护/看门狗守卫与豁免；
   - 日志：生命周期打点齐全、无逐帧新日志、debug 门控；
   - Yumi 零变化：determine 门控 + fps_monitor 启动门控 + FAS_FG_UTIL_RUNTIME false + PackageSwitch 不发射/空 arm；
   - WebUI：标签、禁切换、Home 显示、MockBridge 同步。

### 收尾：AGENTS.md 维护更新

- 目录结构（module/config/normal/fas.yaml、fas/）、配置章节（fas 嵌入 + fas_whitelist.txt 导出 + 不可编辑约定）、ChiRi 子系统新增「FAS 帧感知调度」小节（多实例 C1-C6、60s TTL、doze 保持接管、PackageSwitch、亲和预留接口）、monitor（FAS_FG_UTIL_ENABLED 运行时原子量、fps_monitor 共享 PID watch）、WebUI（FAS 标签）、事件循环低开销原则（FrameUpdate 门控示例）、删除/改写过时的「FAS 暂禁用」表述（含「恢复 FAS 时启用」标注）。

## Assumptions & Decisions

- 「FAS 进程」= 进程内逻辑实例（FasInstance），非 OS 子进程；创建/注销 = 实例增删，频率随去激活即时恢复。
- 引擎唯一改动 = `apply_freqs` 可见性（如非 pub）；算法零修改。
- `FAS_FG_UTIL_ENABLED` 改运行时 AtomicBool（is_chiri_soc && fas_available 置位），保证 Yumi 行为零变化。
- doze 恢复用 `apply_freqs()`（PID 状态按 perf_index 重算），不用 force_reapply。
- i18n/common.rs warn 风格沿用波 1 已落地实现（解析失败为字面量 warn，不走 i18n）。

## Verification

见波 4 清单：两条编译/类型检查命令 + 8 组逻辑走查，反复执行直至全过；完成后更新 AGENTS.md 并向用户交付总结。
