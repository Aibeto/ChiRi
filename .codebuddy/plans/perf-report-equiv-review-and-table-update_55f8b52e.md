---
name: perf-report-equiv-review-and-table-update
overview: 对已实施的 10 项性能优化做逐项等价性复核（确认逻辑与算法结果一致），并把复核结论与完成状态填进报告总表第 50-74 行的「已完成」列、新增等价性复核小节。
todos:
  - id: fill-table
    content: 补齐总表第 50-74 行「完成」列：10 项填「已完成（等价）」并加备注（U3 注仅 CLG 侧、A2 注改用 Arc 快照）、B3 填「降级未做」、A5b 填「跳过」、K3/U1/U2/A3/A7/B2/C1/C3/C4/C5 填「未做」；同时把 K1 行影响判定改为「eBPF 产物编译期嵌入 daemon，两侧天然同批」
    status: completed
  - id: add-equiv
    content: 用 [skill:token-efficient-coding] 新增「[equiv] 等价性复核」一节（放在 [impl] 实施记录之前），含 10 项逐项结论表、2 条口径更正、1 条边界差异与「未实测」声明
    status: completed
    dependencies:
      - fill-table
  - id: sync-wording
    content: 同步 [kernel] 与 [impl] 节的 K1 口径：删掉「需要 bytemuck::Pod / Zeroable」与「跨二进制契约需同步」的旧说法，改为两侧天然同批与实际采用的 aya 内置 Pod
    status: completed
    dependencies:
      - add-equiv
  - id: verify-doc
    content: 校验表格列数与分隔行一致、两处口径无旧表述残留、git diff 仅限该报告文件
    status: completed
    dependencies:
      - sync-wording
---

## 需求

1. **等价性复核**：检查上一轮落地的 10 项优化（K1、K2、U3、A1、A2、A4、A5、A6、B1、C2）改动前后的逻辑与算法结果是否一致。
2. **更新报告**：修改 `e:\code\ChiRi\.codebuddy\docs\2026-09-20-rust-perf-report-final.md` 第 50-74 行的「优化项总表」（表头已有「完成」列，数据行为空占位 `[]`），并同步相关节的口径。

## 复核结论（已完成，可直接采信）

逐条回读改后源码，**10 项全部等价，未发现需要修正的逻辑**：

| 项 | 结论 | 关键证据 |
| --- | --- | --- |
| K1 | 等价 | 探针 `state.last_time/idle/busy/cur_tid/cur_tgid` 与原 5 个 map 一一对应，写序一致；用户态取值元组 `(s.idle, s.busy, s.last_time, s.cur_tid)` 与原来的 raw_idle / raw_busy / last_switch_time / current_tid 逐一对应（cpu_monitor.rs:291-295） |
| K2 | 等价 | 开关与 `FAS_FG_UTIL_ENABLED` 同源同生命周期（均在 start_cpu_loop 启动时定值），「降级路径可能被调用」等价于「记账已开」；旧产物缺 map 时只告警并保持恒记账（等于原行为）。净开销：1 次 Array 查找换掉 1 次 32768 项 hash 查找 |
| U3 | 等价 | `Arc::get_mut` 成功即无其它持有者，原地 `copy_from_slice` 不可能被观测；长度变化或仍被持有则新建。两种路径下 Worker 收到的内容与长度与原实现一致（cpu_load_governor.rs:952-973） |
| A1 | 等价 | `.as_str() == "fas"` 与 clone 后比较同值；临时 MutexGuard 仍在 if 条件求值结束时 drop，锁范围不变 |
| A2 | 等价 | `get_current_package()` 由 clone 改为 to_string（同内容）；`set_current_package` 由 to_string 改为 `Arc::from`；1s 块内 `cur_pkg: &str` 经解引用后比较与传参语义不变；`active_pkg().map(str::to_string)` 保留（后续 `activate(&mut)` 借用所需）；快照生命周期覆盖整个 else-if 块 |
| A4 | 等价 | `set_tid_affinity(tid, cpu_ids: &amp;[usize])` 形参本就是切片，被删的 clone / to_vec 只是复制品 |
| A5 | 等价 | `orig_group: &amp;'static str` 取值仍是那 4 个常量；`bg` 直接借用 BACKGROUND_GROUPS 常量字面量；写入 cpuset 的路径字符串内容不变 |
| A6 | 无语义 | 纯优化器提示 |
| B1 | 等价 | 两个消费方本就忽略 payload，`Box::new` 只改存放位置，发送的仍是同一份配置 |
| C2 | 等价 | tokio 仅裁剪未用 feature；被删的 4 个依赖无任何 use；LazyLock / OnceLock 与 once_cell 的 Lazy / OnceCell 同语义（set 返回值与 Deref 一致），编译通过且无 unused 警告 |


**复核中确认的两条口径更正**（原报告表述有误，需一并改掉）：

- K1 原写「跨二进制契约需同步」不成立：eBPF 产物由 build.rs 编译后经 `include_bytes!` **嵌入 daemon 二进制**，两侧天然同批发布，不存在版本错配。
- 「kernel」节写「需要 `bytemuck::Pod` / `Zeroable`」不准确：用户态用的是 aya 0.14 自带的 `unsafe trait Pod`（aya-0.14.0/src/bpf.rs:45），内核侧 aya-ebpf 0.2.1 的 `PerCpuArray` 无 trait 约束，实装未新增任何依赖。

**如实记录的唯一边界差异**：`CORE_STATE.get_ptr_mut` 失败时探针提前返回（原实现会继续尝试写 3 个 percpu map）。per-cpu array 查找失败在正常内核不可达，且新实现下这三次写本属同一结构，无可观测差异。

## 本轮边界

只改报告文档，不触碰源码、不重跑构建。真机行为与性能 A/B 仍未做（需 CI 产物与 8550 实机）。

# Agent Extensions

- **token-efficient-coding**
- Purpose: 编辑长文档时按行锚点做局部替换、避免整文件重写、精简汇报
- Expected outcome: 报告只发生目标段落的最小 diff，其余内容零改动