---
name: daemon-perf-opt
date: 2026-09-23
overview: 分四个阶段降低 chiri 守护进程自身的唤醒次数与 sysfs 往返，降低功耗足迹与对调度的自干扰；不改 FAS/CLG 决策语义。
todos:
  - id: p1-fast-reader
    content: Phase 1：utils.rs 新增 FastReader（keep-open+重试），替换 tick 路径的 read_to_string/format!
    status: completed
  - id: p2-write-dwell
    content: Phase 2：CLG/tuned 写频加 dwell+死区（hold 已落地，直接叠加）
    status: completed
  - id: p3-event-driven
    content: Phase 3：逐文件审计 sleep 轮询，有事件源的改 uevent/事件驱动，其余核实间隔
    status: completed
  - id: p4-misc
    content: Phase 4：profile 微调（codegen-units=1）+ 白名单精确条目 HashMap + 同步 docs/agents 与 memory 日志
    status: completed
---

## 背景与目标

优化守护进程自身的功耗足迹与抖动（唤醒次数、sysfs 往返），不动 FAS/CLG 决策语义。换语言（C/Go）已评估无收益，本计划是算法/结构层的改进。

## Phase 1 — sysfs 读侧减负

发现：每 tick 大量 `fs::read_to_string` + `format!` 现拼路径（如 [src/chiri/cpu_load_governor.rs](../../src/chiri/cpu_load_governor.rs) :759 防篡改重读、[src/chiri/affinity.rs](../../src/chiri/affinity.rs) :833 uclamp 读、[src/chiri/core_ctl.rs](../../src/chiri/core_ctl.rs) :422/:474），每次都是路径分配 + String 分配 + open/close。

- 在 [src/utils.rs](../../src/utils.rs) 的 [fast_writer] 区块旁新增 `FastReader`：镜像 FastWriter 的 keep-open File + seek(0) 读入可复用 buf，提供 `read_u32() -> Option<u32>`（trim+parse 不经 String 分配）；只读**稳定路径的整型节点**用它。
- keep-open 只收常驻节点（cpufreq policy 的 scaling_*、uclamp 等——policy 节点不随 core_ctl 核心上下线消失）；per-pid /proc 读不入 keep-open（进程退出节点即失效），保持每次 open。读错误丢弃 fd、下次重开。
- 读取三态（正常空值 / 合法缺失 / 读取失败）语义不变；:759 防篡改重读是内容比较点，保留原文比较、不换 `read_u32` 归一化。
- 高频读节点的路径字符串提升到初始化期缓存（对齐 FastWriter 已有做法），删除 tick 路径上的 `format!`。
- 实际落点（2026-09-24 执行核正）：core_ctl tick 读、affinity uclamp 快照读。计划原列三处均不成立、未改：CLG「:759 防篡改重读」实为加载期 `read_affected_cpus` 冷路径（flush 只写不读，防篡改是盲重写）；affinity uclamp 实为接管快照（一次性，非 tick）；cpu_monitor 无稳定节点文件读（负载全来自 eBPF map，仅剩 per-pid 读）。
- 冷路径（启动快照、rhine/down 状态文件）不动。

## Phase 2 — 写频决策层滞回（dwell + 死区）

发现：[src/chiri/cpu_load_governor.rs](../../src/chiri/cpu_load_governor.rs) :376 注释已指出 target_perf 翻摆导致 scaling_max_freq 高频改写（核正：FastWriter 现已无值去重，仅 `write_value_force`，抖动直接落盘）。

