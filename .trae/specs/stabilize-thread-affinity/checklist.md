# Checklist

- [x] `set_tid_affinity` 在期望掩码与当前掩码一致时跳过 `sched_setaffinity` 并返回 true；getaffinity 读失败时回退原写入路径
- [x] boost + `/dev/cpuset/top-app` 可用时，key 线程零掩码写入（完全不调 `set_tid_affinity`）
- [x] cpuset 不可用时 key 线程写入 prime∪big 组掩码；重复 rebalance 不再重复写（`group_pinned` 跳过 + 短路双重去重）
- [x] key 线程不再进入 `pick_core_pref` 与 home_overload 重钉路径（`if is_key` 独立分支）
- [x] `group_pinned` 线程在 cleanup/release/退出 boost/前台切换时恢复全核掩码（`restore_group_mask`），`core_pinned` 计数不漂移（组绑定从不走 `add_pinned`）
- [x] 前台普通线程与 bg promoted 线程的重钉满足三条校验：目标 score ≤ home score − 0.15、目标不在 16s 反跳回冷却内、目标 util ≤ 0.70
- [x] 迁移成功时 `prev_home`/`prev_home_at` 正确记录（`pin_core` 内 `prev_home >= 0` 才记录）；首次钉定（prev_home = -1）不会被反跳回误伤
- [x] 重钉被拒时输出 devimp `overload_hold`（含 tid/home/cand/双方 score），且整体位于 REPIN_DEBOUNCE 分支内（≤8s 一条）
- [x] `core_ctl.rs` 的 `set_tid_affinity` 复用点行为不变（签名未动，仅获得短路优化）
- [x] 模块头注释、新增常量注释与实现一致（含乒乓根因说明、score 选核仅服务单核钉定线程）
- [x] `cargo check --target aarch64-linux-android` 通过（exit 0，无 rustc warning）
