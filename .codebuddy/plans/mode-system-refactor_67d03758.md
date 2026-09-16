---
name: mode-system-refactor
overview: 重构调度模式体系：重申 reduce/default/boost/scenemode/vector/contingency/babel/FAS/down 的特性语义，建立 CLG / stardust / rhine 三个模式家族分组；FAS 从多实例改为单实例（15s 延迟进出）并引入 performance 内核调速器（含快照恢复）；新增 contingency（GPU 自动探测锁频 + 静态分组 + 停线程迁移）与 babel（规整化分组）实现；取消 customize.sh 热更新后自动启动。
todos:
  - id: governor-gpu-layer
    content: 使用 [subagent:code-explorer] 定位 fast.rs 的快照/恢复模式与 utils::FastWriter 接口后，新增 src/chiri/governor.rs 与 src/chiri/gpu.rs（快照/激活/恢复、GPU 节点探测、启动残留清理），补 i18n 打点，遵循 [skill:token-efficient-coding]
    status: completed
  - id: fas-single-instance
    content: 重写 src/chiri/fas_manager.rs 为单实例（删 instances/TTL/reap，15s 延迟退出，deactivate_delay_secs 进 FasRulesConfig 与 fas-example.yaml），接入 governor.activate/release，同步 chiri/mod.rs 的 fas 分支与 scenemode 互斥，更新 i18n
    status: completed
    dependencies:
      - governor-gpu-layer
  - id: contingency-babel-impl
    content: 在 chiri/mod.rs 新增 contingency/babel 模式分支（governor+GPU+停迁移+cpuset 分组），rhine-init.yaml 定义影响项，覆盖启动/亮屏/ModeChange/ConfigReload/DOWN/panic 全部切换入口的接管与释放
    status: completed
    dependencies:
      - governor-gpu-layer
  - id: webui-family-groups
    content: WebUI：data/mode.ts 家族分组（CLG/stardust/rhine）、LAB_ENABLEABLE 加 contingency/babel、LAB_TAKEOVER 同步、locale 补 mode.contingency/mode.babel，更新 lab.test.ts 刚性断言
    status: completed
    dependencies:
      - contingency-babel-impl
  - id: clg-family-docs
    content: 按语义核对 feature.yaml 三档 CLG 参数并重申注释（reduce 保守/boost 激进/boost 大余量），scenemode/down 归 stardust 家族的注释与 WebUI 分组展示，AGENTS.md 模式章节同步（README 不动）
    status: completed
    dependencies:
      - fas-single-instance
  - id: customize-script-robustness
    content: customize.sh 删除热更新后自动重启 service.sh（成功/失败都改为提示手动 Action），核对 action.sh/service.sh 启动路径的 governor/GPU 残留清理与看门狗/panic/ConfigReload 覆盖
    status: completed
    dependencies:
      - contingency-babel-impl
  - id: verify-and-sync
    content: cargo check（touch 强制重编）+ vitest + svelte-check 全绿，rustfmt 新增文件，AGENTS/memory 终稿同步，清理临时文件
    status: completed
    dependencies:
      - fas-single-instance
      - webui-family-groups
      - customize-script-robustness
      - clg-family-docs
---

## 产品概述

对 ChiRi 调度器的模式体系做一次「特性重申 + 补齐实现 + 健壮性加固」：明确 reduce/default/boost（CLG 家族）、scenemode/down（stardust 家族，仅分组概念）、vector/contingency/babel（rhine 实验室家族）三组模式的行为边界，补齐 contingency / babel 的完整实现，把 FAS 从多实例改为单实例（前台判断 + 15s 延迟进出），并建立 governor（performance 调速器）与 GPU 频率的快照/恢复机制，杜绝「FAS 退出后仍是 performance」「单模式崩溃无兜底」一类状态残留。

## 核心功能

- 模式特性重申与核对：reduce（升频保守/降频激进）、default（默认+参考）、boost（升频激进/降频略消极/更大线程与核心余量）、scenemode（息屏压频）、vector（全核最高频）、down（仅日志与看门狗）
- 新增 contingency：CPU+GPU 全核最高频、`performance` 内核调速器、停止线程亲和与迁移、后台进程压到 0-1 两颗小核、前台/顶部全核
- 新增 babel：禁用线程亲和迁移、后台→小核、前台/顶部→大核+超大核、系统进程(system-background)→大核
- FAS 单实例化：删除多实例管理器，白名单前台激活、离开前台延迟 15s 退出；激活期间 `performance` 调速器
- stardust 家族：scenemode/down 的分组概念，体现在注释/AGENTS/代码分组/WebUI 展示，不产生新的模式值。此家族模式启用时需要停止线程迁移亲和，为所有进程分配所有核心并设置所有cpuset为全部核心。
- 健壮性：governor 与 GPU 的快照/恢复覆盖 panic 自愈、看门狗、ConfigReload、DOWN、进程收尾、action/service 重启等全部退出路径；customize.sh 取消热更新后自动启动，改为提示手动 Action
- 配置边界：新增行为参数归嵌入配置（不新增对外暴露文件）；log 按 i18n 中英对称；AGENTS 与 memory 同步；README 不动

## 技术栈

Rust（nightly，aarch64-linux-android）+ Svelte 5 WebUI + 嵌入式 YAML 配置（include_dir!）+ shell 模块脚本。

## 架构设计

