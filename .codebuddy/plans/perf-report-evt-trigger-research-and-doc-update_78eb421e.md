---
name: perf-report-evt-trigger-research-and-doc-update
overview: 把 Android 可用事件触发器的调研结论写成报告新节 [evt]（含现状监听点清单、候选触发器分级、可落地的零依赖优化），并同步报告中 U1/C1/U3 的状态表述与新增候选项。
todos:
  - id: evt-section
    content: 用 [skill:token-efficient-coding] 在报告新增 [evt] 节：现状监听点清单与 B1/B2/B3 分级
    status: completed
  - id: table-update
    content: 总表增补 E1-E4 候选项，并修正 U1/C1/U3 行为真实状态备注
    status: completed
    dependencies:
      - evt-section
  - id: pending-section
    content: 用 [skill:m10-performance] 新增 [pending] 节落档 U1/C1 方案与 U3 量化结论
    status: completed
    dependencies:
      - evt-section
  - id: memory-sync
    content: 追加当日 memory 调研记录，并校验报告状态与仓库真实一致
    status: completed
    dependencies:
      - table-update
      - pending-section
---

## 需求

在已完成 10 项优化、且 U1/C1 实施方案已定稿（尚未落地）的背景下，完成两件事：

1. **更新报告** `e:\code\ChiRi\.codebuddy\docs\2026-09-20-rust-perf-report-final.md`：把本轮调研结论写入文档，并把 U1/C1/U3 的状态如实化（不得写成已完成），同时把已定稿但未实施的方案落档，避免方案只存在于计划文件里。
2. **调研 Android 可用的事件触发器**，目标是进一步降低监听开销：盘点现有监听点、给出候选事件源与分级结论、标注需真机验证的项。

## 交付内容

- 报告新增「Android 事件触发器调研」一节：现状监听点清单表 + 候选触发器三级分类（可低成本扩展 / 零依赖纯优化 / 调研过但不适用）。
- 优化项总表增补 E1-E4 候选项，并修正 U1、C1、U3 行的状态备注。
- 报告新增「已定稿待实施」小节：U1（常驻句柄 + 8KB 低频巡检）、C1（current_thread + features 收窄）、U3（cpu_monitor 侧不做的量化理由）。
- 当日 memory 追加本轮调研记录。

## 核心结论（写入报告）

- 监听层主体**已经事件化**：屏幕走 `NETLINK_KOBJECT_UEVENT`、配置与 DOWN 与实验室走 inotify ×3、帧事件走 RingBuf + mio。剩余周期轮询只有 `pid_watcher`(500ms)、`app_detection_loop`(亮屏 1.5s / 息屏 1s)、`telemetry`(1s)、`cpu_monitor`(160ms/40ms) 与调度循环的 1s/2s/5s 兜底块；后两者受「status.csv 1s 契约」与「eBPF 采样本质」约束，不宜改。
- **最推荐先做（零新依赖、收益确定）**：app_detect 每轮先读 cgroup `tasks` 原文与上轮做字符串比对，未变则复用上轮判定、跳过全部 `/proc/<pid>/cmdline` 读取，稳态把 1~1.5s 一轮的 N 次读取降为 1 次。
- 可低成本扩展：uevent 扩展监听 `cpu`（hotplug，收益偏响应性）与 `power_supply`（充电状态变化，收益很小）。
- 调研后判定价值有限或不适用：cgroup v2 `cgroup.events` 的 poll（只看 `populated` 0↔1，无法感知同组内 A→B 前台切换）、logd `/dev/socket/logdr` 订阅 events buffer（延迟最低但需实现 liblog 协议并验证 SELinux 可达性，列研究项）、PSI 的 poll（与 1s 采样契约冲突）、thermal uevent（广播依赖驱动，不可靠）、timerfd + epoll 全量合并（改造面大，收益主要是线程数）。

## 技术方案

### 任务性质

本轮是**调研 + 文档更新**，不改任何源码；报告必须与仓库真实状态一致（U1/C1 未实施，只能标「已定稿待实施」）。

### 调研方法

