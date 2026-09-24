# FAS 帧感知调度恢复计划（白名单驱动，ChiRi 专属）

## Summary

恢复禁用的 FAS 功能并重构为「白名单驱动 + 每应用嵌入配置」的 ChiRi 专属模式：

- 新增编译期嵌入的 `fas.yaml`（FAS 白名单，支持多应用，daemon 导出供 WebUI 只读）与 `fas/endfield.yaml`（com.hypergryph.endfield 专属 FAS 配置，嵌入不导出）。
- `determine_mode` 白名单最高优先返回 `"fas"` 模式；chiri 调度器恢复全部 FAS 接线，FAS 激活时 CLG/akmode/fast 全部暂停，温控/触摸/scenemode/doze 豁免，一切交由 FAS 帧感知调度。
- **确定性 FAS 生命周期**（用户强调 release 时机）：废弃原 5s 挂起宽限机制；统一切换律——**任何 FAS 激活前先完整 `fas_release()`（保证频率快照真实），任何 FAS 退出在下一 governor init 之前完成 `fas_release()`**。唯一不放手场景：息屏（doze 低频锁，FAS 保持接管）。
- **多 FAS 应用快速切换**：新增 `DaemonEvent::PackageSwitch`（同模式前台包变化时由 app_detect 发出），fas→fas 切换经「release → load_policies(新配置) → set_game」热替换，无 1s 级盲区。
- **线程亲和预留接口**（用户要求）：`fas_affinity_hook()` 预留接入点，FAS 激活/释放/切换时调用，当前无操作。
- Monitor 层恢复 `fps_monitor`（复用共享 PID watch）与 `foreground_max_util`；WebUI 显示 FAS 标签并禁止模式切换。

## Current State Analysis

- FAS 引擎完整保留于 `src/scheduler/fas/`（9 文件），禁用方式 = 模块声明注释 + 接线注释 + `#[allow(dead_code)]`；算法（PID/齿轮/帧流水线/per-policy 锁频）不重写，仅重新接线。
- 配置类型 `src/fas_types.rs`：`FasRulesConfig` 全字段 serde 默认 + `normalize()`，per_app_profiles 支持 target_fps/fps_margin。
- 白名单先例：special_tuned（common.rs include_str! + OnceLock + main.rs 导出 + WebUI getSpecialTuned）。
- 互斥先例：akmode 三入口接管/释放 + 失败 300s 冷却；BpfStats 先例：Yumi match 空 arm 仅为枚举完备性。
- 原挂起机制缺陷（本次废弃的原因）：suspend 期间锁频滞留 + CLG 并存互相污染快照（CLG init 会把 FAS 锁频值快照为"原始状态"，grace 超时 reset_all_freqs 再恢复真实原始值，顺序错乱）。

## Design Decisions（用户已确认）

| 决策点 | 结论 |
|---|---|
| 息屏行为 | **FAS 保持接管**：息屏写入 doze 低频锁（`doze_perf`），亮屏 `force_reapply` 恢复；scenemode 对 fas 豁免 |
| 扩展范围 | **仅频率维度**：`is_boost_mode("fas")` 保持 false，无触摸升频；线程亲和仅预留接口不实现 |
| fas.release() 时机 | **确定性统一规则**：init 前必 release（快照真实）；退出时 release 先于下一 governor init；唯一例外 = 息屏 doze（保持接管）；init 失败/panic/收尾均 release |
| 多 FAS 应用 | 白名单支持 N 个 `包名→配置` 条目；同模式切换经 PackageSwitch 事件即时热替换 |
| 生效设备 | 仅 ChiRi；Yumi 行为零变化（determine 门控 + PackageSwitch 空 arm） |
| 温控 | ThermalGuard 不作用于 FAS（只下发 CLG）；FAS 自带 `core_temp_threshold`（endfield.yaml 置 0 关闭） |
| FAS 模式来源 | 仅白名单；rules.yaml 中 "fas" 映射视为非法告警回退（同特调门控） |

## Proposed Changes

### 1. 新增配置文件（编译期嵌入）

**`module/config/normal/fas.yaml`**（新文件，白名单，多应用结构）：

