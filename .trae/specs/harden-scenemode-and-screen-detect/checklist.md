# Checklist

- [x] uevent backlight 分支使用 `read_backlight_state()`，判定结果与 verify 自愈一致（bl_power=0→亮；非 0 看 actual_brightness>0；不可读→None 静默跳过）
- [x] 亮屏 + bl_power 停留非 0 场景不再产生 ScreenStateChange(false)
- [x] scenemode 进入前计算常驻簇（little+big）max_util，≥ SCENEMODE_SAT_UTIL 时跳过进入且不影响 `screen_off_at` 计时
- [x] 负载回落后下一 tick 能自动进入 scenemode（无需人工干预）
- [x] 正常息屏（低负载）进入路径与现状一致：CLG reload + core_ctl 下线 + 专用核独占
- [x] 门槛拒绝时 devimp 有 `scene_hold` 打点（含 util 值，持续拒绝期去重，息屏/亮屏/饱和退出复位）
- [x] CLG 降频落点与当前频点相同时 decision="hold"、deb 计数清零、不改 `current_perf`
- [x] up/down 分支原逻辑（平滑、速率限制、立即降频、ceiling/floor clamp）无行为变化
- [x] powersave little 稳态（util=1.00、perf≈0.60 同频点）下 devimp tick 行 decision="hold"、deb 不再无限增长
- [x] `cargo check --target aarch64-linux-android` 通过，无新 rustc warning
