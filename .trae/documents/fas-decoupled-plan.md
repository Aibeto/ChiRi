# FAS 帧感知调度恢复计划（解耦多实例架构，ChiRi 专属）

## Summary

恢复禁用的 FAS 功能，重构为**解耦多实例架构**（用户明确要求）：

- **FasManager + FasInstance**：每个 FAS 应用对应一个独立实例（逻辑 FAS 进程），各自持有独立的 `FasController`、专属 `FasRulesConfig`、生命周期状态。
- **创建/注销时机**：FAS 应用进入前台（启动）时立即创建对应实例；应用丢失前台后实例保留 **60s**（支持快速切回），超过 60s 未回归前台则注销（释放内存；频率资源在「失去激活」时已恢复）。
- **多 FAS 应用快速切换**：fas→fas 切换经 `PackageSwitch` 新事件即时热替换（旧实例去激活 → 新实例激活/复用），另有 1s 遥测兜底。
- **确定性释放时机**（用户强调 release 时机）：去激活（频率恢复 + 停止驱动）在任何其他 governor init **之前**完成；注销（内存清理）只是 60s TTL 的簿记，不涉及频率。
- 白名单 `module/config/normal/fas.yaml`（编译期嵌入，运行时导出 `fas_whitelist.txt` 供 WebUI 只读）；每应用配置 `module/config/normal/fas/<配置名>.yaml`（嵌入、不导出、不可编辑），首条 `com.hypergryph.endfield → endfield`。
- FAS 模式下 CLG/akmode/fast 全部暂停，温控/触摸/scenemode/doze-doze 豁免；息屏 FAS **保持接管**（写入 doze 低频锁，亮屏恢复）；扩展范围仅频率维度（线程亲和预留 hook 不实现）。
- WebUI：应用列表显示 FAS 标签 + 禁止该应用切换模式；Home 页正确显示 FAS 模式。
- **日志**：生命周期事件（创建/激活/切换/去激活/注销/doze）打 devimp event 行 + info/debug 打点，遵守事件循环低开销原则（debug 门控、不打逐帧日志）。
- **执行策略**：并行子代理分块实现 + 完成后反复审查逐条逻辑走查（用户要求）。

## Current State Analysis

- FAS 引擎完整保留于 `src/scheduler/fas/`（9 文件），禁用方式 = 模块声明注释 + 接线注释 + `#[allow(dead_code)]`；算法不重写，仅重新接线。
  - `FasController`（controller.rs L36-118）：字段 `pub(super)`，公开 API：`new()/load_policies(&FasRulesConfig)/reload_rules()/set_game(pid,pkg)/clear_game()/set_temperature()/set_temp_threshold()/update_cpu_util()/update_core_utils()/update_frame(u64)/reset_runtime()/reset_all_freqs()`；`policies: Vec<PolicyController>` 为 `pub`。
  - `PolicyController`（policy_controller.rs L27-68）：`pub` 字段 + `pub find_nearest_freq(ratio)/apply_freq_locked(freq)/force_reapply()/current_ratio()`。
  - `FasRulesConfig`（src/fas_types.rs L104-173）：全字段 serde 默认 + `normalize()` + `migrate_legacy_margins()`；`per_app_profiles` 支持 target_fps/fps_margin。
- 白名单先例：`src/chiri/special_tuned.txt` → common.rs include_str! + OnceLock（L280-365）→ main.rs 导出（L77-101）→ WebUI `getSpecialTuned`。
- 互斥先例：akmode 三入口接管/释放 + 失败 300s 冷却（chiri/mod.rs L1186-1206）。
- 原挂起机制（5s grace + `fas_suspended_at`）**废弃**：存在快照污染缺陷（CLG init 会把 FAS 锁频快照为原始状态），且被多实例架构取代；相关注释块（chiri/mod.rs L486-493、L1162-1177、L1489-1498）不再恢复。
- WebUI 标签先例：specialTuned 链路（bridge.ts L226-242 → scheduler.ts L12 → AppRulesView.vue L146-149）。