```yaml
# FAS（帧感知调度）白名单 — 编译期嵌入二进制，用户/WebUI 不可修改。
# 运行时导出到模块根 fas_whitelist.txt 供 WebUI 只读展示。
# 每项：<精确包名>: <fas 配置名>（对应 module/config/normal/fas/<配置名>.yaml）
# 新增 FAS 游戏：本表加一行 + common.rs::embedded_fas_app_str 加一个 arm + 新建配置文件
fas:
  apps:
    com.hypergryph.endfield: endfield
```

**`module/config/normal/fas/endfield.yaml`**（新文件）：

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

- `FasRulesConfig` 追加 `#[serde(default = "d_doze_perf")] pub doze_perf: f32`（默认 0.18），`normalize()` 追加 clamp(0.05, 0.30)，`Default impl` 同步；serde default 保证 rules.yaml 旧格式兼容。

### 3. src/common.rs — 白名单解析 + 新事件变体

- include_str! 两文件（L446-472 嵌入区）：`FAS_WHITELIST_TEXT`、`FAS_APP_ENDFIELD_TEXT` + `embedded_fas_app_str(name)`（match "endfield"，未来游戏加 arm）。
- `static FAS_WHITELIST: OnceLock<HashMap<String, String>>`（pkg→配置名）：`fas_whitelist()`（serde 解析 `fas.apps`，失败 warn `fas-config-parse-failed`）、`fas_whitelist_entry(pkg)` 精确匹配。
- `static FAS_APP_CONFIGS: OnceLock<HashMap<String, FasRulesConfig>>`：`fas_app_config(name)`（解析 `fas_rules` 段 + `normalize()` + `migrate_legacy_margins()`，单文件失败 warn 跳过）；`fas_available() -> bool`（白名单非空且至少一个配置成功）。
- `is_fas_mode(mode) -> bool { mode == "fas" }`。
- **`DaemonEvent`（L42-81）新增变体**：`PackageSwitch { package_name: String, pid: i32 }` —— 同模式前台包变化事件；L45 枚举级 `#[allow(dead_code)]` 移除（FrameUpdate/pid/foreground_max_util 本次全部恢复消费）。

### 4. monitor 层恢复事件生产

- **`src/monitor/cpu_monitor.rs`**：L39 `FAS_FG_UTIL_ENABLED: false → true`；L54/L472/L548 三处 `#[allow(dead_code)]` 移除（恢复 FAS 时启用）。
- **`src/monitor/fps_monitor.rs`**：`start_fps_loop` 追加参数 `rx_pid: watch::Receiver<u32>`；删除内部 watch channel（L245）与 500ms PID 轮询 task（L249-269），改 `rx_pid.changed()` 驱动 `switch_pid`（monitor/mod.rs L129 注释要求的共享广播源）。
- **`src/monitor/mod.rs`**：取消 L156-174 fps_monitor 启动注释，传 `rx_pid_cpu.clone()`（L185-188 cpu_monitor move 原值前先 clone）；启动门控 `is_chiri_soc() && fas_available()`（Yumi 零开销）。
- **`src/monitor/app_detect.rs`**：
  - `determine_mode`（L195-284）特调判定之前插入 FAS 白名单判定：`chiri && fas_whitelist_entry(pkg) 有值 && fas_app_config(配置名).is_some()` → debug `app-detect-fas-fallback` → 返回 `"fas"`。
  - app_modes/global_mode 分支的非法特调判定（L224/L271）追加 `|| mode == "fas"`：非白名单 "fas" 映射 warn（`app-detect-fas-rejected` / `app-detect-fas-global-rejected`）回退。
  - **主循环（L481-503）事件发射规则改为三分支**：
    - 模式变化 → `ModeChange`（现状）；
    - 模式不变但前台包变化 → **新增发送 `PackageSwitch { package_name, pid }`**（fas→fas 快速切换的关键通路）；
    - 均无变化 → 不发。
  - 首轮检测（last_pkg 为空）只更新缓存不发 PackageSwitch。

### 5. src/scheduler/mod.rs — 仅模块声明 + 枚举完备性

- L36 取消 `// pub mod fas;`（chiri 经 `crate::scheduler::fas::FasController` 访问）。
- DaemonEvent match 追加 `PackageSwitch` 空 arm（仅 debug 日志，枚举完备性，同 BpfStats 先例）。
- **其余 FAS 注释块全部保持注释**（Yumi 逻辑冻结；determine 门控保证 Yumi 永不进入 fas，PackageSwitch 到达时 Yumi 空处理）。

