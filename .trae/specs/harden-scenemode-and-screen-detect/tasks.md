# Tasks

- [x] Task 1: 背光判定口径统一（src/monitor/screen_detect.rs）
  - [x] 1.1 uevent backlight 分支删除行内 `bl_power == 0` / `actual_brightness` 旧判定，改调 `read_backlight_state(dev)`
  - [x] 1.2 确认 `None` 返回时静默跳过（不发事件、不改 state_arc），与 verify 自愈行为一致（沿用既有 unreadable debug 分支）
- [x] Task 2: scenemode 进入负载门槛（src/chiri/mod.rs）
  - [x] 2.1 在 `delay_hit` 满足后、reload/init CLG 之前，按饱和退出同口径（`chiri_core_ranges()` little+big → `last_core_utils` 取 max）计算 `standby_max`
  - [x] 2.2 `standby_max >= SCENEMODE_SAT_UTIL` 时跳过本次进入（不置 `scene_mode_active`、不动 `screen_off_at`、不触发 core_ctl），注释说明与饱和退出共用阈值的原因
  - [x] 2.3 devimp 打点：被门槛拒绝时记 `scene_hold`（含 util 值），持续拒绝期去重只记一条；息屏/亮屏/饱和退出三处 `scene_mode_active=false` 复位点同步清零
- [x] Task 3: CLG 决策三分支（src/chiri/cpu_load_governor.rs）
  - [x] 3.1 down 执行分支改为先算降频落点：`find_nearest_freq(target_perf) == current_freq`（钳制稳态）→ decision="hold"、`down_wait` 清零、不写频；落点不同 → 原 down 路径（计数 + 速率限制 + down_fast_threshold 语义不变）；up 分支不动
  - [x] 3.2 函数 doc 注释同步：决策标签 up/down_wait/down/hold 及 hold 动机（ceiling/floor 钳制稳态 + devimp 假 down 误导）
- [x] Task 4: 构建验证
  - [x] 4.1 `cargo check --target aarch64-linux-android` 通过（Finished dev profile，无 rustc error/warning；仅 build.rs eBPF 跳过提示，Windows 本机预期）
  - [x] 4.2 对照 checklist.md 逐条核对

# Task Dependencies

- Task 1、2、3 相互独立，可并行
- Task 4 依赖 Task 1-3 全部完成