## 核心设计：FasManager 多实例生命周期

### 数据结构（新文件 `src/chiri/fas_manager.rs`）

```rust
pub struct FasInstance {
    pub package: String,
    pub controller: FasController,          // 引擎实例（含私有 policies/FastWriter）
    pub rules: &'static FasRulesConfig,     // 该应用专属嵌入配置
    pub last_fg: Instant,                   // 最近一次前台时刻（TTL 计时基准）
}

pub struct FasManager {
    instances: Vec<FasInstance>,            // 通常 0~2 个
    active_pkg: Option<String>,             // 当前驱动调度的实例（= 前台 FAS 应用）
    doze: bool,
    doze_perf: f32,
    last_temp: f64,                         // 3s 刷新的 CPU 温度缓存，激活时喂给新实例
    temp_path: Option<PathBuf>,             // FAS 独立温度源（与 thermal 探测变量分开）
}
```

### 生命周期规则（总纲，所有接线遵循）

| 规则 | 内容 |
|---|---|
| C1 创建 | FAS 应用进入前台（ModeChange→fas / PackageSwitch→fas）时：无实例则**立即创建**（`FasController::new()` + `load_policies(该应用 rules)` + `set_game`）；有实例（60s 内切回）则复用（`force_reapply` 全部 policies + `reset_runtime()` 清陈旧帧态） |
| C2 去激活 | 实例失去激活（fas→其他模式 / fas→fas 换应用）时：`reset_all_freqs()` + `clear_game()`，**必须先于下一个 governor 的 init**（保证 CLG/akmode init 快照到真实系统状态，杜绝快照污染）；实例保留等待 TTL 复用 |
| C3 注销 | `last_fg.elapsed() >= 60s`（`FAS_INSTANCE_TTL`）→ 从 `instances` 移除；此时频率已在 C2 恢复，注销纯内存清理。active 实例必然前台（mode=fas 由前台包名决定），恒满足 `last_fg` 新鲜 |
| C4 息屏保持 | 息屏**不去激活**（唯一例外）：active 实例 policies 写入 doze 低频锁（`find_nearest_freq(doze_perf)` + `apply_freq_locked`）；亮屏 `force_reapply` 恢复；FrameUpdate 漏帧时先退 doze 自愈 |
| C5 失败/收尾 | `load_policies` 后 policies 为空 → 去激活 + 300s 冷却 + CLG balance 回退；panic/线程收尾 → 去激活全部 active（恢复频率） |
| C6 事件路由 | FrameUpdate/SystemLoadUpdate/温度刷新只喂 active 实例；负载看门狗不触碰 FAS（帧驱动不依赖负载流） |

### 过渡矩阵

- fas(X)→fas(Y)：`deactivate(X)` → `activate(Y)`（C1 复用或创建）——经 PackageSwitch 即时触发，1s 遥测兜底。
- fas(X)→非 fas：`deactivate(X)` → `ak/fast/CLG init`（顺序保证）。
- 非 fas→fas(X)：`ak/fast/CLG release` → `activate(X)`。
- fas(X) 前台 → 息屏：doze 锁（C4）；→ 亮屏：`force_reapply`。
- 60s 内切回：C1 复用；>60s：实例已注销，走全新创建。

## Proposed Changes

### 1. 新增配置文件（编译期嵌入）

**`module/config/normal/fas.yaml`**（白名单）：

```yaml
# FAS（帧感知调度）白名单 — 编译期嵌入二进制，用户/WebUI 不可修改。
# 运行时导出到模块根 fas_whitelist.txt 供 WebUI 只读展示。
# 每项：<精确包名>: <fas 配置名>（对应 module/config/normal/fas/<配置名>.yaml）
# 新增 FAS 游戏：本表加一行 + common.rs::embedded_fas_app_str 加一个 arm + 新建配置文件
fas:
  apps:
    com.hypergryph.endfield: endfield
```

