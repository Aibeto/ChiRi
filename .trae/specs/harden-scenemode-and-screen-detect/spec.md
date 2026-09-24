# 息屏场景振荡治理与屏幕状态一致性 Spec

## Why

8550 整夜日志（daemon.log + devimp_*）显示 scenemode 以「进入 → ~11s 饱和退出 → 300s 冷却 → 再进入」循环 9 轮（01:27–02:10），期间反复上下线 5 核 + CLG reload。主因已确认为热更新回滚（旧二进制）导致亮屏状态误判，但对照当前源码仍存在三处会被同类负载/日志再次打中的真实问题，需一并治理。

## 根因（日志证据 → 源码定位）

1. **背光判定口径不一致**：`read_backlight_state`（verify 自愈用，screen_detect.rs:113）已修「bl_power 亮屏路径不清零、长期停留非 0」问题；但 uevent backlight 分支（screen_detect.rs:210-213）仍用旧口径 `bl_power == 0 → 亮,否则灭`。该设备亮屏时 bl_power 非 0 → 每次背光 uevent 误报 OFF，1s 后 verify 拉回 ON（daemon.log 01:22:49/50、01:24:53 成对抖动）；纠正竞态失败时调度器滞留息屏态，亮屏期间误入 scenemode。
2. **scenemode 重入无负载门槛**：饱和退出 + 300s 冷却后无条件重进（mod.rs:1410-1460），不检查当前负载。息屏期间后台负载持续（sync/备份/播放）时，进入后 ~10s 必然饱和退出，300s 后再进——日志中整夜 9 轮拉锯。
3. **devimp tick 行假 down**：`on_load_update`（cpu_load_governor.rs:441-457）中 `target_perf == old_perf`（含 ceiling 钳制后的稳态，如 powersave little 卡 0.60）落入 down 分支：decision 标 "down"、deb_down 每 tick +1 涨到 238+，与 util=1.00 并存自相矛盾——本次日志分析即被误导（误判为「满载反而想降频」的调度 bug）。

## What Changes

- screen_detect 的 uevent backlight 分支改用 `read_backlight_state()`，与 verify 自愈同口径；删除行内旧判定（**BREAKING** 无——仅内部判定函数替换，无配置/接口变化）。
- scenemode 进入（含冷却后重入）增加负载门槛：常驻簇（little+big）max_util ≥ `SCENEMODE_SAT_UTIL` 时本 tick 跳过进入（`scene_mode_active` 保持 false、`screen_off_at` 不动），后续 tick 持续复评，负载回落后自然进入。
- CLG `on_load_update` 改三分支：target < old → down（原路径不变）；target == old → "hold"（不写 current_perf、up_wait/down_wait 清零）；target > old → up（原路径不变）。devimp tick 行不再出现假 down。

## Impact

- Affected specs: 无（独立增量）
- Affected code:
  - `src/monitor/screen_detect.rs`（uevent backlight 分支判定替换）
  - `src/chiri/mod.rs`（scenemode 进入门槛，复用既有 `SCENEMODE_SAT_UTIL` 与 `last_core_utils`）
  - `src/chiri/cpu_load_governor.rs`（决策三分支 + devimp decision 语义）

## ADDED Requirements

### Requirement: 背光判定口径一致

uevent backlight 分支 SHALL 使用 `read_backlight_state()` 判定屏幕状态，与 `verify_screen_state` 完全一致：`bl_power == 0` → 亮；`bl_power != 0` 以 `actual_brightness > 0` 为准；节点不可读时维持现有静默跳过。

#### Scenario: 亮屏时收到 backlight uevent

- **WHEN** 屏幕亮着、bl_power 停留非 0、actual_brightness > 0，收到 backlight change uevent
- **THEN** 不产生 ScreenStateChange(false)，调度器息屏态不翻转，scenemode 计时器不被误触发

### Requirement: scenemode 进入负载门槛

进入 scenemode（`scene_mode_delay_secs` 到期且不在冷却）前 SHALL 按饱和退出同口径计算常驻簇 max_util；≥ `SCENEMODE_SAT_UTIL` 时本 tick 跳过进入，`screen_off_at` 与既有计时逻辑不受影响。

#### Scenario: 后台负载持续

- **WHEN** 息屏超时到达且 little/big max_util ≥ 0.70
- **THEN** 不进入 scenemode（无核心上下线、无 CLG reload）；负载回落后下一 tick 自动进入，无需人工干预

#### Scenario: 正常息屏不受影响

- **WHEN** 息屏超时到达且常驻簇负载低于阈值
- **THEN** 行为与现状完全一致（进入 scenemode、下线核、独占小核）

### Requirement: CLG 稳态决策标记

`target_perf == old_perf` 时 decision SHALL 标 "hold"：不进入 up/down 任何写频路径，`up_wait`/`down_wait` 清零。

#### Scenario: ceiling 钳制稳态

- **WHEN** powersave little util=1.00、current_perf == tgt_perf == 0.60（ceiling 钳制）
- **THEN** devimp tick 行 decision="hold"、deb 计数归零、频率不变；不再出现「满载 decision=down、deb_down 无限增长」的矛盾记录

#### Scenario: 原有升降频路径不回退

- **WHEN** target 与 current 不相等
- **THEN** up/down 分支行为（平滑、速率限制、立即降频）与现状逐字节一致
