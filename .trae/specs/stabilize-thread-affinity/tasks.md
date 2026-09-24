# Tasks

- [x] Task 1: `set_tid_affinity` getaffinity 短路（src/chiri/affinity.rs）
  - [x] 1.1 构建期望 `cpu_set_t` 后先 `sched_getaffinity` 比较，一致则跳过 `sched_setaffinity` 并返回 true；读取失败回退原写入路径
  - [x] 1.2 更新函数 doc 注释：短路语义、SAFETY 说明（zeroed 分配与 CPU_SET 边界沿用现有说明）
- [x] Task 2: 关键线程核组绑定（src/chiri/affinity.rs，依赖 Task 1）
  - [x] 2.1 `ThreadState` 增加 `group_pinned: bool`；前台循环 key 线程不再 `pin_core`：cpuset 收窄生效时零写入，cpuset 不可用时写 prime∪big 掩码一次并置 `group_pinned`
  - [x] 2.2 `cleanup_thread`/退出 boost 路径对 `group_pinned` 线程恢复全核掩码（新增 `restore_group_mask`），`core_pinned` 计数不受影响；devimp aff 行 reason 用 `fg_group`/`fg_group_reset`
  - [x] 2.3 移除 key 线程对 `pick_core_pref(prime_pool, big_pool)` 的调用；`prime_pool` 保留（普通线程溢出路径仍引用）
- [x] Task 3: 普通线程重钉滞回与反跳回（src/chiri/affinity.rs）
  - [x] 3.1 `ThreadState` 增加 `prev_home: i16`（初值 -1）与 `prev_home_at: Instant`；`pin_core` 迁移成功时记录旧 home 与时刻
  - [x] 3.2 新增常量 `OVERLOAD_MARGIN: f32 = 0.15`、`RETURN_COOLDOWN: Duration = 16s`；前台 home_overload 与 bg_overload 两处重钉执行三重校验（分数滞回、反跳回冷却、目标核不过载），拒绝时打 devimp `overload_hold`
  - [x] 3.3 模块头注释同步：分层说明补 key 线程组绑定、重钉两道闸的动机与乒乓根因、新常量说明
- [x] Task 4: 构建验证
  - [x] 4.1 `cargo check --target aarch64-linux-android` 通过（exit 0，无 rustc warning；仅 build.rs 的 eBPF 跳过提示，为 Windows 本机预期）
  - [x] 4.2 对照 checklist.md 逐条核对改动点

# Task Dependencies

- Task 2 依赖 Task 1（组掩码去重使用短路）
- Task 2 与 Task 3 同文件，顺序执行
- Task 4 依赖 Task 1-3 全部完成