**`module/config/normal/fas/endfield.yaml`**（endfield 专属，嵌入不导出）：

```yaml
# FAS 配置：com.hypergryph.endfield（明日方舟：终末地）
# 编译期嵌入，不可编辑，不对外暴露。未列出字段取 FasRulesConfig 默认值。
fas_rules:
  fps_gears: [30.0, 60.0, 90.0, 120.0, 144.0]
  fps_margin: 3.0
  per_app_profiles:
    com.hypergryph.endfield:
      target_fps: [30, 60, 120]
      fps_margin: 3.0
  core_temp_threshold: 0    # FAS 内部限温关闭（一切交由帧感知调度）
  doze_perf: 0.18           # 息屏 FAS doze 锁频比例（新增字段）
```

### 2. src/fas_types.rs — 新增 doze 字段

- `FasRulesConfig` 追加 `#[serde(default = "d_doze_perf")] pub doze_perf: f32`（默认 0.18）；`normalize()` 追加 `clamp(0.05, 0.30)`；`Default impl` 同步。serde default 保证旧 rules.yaml 格式兼容。

### 3. src/common.rs — 白名单解析 + 新事件变体

- 嵌入声明（L446-472 嵌入区）：`FAS_WHITELIST_TEXT = include_str!("../module/config/normal/fas.yaml")`、`FAS_APP_ENDFIELD_TEXT = include_str!("../module/config/normal/fas/endfield.yaml")`；`pub fn embedded_fas_app_str(name: &str) -> Option<&'static str>`（match `"endfield"`，未来游戏在此加 arm）。
- `static FAS_WHITELIST: OnceLock<HashMap<String, String>>`（pkg→配置名）：
  - `fas_whitelist() -> &'static HashMap<String, String>`（serde 解析 `fas.apps` 段，失败 warn `fas-config-parse-failed` 返回空表）
  - `fas_whitelist_entry(pkg) -> Option<&'static String>`
- `static FAS_APP_CONFIGS: OnceLock<HashMap<String, FasRulesConfig>>`：
  - `fas_app_config(name) -> Option<&'static FasRulesConfig>`（解析 `fas_rules` 段 + `normalize()` + `migrate_legacy_margins()`；单文件失败 warn 跳过）
  - `fas_available() -> bool`（白名单非空且至少一个配置解析成功；OnceLock 缓存）
- `pub fn is_fas_mode(mode: &str) -> bool { mode == "fas" }`
- **`DaemonEvent`（L42-81）新增变体**：`PackageSwitch { package_name: String, pid: i32 }`。枚举级 `#[allow(dead_code)]`（L45）移除（FrameUpdate/pid/foreground_max_util 本次全部恢复消费；若 check 报新告警则按需保留并注明）。

### 4. monitor 层恢复事件生产

