# devimp 日志拆分重构计划：main* 主诊断 + aff* 线程流

> 2026-09-22 · 基于 brainstorming 设计定案（职责范围/帧格式/快照粒度/改名范围/性能与压缩均已确认）

## Summary

把现有 `devimp/devimp_<pkg>_<ts>.log` 单文件诊断日志拆成两份并重设计：

1. **改名**：主诊断文件 `devimp_<pkg>_…` → **`main_<pkg>_<ts>.log`**，全链改名（函数/常量/文档/测试），**目录仍叫 `devimp/`**（归档物、记账键、webui 目录标识保留 devimp 名）。
2. **收缩**：main\_ 的 48 列 CSV 收缩到 **44 列**——`place/aff/core/tgtop` 四类线程相关行迁出/废弃后，删除其专用列。
3. **新建**：`devimp/aff_<ts>.log` 线程数据文件——**文本帧**（`@A` 动作帧 + `@S` 每秒快照帧），记录线程全部动作（**含结果 ok/errno**，补齐当前「写入失败静默」的观测缺口）与每秒进程快照（top-N 进程 + 前台树/被管进程线程下钻）。
4. **性能**：快照数据源复用 eBPF map 差分（TGID_RUN_TIME/THREAD_RUN_TIME），不扫全系统 /proc；核掩码等贵查询只对落盘行做；与 devimp 共用 `dev_record` 门控，日常零开销。

## Current State Analysis（探索结论摘要）

- **devimp 子系统**全在 [src/logger.rs](../src/logger.rs)：`[devimp_state]`(713-844) `[devrow]`(850-946) `[devimp_writer]`(963-1250) `[devimp_api]`(1256-1505) `[archive]`(1507-1928)。写入器持裸 `fs::File` 每行两次 write*all；128MB 轮转（`DEVIMP_MAX_BYTES` :820）、每 256 行巡检（:822）、保留 20 份（:818）、启动清理 `devimp_prepare`(:1257)、归档 `archive_on_startup`(:1726) 产 `logd/devimp*<ts>.tar`（无压缩）。
- **行写入点共 21 处**：tick×2（cpu_load_governor.rs:515、tuned.rs:475）、snap×1（chiri/mod.rs:1413）、place×1（affinity.rs:1625）、aff×6（affinity.rs:910/939/969/1166/1278/1696）、core×1（affinity.rs:1642）、tgtop×1（cpu_monitor.rs:481）、event×11。
- **线程写入失败全部静默**（本次要补的观测缺口）：`set_tid_affinity`(affinity.rs:412, 返回 bool, 失败调用方静默) 8 个调用点；`move_tid_group`(affinity.rs:943) 3 个调用点丢结果；uclamp 写入（affinity.rs:1823-1970）大多 `let _ =`；core_ctl `min_cpus`/`online` 写入（core_ctl.rs:202-222/466-474）结果仅内置 debug/warn。
- **数据源（重要）**：进程/线程 util 不靠 /proc 差分，而来自 eBPF map——`TGID_RUN_TIME`（全系统 TGID 累计运行 ns，cpu_monitor.rs:546-609 差分逻辑现成）、`THREAD_RUN_TIME`（cpu_monitor.rs:615-662）；tgtop 已在遍历 `tgid_run_map.keys()` 全系统 TGID（cpu_monitor.rs:464）。/proc 仅用于进程名（`proc_name` cpu_monitor.rs:518）、单进程 TID 枚举（`get_thread_tids` :62）、stat 差分（`sample_one_tid` affinity.rs:517）。**每秒快照的扫描成本因此很低**（map 遍历 + 少量 /proc 读）。
- **线程状态现成**：AffinityManager 状态表持有 home/pinned/promoted（affinity.rs threads map）；核掩码唯一查询封装在 `set_tid_affinity` 内部（affinity.rs:424-436 sched_getaffinity），需提为独立工具函数。
- **口径漂移（顺带修）**：webui `clearArchives` 注释称删 logd/+devimp/ 实际只删 logd/（sources.ts:80-90）；main.rs:103 注释写 `.zip`（实为 `.tar`）；webui README.md:58 写 `.tar.xz`（实为 `.tar.gz` 导出）；02-convention.md:49 与 main.rs:80 写「毫秒时间戳」（实为秒级 `MMDD-HHmmss` + 同秒 `-N` 去重）；03-chiri.md:165「core 行每 2 轮」（实为 `DEVL_ROW_EVERY_ROUNDS=4`）。