### 6. src/chiri/mod.rs — 调度接线（核心）

#### 6.0 FAS 生命周期统一规则（本节为所有接线的总纲）

```rust
// fas_release：FAS 唯一退出原语。恢复快照频率 + 清游戏态 + 清 policies + 清 doze/激活包名。
// 规则：
//   R1 任何 fas 激活（进入/切换）前必须先 fas_release()——保证 load_policies 快照是真实系统状态；
//   R2 任何 fas 退出（模式切换/应用切走）时 fas_release() 必须先于下一 governor 的 init；
//   R3 init 后 policies 为空（无 cpufreq）→ fas_release() + 300s 冷却 + CLG balance 回退；
//   R4 息屏不 release（doze 保持接管）；负载看门狗不 release（FAS 帧驱动不依赖负载流）；
//   R5 panic/线程收尾 fas_release()。
fn fas_release(fas: &mut FasController, fas_doze: &mut bool, fas_active_pkg: &mut String) {
    fas.reset_all_freqs();
    fas.clear_game();
    fas.policies.clear();
    *fas_doze = false;
    *fas_active_pkg.clear();
}
```

（废弃原 `fas_suspended_at`/`fas_suspended_package`/`FAS_SUSPEND_GRACE_SECS` 挂起机制——L486-493/L1162-1177/L1489-1498 注释块**不再恢复**，改为上述 R1-R5；L1489-1498 挂起超时检查块删除。）

状态变量（L486-493 区域）：`fas_controller`（L465 取消注释）+ `fas_doze: bool` + `fas_doze_perf: f32` + `fas_active_pkg: String` + `fas_cooldown_until: Option<Instant>`（常量 `FAS_COOLDOWN = 300s`，镜像 AKMODE_COOLDOWN）。

#### 6.1 ModeChange 分支（L1142-1230）

- **进入 fas**（`if mode == "fas"`，L1143-1159 整块重写，不恢复原注释）：
  1. 防御复查白名单：`fas_whitelist_entry(&package_name)` → `fas_app_config(配置名)`；未命中（不应发生）→ warn `scheduler-fas-init-failed` → 按 balance 初始化 CLG。
  2. 冷却检查：`fas_cooldown_until` 未过期 → CLG balance 回退（同 akmode 模式）。
  3. **R1：`fas_release(...)`**；`ak_governor.release()` + `fast_lock.release()` + `cpu_governor.release()`（三 gubernator 全停）。
  4. `fas_controller.load_policies(&rules)`；`policies.is_empty()` → R3（冷却 + CLG 回退）。
  5. `set_game(pid, &package_name)`（解构处 `pid: _` 改消费）+ `set_temperature` + `set_temp_threshold(rules.core_temp_threshold)`；记录 `fas_doze_perf` / `fas_active_pkg`。
  6. `fas_affinity_hook(&mut affinity_mgr, &mut corectl_mgr, true, pid)`（预留接口，见 6.5）。
  7. info 打点 `scheduler-fas-activate`。
- **退出 fas → 其他模式**（else 分支 L1160+，替换原挂起注释块 L1162-1177）：
  - **R2：`fas_release(...)` 放在 `if is_screen_on` 之前、一切 governor init 之前**（无条件执行，息屏退出也先恢复频率）；
  - `fas_affinity_hook(..., false, 0)`；
  - 后接现有 `if is_screen_on` 内 特调/fast/CLG 分支不变（CLG init 此时快照到的是已恢复的真实状态）。
- **同模式 fas 刷新温度**（L1227-1230 注释）：恢复——`old_mode == mode && mode == "fas"` 时 `set_temperature`。
- **过渡矩阵**（均满足 R1/R2，无需特判）：fas→akmode = fas_release → ak init；akmode→fas = ak release → R1 release → fas init；fas→fast / fast→fas 同理；fas→CLG = fas_release → CLG init。

#### 6.2 PackageSwitch 分支（match 新 arm，插在 ModeChange 后）