- **`src/monitor/cpu_monitor.rs`**：L39 `FAS_FG_UTIL_ENABLED: false → true`（恢复 foreground_max_util 计算）；L54/L472/L548 三处 `#[allow(dead_code)]`（get_thread_tids/compute_tgid_util/compute_thread_level_util）移除，注明恢复启用。
- **`src/monitor/fps_monitor.rs`**：`start_fps_loop` 签名追加 `rx_pid: tokio::sync::watch::Receiver<u32>`；删除内部重复 watch channel（L245）与 500ms PID 轮询 task（L249-269），改 `rx_pid.changed()` 驱动 `switch_pid`（monitor/mod.rs L129 注释要求的共享广播源复用）。
- **`src/monitor/mod.rs`**：取消 L156-174 fps_monitor 启动注释，传 `rx_pid_cpu.clone()`（在 L185-188 cpu_monitor move 原值**之前** clone）；启动门控 `crate::common::is_chiri_soc() && crate::common::fas_available()`（Yumi 设备零开销）。
- **`src/monitor/app_detect.rs`**：
  - `determine_mode`（L195-284）在特调判定**之前**插入 FAS 判定：`chiri && fas_whitelist_entry(pkg).is_some() && fas_app_config(配置名).is_some()` → debug 打点 `app-detect-fas-fallback` → 返回 `"fas"`。
  - app_modes/global_mode 分支的非法特调判定（L224、L271）追加 `|| mode == "fas"`：非白名单 "fas" 映射 warn（`app-detect-fas-rejected` / `app-detect-fas-global-rejected`）回退全局/balance。
  - **主循环（L481-503）事件发射改三分支**：模式变化 → `ModeChange`（现状）；模式不变但前台包变化 → 发 `PackageSwitch { package_name, pid }`（fas→fas 快速切换通路）；均无变化 → 不发。首轮（缓存为空）只更新缓存。

### 5. src/scheduler/mod.rs — 仅模块声明 + 枚举完备性

- L36 取消 `// pub mod fas;` 注释（chiri 经 `crate::scheduler::fas::FasController` 访问引擎）。
- DaemonEvent match 追加 `PackageSwitch` 空 arm（debug 日志，枚举完备性，同 BpfStats 先例）。
- **Yumi 其余 FAS 注释块全部保持注释**（逻辑冻结；determine 门控保证 Yumi 永不进入 fas 模式，运行时行为零变化）。

### 6. src/chiri/fas_manager.rs — 新文件（核心）

按「核心设计」实现 `FasManager`：

- `new(temp_path: Option<PathBuf>) -> Self`
- `activate(&mut self, pkg: &str, pid: i32) -> bool`：C1（创建或复用）+ `set_game(pid,pkg)` + `set_temperature(last_temp)` + `set_temp_threshold(rules.core_temp_threshold)` + `active_pkg=Some(pkg)` + `last_fg=now` + devimp/info 打点。返回 false = load_policies 后 policies 为空（调用方走冷却回退）。
- `deactivate(&mut self, pkg: &str)`：C2（`reset_all_freqs` + `clear_game`，不动实例结构）；`deactivate_all(&mut self)` 用于收尾。
- `enter_doze(&mut self)` / `exit_doze(&mut self)`：C4（active 实例 policies 写 doze 锁 / `force_reapply`）。
- `on_frame(&mut self, delta_ns: u64)`：C6——doze 时先 `exit_doze` 自愈；active 实例 3s 温度刷新 + `update_frame`；非 active 忽略。
- `on_load_update(&mut self, fg_util: f32, core_utils: &[f32])`：C6——只喂 active 实例。
- `reap(&mut self)`：C3——`instances.retain(|i| i.last_fg.elapsed() < FAS_INSTANCE_TTL)`，注销时 devimp/info 打点（低频，1s 周期块内调用）。
- `is_active(&self) -> bool` / `active_pkg(&self) -> Option<&str>`。
- 生命周期日志：create/activate/switch/deactivate/destroy/doze 各状态变化打 devimp event 行（`logger::devimp_event("fas", pkg, action)`，签名按 mod.rs L1017 现有用法对齐）+ info；逐帧逻辑不打新日志（沿用引擎内部 60 帧周期 debug 摘要），遵守 AGENTS 低开销约定。

### 7. src/chiri/mod.rs — 调度接线

