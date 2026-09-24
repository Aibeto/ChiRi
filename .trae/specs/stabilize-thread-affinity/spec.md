# 线程亲和稳定性改进 Spec

## Why

ChiRi 的前台线程钉定（`src/chiri/affinity.rs`）把关键线程钉死在单核上。重负载游戏的主线程会因 home_overload 重钉在 prime 和大核之间以 8 秒节拍来回跳，每次迁移 cache/TLB 全冷，表现是灾难性卡顿；另外多个 key 线程抢唯一的 prime，谁先被 `read_dir` 扫到谁占，主线程可能只分到一颗大核。参考 AppOptR（掩码绑定核组、`sched_getaffinity` 零开销短路、迁移少而准）改进。

## 根因（现状机制）

- 单核钉定：key 线程走 `pick_core_pref(prime_pool, big_pool)` 优先 prime，普通线程优先 big，全部 `pin_core` 到单核。
- home_overload 重钉：home 核 util > 0.70（`CORE_OVERLOAD_UTIL`）且距上次迁移 ≥ 8s（`REPIN_DEBOUNCE`）时重钉。此时 prime 被线程自己钉住（`pinned_of==1`），`pick_core_pref` 的 `main_free` 为 false，key 线程与空闲大核竞争 score，被挤到单颗大核；prime 空出后（pinned==0、util 回落）又满足回迁条件，于是 prime↔big 以 8 秒节拍稳定振荡。
- 现有防护只有时间防抖，没有分数滞回，也没有反跳回：迁到 B 后 B 过载、A 已冷却，就会 B→A→B 循环。普通线程在两颗大核间同理。

## What Changes

- 关键线程改核组绑定（AppOptR 式）：不再单核 `sched_setaffinity`。boost 下 top-app/foreground cpuset 已收窄为 prime∪big（`boost_list`），key 线程直接在该组内由内核调度，零掩码写入；cpuset 不可用时写一次 prime∪big 组掩码兜底（经 getaffinity 短路去重）。key 线程不再进入 `pick_core_pref` 与 home_overload 重钉。
- 普通前台线程保留单核钉定，重钉加两道闸：分数滞回（目标核 score 至少比 home 核低 `OVERLOAD_MARGIN`=0.15）+ 反跳回冷却（`RETURN_COOLDOWN`=16s 内禁止迁回该线程最近迁离的核）。后台 promoted 线程的 bg_overload 重钉同口径。
- `set_tid_affinity` 加 `sched_getaffinity` 短路：期望掩码与当前一致就跳过写入。core_ctl 的复用点（scenemode 专用核自钉）自动受益，签名不变。
- devimp 增加 `overload_hold` 打点：重钉被滞回或反跳回拒绝时记录，便于验证乒乓消除。
- 不新增 config.yaml 字段，阈值以常量固化在 affinity.rs。

## Impact

- Affected specs: 无（首个 spec）
- Affected code:
  - `src/chiri/affinity.rs`（主要：`set_tid_affinity` 短路、rebalance 前台 key/普通分支、`pin_core` 记录迁离核、重钉校验、`ThreadState` 新字段、模块头注释）
  - `src/chiri/core_ctl.rs`（无改动，仅复用 `set_tid_affinity`）

## ADDED Requirements

### Requirement: 关键线程核组绑定

boost 且 `pin_foreground_threads` 时，关键线程（`tid == 主线程 tid` 或 comm 命中 `KEY_THREAD_COMMS`）SHALL 绑定性能核组而非单核：

- `/dev/cpuset/top-app` 存在且 boost 布局已应用（`applied_kind == KIND_BOOST`）：不写任何掩码。
- cpuset 不可用：对该 key 线程写一次 prime∪big 组掩码（无 prime 的 SoC 即 big 全组），状态记 `group_pinned`，供 unpin/cleanup/release 恢复全核。

#### Scenario: boost 前台游戏

- **WHEN** boost 模式下前台游戏的 key 线程进入 rebalance
- **THEN** 不发生单核 `sched_setaffinity`，线程在 prime∪big 组内由内核调度，prime↔big 迁移不存在

#### Scenario: cpuset 不可用兜底

- **WHEN** `sys.cpuset_top_app_exist == false`
- **THEN** key 线程收到 prime∪big 组掩码；后续 rebalance 不重复写（短路或 `group_pinned` 跳过）

#### Scenario: 状态清理

- **WHEN** 退出 boost、前台切换（departed 清理）、线程消失或 `release()` 清理 key 线程
- **THEN** 曾写过组掩码的线程恢复全核掩码，`core_pinned` 计数不受影响

### Requirement: 普通线程重钉滞回与反跳回

home_overload / bg_overload 重钉 SHALL 同时满足：

1. 目标核 score ≤ home 核 score − `OVERLOAD_MARGIN`（score = util + pinned×`PINNED_WEIGHT`，与 `pick_core` 同口径）
2. 目标核不在反跳回冷却内：`RETURN_COOLDOWN` 内禁止选回该线程最近迁离的核（`prev_home`）
3. 目标核 util ≤ `CORE_OVERLOAD_UTIL`（既有条件保留）

不满足则本轮静止，并打 devimp `overload_hold`（频率已被 REPIN_DEBOUNCE 分支节流）。

#### Scenario: 两核乒乓被打破

- **WHEN** 线程从大核 A 迁到 B 后 B 也过载
- **THEN** `RETURN_COOLDOWN` 内不会迁回 A；若 A 冷却后 score 未显著低于 B，线程静止

#### Scenario: 分散能力不回退

- **WHEN** home 核持续过载且存在 score 显著更低、未处于冷却的目标核
- **THEN** 照常重钉

### Requirement: getaffinity 短路

`set_tid_affinity` SHALL 先构建期望 `cpu_set_t`，`sched_getaffinity` 读取当前掩码，两者一致时跳过 `sched_setaffinity` 并返回 true；读取失败按原路径写入。

#### Scenario: 重复钉定零写入

- **WHEN** 目标掩码与当前掩码一致
- **THEN** 不发生 `sched_setaffinity`，返回 true，无迁移副作用

## MODIFIED Requirements

无独立既有 spec；`affinity.rs` 内行为变化由 ADDED 覆盖。

## REMOVED Requirements

### Requirement: 关键线程单核优先钉定（`pick_core_pref` 的 key 分支：main=prime_pool, overflow=big_pool）

**Reason**: 单核钉定叠加 home_overload 重钉是 prime↔big 乒乓与「主线程只分一颗大核」的直接来源；boost 的 cpuset 收窄已提供核组约束。
**Migration**: key 线程走核组绑定；`pick_core_pref` 仅保留普通线程路径（main=big, overflow=prime）；`prime_pool` 若无其他引用则移除。