## Proposed Changes

### A. logger.rs：main\_ 改名 + 列收缩（what/why/how）

**what**：主诊断文件前缀 `devimp_` → `main_`；删 `place/aff/core/tgtop` 四个写入函数；48 列删 4 列。
**why**：拆分后 main\_ 只承载决策/状态/事件三类行，aff 专用列成僵尸列。
**how**：

1. `devimp_new_name`(:1006) → `main_new_name`：产 `main_<pkg_seg>_<MMDD-HHmmss>.log`（nopkg 规则、同秒 `-N` 去重不变）。
2. 删函数：`devimp_place`(:1432)、`devimp_aff`(:1448)、`devimp_core`(:1473)、`devimp_tgtop`(:1488)，及全部调用点（见 C/D 的去向）。
3. 列收缩：`DEVIMP_HEADER`(:857) 删 `util_pct`(D12)、`from_core`(D10)、`to_core`(D11)、`pinned`(D24) 四列 → 44 列；`DevRow` 改 `[String; 44]`；索引常量重排（保持其余列相对顺序，40-47 的 CPU/GPU 实际频率列语义保留）；表头注释写明「44 列，v2026-09-22 拆分版」。剩余行类型：`tick`（含 CLG/特调两来源）/`snap`/`event`（overload*hold、fas、screen、mode_change、scene*\*、thermal_change、config_reload 全部保留原语义，`reason` 列继续承载事件详情）。
4. 改名映射（函数/常量/类型）：`devimp_tick→main_tick`、`devimp_snap→main_snap`、`devimp_event→main_event`、`devimp_write_line→main_write_line`、`devimp_open→main_open`、`devimp_check→main_check`、`devimp_new_name→main_new_name`、`DevRow→MainRow`、`DevimpWriter→MainWriter`、`DEVIMP_HEADER→MAIN_HEADER`、`DEVIMP_WRITER→MAIN_WRITER`、`D_*` 列索引前缀 `DM_*`。生命周期共用函数（两文件共用）：`devimp_meta→diag_meta`、`devimp_file_head→diag_file_head`（参数化表头）、`devimp_prune→diag_prune`、`devimp_prepare→diag_prepare`、`devimp_check→diag_check`（参数化前缀）、`devimp_tick_state*→main_tick_state*`（tick 节流仅 main 用）。
5. **保留 devimp 名的边界**（目录/归档/记账派生物，不动）：`DEVIMP_DIR_REL="devimp"`(:815) 及 `archive_on_startup` 内 `root.join("devimp")`(:1735)、staging `ziped_devimp_{}`(:1786)、`logd/devimp_<ts>.tar` 产物名、`note_write(…, "devimp/", …)`(:1249)、`DEVIMP_BYTES_WRITTEN/DIR_MAX/DIR_TARGET` 计数与目录预算、i18n key `main-devimp-archive-submitted`、webui `devimpDir:'devimp'`/`devimpFiles`、`DEVIMP_ACTIVE/MODE/FG_PKG/FROZEN` 门控（改名 `diag_*` 即可：`diag_active()` 等，`set_devimp_*`→`set_diag_*`，`devimp_active()`→`diag_active()`）。
6. `diag_prepare`/`diag_prune` 过滤前缀扩为 `main_` 与 `aff_` 两种（旧 `devimp_` 前缀历史文件自然过期淘汰，不做迁移）。

### B. logger.rs：新增 aff 子系统（`[aff_writer] [aff_api]` 两个新区块）

**what**：新文件 `devimp/aff_<MMDD-HHmmss>.log`，单滚动文件不分包；帧式写入。
**why**：线程动作跨包、快照全系统视角，按前台包分文件会把流切碎。
**how**：