以「现状盘点 → 事件源候选 → 适配度分级 → 标注待验证项」四步推进，全部结论基于源码事实与内核机制常识，不使用未经核实的技术断言：

1. **现状盘点**：逐个线程核对触发方式与周期，来源为 `src/monitor/mod.rs`（screen_watcher / config_watcher / fps_monitor_ebpf / cpu_monitor_ebpf / telemetry_monitor / pid_watcher / app_detection_loop）、`src/monitor/screen_detect.rs`（netlink uevent 已覆盖 power / backlight / leds，含多源投票与退役切换）、`src/monitor/app_detect.rs`（cgroup tasks + `/proc/<pid>/cmdline` 倒序扫描，通常 1~2 次读即命中）、`src/chiri/mod.rs` 的 1s / 2s / 5s 周期块。
2. **候选事件源**：只考虑 Android 8+ 上真实存在、且有现成基础设施或明确实现路径的机制（kobject uevent、inotify、RingBuf/mio、logd reader socket、cgroup v2 events、PSI poll）。
3. **分级**：按「收益确定性 + 改动面 + 风险」分为三档写入报告，避免后人重复尝试已否决的方案。

### 关键决策

- **E1（app_detect tasks 内容比对短路）定为最优先**：零新依赖、行为等价（内容相同即映射到同一前台进程，进程被替换的极端情形由下一轮变化检测覆盖）、稳态把每轮 N 次 `/proc` 读取降为 1 次 cgroup tasks 读。
- **E2（uevent 扩展）如实标注收益量级**：CPU hotplug 主要是响应性提升（在线核位图从 ≤8s 缩短到即时），power_supply 的开销节省在微秒级每秒，不要夸大成「显著降开销」。
- **E3（logd events buffer）只作为研究项**：需先真机验证 `/dev/socket/logdr` 可达性与 tag 名版本差异，再决定是否投入 150-250 行的协议实现。
- **E4（pid_watcher 并入 app_detect）默认不做**：会换取前台 PID 广播时延从 ≤500ms 变成 ≤1.5s，属需要用户拍板的取舍，只记录不实施。
- **明确否决项要留痕**：cgroup v2 `cgroup.events`、PSI poll、thermal uevent、timerfd+epoll 全量合并各自说明原因，防止后续重复评估。

### 文档结构

- 新增 `## [evt] Android 事件触发器调研（2026-09-20）`：现状清单表 + 三级分类（B1 可扩展 / B2 零依赖优先 / B3 不适用）+ 每项收益与成本。
- 总表 `[table]` 增补 E1-E4（编号避开已有的 K/U/A/B/C），并修正 U1、C1、U3 行的备注。
- 新增 `## [pending] 已定稿待实施`：U1 的常驻句柄 + 8KB 低频巡检 + 四条验收场景（外部删文件、删整个目录、写满轮转、`update_level` 重建 appender）与有意权衡；C1 的 `Builder::new_current_thread().enable_time().build()` 与 tokio features 收窄；U3 的三条替代路径量化结论。
- `.codebuddy/memory/2026-09-20.md` 追加本轮记录（现状清单结论、E1-E4、四个不适用项）。

### 约束与验证

- 只写文档，不动源码；U1/C1 的代码实施与验证仍按既有计划另行执行。
- 收益表述需可核对，不夸大；凡属「需真机验证」的结论（logd socket 可达性、uevent 子系统实际覆盖、CPU hotplug uevent 是否广播）必须显式标注。
- 校验点：表格列数与分隔行一致；报告内不出现与仓库真实状态矛盾的「已完成」标记；文案无句尾句号、避免破折号堆砌。

## Agent Extensions

### Skill

- **token-efficient-coding**
- Purpose: 报告是长文档，更新时按小节锚点做局部替换，避免整文件重写
- Expected outcome: 报告只发生目标段落的最小 diff，其余内容零改动
- **m10-performance**
- Purpose: 校核调研结论中的收益与开销表述是否成立、是否夸大（尤其 E1 的「N 次读取降为 1 次」与 E2 的「收益偏响应性」）
- Expected outcome: 报告中每项触发器的收益量级都有可核对的来源与限定条件