### 新增两个接管层（与 FastLock 同构、调度线程独占）

1. `src/chiri/governor.rs` — GovernorGuard：对每个 CPU policy 的 `scaling_governor` 快照原值 → 写 `performance`；`release()` 恢复快照（缺失/写失败逐 policy 记 warn）。daemon 启动时若检测到残留 `performance` 且无接管方活跃 → 清理回 `schedutil`（覆盖上次异常退出）。
2. `src/chiri/gpu.rs` — GPU 频率锁：启动探测一次（Adreno `/sys/class/kgsl/kgsl-3d0/`、通用 devfreq `/sys/class/devfreq/*gpu*/max_freq`、Mali 路径），能读到可用频率表/现值就锁最高，探测失败跳过 + 一次性 warn；同样快照/恢复。

### 模式分支（chiri/mod.rs）

- `contingency`：`governor.activate()` + `gpu.lock()` + `affinity_mgr.release()`（停迁移）+ cpuset 组级写入（background/restricted → "0-1" 取 `chiri_core_ranges` little 前两颗；foreground/top-app → 全核）。
- `babel`：`affinity_mgr.release()` + cpuset 组级（background/restricted → 小核；foreground/top-app → 大核+超大核；system-background → 大核）。
- 两者的 activate/release 并入 fast_lock 同款的**全部模式切换入口**：启动、亮屏恢复、ModeChange、ConfigReload、DOWN 进出、panic 自愈——漏一个就残留（这是本轮健壮性核心，逐入口核对）。
- `is_boost_mode` 不含 contingency/babel（它们有独立分支，不走 boost 亲和收窄）。

### FAS 单实例化（fas_manager 重写）

删除 `instances: Vec` / `FAS_INSTANCE_TTL(60s)` / `reap()` / C1 复用；保留单 `{active_pkg, last_fg, controller}`。离开前台不立即退出：`last_fg + 15s` 内回来则继续；到期才 `deactivate`（恢复 governor）。延迟时长进 `FasRulesConfig`（`deactivate_delay_secs`，serde default 15，写入 `normal/fas-example.yaml` 说明）——遵循「行为参数归嵌入配置、不新增对外文件」。激活时接管 governor，退出/停摆/panic/收尾恢复。scenemode 互斥条件从 `has_any_instance` 简化为 `is_active`。

### rhine 实验室（rhine-init.yaml + WebUI）

- `contingency`/`babel` 影响项：`global_mode: <自身>` + `fas_enabled: false` + `scenemode_enabled: false` + `special_tuned: false`（与 vector 同构：实验室期间不被 FAS/scenemode/特调抢接管；chiri 按模式名走各自分支）。
- WebUI：`LAB_ENABLEABLE` 加入 `contingency`/`babel`（frozen 保持预留）；`LAB_TAKEOVER` 同步两表条目；`data/mode.ts` 把 vector/contingency/babel 归入 rhine 家族（新增 lab 分组 kind），reduce/default/boost 为 CLG 家族，scenemode/down 为 stardust 家族分组展示；locale 补 `mode.contingency` / `mode.babel` 中英对称。

### 脚本

`customize.sh` [hot-update-flow]：删除「setsid 重启 service.sh + 轮询确认」段，成功/失败路径统一改为打印「请手动执行 Action 启动」（复用/调整现有 MSG_HOT_UPDATE_ABORT 文案，中英双语已有）。

### 性能与安全

- governor/GPU 快照只做一次 IO 批量读写；探测启动一次、结果缓存。
- 全部 sysfs 写沿用 `utils::try_write_file` / `FastWriter`，无 unwrap/panic；启动期（logger::init 前）的打点沿用「带出结果由调用方补打」模式；新日志 i18n 中英对称。
- 嵌入配置改动（feature.yaml 注释重申、fas-example.yaml 新字段）需重编译生效；`build.rs::assert_required_configs` 断言核对；不新增任何对外暴露文件。

### 关键目录

```
src/chiri/governor.rs          # [NEW] 调速器快照/恢复
src/chiri/gpu.rs               # [NEW] GPU 频率探测/锁定/恢复
src/chiri/fas_manager.rs       # [MODIFY] 多实例 → 单实例 + 15s 延迟
src/chiri/mod.rs               # [MODIFY] contingency/babel 分支 + 七入口接管/释放
src/fas_types.rs               # [MODIFY] deactivate_delay_secs
module/config/rhine-init.yaml  # [MODIFY] contingency/babel 影响项
module/config/normal/fas-example.yaml # [MODIFY] 新字段说明
module/customize.sh            # [MODIFY] 取消热更新自动启动
webui/src/data/{mode,lab}.ts   # [MODIFY] 家族分组 + 启用入口 + 接管表
webui/src/i18n/locales/*.ts    # [MODIFY] 新模式文案
AGENTS.md / memory             # [MODIFY] 同步（README 不动）
```

## Agent Extensions

### Skill

- **token-efficient-coding**
- Purpose: 编码全程遵循区块注释锚点、分片按需读取、Edit 局部修改、精简输出
- Expected outcome: 新增文件带 `// [block]` 区块注释；改动全部走 Edit；日志与文案精简

### SubAgent

- **code-explorer**
- Purpose: 执行阶段按需探查 chiri/mod.rs 各模式切换入口、fas_manager 调用点、affinity 分组常量等大文件细节
- Expected outcome: 精确的行级修改锚点，避免整读大文件