1. **L196-198**：注释改写为文档（引擎在 scheduler::fas，ChiRi 侧管理器为 fas_manager）。
2. **L465 区域**：`cpu_governor` 创建后新增 `let mut fas_mgr = FasManager::new(fas_temp_path);`（`fas_temp_path` 独立温度探测，不复用 L512 thermal 的 `temp_sensor_path`，避免语义混淆；无传感器传 None，FAS 温度限温因 core_temp_threshold=0 天然关闭）。
3. **ModeChange 进 fas（L1143-1159 整块重写）**：
   1. 防御复查白名单 `fas_whitelist_entry(&package_name)` + `fas_app_config(配置名)`；未命中（不应发生）→ warn `scheduler-fas-init-failed` → 按 balance 初始化 CLG 回退。
   2. 冷却检查（`FAS_COOLDOWN=300s`，镜像 AKMODE_COOLDOWN）未过期 → CLG balance 回退。
   3. `ak_governor.release()` + `fast_lock.release()` + `cpu_governor.release()`（三 governor 全停）。
   4. `fas_mgr.activate(&package_name, pid)`（内部 C1）；失败 → `fas_mgr.deactivate_all()` + 置冷却 + CLG balance 回退。
   5. `fas_affinity_hook(&mut affinity_mgr, &mut corectl_mgr, true, pid)`（预留接口，见 #9）。
   6. info `scheduler-fas-activate`。
4. **ModeChange 退 fas（else 分支，替换原挂起注释块 L1162-1177）**：`fas_mgr.deactivate(active_pkg)`（C2，放在 `if is_screen_on` 之前、一切 governor init 之前）→ `fas_affinity_hook(..., false, 0)` → 后接现有特调/fast/CLG 分支不变。
5. **PackageSwitch 新 arm（插在 ModeChange 后）**：`current_mode == "fas"` 时——新包 = active 包则忽略（去重）；否则 `fas_mgr.deactivate(旧)` → 白名单复查 → `fas_mgr.activate(新, pid)` → `fas_affinity_hook(..., true, pid)`；息屏中切换后重新写 doze 锁（`if fas_mgr.doze() { fas_mgr.enter_doze(); }`）。非 fas 模式忽略。
6. **ScreenStateChange 息屏（L970-984）**：特调保持分支后插入 fas 分支——`fas_mgr.enter_doze()`（C4 保持接管，不 release）+ info `scheduler-fas-doze-enable`；后续 `apply_affinity_and_corectl` 调用不变（fas 非 boost）。
7. **ScreenStateChange 亮屏（L1066-1070 fas 空壳分支）**：`fas_mgr.exit_doze()` + info `scheduler-fas-doze-restore`。
8. **SystemLoadUpdate（L1234-1262）**：负载投喂优先级链最前插入：`fas_mgr.is_active()` → `fas_mgr.on_load_update(foreground_max_util, &core_utils)`；解构处 `foreground_max_util: _` 改消费。
9. **scenemode 进入判定（L1276）**：追加 `&& current_mode != "fas"` 豁免；饱和退出逻辑不变（fas 不进 scenemode）。
10. **FrameUpdate（L1382-1394 重写）**：`fas_mgr.is_active()` → `fas_mgr.on_frame(frame_delta_ns)`（内部含 doze 自愈 + 3s 温度刷新）；debug 周期打点保留。
11. **热保护块（L780-868）**：`current_mode == "fas"` 时整体跳过（温控移除 + 省 2s 周期计算）；1s 遥测温度读数保留并刷新 `fas_mgr` 温度缓存。
12. **1s 遥测块（L685-778）**：追加两件事——① `fas_mgr.reap()`（C3 注销超时实例）；② PackageSwitch 兜底：`fas_mgr.is_active()` 且 `get_current_package() != active_pkg` 且新包命中白名单 → 走 #5 同款热切换（事件丢失安全网）。
13. **config_dirty（L617-664）与 ConfigReload（L1402-1459）**：「其他模式」分支补 `current_mode != "fas"` 守卫（FAS 配置编译期嵌入，重载与 FAS 无关；不得在 FAS 激活时动 CLG）；fas 分支无操作。
14. **看门狗 Timeout（L909-940）与 is_boost_mode（L292-294）**：不变（C6；fas 非 boost，无触摸升频）。
15. **panic/线程收尾（L1501-1514）**：`fas_mgr.deactivate_all()`（C5，置于 cpu/ak/fast release 之后、corectl/affinity release 之前）。
16. 原 L486-493 / L1162-1177 / L1489-1498 挂起机制注释块**删除**（被 FasManager 取代），注释指向 fas_manager.rs。