1. `struct AffWriter { file: Option<fs::File>, cur_name: Option<String>, since_check: u64 }`，`static AFF_WRITER: Mutex<AffWriter>`；复用 `filename_ts()`、同秒 `-N` 去重、128MB 轮转、KEEP 20、256 行巡检、`note_write(&DEVIMP_BYTES_WRITTEN, "devimp/", …)`（同目录同预算）。文件头 = aff 专用列说明注释 + `diag_meta()`（SoC/系统/模块版本元信息两文件共用）。
2. **帧格式（定案）**：帧类型首字符区分，快照帧头带行数、天然抗截断（解析器丢末帧即可）：
   - `@A` 动作帧（单行自帧）：`@A ts=<MMDD-HHmmss> act=<动作> pid=<> tid=<> pkg=<> comm=<> dst=<> value=<> result=<ok|e{errno}> reason=<>`
     `act` 枚举：`pin`（钉核/组绑定，dst=核号或 `group`）、`unpin`、`restore`（全核/组还原）、`move_group`（cpuset 迁组，dst=组名）、`cpuset_cpus`（组 cpus 批量写，dst=组名列表）、`uclamp`（dst=节点名 value=写入值）、`corectl`（dst=`min_cpus|online|reassert` value=写入值）、`bind_release`（thread_bind 关闭/清理）、`self_pin`/`self_unpin`（daemon 自身钉核）。
   - `@S` 快照帧：帧头行 `@S ts=<MMDD-HHmmss> ntop=<topN 落盘数> nfg=<下钻线程行数>`，随后 `ntop` 行进程 + `nfg` 行线程，行数与帧头计数必须一致：
     - 进程行：`p <rank> <pid> <pkg|comm> u=<整数 util%> mask=<核占用 hex> home=<核|-1>`
     - 线程行（重点下钻：前台树 + 被管进程的线程）：`t <pid> <tid> <comm> u=<整数> core=<核|-1> home=<核|-1> pin=<0|1> uclamp=<值|-1>`
   - **二进制扩展位**：帧首字符保留 `\x01` 给将来二进制 payload 帧（表头注释写明；本版不实现）。
3. 公开 API（内部并发与锁序约定沿用 02-convention 的四锁约定，扩为两写入器不嵌套持锁）：
   - `pub fn aff_action(act: &str, pid: i32, tid: i32, pkg: &str, comm: &str, dst: &str, value: &str, result: &str, reason: &str)`
   - `pub fn aff_snapshot(rows: &[String])`——行已由调用方拼好（帧头计数由行数反推，`p`/`t` 行数分别计）。
4. 门控：写入前闸门统一 `diag_active()`（`dev_record` + frozen 语义平移，aff 与 main 同门控）。

### C. affinity.rs / core_ctl.rs：动作帧接线 + 失败观测

**what**：全部线程/节点写入点接 `aff_action`，含成功与失败；`set_tid_affinity` 签名改错误可见。
**why**：这是需求核心——「线程相关所有动作 + 结果」，补上写入失败静默的观测缺口。
**how**：

1. `set_tid_affinity`(affinity.rs:412) 签名 `bool` → `std::io::Result<()>`（`Err` 携带 `io::Error::last_os_error()`，调用方格式化 `result=e{errno}`）。8 个调用点同步：`pin_core`(:894)、`unpin_core`(:938)、`restore_group_mask`(:964)、rebalance 两处(:1162/:1274)、`promote_busy_foreground`(:1692)、core_ctl `pin_self_dedicated`(:309)/`unpin_self`(:331)——**失败不再静默**，写 `aff_action(result=e{errno})` 后保持原有重试语义。
2. 动作映射（原 devimp_aff 6 处 + 无日志写入点）：
   - pin_core → `act=pin dst=<core>`（原 reason：fg_pin/home_overload/bg_busy 等保留 reason 列）；unpin_core/restore_group_mask → `act=restore dst=full`；组绑定 3 处 → `act=pin dst=group`；`promote_busy_foreground` → `act=pin dst=group reason=normal_busy`。
   - `move_tid_group` 3 处（affinity.rs:990/:1549/:1734）→ `act=move_group dst=<组名>`，result 取 `try_write_file` 结果。
   - cpuset cpus 批量写（`write_cpuset_cpus_items` affinity.rs:194、`write_cpuset_paths_items` :206，及调用它们的 `exclude_core_from_cpusets`/`restore_excluded_cpusets`/`set_groups_cpus_tracked`/`pin_background`/`restore_foreground_groups`/`lab_static_deactivate`）→ 每次批量写一条 `act=cpuset_cpus dst=<key> value=<N组>` 汇总帧（逐组明细不落，控制行量）。
   - uclamp 全部写入（`apply_uclamp_max` :1853 含回读验证、`apply_uclamp` min :1823、`write_bg_uclamp_max` :78、`restore_uclamp` :1845、`restore_uclamp_max` :1923、`restore_bg_uclamp_max` :1919、`set_boost_uclamp_override` :1941）→ `act=uclamp dst=<节点名> value=<写入值>`。
   - core_ctl（`set_power_state` min_cpus 写入 :202-222、`restore_min_cpus` :466、online 下线 :254/恢复 :369、`reassert_offline` :352、`pin_self_dedicated`/`unpin_self`）→ `act=corectl dst=min_cpus|online|reassert`；self 两处 → `act=self_pin|self_unpin`。
   - `release()`(affinity.rs:1979) 与 `cleanup_thread`(:984) 的恢复动作走上述 restore/move_group 帧 + 一条 `act=bind_release reason=<departed|gone|stale|release|disabled>` 汇总帧。