- 前置已解除：`on_load_update` 的 hold 三分支已落地（:445-448），dwell 直接叠加，不重复改该函数。
- CLG/tuned 写频入口加最小驻留时间（dwell，可配置、缺省如 2 个 tick）与死区（|target−current| 小于阈值不写）。
- 豁免（不损失响应性）：触摸升频、立即降频、模式接管/释放（fast 锁频、CLG reload）、防篡改 force 重写。
- 热保护 clamp **不豁免、不定义为安全事件**：过热有内核温控与系统服务兜底，走正常 dwell 即可；若豁免，温度毛刺会绕过滞回直写频率，反而造成策略频繁调整。
- 验证：devimp tick 行观察 scaling_max 写次数下降、decision 语义不变。
- 执行结论（2026-09-24，未编译/未真机验证）：CLG `write_freq` 加滞回 gate（dwell 缺省 320ms + 死区 write_deadzone=0.03，config.rs 可配、0=关闭），tuned `gated_write` 同口径（80ms，沿用既有 hysteresis 比较式）。豁免落地：触摸提频（floor 升向）、极低负载立即降频（fast_down 降向）、接管/释放（spawn 初写/restore_policy/reload/fast.rs 不经 gate）、写失败补写（last_failed 下次豁免）。热保护 clamp 零豁免代码。decision 标签赋值一行未动。核正两处：「防篡改 force 重写」无独立条目——flush 单一 write_freq 调用，且 write_freq 值去重使同值重写为 no-op（注释宣称的防篡改重写实际不生效，现状维持、未凭空新增 force 写）；tuned.rs:85 注释「120ms」为陈旧数字，dwell 按 160/40ms tick 口径取 320/80ms。

## Phase 3 — 轮询改事件驱动（逐文件审计，行为改动需真机验证）

- 逐个审计 sleep 轮询循环（[src/monitor/app_detect.rs](../../src/monitor/app_detect.rs) 100ms/1s、[src/monitor/telemetry.rs](../../src/monitor/telemetry.rs) 1s 电池功率、[src/monitor/screen_detect.rs](../../src/monitor/screen_detect.rs) 100ms verify、[src/chiri/touch_detect.rs](../../src/chiri/touch_detect.rs) 500ms）。
- 有事件源的换事件驱动：电池/功率走 power_supply uevent（依赖已有 kobject-uevent）；screen 的 verify 循环确认是否只在事件后短暂运行（是则不动）；touch_detect 的 sleep 若只是断线退避则不动。
- 无事件源的保留定时器，但核实间隔是否可放宽（app_detect 100ms 是否必要）。
- 原则：改一个验证一个，行为回归以 devimp/daemon.log 对照。
- 执行结论（2026-09-24，未编译/未真机验证）：四文件审计后仅改 touch_detect.rs——poll 超时 200ms→-1 消灭 5 次/秒空唤醒 + POLLHUP/ERR/NVAL 异常位补断线感知。telemetry.rs 不动（PSI/GPU 无事件源，1s 节拍必须保留；事件化会打破 PowerAVG 等间隔加权窗口，取样契约不变）；app_detect.rs 不动（前台切换无可靠事件源，1500ms/1s/100ms 均绑定切换延迟或 verify 自愈节拍）；screen_detect.rs 不动（sleep 均为事件后沉降/错误退避，非轮询）。附带发现：touch_detect 外层注释称设备增删重枚举但热插拔实际不感知（需 inotify /dev/input），属行为增强未做。

## Phase 4 — 杂项收尾

- [Cargo.toml](../../Cargo.toml) [profile.release] 的 `codegen-units = 1` **本已存在**（:64），无需补；opt-level 维持 "z"。**不加** `panic = "abort"`（已确认未加）：`spawn_guarded` 的 `catch_unwind` 与看门狗重启契约依赖 unwind。
- [src/common.rs](../../src/common.rs) :393 起的白名单精确条目改 HashMap 查找（regex 条目已一次编译、保持线性；收益小，顺手做）。
- 按仓库规约同步 `docs/agents/` 与 `.cursor/memory/` 当日日志。

## 验证口径

- 每阶段：`cargo check -p chiri --target aarch64-linux-android`；eBPF 检查按 MEMORY 口径（本机只信「⚠ 跳过 eBPF 编译」）。
- 效果对照：真机 `/proc/<pid>/status` 线程数与 chiri CPU 时间（devimp/PowerAVG.chr 日志）前后对比；行为不回归以 daemon.log + status.csv 对照。
- Phase 2/3 涉及决策/判定语义，需真机日志验证后再进下一阶段。

## 明确不做

- 不换语言（C/Go）：已评估无性能收益。
- 不动 eBPF 探针：内核侧开销与语言无关。
- 不做双进程/IPC 拆分。