### 8. src/main.rs — 运行时导出

- L77-101 special_tuned 导出块后追加：`chiri_active` 门控内遍历 `fas_whitelist()` 写模块根 `fas_whitelist.txt`（每行 `{pkg}:{cfg}\n`，`utils::try_write_file`），info `main-fas-whitelist-exported`（fluent count 参数）。

### 9. 线程亲和预留接口（chiri/mod.rs）

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

`is_boost_mode` 保持不含 fas（仅频率维度，无 cpuset/core_ctl 变更）。

### 10. i18n（`module/config/i18n/zh.ftl` + `en.ftl` 同步，命名 `模块-描述`）

新增：`main-fas-whitelist-exported`、`fas-config-parse-failed`、`app-detect-fas-fallback`、`app-detect-fas-rejected`、`app-detect-fas-global-rejected`、`scheduler-fas-activate`、`scheduler-fas-switch`、`scheduler-fas-deactivate`、`scheduler-fas-destroy`、`scheduler-fas-doze-enable`、`scheduler-fas-doze-restore`、`scheduler-fas-init-failed`、`scheduler-fas-cooldown`。

### 11. WebUI（FAS 标签 + 禁止切换模式）

- **`webui/src/utils/bridge.ts`**：`PATHS` 加 `FAS_WHITELIST = ${MODULE_BASE_PATH}/fas_whitelist.txt`（L14-23 区域）；新增 `getFasWhitelist(): Promise<Record<string, string>>`（仿 `getSpecialTuned` L226-242：按行 `split(':')` 解析 `pkg:config`，异常返回 `{}`）。
- **`webui/src/utils/mock.ts`**：`mockFasWhitelist = { 'com.hypergryph.endfield': 'endfield' }` + `getFasWhitelist` 方法（MockBridge 联合类型缺方法 type-check 报错，必须同步）。
- **`webui/src/stores/scheduler.ts`**：state 加 `fasWhitelist: Record<string, string>`（仿 L12），`initData` 的 `Promise.all`（L21-27）并行加载。
- **`webui/src/views/AppRulesView.vue`**：① 标签：L146-149 特调 van-tag 旁追加 FAS 标签 `v-if="store.isChiri && store.fasWhitelist[pkg]"`，文本 `t('mode_fas')`，`type="primary"`；② 禁止切换：`openMenu`（L96-99）开头对 FAS 命中应用提前 return，van-cell 加 disabled 样式（参照 L174 `.rescan-btn.disabled { pointer-events: none }` 模式）；③ L154「未配置」占位条件排除 FAS 命中。
- **`webui/src/views/HomeView.vue`**：`currentModeInfo`（L29-43）追加 fas 分支：`currentMode === 'fas'` → 显示 `t('mode_fas')` / `t('desc_fas')`。
- 动作单 `mode_fas` 选项（L28 注释）**保持注释**：FAS 仅白名单驱动，不作为用户可选模式。

### 12. AGENTS.md 维护更新（会话收尾）

- 目录结构（`module/config/normal/fas.yaml`、`fas/`）、配置章节（fas 嵌入 + fas_whitelist.txt 导出）、ChiRi 子系统新增「FAS 帧感知调度」小节（解耦多实例 C1-C6 生命周期、60s TTL、doze 保持接管、PackageSwitch、亲和预留接口）、monitor 条目（FAS_FG_UTIL_ENABLED=true、fps_monitor 恢复）、WebUI（FAS 标签）、事件循环低开销原则（fas 分支纳入）。

## 执行策略（用户要求：子代理并行加速 + 反复审查）

分四波，波内并行：