3. `sched_getaffinity` 提为 `pub fn read_tid_mask(tid: i32) -> Option<Vec<usize>>`（affinity.rs 或 utils.rs），`set_tid_affinity` 内部复用。
4. 原 `devimp_event("overload_hold", …)` 两处（affinity.rs:1239/:1402）保留——事件属 main\_，走 `main_event` 原语义。

### D. 每秒进程快照（@S 帧）：cpu_monitor 供数 + chiri 1s 块组装

**what**：每秒一帧进程快照：top-N（默认 10）进程 + 前台树/被管进程线程下钻。
**why**：需求定案（进程级 + 重点下钻；top-N 压缩体积）。
**how**：

1. **cpu_monitor.rs 新增** `pub fn snapshot_procs() -> Vec<ProcSnap>`：遍历 `tgid_run_map.keys()`（复用 :464 现成模式与 `compute_tgid_util` 差分基线，1s 窗口），产出全系统每进程 util；`struct ProcSnap { pid: u32, comm: String, util: f32 }` 定义在 common.rs。进程名复用 `proc_name`(:518)（内部去重，每进程一次 /proc 读）。原 `devimp_tgtop` 块（:456-493）整体删除（30s top-5 被每秒快照覆盖）。
2. **chiri/mod.rs 1s 块**（`devimp_snap` 现址 :1399 旁，同 `diag_active()` 门控）组装 `@S`：
   - 调 `cpu_monitor::snapshot_procs()` 取 util，排序取 top-N（N = `config.meta.devimp_top_n`，见 E）；
   - 落盘集合 = top-N ∪ 前台树（`get_thread_tids(fg_pid)` + 前台子进程；**util=0 也落盘**）∪ 被管进程（AffinityManager 状态表内 tid 所属 pid）；
   - 核掩码只对落盘进程/线程查 `read_tid_mask`（约几十次 syscall）；`home/pin/uclamp` 从 AffinityManager 状态表直取；线程 util 走 `THREAD_RUN_TIME` 差分（复用 cpu_monitor 降级路径 :615 逻辑提为 pub），无 map 条目退化 `sample_one_tid`（affinity.rs:517 提为 utils 共享）；
   - util 一律整数百分比；核占用掩码 hex（如 `mask=ff`）；行数与帧头 `ntop`/`nfg` 对齐。
3. 性能预算：map 遍历 + top 行少量 /proc 读 + 数十次 getaffinity ≈ 数 ms/s，仅 `dev_record` 开启时执行；frozen 下 `diag_active()` 为 false，快照与动作全停（与 main\_ 一致）。

### E. 配置：`devimp_top_n`

**what**：meta 段新字段 `devimp_top_n`（快照 top-N 进程数）。
**how**：`chiri/config.rs` `Meta` 结构加 `devimp_top_n: usize`（缺省 10，`normalize` clamp `1..=64`），紧邻 `dev_record` 字段(:77)；`config-example.yaml:22` 的 `dev_record` 说明同步扩写（双文件命名 `main_<pkg>_<MMDD-HHmmss>.log` / `aff_<MMDD-HHmmss>.log` + top-N 说明）；root 与 8550/8745/8475/8998 的 meta.yaml 缺省不写（走代码缺省）。

### F. webui

**what/why/how**：