```rust
DaemonEvent::PackageSwitch { package_name, pid } => {
    let current_mode = mode_clone.lock().unwrap().clone();
    if current_mode == "fas" && !fas_controller.policies.is_empty() {
        // fas→fas 热切换（R1：先 release 保证新配置快照真实）
        if let Some(rules) = fas_whitelist_entry(&package_name).and_then(|e| fas_app_config(&e)) {
            fas_release(...);
            fas_controller.load_policies(&rules);
            fas_controller.set_game(pid, &package_name);
            fas_controller.set_temperature(/* 复用最近温度缓存 */);
            fas_controller.set_temp_threshold(rules.core_temp_threshold);
            fas_active_pkg = package_name.clone();
            if fas_doze { /* 息屏中切换：重新写入 doze 低频锁 */ }
            info!(t_with_args("scheduler-fas-switch", ...));
        }
    }
    // 非 fas 模式：忽略（普通模式的包切换无调度动作）
}
```

#### 6.3 ScreenStateChange（L964-1087）

- **息屏**（L970-984）：特调保持分支后插入 fas 分支（替换原「息屏剥夺」注释块 L970-977）——**FAS 保持接管，不 release**（R4）：
  ```rust
  } else if current_mode == "fas" {
      // FAS doze：写入低频锁；帧驱动静止后频率保持低位；亮屏恢复
      for p in &mut fas_controller.policies {
          let f = p.find_nearest_freq(fas_doze_perf);
          p.apply_freq_locked(f);
      }
      fas_doze = true;
      info!(t("scheduler-fas-doze-enable"));
  } else { /* 现有 CLG doze 分支 */ }
  ```
  （`find_nearest_freq`/`apply_freq_locked` 均为 pub 方法，实现时核实签名。）
- **亮屏**（L1066-1070 fas 空壳分支）：`if fas_doze { 全 policies force_reapply; fas_doze = false; info!(t("scheduler-fas-doze-restore")); }`。
- 息屏/亮屏的 `apply_affinity_and_corectl` 调用不变（fas 非 boost，走正常布局路径）。

#### 6.4 SystemLoadUpdate / FrameUpdate / scenemode / 热保护

- **负载投喂（L1234-1262）**：优先级链最前插入（恢复注释 L1247-1252）：`current_mode == "fas" && !policies.is_empty()` → `fas_controller.update_cpu_util(foreground_max_util)` + `update_core_utils(&core_utils)`；解构处 `foreground_max_util: _` 改消费。
- **scenemode 进入（L1276）**：追加 `&& current_mode != "fas"` 豁免；饱和退出逻辑不变。
- **FrameUpdate（L1382-1394）**：恢复注释块——`current_mode == "fas"` 时：3s 温度刷新（新增独立变量 `fas_temp_path`/`last_fas_temp`，避免与 L512 thermal 探测重名）+ `fas_controller.update_frame(frame_delta_ns)`；`fas_doze` 时先 force_reapply 退出 doze（自愈漏发的亮屏事件）。
- **热保护块（L780-868）**：`current_mode == "fas"` 时整体跳过（温控移除 + 省 2s 周期计算）；1s 遥测温度读数保留。
- **config_dirty（L617-664）与 ConfigReload（L1402-1459）**：「其他模式」分支补 `current_mode != "fas"` 守卫（FAS 配置编译期嵌入，重载与 FAS 无关，且不得在 FAS 激活时动 CLG）；fas 分支无操作。
- **看门狗 Timeout（L909-940）与 is_boost_mode（L292-294）**：不变（R4；fas 非 boost）。

#### 6.5 线程亲和预留接口（用户要求）

```rust
/// FAS 线程亲和预留接入点（当前无操作）。
/// 预留：FAS 模式下前台关键线程钉核/大核亲和扩展时在此实现，
/// 激活(true)/释放(false)/doze 进出时由事件循环调用，无需再改接线。
fn fas_affinity_hook(
    affinity_mgr: &mut affinity::AffinityManager,
    corectl_mgr: &mut core_ctl::CoreCtlManager,
    active: bool,
    fg_pid: i32,
) {
    let _ = (affinity_mgr, corectl_mgr, active, fg_pid);
}
```

调用点：FAS 激活（6.1 第 6 步）、退出（6.1 else）、（doze 进出可选）。`is_boost_mode` 保持不含 fas。

#### 6.6 收尾

- **panic/线程收尾（L1501-1514）**：恢复 L1512-1514 注释为 `fas_release(...)`（R5，置于 cpu/ak/fast release 之后、corectl/affinity release 之前）。
- **1s 遥测块兜底（L685-778）**：`current_mode == "fas" && !policies.is_empty()` 时比对 `app_detect::get_current_package()` 与 `fas_active_pkg`，变化则走 6.2 同款热切换（PackageSwitch 事件丢失时的安全网）。