1. **波 1（4 个并行子代理）**：
   - A：配置文件 + i18n（fas.yaml、endfield.yaml、zh/en ftl）
   - B：fas_types.rs doze 字段 + common.rs 白名单解析/事件变体
   - C：WebUI 全部（bridge/mock/store/views，4 文件）
   - D：scheduler/mod.rs 模块声明 + PackageSwitch 空 arm；main.rs 导出
2. **波 2（2 个并行子代理，依赖波 1 的 common.rs API）**：
   - E：monitor 层（cpu_monitor 常量、fps_monitor 参数化、monitor/mod.rs 启动、app_detect determine_mode + PackageSwitch 发射）
   - F：chiri/fas_manager.rs 新文件（独立可写，接口按本计划 C1-C6）
3. **波 3（主线程，依赖 fas_manager API 定型）**：chiri/mod.rs 16 处接线（最关键文件，主线程亲自改，避免子代理上下文不足出错）。
4. **波 4：反复审查**——`cargo +nightly check` + `npm run type-check` + 按下方走查清单逐条核对，发现问题回到对应波次修复再查，直至清单全过。

## Assumptions & Decisions

- FAS 引擎算法不改；「允许完全打破现有逻辑」落实为：解耦多实例架构、白名单驱动、确定性去激活时机、息屏保持接管、温控/scenemode 豁免、同模式热切换，并**废弃原 5s 挂起宽限机制**。
- 「FAS 进程」为逻辑实例（线程内对象，非 OS 进程）；创建/注销 = 实例增删，频率资源随去激活即时恢复，60s TTL 只影响内存与复用。
- 复用实例时 `reset_runtime()` 清陈旧帧态 + `force_reapply` 重写频率；policies 的 FastWriter 缓存有效无需重建。
- rules.yaml 的 fas_rules 注释模板与 `RulesConfig.fas_rules` 字段不动（Yumi 冻结区）；FAS 配置完全脱离 rules.yaml。
- endfield.yaml 初值为工程合理默认（帧率档位/PID 系数待实机调优）。
- `config-example.yaml` 不新增（fas.yaml 独立嵌入文件，同 akmode.yaml 先例，文件头自带文档）。
- Yumi 行为零变化：determine 门控 + fps_monitor 启动门控 + PackageSwitch 空 arm。

## Verification

1. `cargo +nightly check -p yumi --target aarch64-linux-android`（Windows 本地走 ebpf stub 回退；验证编译与 dead_code 清理后无新告警）。
2. `cd webui && npm run type-check`。
3. **逻辑走查清单（波 4 反复核对，全过才算完成）**：
   - determine_mode 优先级：FAS 白名单 > 特调 > app_modes（fas 非法映射拒绝）> global_mode；
   - 生命周期矩阵：进 fas 前 governor release ✓；fas 退出先 deactivate 再下一 governor init（快照无污染）✓；init 失败冷却 + 回退 ✓；收尾 deactivate_all ✓；60s 注销仅内存清理 ✓；60s 内切回复用（reset_runtime + force_reapply）✓；
   - fas→fas 切换：PackageSwitch 即时（旧实例去激活→新实例激活）+ 1s 遥测兜底 + 息屏中切换后 doze 锁恢复 ✓；
   - 事件路由：FrameUpdate/SystemLoadUpdate/温度只喂 active 实例；非 active 实例零写入 ✓；
   - 息屏 doze：enter/exit 配对、FrameUpdate 漏帧自愈、scenemode/config_dirty/ConfigReload/热保护/看门狗均被 fas 守卫或豁免 ✓；
   - 日志：生命周期打点齐全（create/activate/switch/deactivate/destroy/doze）、无逐帧新日志、debug 门控 ✓；
   - Yumi 零变化 ✓；WebUI：FAS 标签显示、模式切换禁用、Home 模式显示、MockBridge 同步 ✓。