1. `sources.ts:80-90` `clearArchives` 补删 `devimpDir`（顺带修「注释称删两目录只删一个」的既有漂移）。
2. `mock-shell.ts:208` 样例名改 `main_com.tencent.tmgp.sgame_0913-120000.log` + 新增 `aff_0913-120000.log` 样例（内含一帧 @A + 一帧 @S 示例）；`contract.test.ts:26` 样本同步改。
3. `paths.ts:47` `devimpDir:'devimp'` 不动（目录名保留）；LogsView devimp Panel 的 i18n 标题文案（zh/en）改为「诊断日志（main* 决策 / aff* 线程）」口径。
4. webui/README.md:58 导出口径修正（`.tar.xz`→`.tar.gz`、`devimp_`→`logd_`）。

### G. 文档同步（docs/agents + 记忆，按仓库惯例）

1. `02-convention.md` :41（开发诊断日志条重写为双文件+帧格式）、:43（ts 口径）、:45/:47（tgtop 条删除、新增 aff 帧条）、:49（「毫秒时间戳」→「MMDD-HHmmss 秒级+同秒 -N」）、:27（锁序约定扩两写入器）。
2. `03-chiri.md` :165（轮次改 4）、:77（max*util 列说明保持）、frozen 条(:137) 补 aff* 同停。
3. `main.rs:103-107` 注释（`.zip`→`.tar`、双文件口径）。
4. `config-example.yaml`（见 E）；`.codebuddy/memory/MEMORY.md` 契约数字（devimp 48 列→main* 44 列 + aff* 帧格式）与当日日志追加。

## Assumptions & Decisions（执行口径，勿再自行发挥）

1. **帧格式定案**：文本帧（`@A` 单行 / `@S` 计数块），`\x01` 保留给二进制帧，本版不实现二进制。
2. **改名边界**：「devimp」= 目录/归档/记账派生物的名字（保留）；「main*」= 主诊断文件及其函数/常量；「aff*」= 线程流文件及其函数/常量。旧 `devimp_*.log` 不迁移，靠 KEEP 20 自然淘汰。
3. **快照生成位置**：chiri/mod.rs 1s 块组装（能直取 AffinityManager 状态），cpu_monitor 只负责供 `snapshot_procs()` 数据。
4. **top-N 默认 10**，clamp 1..=64；落盘集合恒含前台树与被管进程（不受 N 截断）。
5. **main\_ 44 列**：删 util_pct/from_core/to_core/pinned；其余列相对顺序不变。
6. **失败语义**：写入失败一律落 `@A result=e{errno}`，保持原有重试/静默降级行为不变（只加观测，不改控制流）。
7. **frozen 下 affinity rebalance 无显式短路**是既有行为（探索发现与 03-chiri 文档表述有出入），**本任务不改**，仅在文档注记现状。
8. util*smoothing 相关（max_util 写 raw）不受影响；`event` 事件帧继续留在 main*。

## Verification

1. `cargo +nightly check -p chiri --target aarch64-linux-android` 0 警告（改名/删函数的全部调用点闭环）。
2. webui：`npx vitest run`（契约测试含新文件名样本）+ `npx svelte-check` 0 错。
3. 真机冻结诊断（dev_record=true）录一段游戏 + 切后台：
   - `devimp/` 下生成 `main_<pkg>_….log`（44 列表头、tick/snap/event 行正常）与 `aff_….log`（@A/@S 帧、`@S` 帧头计数与实际行数一致）；
   - @S 集合 = top-10 + 前台树 + 被管进程，idle 长尾不落盘；util 整数、mask hex；
   - 注入失败（如对已退出 tid 触发钉核）得到 `result=e{errno}` 行；uclamp/corectl 动作均有 @A 行且 result 正确；
   - 归档后 `logd/devimp_<ts>.tar` 含 main*+aff* 双文件；128MB 轮转与 KEEP 20 对 aff\_ 同样生效。
4. 离线解析冒烟：按帧格式说明手写/更新解析脚本读 aff\_ 样本（devimpbin 老 analyze 脚本不改，日志注明口径变化）。

## 额外注意事项

1. 注意性能开销
2. 不能影响现有的任何业务，日志本身是独立的块
3. 写完之后审查逻辑，确保没有引入新的问题