### 7. src/main.rs — 运行时导出

- L77-101 special_tuned 导出块后追加：`chiri_active` 门控内遍历 `fas_whitelist()` 写 `fas_whitelist.txt`（`{pkg}:{cfg}\n`），info `main-fas-whitelist-exported`（count 参数）。

### 8. i18n（zh.ftl + en.ftl 同步）

新增：`main-fas-whitelist-exported`、`fas-config-parse-failed`、`app-detect-fas-fallback`、`app-detect-fas-rejected`、`app-detect-fas-global-rejected`、`scheduler-fas-activate`、`scheduler-fas-switch`、`scheduler-fas-doze-enable`、`scheduler-fas-doze-restore`、`scheduler-fas-init-failed`、`scheduler-fas-cooldown`。

### 9. WebUI（FAS 标签 + 禁止切换模式）

- **`webui/src/utils/bridge.ts`**：`PATHS` 加 `FAS_WHITELIST`（L14-23）；新增 `getFasWhitelist(): Promise<Record<string, string>>`（仿 `getSpecialTuned` L226-242，解析 `pkg:config` 行）。
- **`webui/src/utils/mock.ts`**：`mockFasWhitelist` + `getFasWhitelist`（MockBridge 联合类型必须同步）。
- **`webui/src/stores/scheduler.ts`**：state 加 `fasWhitelist`，`initData` 并行加载（L21-27）。
- **`webui/src/views/AppRulesView.vue`**：FAS 标签（`store.isChiri && store.fasWhitelist[pkg]`，文本 `t('mode_fas')`）；`openMenu`（L96-99）对 FAS 应用提前 return + van-cell disabled 样式（参照 L174 pointer-events 模式）；L154「未配置」占位排除 FAS。
- **`webui/src/views/HomeView.vue`**：`currentModeInfo`（L29-43）加 fas 分支（`t('mode_fas')`/`t('desc_fas')`）。
- 动作单 `mode_fas` 选项（L28 注释）保持注释——FAS 仅白名单驱动，非用户可选模式。

### 10. AGENTS.md 维护更新（会话收尾时）

- 目录结构（`module/config/normal/fas.yaml`、`fas/`）、配置章节（嵌入+导出约定）、ChiRi 子系统新增「FAS 帧感知调度」小节（白名单、生命周期 R1-R5、doze、PackageSwitch、亲和预留接口）、monitor（FAS_FG_UTIL_ENABLED=true）、WebUI（FAS 标签）。

## Assumptions & Decisions

- FAS 引擎算法不改；「打破现有逻辑」体现为接管语义重构（白名单驱动 + 确定性 release 生命周期 + 息屏保持接管 + 温控豁免 + 同模式热切换），并废弃原 5s 挂起宽限机制（快照污染缺陷）。
- `rules.yaml` fas_rules 注释模板与 `RulesConfig.fas_rules` 字段不动（Yumi 冻结区）；FAS 配置完全脱离 rules.yaml。
- `config-example.yaml` 不新增（fas.yaml 为独立嵌入文件，同 akmode.yaml 先例，文件头自带文档）。
- endfield.yaml 初值为工程合理默认，实机调优后迭代。
- PackageSwitch 事件对 Yumi 为空 arm（BpfStats 先例），非 fas 模式下 chiri 也忽略——零行为变化。

## Verification

1. `cargo +nightly check -p yumi --target aarch64-linux-android`（Windows 走 ebpf stub 回退；验证 dead_code 清理后无新 warning）。
2. `cd webui && npm run type-check`。
3. 逻辑走查清单：
   - determine_mode 优先级：FAS 白名单 > 特调 > app_modes（fas 非法映射拒绝）> global_mode；
   - **release 时机矩阵**：fas 进入/切换前 R1 ✓；退出先于下一 init R2 ✓；init 失败 R3 ✓；息屏不 release R4 ✓；收尾 R5 ✓；akmode/fast↔fas 双向切换快照顺序正确；
   - fas→fas 切换：PackageSwitch 即时热替换 + 1s 兜底；息屏中切换后 doze 锁恢复；
   - scenemode/config_dirty/ConfigReload/热保护/看门狗均被 fas 守卫或豁免；
   - Yumi 零变化（determine 门控 + fps_monitor 启动门控 + PackageSwitch 空 arm）。
