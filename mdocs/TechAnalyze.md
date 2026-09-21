# ChiRi 调度技术与算法披露

> 编写日期：2026-09-19
> 代码基线：本仓库当前工作区（`module/module.prop` 版本 `Alpha03-Canary84`，`Cargo.toml` 版本 `2.0.2`）
> 文档性质：源码审查记录，含算法与参数的完整披露

---

## 0. 文档说明

### 0.1 范围

分析对象为 ChiRi 仓库中除 Yumi 调度本体之外的全部代码，具体范围：

| 范围                                       | 说明                                                                                                                             |
| ------------------------------------------ | -------------------------------------------------------------------------------------------------------------------------------- |
| `src/chiri/`                               | ChiRi 调度主线，14 个文件                                                                                                        |
| `src/monitor/`                             | 监控层，7 个文件                                                                                                                 |
| `src/scheduler/fas/`                       | FAS 帧感知引擎。位于 `scheduler/` 目录下，但由 ChiRi 调用，属 ChiRi 功能                                                         |
| `src/` 根目录                              | `main.rs`、`common.rs`、`logger.rs`、`rhine.rs`、`notify.rs`、`utils.rs`、`i18n.rs`、`down.rs`、`fas_types.rs`、`webui_asset.rs` |
| `yumi-ebpf/src/main.rs`                    | eBPF 探针。命名含 yumi，但内部含有 ChiRi 全部采集数据的来源                                                                      |
| `module/`                                  | 安装脚本、开机脚本、全部配置                                                                                                     |
| `webui/`                                   | Svelte 5 管理界面                                                                                                                |
| `analyze/`                                 | ChiRi CSV 分析台                                                                                                                 |
| `build.rs`、`xtask/`、`.github/workflows/` | 构建与 CI                                                                                                                        |

`src/scheduler/` 下的 Yumi 调度相关代码不纳入范围：`cpu_load_governor.rs`、`scheduler/config.rs` 的 Yumi 分支、以及 `scheduler/mod.rs` 的 Yumi 接线。该部分在项目中处于备用回退状态，仅在需要对比时引用（例如其 `smoothing_down` 系列参数）。

完整覆盖清单与未覆盖项的原因见 12.6 节。

### 0.2 算法披露的表述约定

本文所指的「算法披露」，是把实际运行于设备上的判定条件、系数、阈值、状态机按原样记录，不作简化或修饰。因此正文包含大量精确数值与直接摘自源码的公式。

文中对以下三类情况均有标注：

- **存在但无生效路径的配置项**（如 `CoreCtl.scenemode_offline`）：标注为无实际作用。
- **代码存在但被常量关闭的功能**（如触摸升频 `TOUCH_BOOST_SUSPENDED = true`）：标注为「当前停用」。

### 0.3 文档结构

第 1 章给出后文涉及的术语定义，不涉及 ChiRi 自身实现。第 2 章至第 4 章描述系统架构、运行骨架与监控层。第 5 章为调度算法主体，按控制频率的机制逐项展开。第 6 章至第 10 章为配套机制与外围组件。第 11 章汇总代码审查中发现的问题与风险。第 12 章为附录，含常量速查、术语对照、文件清单与许可信息。

---

## 1. 背景知识：Android 调频到底在调什么

本章给出后文涉及的平台术语定义，不涉及 ChiRi 自身实现。术语与后文用法一致。

### 1.1 CPU 频率与 OPP

移动 SoC 的每个核心可运行于多个离散频率点，称为 OPP（Operating Performance Point）。频率越高，算力与功耗、发热同步上升。某个核心支持的频率集合由内核通过 `/sys/devices/system/cpu/cpufreq/policy<N>/scaling_available_frequencies` 暴露。

调度的基本动作是：**依据当前负载，确定频率上限所在的档位**。

### 1.2 簇、policy 与 governor

核心按能力分为三类：

- **little（小核）**：能效优先，算力最弱
- **big（大核）**：算力与功耗居中
- **prime（超大核）**：算力最强，功耗最高

共享同一套频率控制的核心构成一个 **cpufreq policy**。内核为每个 policy 暴露目录 `/sys/devices/system/cpu/cpufreq/policy<N>/`：

| 节点                                    | 含义                       |
| --------------------------------------- | -------------------------- |
| `scaling_min_freq` / `scaling_max_freq` | 允许运行的频率下限 / 上限  |
| `scaling_cur_freq`                      | 当前实际频率（只读）       |
| `scaling_available_frequencies`         | 该 policy 支持的全部频率点 |
| `scaling_governor`                      | 当前接管的调速器           |

**governor（调速器）** 是在该区间内决定取频策略的内核模块，常见取值举例：

- `performance`：固定取 `scaling_max_freq`
- `schedutil`：依据调度器负载预测动态取频，为现代 Android 默认值
- `powersave`：固定取 `scaling_min_freq`

ChiRi 的频率控制方式：**仅修改 `scaling_max_freq` 上限，由 schedutil 在「硬件最低频 ~ 新上限」区间内自主取频**，调度器只约束可选范围，不做逐时刻决策。该设计贯穿全部模式。

### 1.3 util

内核调度器（EAS，Energy Aware Scheduling）为每个任务计算 **util**，取值 0 至 1，表示任务对 CPU 的占用程度。schedutil 依据该值查表得到目标频率。

ChiRi 另行计算一份 util（`src/monitor/cpu_monitor.rs`），数据源为 eBPF：内核暴露的 util 存在滞后与钳制，不足以驱动 ChiRi 的决策。

### 1.4 cpuset

`/dev/cpuset/` 下划分若干组，每组限定其成员进程可运行的核心范围。Android 常用分组：

| cpuset 组           | 典型内容           |
| ------------------- | ------------------ |
| `top-app`           | 当前前台应用       |
| `foreground`        | 可见但不一定在最前 |
| `background`        | 后台应用           |
| `system-background` | 系统后台           |
| `restricted`        | 受限后台           |

将进程 PID 写入某组的 `cgroup.procs` 文件即完成归组。ChiRi 通过该机制在 boost 模式下把前台收窄至大核与超大核。

### 1.5 uclamp

cpuset 为硬约束（限定可用核心集合），uclamp 为软性诉求（表达性能倾向）。涉及两个参数：

- `cpu.uclamp.min`：抬升诉求，使 EAS 倾向分配高频或大核
- `cpu.uclamp.max`：压制诉求，使 EAS 倾向分配低频或小核

ChiRi 使用 `uclamp.max` 压制后台组（`background`、`restricted`），降低后台任务抢占大核与高频的倾向，但**不禁止**其使用（空闲时仍可被调度至大核）。

### 1.6 core_ctl

内核（仅在 Qualcomm 内核测试）提供 `core_ctl` 模块，节点路径为 `/sys/devices/system/cpu/cpu<N>/core_ctl/`，用于控制每个簇保持在线的最少核心数（`min_cpus`）。将该值设为簇大小则整簇常在线；设为较小值则允许内核在轻载时下线部分核心以降低漏电流。

ChiRi 在 boost 模式下将大核簇与超大核簇设为整簇常在线。息屏场景模式原设计包含下线 prime 簇，但该路径在当前版本中已废弃（见 5.6 节）。

### 1.7 eBPF

eBPF 允许在不编写内核模块、不重编译内核的前提下，向内核指定位置挂载程序。挂载点分两类：

- **tracepoint**：内核预埋的静态钩子，例如 `sched_switch`（每次任务切换触发）
- **uprobe**：用户态函数钩子，例如挂载于 `libgui.so` 的 `Surface::queueBuffer`，每次提交帧时触发

ChiRi 使用 `sched_switch` 采集各核心忙闲时间，使用 `queueBuffer` 采集帧间隔。数据经 RingBuf 或 map 回传用户态。

### 1.8 其它术语

| 名词                  | 说明                                                                                  |
| --------------------- | ------------------------------------------------------------------------------------- |
| **PSI**               | Pressure Stall Information，`/proc/pressure/*` 节点，表示 CPU/IO/内存的停滞压力百分比 |
| **devfreq**           | 设备频率框架，GPU 调频使用，节点位于 `/sys/class/devfreq/`                            |
| **jank**              | 卡顿，指单帧耗时显著超出预算，可被用户感知                                            |
| **vsync**             | 屏幕垂直同步信号，60Hz 屏为 16.67ms 一次，144Hz 屏为 6.94ms 一次                      |
| **帧预算**            | 单帧允许的最长耗时，60fps 为 16.67ms，120fps 为 8.33ms                                |
| **PID 控制器**        | 闭环控制算法，以误差的比例项(P)、积分项(I)、微分项(D) 合成输出                        |
| **hysteresis**        | 人为的困境，用于抑制阈值边缘的反复切换                                                |
| **EMA**               | 指数移动平均，平滑滤波，`新值 = α × 本次采样 + (1-α) × 上次平滑值`                    |
| **Magisk / KernelSU** | Android 常见 Root 方案                                                                |
| **flock**             | 文件锁                                                                                |
| **inotify**           | Linux 文件变更通知                                                                    |

---

## 2. 项目总览

### 2.1 它是什么

ChiRi 是一个 Android CPU 调度模块，以 Magisk / KernelSU 模块的形式安装。装好之后，设备上会多出一个 Rust 守护进程（二进制名 `chiri`）常驻后台，它做三件事：

1. **采集**：通过 eBPF 拿内核级的 CPU 和帧数据
2. **决策**：按当前模式策略算出每个核心组的频率上限
3. **执行**：把结果写进 sysfs，同时管理线程摆放、核心上下线、GPU 锁频

其定位为：以日用流畅度为主，其次轻度游戏，低功耗不是首要目标。

### 2.2 两套调度并存

仓库里有两套调度实现，靠 SoC 型号二选一：

```
main.rs
  ├─ common::is_chiri_soc() == true  → chiri::start_scheduler_thread()   ← 主线
  └─ common::is_chiri_soc() == false → scheduler::start_scheduler_thread() ← Yumi 回退
```

判定逻辑在 `src/common.rs`：

```rust
const CHIRI_SOC_HINTS: [&str; 3] = ["8550", "8475", "8998"];
```

`is_chiri_soc()` 不是看磁盘目录，而是把 7 个来源拼成一个小写字符串做子串匹配：

- `/sys/devices/soc0/machine`
- `/sys/devices/soc0/plat_name`
- `getprop ro.soc.model`
- `getprop ro.board.platform`
- `getprop ro.product.board`
- `getprop ro.hardware`
- `/proc/cpuinfo`

匹配时要求片段长度至少 3 个字符，命中任一即算。结果用 `OnceLock` 缓存，只计算一次。

注意 部分 SoC 无 prime 核心，对这种情况有专门处理：`prime_pool` 退化成 big。

### 2.3 目录结构

```
src/
  main.rs            启动入口，进程级编排
  common.rs          公共工具：SoC 判定、配置嵌入、白名单解析
  logger.rs          日志系统（daemon.log / status.csv / devimp）
  rhine.rs           实验室模式
  notify.rs          系统通知
  utils.rs           文件读写、目录监听、FastWriter
  i18n.rs            Fluent 多语言
  down.rs            DOWN 模式
  fas_types.rs       FAS 配置类型
  webui_asset.rs     WebUI 资源还原
  monitor/           监控层（两套调度共用）
  chiri/             ChiRi 调度主线
  scheduler/         Yumi 调度 + fas/（ChiRi 修改后复用）
yumi-ebpf/           eBPF 探针
module/              Magisk 模块载体
webui/               Svelte 管理界面
analyze/             ChiRi CSV 分析台
xtask/               构建打包
```

### 2.4 数据流总览

完整数据链路按「从内核到频率」的顺序如下：

```
内核层
  sched_switch tracepoint ─┐
  queueBuffer uprobe ──────┤
                           ↓
                    eBPF map / RingBuf
                           ↓
监控层（monitor/）
  cpu_monitor   → SystemLoadUpdate { core_utils, foreground_max_util }
  fps_monitor   → FrameUpdate { frame_delta_ns }
  app_detect    → ModeChange / PackageSwitch
  screen_detect → ScreenStateChange
  telemetry     → 进程共享原子量（不走事件通道）
                           ↓
            有界事件通道 sync_channel(64)
                           ↓
决策层（chiri/ 或 scheduler/）
  CLG 负载调速器 / 特调 / FAS / fast_lock
                           ↓
执行层
  sysfs 写 scaling_max_freq
  cpuset 写 cgroup.procs
  core_ctl 写 min_cpus
  devfreq 写 GPU 频率
```

监控层为两套调度共用同一文件，但所需采样数据不同，`src/monitor/mod.rs` 的注释对此有明确说明。ChiRi 与 Yumi 的唯一区别在于消费同一事件通道的消费者不同。

---

## 3. 运行骨架

本章描述守护进程从拉起、进入主循环、到崩溃后恢复的完整流程。

### 3.1 启动顺序

`main.rs` 的执行顺序存在硬性依赖，源码中多处注释标注「必须在 …… 之前」。实际顺序如下：

**第 1 步：脱离父进程**

先 `prctl(PR_SET_PDEATHSIG, 0)` 清除父进程退出信号，再 `setsid()` 建立新会话，最后把标准输入输出重定向到 `/dev/null`。目的是切断与管理器 su 会话的管道连接，否则管理器退出时守护进程会收到 SIGHUP 而终止。

**第 2 步：切换工作目录**

从命令行参数取一个路径 `chdir` 过去。

**第 3 步：抢文件锁**

打开 `<模块根>/daemon.lock`，用 `flock(LOCK_EX | LOCK_NB)` 尝试独占加锁。失败时输出一行信息并 `exit(0)`，由 WatchDog 在 3 秒后重试。成功时文件描述符经 `mem::forget` 故意泄漏，持锁至进程退出，由内核在进程终止时自动释放。

锁文件置于模块根而非 `logs/`，原因是 `logs/` 每次启动都会被整体改名归档，锁锚点随之消失。

这一步必须先于日志归档。若第二个实例先执行归档，会将第一个实例正在使用的 `logs/` 目录整体改名，导致第一个实例的日志写入 inode。

**第 4 步：归档上一轮日志**

调用 `logger::archive_on_startup(&root)`，详见第 9 章。

**第 5 步：判断 SoC 与配置路径**

`common::is_chiri_soc()` 得出 `chiri_active`，然后 `get_config_path()` 决定用哪份配置，并把结果写进 `active_config.chr`，供 WebUI 读取同一份。

**第 6 步：处理 nofix 与清理遗留**

`nofix` 是个开关，打开后跳过所有「二进制覆盖外部文件」的动作。之后删除模块根与各 SoC 子目录下遗留的旧 `config.yaml`。

**第 7 步：导出白名单快照**

只在 ChiRi SoC 上做。把编译期嵌入的特调白名单里**精确包名**的条目导出成磁盘上的 `special_tuned.yaml`，把 FAS 白名单导出成 `fas_whitelist.yaml`。这两个文件是给 WebUI 只读展示用的，改了不生效。

正则条目不导出（无法按包名精确查找）。

**第 8 步：实验室状态处理**

`rhine::on_startup()`，必须在第一次 `Config::load()` 之前，因为实验室会改写配置。

**第 9 步：初始化语言与日志**

先加载 i18n bundle，再 `logger::init()`。

**第 10 步：还原 WebUI 资源**

把编译进二进制的 WebUI 静态文件写回磁盘的 `webroot/`。只覆盖「缺失或内容不一致」的文件，单文件失败就跳过。

**第 11 步：装 panic 钩子**

把 panic 的 payload、位置、线程名以 `[PANIC]` 前缀写进日志。约束：**panic 钩子内不得调用 i18n**——i18n 的 bundle 使用 `RwLock`，panic 发生在持锁期间时再次取锁会死锁或直接 abort。

**第 12 步：起心跳线程**

每 15 秒往 `LiveTime.chr` 写一次时间。WebUI 靠这个文件判断守护进程是死是活，容差 20 秒。

**第 13 步：建事件通道并选择调度器**

```rust
let (tx, rx) = mpsc::sync_channel::<DaemonEvent>(64);
```

容量 64 是有界通道。满了之后 `send` 会阻塞，形成背压，防止事件无限堆积。

**第 14 步：启动监控层**

监控层跑在独立线程里。它崩了只打错误日志，不会拖垮主线程。

### 3.2 WatchDog

守护进程自己不会起死回生，负责这件事的是 `module/service.sh` 里的一个 shell 循环：

```sh
while :; do
    # 退出条件：正在卸载，或主二进制被删
    "$DAEMON"
    # 如果这次存活时间 < 60s，判为异常短命
done
```

关键参数：

| 参数                       | 值                                   |
| -------------------------- | ------------------------------------ |
| 基础重启间隔               | 3 秒                                 |
| 判为「异常短命」的存活时长 | < 60 秒                              |
| 短命时的退避序列           | 3 → 10 → 30 → 60 秒，封顶 60         |
| 恢复正常的条件             | 存活超过 60 秒，或 `date` 命令不可用 |

WatchDog 启动时将自己的 PID 写入 `logs/watchdog.pid`。WebUI 的「关闭调度」功能先读取该文件终止 WatchDog，再终止守护进程；缺少该步骤时，守护进程被终止后 3 秒会被重新拉起。

WatchDog 是守护进程的**父进程**，因此守护进程可通过 `getppid()` 获取其 PID。日志模块包含一处自愈逻辑：若 `logs/` 被外部删除，pid 文件随之消失，此时日志模块会检查「ppid > 1 且父进程的 comm 为 sh 或 mksh」，满足条件即重建该文件。该检查用于防止调试时直接运行二进制（父进程非 shell）导致误写。

### 3.3 主事件循环

主循环在 `src/chiri/mod.rs` 的 `scheduler_ipc` 线程里。它的核心是一段**动态超时**的计算：

```rust
let until = |last: Instant, period: Duration| -> Duration {
    period.checked_sub(now_loop.duration_since(last)).unwrap_or(Duration::ZERO)
};
let mut wait = until(last_telemetry_log, TELEMETRY_LOG_INTERVAL)     // 1s
    .min(until(last_thermal_check, THERMAL_CHECK_INTERVAL))          // 2s
    .min(until(last_mode_file_write, MODE_FILE_REWRITE_INTERVAL))    // 5s
    .min(EVENT_POLL_MS);                                             // 1s 兜底上限
if let Some(d) = fast_next { wait = wait.min(d); }                   // fast_lock 重写
rx.recv_timeout(wait)
```

计算方式为：先求每个周期任务「距离下次执行时刻的剩余时间」，取其中最小值作为阻塞时长。

不采用固定轮询的原因，源码注释已作说明：所有性能敏感路径（负载事件、模式切换、触摸）均为**推送事件**，事件到达时 `recv_timeout` 立即返回，延迟为零；固定轮询只影响非性能的定时任务。改为动态 deadline 后，空闲状态下每秒循环次数从 10 次降至约 1 次。

源码注释对该约定有重点标注：「新增周期任务时必须把它的 deadline 纳入 `recv_timeout` 的 wait 计算，否则任务会被拖到最长 1s 粒度」。

### 3.4 周期任务表

| 周期  | 任务                                      | 位置            |
| ----- | ----------------------------------------- | --------------- |
| 1 秒  | 遥测快照写 `status.csv` 与 `PowerAVG.chr` | `chiri/mod.rs`  |
| 1 秒  | 温度读取与滤波                            | 同上            |
| 1 秒  | FAS 延迟退出巡检                          | 同上            |
| 2 秒  | 热保护计算                                | 同上            |
| 2 秒  | 亲和与 core_ctl 刷新（复用同一计时器）    | 同上            |
| 5 秒  | `current_mode.chr` 自愈重写               | 同上            |
| 5 秒  | `fast_lock` 防篡改重写                    | `chiri/fast.rs` |
| 5 秒  | 状态通知（每 5 次 1s 循环）               | `notify.rs`     |
| 15 秒 | 心跳写 `LiveTime.chr`                     | `logger.rs`     |
| 20 秒 | 遥测 debug 摘要                           | `chiri/mod.rs`  |
| 每轮  | `config_dirty` 轮询、触摸队列清空         | 同上            |

### 3.5 崩溃自愈

主循环外层包裹 `catch_unwind` 与重启循环：

```rust
const SCHEDULER_IPC_RESTART_MAX: u32 = 5;
const SCHEDULER_IPC_RESTART_BACKOFF: Duration = Duration::from_secs(1);
```

panic 之后先把状态清理到安全态（各个 `release()` 都是幂等的，收尾本身也包在 `catch_unwind` 里），重启计数加一。超过 5 次就 `exit(1)`，把问题交给 WatchDog 按进程级重启处理——这是在防「确定性 panic 变成打满 CPU 的重启风暴」。没超过就等 1 秒，把状态机重置为亮屏安全态，再按当前模式重新接管。

另有一条 WatchDog 逻辑，针对**负载数据源失效**：

```rust
const CLG_STALE_MAX: Duration = Duration::from_secs(5);
```

如果超过 5 秒没收到任何 `SystemLoadUpdate` 事件，就依次释放特调、CLG、fast_lock 三个接管，把频率控制权还给系统。

代码注释举了个例子来解释这个机制的必要性，说「8550 的 default 模式 `perf_init` 是 1.0，负载源挂了还保持接管会锁满全核高频」。这个例子本身是过时的——8550 现在的 `default.perf_init` 是 0.40。但机制的理由仍然成立：任何非零的 `perf_init` 在负载源失效后都会一直保持，而 CLG 已经无法根据真实负载调整它了。

另外，`BpfStats` 事件刻意**不刷新**这个 WatchDog 的心跳。因为 BpfStats 是计数器增量，跟负载是否正常没关系，让它刷新会掩盖真正的问题。

---

## 4. 监控层：调度器的眼睛

监控层在 `src/monitor/`，两套调度共用。它负责把内核状态变成结构化事件。

### 4.1 线程清单

| 线程                | 入口                 | 启动条件                 |
| ------------------- | -------------------- | ------------------------ |
| `screen_watcher`    | netlink uevent 直推  | 常驻                     |
| `config_watcher`    | inotify 监听配置目录 | 常驻                     |
| `pid_watcher`       | 500ms 广播前台 PID   | 常驻                     |
| `fps_monitor_ebpf`  | 帧采集               | 仅 ChiRi SoC 且 FAS 可用 |
| `cpu_monitor_ebpf`  | CPU 采集             | 常驻                     |
| `telemetry_monitor` | PSI/GPU/电池采集     | 仅 ChiRi SoC             |

启动前会先调 `setrlimit(RLIMIT_MEMLOCK, RLIM_INFINITY)` 解除 eBPF map 的内存锁限制。

### 4.2 事件类型

`DaemonEvent` 一共 7 个变体，定义在 `common.rs`：

| 变体                | 携带数据                                    | 生产者         |
| ------------------- | ------------------------------------------- | -------------- |
| `ModeChange`        | package_name, pid, mode, temperature        | app_detect     |
| `PackageSwitch`     | package_name, pid                           | app_detect     |
| `FrameUpdate`       | frame_delta_ns                              | fps_monitor    |
| `SystemLoadUpdate`  | core_utils: Vec\<f32\>, foreground_max_util | cpu_monitor    |
| `ConfigReload`      | RulesConfig                                 | config_watcher |
| `ScreenStateChange` | bool                                        | screen_detect  |
| `BpfStats`          | wakeups, migrations, freq_transitions       | cpu_monitor    |

### 4.3 前台应用怎么识别

该模块的实现方式与 Android 常规做法不同：**不读取 ActivityManager，完全依赖 cgroup**。

候选路径按顺序试三个：

1. `/dev/cpuset/top-app/cgroup.procs`
2. `/sys/fs/cgroup/cpuset/top-app/cgroup.procs`
3. `/dev/stune/top-app/cgroup.procs`

命中哪个就把索引缓存下来，后续直接用。读到的是一串空白分隔的 PID，代码**倒序遍历**——后写进去的通常是最近切到前台的。对每个 PID 读 `/proc/<pid>/cmdline`，取第一个 `\0` 之前的部分作为包名。

包名还要过一遍过滤（`is_valid_user_app`）：

- 空、不含 `.`、以 `/` 或 `.` 开头、含 `:` 的丢掉
- 命中输入法黑名单的丢掉
- 命中 `rules.yaml` 的 `ignored_apps` 的丢掉
- 一批硬编码的系统包丢掉，包括 `com.android.systemui`、`system_server`、`surfaceflinger`、`com.android.phone`、`chiri` 自己等
- 包名里含 `magisk`、`mtiodaemon`、`ads_monitor`、`inputmethod` 的也丢掉

输入法列表是动态拿的，通过 `settings get secure enabled_input_methods`；拿不到就回退到 5 个硬编码包名。

**防抖**：新包名要连续稳定 500ms 才被确认。主循环每轮末尾睡 1500ms。

### 4.4 模式判定的优先级

这是整个调度里最像「路由表」的一段，实现在 `app_detect.rs` 的 `determine_mode`。顺序不能调换：

**第一优先：FAS 白名单**

条件全部满足才返回 `"fas"`：是 ChiRi SoC、`fas_available()` 为真、包名在 `normal/fas.yaml` 里、且该配置名对应的 `normal/fas/<配置名>.yaml` 能解析成功。

**第二优先：特调白名单**

命中时返回该条目的**回退模式**（`special_tuned.yaml` 每行第三段）。

**第三：如果 `dynamic_enabled` 为假**

直接返回 `global_mode`，不做后面的匹配。

**第四：`rules.yaml` 的 `app_modes`**

该判定包含一处防绕过设计：若 `app_modes` 为某应用指定了 FAS 模式，**一律拒绝**并回退到 `global_mode`。原因是 FAS 仅允许通过白名单进入，不得由配置文件绕过。特调模式则要求同时满足「包名在白名单中」与「该模式位于该条目的模式列表内」两个条件。

**第五：特调白名单的 fallback**

`app_modes` 没命中时，再看一次特调白名单。

**第六：全局兜底**

`global_mode` 如果是 FAS 模式，拒绝，返回 `"default"`；如果是特调模式且不被允许，也返回 `"default"`；否则原样返回。

### 4.5 CPU 利用率怎么算

数据源是 eBPF 的 `sched_switch` tracepoint，不是 `/proc/stat`。内核侧维护这些 map：

| map                 | 类型           | 内容                 |
| ------------------- | -------------- | -------------------- |
| `CORE_IDLE_TIME`    | PerCpuArray    | 每个核的累计空闲时间 |
| `CORE_BUSY_TIME`    | PerCpuArray    | 每个核的累计忙碌时间 |
| `CORE_LAST_TIME`    | PerCpuArray    | 上次切换的时间戳     |
| `CORE_CURRENT_TID`  | PerCpuArray    | 当前运行的线程 ID    |
| `CORE_CURRENT_TGID` | PerCpuArray    | 当前运行的进程 ID    |
| `THREAD_RUN_TIME`   | HashMap(32768) | 每线程累计运行时间   |
| `TGID_RUN_TIME`     | HashMap(1024)  | 每进程累计运行时间   |

用户态每个采样周期算一次：

```
pending_delta = now - last_switch_time        // 当前任务已经跑了多久但还没切换
if current_tid == 0: adj_idle += pending_delta
else:                adj_busy += pending_delta

idle_diff = idle_now - idle_prev
busy_diff = busy_now - busy_prev
util = busy_diff / (idle_diff + busy_diff)    // 再 clamp 到 [0, 1]
```

`pending_delta` 是必要的补偿，因为一个任务可能在两次采样之间一直没发生切换。如果 `pending_delta` 超过 1 秒就置 0，当作异常处理。

util 数组按**真实 CPU ID 索引**，长度是 `max_cpu_id + 1`。这样 CLG 里按 `cpu_id` 取用不需要做映射。这个约定在项目记忆里被列为硬约束。

**采样间隔**：ChiRi 是 160ms，Yumi 是 200ms，特调激活时切到 40ms。切换靠一个 `Arc<AtomicBool>`（`ak_active`）在每轮末尾重建 interval。

**线程级 util**：主路径读 `TGID_RUN_TIME`（只要一个 key，避开 `THREAD_RUN_TIME` 的哈希驱逐问题）。降级路径遍历 `/proc/<pid>/task` 取最大线程 util。这条路径只在 FAS 可用时才启用。

### 4.6 帧采集

帧数据来自 `libgui.so` 的 `Surface::queueBuffer` uprobe：

- 路径：`/system/lib64/libgui.so`
- 符号：先用短签名 `_ZN7android7Surface11queueBufferEP19ANativeWindowBufferi`，失败回退长签名
- 作用域：`UProbeScope::OneProcess`，只挂当前前台进程

内核侧把 `{pid, ktime_ns}` 塞进一个 32KB 的 RingBuf。用户态算相邻时间戳的差作为帧间隔，只接受 1ms 到 200ms 之间的值，环形历史保留 144 帧。

**反偷跑门控**是该模块的关键设计。默认状态下：

1. FAS 未激活时，`fps_monitor_ebpf` 线程连 tokio runtime 和 eBPF 都不加载，1 秒轮询等标志
2. 标志置位后，`fps_probe` 线程启动但**不挂载任何 uprobe**：仅加载 eBPF，无 attach 点即零执行
3. 激活瞬间从 `watch` channel 直接借当前前台 PID 补挂。这一步不能省：如果 FAS 应用在激活前就已经在前台，PID 变化事件不会再来，不主动补挂就永远挂不上
4. 去激活后主循环下一轮 `switch_pid(0)` 做纯 detach

该门控的必要性在于：非 FAS 会话（桌面、普通应用）每渲染一帧都会经过 `queueBuffer`，若探针常驻，等于每帧支付一次无谓开销，而 `FrameUpdate` 事件在调度侧还会被直接丢弃。

### 4.7 亮灭屏检测

主路径是 netlink uevent，零轮询延迟：

| 监听对象       | 事件           | 判定                                         |
| -------------- | -------------- | -------------------------------------------- |
| `power` 子系统 | `POWER_ACTION` | `early_suspend` → 息屏，`late_resume` → 亮屏 |
| `backlight`    | `Change`       | 睡 100ms 后读亮度值                          |
| `leds`         | `Change`       | 仅名字含 `backlight` 的节点，睡 100ms 后读   |

接收缓冲区设为 2MB。uevent 存在漏报可能，因此另有一条自愈路径：`verify_screen_state` 由 `app_detect` 每轮调用，直接读取 sysfs 校正。

检测源有三类：

- `Backlight`：`/sys/class/backlight`，读 `bl_power` 或 `actual_brightness`
- `Leds`：`/sys/class/leds/*backlight*`，看 `brightness > 0`
- `FbBlank`：`/sys/class/graphics/fb0/blank == 0`

**息屏仲裁**用的是投票制。亮转息时所有有效节点投票，需要 2 票（`OFF_QUORUM = 2`）才确认。只有一个有效节点时按一票算。所有节点都读不到时，fail-safe 选择「恒亮屏」——宁可多耗电，也不要错误地进入省电模式。

相关常量：

| 常量                         | 值  | 作用                           |
| ---------------------------- | --- | ------------------------------ |
| `SCREEN_READ_FAIL_LIMIT`     | 8   | 连续读失败这么多次就退役该节点 |
| `OFF_REVIEW_INTERVAL`        | 10s | 息屏稳态下全节点复核周期       |
| `INCONSISTENCY_SWITCH_DELAY` | 15s | 节点读数不一致时的切换延迟     |
| `VETO_RETIRE_EPISODES`       | 3   | 被驳回这么多次后退役           |
| `OFF_QUORUM`                 | 2   | 息屏确认所需票数               |

### 4.8 遥测

1 秒轮询一次，采集三类数据：

**PSI**：读 `/proc/pressure/cpu`、`io`、`memory` 的 `some avg10`，解析成 0.0 到 1.0 的小数。文件不存在按 0 处理。

**GPU 占用**：候选两个节点，探测成功就缓存：

- 高通 Adreno：`/sys/class/kgsl/kgsl-3d0/gpu_busy_percentage`
- MTK：`/sys/kernel/ged/hal/gpu_loading`

**电池**：优先 OPlus 私有节点 `/sys/class/oplus_chg/battery/bcc_parms`（逗号分隔，下标 6 是电芯 0 电压、8 是电流、11 是电芯 1 电压）。双电芯并联时电压取均值、电流乘 2。失败就回退标准节点 `/sys/class/power_supply/battery/{current_now,voltage_now}`。私有节点的存在性每 60 秒复查一次。

**存储方式**：进程共享的原子量，不是事件通道。f32 用 `to_bits()` 存进 `AtomicU32`，电池不可用存 `i32::MIN`，GPU 不可用存 NaN 的位模式——避免占用容量 64 的事件通道。

历史遗留命名：`batt_current_ma()` 返回值单位为**安培**而非毫安。功率计算式为 `|I| × V`。

### 4.9 pid_watcher

唯一的前台 PID 广播源。500ms 一轮读 `CURRENT_PID` 原子量，只在 PID 变化且大于 0 时通过 `tokio::sync::watch` 广播。

消费方有两个：`cpu_monitor` 每 tick 检查 `has_changed()` 再更新；`fps_monitor` 内部起一个 tokio 任务把 watch 变化桥接到 `std::sync::mpsc`，因为 `fps_probe` 是同步线程，用不了 async 的 watch。

这个设计取代了「每个监控各自轮询」的旧做法。

---

## 5. 调度算法披露

本章为调度算法主体，按控制频率的机制分节展开。各机制之间为互斥或接管关系。

各机制的关系如下表：

| 机制                          | 触发条件             | 控制粒度             | 是否启用     |
| ----------------------------- | -------------------- | -------------------- | ------------ |
| CLG 负载调速器                | 默认，按模式参数     | 每个 policy 一个上限 | 启用         |
| 特调（akmode / playback）     | 前台命中特调白名单   | 每个核心组一个上限   | 启用         |
| FAS                           | 前台命中 FAS 白名单  | 每帧锁 min=max       | 启用         |
| fast_lock（vector）           | `global_mode=vector` | 锁 min=max=硬件最高  | 启用         |
| 热保护                        | 温度超阈值           | 压制上面几者的上限   | 启用         |
| 息屏场景模式                  | 息屏超时             | 独立一套 CLG 参数    | 启用         |
| 实验室（contingency / babel） | `rhine.chr` 启用     | 静态配置             | 启用         |
| 触摸升频                      | 触摸事件             | 短时抬升上限         | **当前停用** |
| DOWN 停摆                     | `down.chr` 写入      | 全部释放             | 启用         |

### 5.1 CLG 负载调速器

代码在 `src/chiri/cpu_load_governor.rs`。CLG 是「CPU Load Governor」的缩写。

#### 5.1.1 运行模型

CLG 不是定时器驱动，而是**事件驱动**。`cpu_monitor` 每 160ms（特调 40ms）发一次 `SystemLoadUpdate`，每个 policy 对应的 `CoreGroupWorker` 线程立刻被唤醒执行决策。

每个 worker 的循环长这样：

```rust
let tick_interval = Duration::from_secs(1);
loop {
    match load_rx.recv_timeout(tick_interval) {
        Ok(core_utils) => {
            if !core_utils.is_empty() { on_load_update(&core_utils) }  // 决策
            flush(&core_utils, &mut log_counter);                       // 写频
        }
        Err(Timeout) => {
            // 1 秒超时分支只做两件非性能事务
            // 1. 清理过期触摸窗口
            // 2. 重写当前频率防篡改
        }
        Err(Disconnected) => break,
    }
}
```

**空包表示触摸唤醒信号**：触摸发生时主循环向通道投递一个空 `Vec`，worker 跳过决策但仍执行 `flush`，按触摸抬升后的上限写一次频率，因此判断条件为 `!core_utils.is_empty()`。

负载通道容量为 1，满时丢弃本 tick 的数据包：负载数据仅最新值有意义，积压旧数据无价值。

#### 5.1.2 决策算法（`on_load_update`）

以下按源码执行顺序拆解。设：

- `C` = 这个 policy 覆盖的核心集合（`affected_cpus`）
- `U[c]` = 核心 c 的利用率

**第一步：取组内最大利用率**

```
u_raw = max{ U[c] : c ∈ C }
```

取最大值而非平均值：一个核打满即代表整个簇吃不消，平均值会掩盖这种情况。

**第二步：尖峰抑制**

```
if u_raw > last_util + spike_jump_threshold:
    u = last_util + (u_raw - last_util) × spike_decay
else:
    u = u_raw
last_util = u_raw        // 注意：存的是原始值
```

`last_util` 保存原始值这一设计决定了后续行为：**持续高负载只会被衰减一次**。第二个 tick 时 `u_raw` 与 `last_util` 相等，增量归零，`u` 直接等于 `u_raw`；仅单次跳变会被削减。

以 8550 default 为例，`spike_jump_threshold = 0.40`，`spike_decay = 0.25`。如果 util 从 0.30 跳到 1.00，跳变 0.70 超过阈值，那么 `u = 0.30 + 0.70 × 0.25 = 0.475`。

**第三步：headroom 线性过渡**

headroom 为余量系数，在 `up_threshold` 附近采用斜坡过渡而非阶跃：

```
ramp_start = up_threshold - headroom_ramp

if u >= up_threshold:
    h = headroom_factor
elif u > ramp_start:
    t = clamp((u - ramp_start) / headroom_ramp, 0, 1)
    h = 1.0 + (headroom_factor - 1.0) × t
else:
    h = 1.0
```

以 8550 default 为例，`up_threshold = 0.72`，`headroom_ramp = 0.12`，`headroom_factor = 1.20`。那么在 `u = 0.72` 时 `h = 1.20`，在 `u = 0.60` 时 `h = 1.0`，中间线性过渡。

源码注释说明了该斜坡的作用：「避免阶跃导致的振荡」。若 headroom 在 0.72 处从 1.0 突跳至 1.2，目标值将出现 0.144 的瞬时跳变，容易引发升降频反复切换。

**第四步：算目标性能比**

```
target_perf = clamp(u × h, perf_floor, perf_ceil)
```

`perf_floor` 和 `perf_ceil` 是这套算法里的核心概念。它们是**归一化的性能比**，不是频率。含义是「允许 schedutil 使用的频率区间占硬件全频段的比例」。

比如 `perf_ceil = 0.60`，意味着把 `scaling_max_freq` 设在「硬件最低频 + 60% × (最高频 - 最低频)」对应的那个档位附近。

**第五步：升频分支**

```
if target_perf > current_perf:
    down_wait = 0
    up_wait += 1
    if up_wait < up_rate_limit_ticks: return        // 限速，本 tick 不动作

    is_high_load = u >= up_threshold
    is_significant_jump = target_perf > current_perf + up_jump_threshold

    if is_high_load or is_significant_jump:
        // 快速通道
        current_perf += (target_perf - current_perf) × smoothing_up
    else:
        // 滞回带内，渐进
        span = up_threshold - down_threshold
        gap = clamp((u - down_threshold) / span, 0, 1)
        speed = smoothing_up × (slow_up_scale + (1 - slow_up_scale) × gap)
        current_perf += (target_perf - current_perf) × speed
```

`smoothing_up` 的语义需明确：它**并非对 util 做平滑**，而是「current_perf 朝 target_perf 逼近的一阶步进系数」。其差分方程为：

```
perf[n+1] = perf[n] + k × (target - perf[n])
```

这是一阶低通，等效时间常数约为 `τ ≈ Δt × (1-k)/k`。以 8550 default 的 `smoothing_up = 0.75`、`Δt = 160ms` 计算，τ ≈ 53ms；boost 模式的 `smoothing_up = 0.95` 对应 τ ≈ 8.4ms，接近即时到位。

滞回带内的 `speed` 会再乘以一个与 `gap` 相关的因子。`gap` 表示当前 util 在 `down_threshold` 至 `up_threshold` 区间内的相对位置，越接近 `up_threshold` 越接近 1。`slow_up_scale` 为下限，8550 default 取 0.03，即滞回带最底部的步进系数为 `0.75 × 0.03 = 0.0225`，推进速率极低。

**第六步：降频分支**

```
else:
    up_wait = 0
    target_freq = find_nearest_freq(target_perf)

    if target_freq == current_freq:
        down_wait = 0
        decision = "hold"           // 不计数、不写频
    else:
        down_wait += 1
        if down_wait >= down_rate_limit_ticks or u < down_fast_threshold:
            decision = "down"
            current_perf = ratio_of_freq(target_freq)   // 一步到位
        else:
            decision = "down_wait"
```

降频**不做平滑**，直接跳至目标档，与升频的渐进式形成对比。这是刻意的非对称设计：升频滞后会导致卡顿，降频过快仅影响功耗。

`target_freq == current_freq` 这一判断为后续补充。源码注释记录了原因：在 `reduce` 模式下，小核的 `perf_ceil = 0.60` 会钳制上限。此时 util 可能为 1.00，算出的 `target_perf` 略低于 `current_perf`，但两者映射到**同一频率档**。旧代码在此情形下会每个 tick 都标记 `down` 并使 `deb_down` 无限增长，日志中出现「满载却 decision=down」的矛盾记录。

`u < down_fast_threshold` 提供「极低负载跳过确认期」的快速通道。8550 default 为 0.20，boost 为 0.05。

#### 5.1.3 频率映射

频率表来自 `scaling_available_frequencies` 加上 `boost_frequencies`，排序去重。对每个频点预计算一个比例：

```
ratio[i] = (freq[i] - freq_min) / (freq_max - freq_min)
```

查找用二分（`partition_point`）找到第一个 `ratio >= target` 的位置，然后**比较相邻两个档位取更近的那个**——不是向下取整，也不是向上取整。

```
idx = partition_point(|r| r < target_ratio)
if idx == 0:      return freq[0]
if idx >= len:    return freq[len-1]
if |ratio[idx] - target| < |ratio[idx-1] - target]: return freq[idx]
else:             return freq[idx-1]
```

#### 5.1.4 写频与 flush

`flush` 的执行顺序不能变：

```
1. 触摸升频检查（若启用）：current_perf = max(current_perf, touch_floor)
2. 性能区间 clamp：current_perf = clamp(current_perf, perf_floor, perf_ceil)
3. 热保护 clamp：if current_perf < free_above { current_perf = min(current_perf, thermal_cap) }
4. find_nearest_freq → 写 scaling_max_freq
```

热保护位于第 3 步，并带有 `free_above` 豁免档。源码注释说明了原因：「持续高负载会平滑涨到豁免档以上，这时温度再高也不挡路（内核兜底）」。即当负载已将 perf 推至 0.80 以上时，软件层不再介入，交由内核温控处理。

**只写 `scaling_max_freq`，不写 `scaling_min_freq`**。min 在接管时一次性压到硬件最低并保持不变。实际频率由内核 schedutil 在 `[硬件最低, 新上限]` 区间内自主决定。

仅在写频成功后更新 `current_freq`；失败时下个 tick 自动重试。

#### 5.1.5 接管与释放

`init_policies` 的顺序：

1. 停掉旧 worker
2. 快照每个 policy 的 `governor`、`min_freq`、`max_freq`、`hw_max`
3. 写 `scaling_governor = schedutil`
4. `scaling_min_freq` 压到硬件最低
5. `scaling_max_freq` 设为 `clamp(perf_init, perf_floor, perf_ceil)` 对应的档
6. 起 worker

`release` 走 `restore_policy`，写序保证任意中间态都满足 `min <= max`。

`reload_config` 会停旧 worker、用新配置重建，并且把 `current_perf` **重置到 `perf_init`**。注释说明原因：避免息屏时 perf 掉到接近 0 之后，亮屏要慢慢爬升。

#### 5.1.6 8550 三档参数全表

这是实际跑在 8550 上的数字，来自 `module/config/8550/feature.yaml`：

| 字段                    | reduce | default | boost |
| ----------------------- | ------ | ------- | ----- |
| `up_threshold`          | 0.90   | 0.72    | 0.55  |
| `down_threshold`        | 0.50   | 0.55    | 0.40  |
| `smoothing_up`          | 0.50   | 0.75    | 0.95  |
| `down_rate_limit_ticks` | 2      | 3       | 5     |
| `up_rate_limit_ticks`   | 2      | 2       | 1     |
| `headroom_factor`       | 1.05   | 1.20    | 1.40  |
| `headroom_ramp`         | 0.12   | 0.12    | 0.12  |
| `perf_floor`            | 0.05   | 0.10    | 0.40  |
| `perf_ceil`             | 0.60   | 1.0     | 1.0   |
| `perf_init`             | 0.25   | 0.40    | 0.60  |
| `up_jump_threshold`     | 0.35   | 0.30    | 0.25  |
| `slow_up_scale`         | 0.02   | 0.03    | 0.05  |
| `down_fast_threshold`   | 0.10   | 0.20    | 0.05  |
| `spike_jump_threshold`  | 0.35   | 0.40    | 0.40  |
| `spike_decay`           | 0.30   | 0.25    | 0.20  |
| `touch_boost_ms`        | 350    | 400     | 500   |
| `touch_boost_tiers`     | 1      | 1       | 1     |

三个档位的取向可从参数直接看出：reduce 门槛高、上限低、升频慢，面向待机；default 居中；boost 门槛低（0.55 即判定为高负载）、升频接近无延迟（0.95）、下限即给到 0.40。

default 的 `down_rate_limit_ticks` 为 3，reduce 为 2。配置文件注释解释了该取值：8550 原先使用 1/1，实测等同于无防抖，上限与方向每个 tick 均可能翻转。

作为对比，8475 与 8998 的 default 使用 `1/1`。三个 SoC 的差异还包括：

| 配置项                            | 8550 | 8475 | 8998  |
| --------------------------------- | ---- | ---- | ----- |
| `Affinity.top_app_uclamp_max_pct` | 85   | 85   | 0     |
| `CoreCtl.scenemode_offline`       | true | true | false |
| `IO_Settings.Scheduler`           | ""   | ""   | ""    |

8998 关掉 `scenemode_offline` 的原因写在配置注释里：4.4 内核的热插拔路径质量未知。

#### 5.1.7 代码默认值

如果某个字段在 yaml 里没写，会回退到 `src/chiri/config.rs` 里的 `#[serde(default)]` 函数。这些默认值和 8550 的实测值不一样，列出来做参考：

| 字段                                     | 代码默认          |
| ---------------------------------------- | ----------------- |
| `enabled`                                | true              |
| `up_threshold`                           | 0.80              |
| `down_threshold`                         | 0.50              |
| `smoothing_up`                           | 0.60              |
| `down_rate_limit_ticks`                  | 3                 |
| `up_rate_limit_ticks`                    | 2                 |
| `headroom_factor`                        | 1.25              |
| `headroom_ramp`                          | 0.15              |
| `perf_floor` / `perf_ceil` / `perf_init` | 0.15 / 1.0 / 0.50 |
| `up_jump_threshold`                      | 0.35              |
| `slow_up_scale`                          | 0.02              |
| `down_fast_threshold`                    | 0.10              |
| `spike_jump_threshold`                   | 0.35              |
| `spike_decay`                            | 0.30              |
| `touch_boost_enabled`                    | true              |
| `touch_boost_ms` / `touch_boost_tiers`   | 400 / 1           |

`normalize()` 会做交叉约束：`headroom_factor` 限在 [1, 3]，`touch_boost_ms` 限在 [1, 1000]，`touch_boost_tiers` 不超过 8，强制 `down_threshold <= up_threshold`，`perf_init` 必须落在 [floor, ceil] 内。

#### 5.1.8 触摸升频（当前停用）

算法本身写完了，但被一个常量关掉了：

```rust
const TOUCH_BOOST_SUSPENDED: bool = true;
```

`on_touch` 直接 return。代码注释留了一句：「确认长期关闭时应改配置而非留常量」——意思是现在这个状态是临时挂起，不是最终决定。

设计值是：`floor = min(perf_init + touch_boost_tiers × 0.05, perf_ceil)`，档位步进 0.05。窗口内持续触摸会刷新截止时间。应用条件有三个：`touch_boost_enabled` 为真、floor 大于 0、且这个 policy 覆盖大核区间（`is_big_cluster`）。

### 5.2 特调（Special Tuning）

代码在 `src/chiri/tuned.rs`，配置在 `module/config/normal/tuned_profiles.yaml`，白名单在 `src/chiri/special_tuned.yaml`。

特调和 CLG 的思路不同。CLG 是「按负载分档、档位参数固定」，特调是「按负载连续控制、没有档位概念，上限在内核频率表里逐档升降」。

#### 5.2.1 白名单格式

```
匹配器:模式列表(逗号分隔):优先回退模式
```

当前的全部条目：

| 匹配器                             | 模式     | 回退     |
| ---------------------------------- | -------- | -------- |
| `com.hypergryph.arknights`         | akmode   | akmode   |
| `com.YoStarJP.Arknights`           | akmode   | akmode   |
| `re:(?i)arknights`                 | akmode   | akmode   |
| `tv.danmaku.bili`                  | playback | playback |
| `re:(?i)(bilibili\|danmaku\.bili)` | playback | playback |

匹配顺序是**先精确包名（按文件内顺序），未命中再按正则（按文件内顺序）**。

解析逻辑中有一处 2026-09-18 修复的缺陷：旧代码直接对整行执行 `splitn(3, ':')`，导致 `re:` 条目的包名部分成为字面量字符串 `"re"`，无法匹配任何包名。正确做法是先 `strip_prefix("re:")` 摘除前缀，再行切分。

#### 5.2.2 参数组

| 参数组           | headroom | perf_floor | hysteresis | down_hold_ms | util_smoothing | boost_affinity |
| ---------------- | -------- | ---------- | ---------- | ------------ | -------------- | -------------- |
| akmode（缺省段） | 1.15     | 0.0        | 0.03       | 100          | 1.0（不平滑）  | true           |
| playback         | 1.05     | 0.0        | 0.04       | 100          | 0.35           | false          |

如果白名单里注册了某个模式但 `tuned_profiles` 段里没有同名参数组，会回退到 `akmode` 段，并在启动时打一条 `tuned-profile-missing` 告警。

`boost_affinity` 是有实际效果的，实现在 `src/chiri/mod.rs` 的 `tuned_boost_affinity()` 和 `apply_affinity_and_corectl()`。它参与决定两件事：要不要把 cpuset 收窄到 `big ∪ prime`，以及要不要让 core_ctl 保大核常在线。非特调模式恒为 `true`，所以这个字段只对特调有意义。

akmode 是 `true`（游戏要保响应），playback 是 `false`（视频要贴负载省电）。代码注释解释了 playback 为什么不走 boost：收窄 cpuset 和保大核常在线与大核空转漏电的省电目标是相反的，而且会把视频解码线程钉到大核组里被低上限压住。

#### 5.2.3 决策算法

特调激活时，`cpu_monitor` 的采样间隔从 160ms 切到 **40ms**。每个 tick 对每个核心组独立决策：

**第一步：组内最大占用**

```
g = max{ U[c] : c ∈ 组区间 }
```

**第二步：EMA 平滑**

```
if util_smoothing >= 0.999 or 未初始化:
    u = g
else:
    u = α × g + (1 - α) × u_prev        // α = util_smoothing
```

时间常数约 `τ ≈ Δt × (1-α)/α`。playback 的 α = 0.35、Δt = 40ms，算出 τ ≈ 75ms。代码注释提到：日志回放用的是 160ms 采样，此时 τ ≈ 300ms，所以回放得出的收紧幅度比实机乐观。

akmode 用 1.0，不平滑。注释解释了原因：游戏的瞬时升频是响应性的一部分，不能滤掉。

**第三步：算目标上限**

```
target_ratio = clamp(u × headroom, perf_floor, 1.0)
target_max   = min{ f ∈ 频率表 : f >= hw_max × target_ratio }   // 向上取档
hyst_freq    = hw_max × hysteresis
```

此处为**向上取档**，与 CLG 的「取最近档」不同。特调策略倾向于取偏高值。

**第四步：三态决策**

```
if target_max > current_max + hyst_freq:
    // 升频：立即执行
    write(target_max)
    down_since = None
    decision = "up"

elif target_max < current_max - hyst_freq:
    // 降频：需要持续 down_hold_ms
    if down_since is None:
        down_since = now; down_target = target_max
        decision = "down_wait"
    elif target_max > down_target + hyst_freq:
        // 目标明显回升，重置计时
        down_since = now; down_target = target_max
        decision = "down_wait"
    else:
        down_target = target_max
        if elapsed(down_since) >= down_hold_ms:
            write(target_max); current_max = target_max; down_since = None
            decision = "down"
        else:
            decision = "down_wait"
else:
    // 死区内
    down_since = None
    decision = "hold"
```

计时重置条件：**仅在目标明显回升（超过 `down_target + hyst_freq`）时重置**。源码注释记录了原因：原实现采用「档位不等即重置」，经 8550 日志回放验证，会导致上限长期维持在高位无法下调，连续控制退化为「只升不降」。当前实现中，目标继续下探时仅更新 `down_target`（等待更深的降频档位），不重置计时。

仅在写频成功后前移状态；失败时保留计时起点，下个 tick 立即重试。

#### 5.2.4 接管与释放

接管顺序：先 `release()`，逐 policy 读取频率表、判断核心组名、快照 governor 与频率区间，写入 `schedutil`，将 `scaling_min_freq` 压至硬件最低，**`scaling_max_freq` 直接置为 `hw_max`**（接管瞬间给满，因特调场景均为游戏与视频，优先保证性能），随后置 `ak_active = true` 通知监控层切换至 40ms 采样。

释放顺序是 **max → min → governor**，和 CLG 的写序不同。同样是保证中间态 `min <= max`。

`reload_config` 只换参数，保留动态 max 状态。

#### 5.2.5 WatchDog 与冷却

| 常量                 | 值   | 位置           | 作用                           |
| -------------------- | ---- | -------------- | ------------------------------ |
| `CLG_STALE_MAX`      | 5s   | `chiri/mod.rs` | 负载源失效就释放接管           |
| `TUNED_COOLDOWN`     | 300s | `chiri/mod.rs` | 接管失败后 5 分钟内不再尝试    |
| `FAS_COOLDOWN`       | 300s | `chiri/mod.rs` | FAS 激活失败后回退 CLG default |
| `SCENEMODE_COOLDOWN` | 300s | `chiri/mod.rs` | 息屏场景模式饱和退出后冷却     |

### 5.3 FAS 帧感知调度

FAS 是「Frame Aware Scheduling」的缩写。引擎在 `src/scheduler/fas/`，生命周期管理在 `src/chiri/fas_manager.rs`。

#### 5.3.1 核心思想

其余机制均以负载为决策输入，FAS 以帧时间为输入。其控制变量是 0 至 1 的归一化性能指标 `perf_index`，所有决策最终作用于该变量，再由 `apply_freqs` 一次性映射到各簇频率。

锁频方式为**将 `scaling_min_freq` 与 `scaling_max_freq` 写入同一值**，即固定频率，同时将 governor 切换为 `performance`。

锁频的原因：游戏场景下 schedutil 的负载预测滞后于帧需求，由帧时间直接决定比内核预测更可靠。每收到一个真实新帧即推进一次状态机，不设周期性调频定时器。

#### 5.3.2 帧处理流水线

`update_frame(frame_delta_ns)` 按顺序执行 7 个阶段。

**入口过滤**：

- `delta == 0` 或没有 policy → 直接返回
- `delta < min_frame_ns()` 丢弃。`min_frame_ns = 1e9 / max_gear / 2`，即最高档帧间隔的一半，用于防止重复回调
- `delta > fixed_max_frame_ms × 1e6` 丢弃（默认 500ms）

**Phase 1：冷启动与切应用**

- `init_time` 未满 `cold_boot_ms`（默认 3500ms）时，把 `perf_index` 抬到 `perf_cold_boot`（0.85）
- `actual_ms > app_switch_gap_ms`（3000ms）视为切应用，重置运行时状态，设 `perf_index = app_switch_resume_perf`（0.60）

**Phase 2：加载态检测**

```
is_heavy = actual_ms > heavy_frame_threshold_ms    // 默认 150ms
```

累计重帧时长超过 `loading_cumulative_ms`（2500ms）就进入加载态，此时 `perf_index` 被夹在 `[0.60, 0.70]` 之间。退出加载后 `perf_index = post_loading_perf`（0.65），并设 `post_loading_ignore_frames = 5`、`post_loading_downgrade_guard = 90`（90 帧内禁止降档）。

**Phase 2.5：温度护栏**

迟滞锁存：温度 ≥ 阈值进入，温度 < 阈值 - 3℃ 退出。激活时 `perf_index` 被 `core_temp_throttle_perf`（0.70）封顶。阈值为 0 表示禁用。终末地的配置是 `core_temp_threshold: 45.0`（电池温度）。

**Phase 3：档位评估**

调 `evaluate_gear`，如果命中升档或降档，立刻执行切换并 `return`——这一帧不再走 PID。

**Phase 4 / 4.5：EMA 与动态目标**

更新帧时间 EMA，并根据 CPU 利用率调整目标帧率。

EMA 更新包含一步异常压缩：

```
base = if 窗口>=8 且 avg_fps ∈ (5, eff_target×0.50): 1000/avg_fps else budget
if delta > base×2:  delta = base×2
if delta > base×4:  delta = base + 1.0
```

滤波系数是非对称的：

```
a_up   = (0.15 × fps_factor).clamp(0.10, 0.35)
a_down = ((0.25 + (1-norm)×0.25) × fps_factor).clamp(0.15, 0.45)
fps_factor = (target/60).clamp(0.5, 2.5)
```

下降比上升快，因为帧时间变差需要更快反应。

**Phase 5：PID 与 Jank**

见 5.3.4。

**Phase 6：稳态衰减与宽容**

连续正常帧超过 `steady_decay_frame_threshold`（75 帧）且 `perf_index > 0.70` 时，开始缓慢降低性能：

```
step = ((perf - 0.50) / 0.50) × max_step(0.022) × fps_dampen × decay_scale
fps_dampen = (60/target)^0.40
decay_scale = 0.6 当 target > 90，否则 1.0
```

第二条路径：`avg_fps > target + margin×2.5` 且窗口 ≥ 30 时额外微降。

**Phase 7：温度终值钳制**

放在最后，覆盖 PID 和 Jank 产生的所有增量。代码注释明确说这是为了阻断「越热越抬频」的正反馈。

#### 5.3.3 档位状态机

档位表默认是 `[30, 60, 90, 120, 144]`。终末地的配置多一个 165。

**升档有三条路径**：

1. **过冲快升**：`avg/target > 1.35`、窗口 ≥ 15、最近 30 帧均值 > `target × 1.2`、`perf < 0.45`、且不在 boost 期。从档位表里选 `≤ 最近15帧均值 + 15` 的最高档，`perf = 0.60`，阻尼 `scale_frames(30)`
2. **常规确认**：升档冷却为 0、最近 30 帧 ≥ `next - 10`、均值 ≥ `target × 0.9`、窗口 ≥ 60，累计确认帧数达到 `scale_frames(60)` 后升档
3. **低 perf 死锁破解**：`avg ≥ target - 2`、`perf < 0.35`、窗口 ≥ 90、标准差 < `target × 0.08`，确认计数每次 +2，达阈值后升档并 `perf = min(perf + 0.20, 0.65)`

未满足条件时，确认计数每帧 -3（衰减）。

**降档**条件：`avg < target - 10`。如果 `post_loading_downgrade_guard > 0` 则禁止降档。

极端帧（`avg < target × 0.40`、窗口 ≥ 10、标准差 < `avg × 0.25`）会先尝试识别原生档位：窗口 ≥ 20、标准差 < `avg × 0.10`，找 `|avg - g| < 8.0` 且低于当前档的档位，命中就直接降到那一档，`perf = 0.55`。

否则启动**降档 Boost**：`perf += base 0.18 × sqrt(60/target)`，夹在 [0.06, 0.20] 之间，持续 `scale_frames(45)` 帧。到期后用 `perf × 0.70 + saved × 0.30` 平滑回落。

确认帧数达到 `scale_frames(90)` 后真正降档，并设升档冷却：

```
upgrade_cooldown = 90 × (1 << min(连续降档次数, 4))
```

指数退避，防止在档位边界反复横跳。

**切档后的处理**：

- 升档时 `perf = max(perf, (new/144).clamp(0.45, 0.70))`
- 降档时 `perf = min(perf, (new/144 + 0.30).clamp(0.45, 0.75))`
- 重置 PID、清空窗口、清空 EMA、清 jank 保护地板

#### 5.3.4 PID 控制器

这是整个项目里数学味最重的一段。基准系数（以 60fps 为基准）：

```
Kp = 0.050
Ki = 0.010
Kd = 0.006
```

**误差定义**：

```
budget_ms     = 1000 / eff_target
ema_budget    = 1000 / (eff_target - fps_margin)
ema_err       = ema_budget - ema_actual_ms     // 主输入
inst_err      = budget_ms - actual_ms          // 只给 P 项用
```

误差为**正**表示实际比预算快（可以降频），为**负**表示超预算（需要升频）。

**系数随帧率自适应**（设 `ratio = target/60`）：

```
kp = 0.050 × ratio            // 线性
ki = 0.010 × ratio^0.5        // 平方根，防高刷积分饱和
kd = 0.006 × ratio^0.3        // 保守，高刷噪声大
integral_limit = 0.15 × sqrt(60/target)
```

切换帧率时积分器**只 clamp 不 reset**，保持连续性。

**单帧计算**：

```
safe_norm = clamp(norm, 0.5, 2.5)          // norm = sqrt(60/target)

if error < 0:
    integral += error × safe_norm           // 只在欠频时累积
else:
    leak = clamp(0.70 + safe_norm × 0.08, 0.70, 0.85)
    integral *= leak                        // 抗饱和泄漏

dyn_limit = integral_limit × clamp(safe_norm, 0.7, 1.3)
integral = clamp(integral, -dyn_limit, dyn_limit)

raw_deriv = (error - prev_error) / safe_norm
d_alpha = clamp(0.30 × sqrt(60/adapted_fps), 0.10, 0.30)
filtered_deriv = filtered_deriv × (1 - d_alpha) + raw_deriv × d_alpha

// 利用率感知：CPU 利用率低说明瓶颈可能在 GPU 或 IO，拉频无效
util_gain = if fg_util ∈ (0.01, 0.30): 0.3 + fg_util × 2.3 else: 1.0

output = kp × inst_err × util_gain + ki × integral + kd × filtered_deriv
```

`d_alpha` 随帧率升高而降低：60fps 为 0.30，120fps 为 0.21，144fps 为 0.19。源码注释说明了原因：高刷新率下帧间微小抖动被放大至微秒级，固定参数滤波器无法有效抑制。

`util_gain` 用于避免无效升频。若前台 CPU 利用率仅为 0.10，说明瓶颈通常不在 CPU，PID 拉频不会改善帧率，只会增加功耗。此时 P 项增益被衰减至 `0.3 + 0.10 × 2.3 = 0.53`。

**输出映射到 perf_index**：

输出不是频率，是 `perf_index` 的增量。

```
if raw > 0:      // 可以降频
    perf -= raw × d × norm × floor_guard × split_guard
else:            // 需要升频
    perf += (-raw) × jd × bd × dd × norm
```

三个保护因子：

| 因子          | 触发条件                      | 值   | 作用                              |
| ------------- | ----------------------------- | ---- | --------------------------------- |
| `floor_guard` | `perf < floor + 0.15`         | 0.3  | 接近地板时减速，防止跌破          |
| `split_guard` | 目标分裂且 avg 落在两目标之间 | 0.3  | 防 PID 误判「任务完成」而激进衰减 |
| `jd`          | jank 冷却期                   | 0.25 | 防断崖                            |
| `d`           | 降档 Boost 期                 | 0.0  | 禁止衰减                          |
| `dd`          | 阻尼期                        | 0.5  | 降速                              |

**单帧增量限幅**：

```
scale = clamp((target/60)^0.3, 0.8, 1.8)

crit / emergency: max_inc = max(max_inc_normal × scale × 2.5, 0.15)
阻尼期:           max_inc = max(max_inc_damped × scale, 0.040)
正常:             max_inc = max(max_inc_normal × scale, 0.065)
```

阻尼期另有一项硬上限：`damped_perf_cap = 0.92`。

#### 5.3.5 Jank 判定与处理

**判定阈值**：

```
crit_ratio = clamp(0.40 + (1 - norm) × 0.60, 0.40, 0.80)
crit_ms    = budget_ms × crit_ratio
heavy_ms   = max(ema_budget × clamp(0.15 + (1 - norm) × 0.20, 0.15, 0.35), 2.0)
```

- `crit`：`inst_err < -crit_ms`，瞬时严重超标
- `heavy`：`ema_err < -heavy_ms`，平滑后仍超标

`norm` 越大（帧率越低），阈值越宽松。60fps 时 `norm = 1.0`，`crit_ratio = 0.40`，`crit_ms = 16.67 × 0.40 = 6.67ms`。144fps 时 `norm = sqrt(60/144) = 0.645`，`crit_ratio = 0.61`，`crit_ms = 6.94 × 0.61 = 4.24ms`。

**crit 处理**（不走 PID，直接阶跃抬升）：

```
jank_streak += 1
streak_m   = clamp(1 - e^(-0.4 × streak), 0.30, 0.85)
fps_urgency = clamp(sqrt(target/60), 1.0, 1.6)
inc = (阻尼期 ? 0.050 : 0.080) × fps_urgency
perf += inc × norm × streak_m
```

`streak_m` 是连续卡顿的加权，连续卡得越多抬得越狠，但封顶 0.85。

**紧急跳频**：如果 `actual_ms > 50.0` 且 `perf_index < 0.70`，直接跳到 0.70。注释解释了为什么不等 PID 慢慢爬：按 `max_inc ≈ 0.09/帧` 从 0.40 爬到 0.70 需要 3 到 4 帧，在 120fps 下意味着 25 到 33ms 的额外卡顿窗口。

**恢复保护**：jank 之后 PID 会在恢复帧立刻衰减，原版 3 帧内能从 1.0 掉到 0.35，频率断崖导致后续帧再次 jank。所以设了保护地板：

| 类型  | 保护地板                 | 保护帧数           |
| ----- | ------------------------ | ------------------ |
| crit  | `max(perf × 0.55, 0.50)` | `scale_frames(60)` |
| heavy | `max(perf × 0.50, 0.45)` | `scale_frames(30)` |

保护期内 `perf` 不会低于这个值，且地板本身会线性衰减，给出平滑的过渡窗口。

**heavy 处理**：

```
streak_m = clamp(1 - e^(-0.35 × streak), 0.30, 0.70)
inc = (阻尼期 ? 0.025 : 0.040) × clamp(sqrt(target/60), 1.0, 1.4)
perf += inc × norm × streak_m
```

**两个兜底机制**：

_紧急兜底_：窗口 ≥ 10、`avg < target × 0.65`、`perf < 0.50`、非加载态、已过冷启动时，`perf += clamp(0.06 × (1 - avg/target) × norm, 0.02, 0.10)`。

_地板死锁自救_：`perf ≤ floor + 0.01` 且 `avg < target × 0.50` 连续 `target × 2` 帧，就把 `perf` 重置到 `perf_cold_boot` 并 `pid.reset()`。这是在处理「PID 把 perf 压到地板、然后又爬不上来」的死锁。

#### 5.3.6 频率下发

`apply_freqs` 做三件事。

**强制重写**：每 `freq_force_reapply_interval`（默认 30）次强制重写一次，防厂商篡改。

**利用率软封顶**：

```
if 非 jank 且 非近地板 且 非热降频疑似 且 ema_fg_util > 0.05:
    util_cap = clamp(ema_fg_util / util_cap_divisor, 0.40, 1.0)    // divisor 默认 0.45
    effective_perf = perf × 0.92 + util_cap × 0.08
```

这是把「CPU 利用率」和「PID 算出的 perf」按 92:8 混合。

**分簇映射**：

```
w = capacity_weight                        // 该簇的算力权重
pow_adj = ratio ^ sqrt(w)
blend = clamp(w - 1, 0, 1.5) / 1.5
adj = ratio × (1 - blend × 0.5) + pow_adj × (blend × 0.5)
target_freq = find_nearest_freq(adj)
```

`capacity_weight` 优先由 `auto_compute_capacity_weights` 实测得出，失败回退 yaml 里的 `cluster_profiles`（默认 `[1.0, 1.5, 2.5, 3.5]`）。

**迟滞**：变化比例 `diff ≤ freq_hysteresis`（0.015）且非强制时跳过写入。

**写频校验**：写完后按 `verify_freq_interval_secs`（默认 1s）回读 `scaling_cur_freq`。**只校验下沿**——实际值低于目标（量化到频点表）才判定被 thermal 或 QoS 覆写，触发重新挂载和重写。高于目标不管。

写频时 `freq_hold_frames = 2`，两帧内跳过重写。

#### 5.3.7 生命周期

`FasManager` 是单实例的（`instance: Option<FasInstance>`）。

**激活流程**（`activate`）：

1. 复查白名单和配置
2. `exit_delay = rules.deactivate_delay_secs.max(1)`
3. 如果是另一个包，先 `deactivate_active`
4. 如果是同一个包，只 `set_game` 续期
5. 首次创建时 `FasController::new` 加 `load_policies`，如果 policies 为空返回失败
6. `governor.activate()` 切成 performance
7. 下发游戏信息、温度、温度阈值
8. 置 `fas_active_flag = true`

**延迟退出**：默认 `exit_delay = 15s`。失去前台不立刻退，而是设 `exit_deadline`。期内切回同一个包会取消。`tick` 每秒检查一次，到期才真正退出并返回 true，调用方按目标模式重新接管。

**温度刷新**：每 3 秒读一次，原始值除以 `battery_temp_divisor()`（拿不到就用 10.0）。

**FPS 查询**：`current_fps()` 返回帧窗口的均值，窗口内没有样本时返回 `None`，`status.csv` 里写 `-`。

#### 5.3.8 帧窗口统计

固定 120 帧的环形缓冲，维护增量 `sum` 和 `sq_sum`，每次 push 是 O(1)。**每 64 次 push 全量重算一次**，抑制浮点累积误差。144fps 下大约 0.44 秒重算一次。

提供三个统计量：`mean()`（均值，与齿轮决策同口径）、`recent_mean(n)`（最近 n 帧均值）、`stddev()`（总体标准差）。

未提供百分位统计，即不计算 p99 或 1% Low。

#### 5.3.9 FAS 配置默认值全表

来自 `src/fas_types.rs`：

| 字段                                     | 默认值                   |
| ---------------------------------------- | ------------------------ |
| `fps_gears`                              | `[30, 60, 90, 120, 144]` |
| `fps_margin`                             | 3.0                      |
| `pid.kp` / `ki` / `kd`                   | 0.050 / 0.010 / 0.006    |
| `cluster_profiles`                       | `[1.0, 1.5, 2.5, 3.5]`   |
| `auto_capacity_weight`                   | true                     |
| `perf_floor` / `perf_ceil` / `perf_init` | 0.22 / 1.0 / 0.45        |
| `perf_cold_boot`                         | 0.85                     |
| `freq_hysteresis`                        | 0.015                    |
| `heavy_frame_threshold_ms`               | 150.0                    |
| `loading_cumulative_ms`                  | 2500.0                   |
| `loading_normal_tolerance`               | 3                        |
| `loading_perf_floor` / `ceiling`         | 0.60 / 0.70              |
| `post_loading_ignore_frames`             | 5                        |
| `post_loading_perf`                      | 0.65                     |
| `post_loading_downgrade_guard`           | 90                       |
| `upgrade_confirm_frames`                 | 60                       |
| `downgrade_confirm_frames`               | 90                       |
| `upgrade_cooldown_after_downgrade`       | 90                       |
| `gear_dampen_frames`                     | 60                       |
| `downgrade_boost_perf_inc`               | 0.18                     |
| `downgrade_boost_duration`               | 45                       |
| `steady_decay_frame_threshold`           | 75                       |
| `steady_decay_perf_threshold`            | 0.70                     |
| `steady_decay_max_step`                  | 0.022                    |
| `steady_decay_min_step`                  | 0.004                    |
| `deactivate_delay_secs`                  | 15（clamp 到 [1, 600]）  |
| `jank_cooldown_frames`                   | 15                       |
| `max_inc_damped`                         | 0.045                    |
| `max_inc_normal`                         | 0.075                    |
| `damped_perf_cap`                        | 0.92                     |
| `app_switch_gap_ms`                      | 3000.0                   |
| `app_switch_resume_perf`                 | 0.60                     |
| `freq_force_reapply_interval`            | 30                       |
| `fixed_max_frame_ms`                     | 500.0                    |
| `cold_boot_ms`                           | 3500                     |
| `verify_freq_interval_secs`              | 1                        |
| `core_temp_threshold`                    | 0.0（禁用）              |
| `core_temp_throttle_perf`                | 0.70                     |
| `util_cap_divisor`                       | 0.45                     |

`normalize()` 会做 NaN/Inf 回退、perf 交叉约束（`floor ≤ init/cold_boot ≤ ceil`）、过滤非法的 `fps_gears`、保证 `steady_decay_min ≤ max × 0.6`。

**当前唯一的 FAS 条目**是 `com.hypergryph.endfield → endfield`（《明日方舟：终末地》）。它的配置：

```yaml
fps_gears: [30.0, 60.0, 90.0, 120.0, 144.0, 165.0]
fps_margin: 3.0
per_app_profiles:
  com.hypergryph.endfield:
    target_fps: [60]
    fps_margin: 3.0
core_temp_threshold: 45.0
```

`module/config/normal/fas-example.yaml` 仅作文档用途，不参与编译加载；其中的 `verify_freq_interval_secs: 3` 与 `target_fps: [30,60,120]` 为示例值，不代表实际配置。

### 5.4 fast_lock（vector 模式）

代码位于 `src/chiri/fast.rs`。该机制的实现最为直接：将所有 policy 的 `scaling_min_freq` 与 `scaling_max_freq` 均写入硬件最高频，同时写入 `scaling_governor = schedutil`。

硬件最高频的算法是把 `scaling_available_frequencies` 和 `boost_frequencies` 合并去重后取最大值。

**快照**：`init()` 先调 `release()`，然后逐 policy 快照 `governor`、`min_freq`、`max_freq`、`hw_max`。

**写序**：首次锁频时先把 max 拉高再拉 min，保证中间态 `min <= max`。

**恢复写序**（`restore_policy`）：

```
1. governor
2. max 放宽到 hw_max
3. min
4. max
```

该顺序保证任意中间态均合法。写入失败时快照保留并重试（通过 `retain` 保留失败项）。

**防篡改**：`tick()` 每 5 秒（`REWRITE_INTERVAL`）无条件重写 `max = hw_max`、`min = hw_max`。此处使用 `write_value_force`，**不经过 `current_freq` 去重**，原因是厂商可能将值改回，去重会导致漏写。`tick()` 返回距下次重写的剩余时间，供主事件循环纳入 deadline 计算。

README 中关于 vector 的说明为「请谨慎使用，此模式下 CPU 频率可能会锁定为最大」，代码实现与之一致。

### 5.5 热保护

热保护是叠加于上述各机制之上的压制层，在 `chiri/mod.rs` 中每 2 秒计算一次。

**双源评估**：电池温度和 CPU 温度各算一个 cap，取较小值。

```
eval_thermal_cap(temp, soft, hard, soft_cap, hard_cap, hysteresis, current):
    if temp >= hard:                  return hard_cap          // 0.40
    if temp >= soft:                  return soft_cap          // 0.70
    if temp < soft - hysteresis:      return 1.0
    // 回滞带内
    if 正在压制:                       return min(current, soft_cap)
    return 1.0
```

`hysteresis_c` 默认 3.0。回滞带内「取 min(current, soft_cap)」这一句是为了防阶跃。

**解除限速**：

```rust
const THERMAL_UNPRESS_STEP: f32 = 0.15;
```

压制加深立即生效，但**解除方向每 2 秒最多恢复 0.15**。该限制源自实测问题，源码注释有详细记录：温度围绕阈值震荡时，若解除方向立即全量恢复，温度会立刻反弹并触发再压制，cap 将在 40/70/100 之间以约 8 秒周期反复跳变。实测 8475 高负载游戏有 84% 的时间处于压制状态，每次跳变都把 `current_perf` 压回低位再缓慢爬升，表现为周期性掉帧。限速后恢复过程为 8 秒级渐进，温度反弹会在中途重新压制，不会穿透阈值。

**温度滤波**：温度必须传入滤波值，不可使用原始读数。源码注释说明了原因：`soc_max` 合成温区在实测中 2 秒内可跳变 ±25°C，不滤波时 cap 会以约 8 秒周期在 40/70/100 之间反复振荡。

两个滤波器：

| 滤波器   | 下限  | 上限  | 参数      | 位置           |
| -------- | ----- | ----- | --------- | -------------- |
| 电池温度 | -10.0 | 70.0  | 10.0, 1.0 | `chiri/mod.rs` |
| CPU 温度 | 5.0   | 110.0 | 12.0, 3.0 | 同上           |

**应用点**：热保护不在决策里生效，而是在 worker 的 `flush` 里 clamp，且带 `free_above` 豁免档：

```
if current_perf < free_above:      // 默认 0.80
    current_perf = min(current_perf, cap)
```

即当 perf 已升至 0.80 以上时不施加压制，交由内核温控处理。8550 的配置注释说明了该取值的理由：`soc_max` 在大型游戏高负载下常态超过 85°C，若软件在 75/85 即介入，会导致过半时间处于压制状态，表现劣于官方调度。当前取值 90/95 仅在接近内核起控点时提前轻度压制。

**默认阈值**：

| 字段               | 默认值 |
| ------------------ | ------ |
| `batt_soft_temp_c` | 41.0   |
| `batt_hard_temp_c` | 45.0   |
| `cpu_soft_temp_c`  | 90.0   |
| `cpu_hard_temp_c`  | 95.0   |
| `soft_perf_cap`    | 0.70   |
| `hard_perf_cap`    | 0.40   |
| `free_above`       | 0.80   |
| `hysteresis_c`     | 3.0    |

### 5.6 息屏场景模式（scenemode）

这个功能不在监控层，在 `chiri/mod.rs` 里。监控层只负责把 `ScreenStateChange` 事件送过来。

**进入条件**（全部满足）：

1. 屏幕是关的
2. `scenemode_enabled()` 为真（配置总闸）
3. 当前不在 scenemode
4. 冷却期已过（`SCENEMODE_COOLDOWN = 300s`）
5. 息屏时长 ≥ `scene_mode_delay_secs.max(60)`，8550 配的是 300 秒
6. 当前不是特调模式
7. `!fas_mgr.has_any_instance()`：FAS 优先，存在任一实例即不进入
8. 常驻簇（小核+大核）的最大 util < `SCENEMODE_SAT_UTIL`（0.75）

第 8 条存在例外：息屏时长 ≥ `delay × 4` 的「长息屏兜底」会跳过该门槛。其依据是：息屏时间足够长时，即使存在后台负载也应进入省电模式。

**生效内容**：

- 频率上限压到 15%（`scenemode.yaml` 的 `perf_ceil: 0.15`、`perf_init: 0.15`）
- 线程摆放全部交还系统：调 `affinity.release()`，把此前收窄的 cpuset 组按快照恢复成全核
- `core_ctl` 回 Normal 态（`set_power_state(false, false)`）

「下线 prime 簇」的源码注释与实际行为不一致：

`src/chiri/core_ctl.rs` 里确实实现了完整的离线逻辑（`STATE_SCENEMODE` 状态、`scenemode_targets()` 选目标、逐核写 `online = 0` 并回读验证、`reassert_offline()` 每 2 秒纠偏）。但 `set_power_state` 的第二个参数（scenemode 标志）在所有 4 个调用点都是 `false`，所以 `STATE_SCENEMODE` 这条分支**当前不可达**。

`mod.rs` 的注释说明了原因：当前 scenemode 语义为「全核心 + 仅压频」，不做核心或线程特化；原「prime 整簇下线 + 保留核独占」已废弃，core_ctl 回 Normal。

`CoreCtl.scenemode_offline` 配置项因此亦无实际作用：8550 与 8475 配置为 `true`、8998 配置为 `false`，行为一致。

**退出条件**：

- 亮屏：立刻清空计时和状态
- 饱和退出：常驻簇 max_util 持续 10 秒（`SCENEMODE_SAT_SECS`）≥ 0.75，说明后台负载压不住，先恢复全核再切回 reduce，并设 300 秒冷却
- `scenemode_enabled` 热重载为 false
- FAS 激活抢占

**scenemode 参数**（`module/config/normal/scenemode.yaml`）：

| 字段                                     | 值                |
| ---------------------------------------- | ----------------- |
| `enabled`                                | true              |
| `up_threshold`                           | 1.0               |
| `down_threshold`                         | 0.01              |
| `smoothing_up`                           | 0.01              |
| `down_rate_limit_ticks`                  | 1                 |
| `up_rate_limit_ticks`                    | 10                |
| `headroom_factor`                        | 1.0               |
| `perf_floor` / `perf_ceil` / `perf_init` | 0.0 / 0.15 / 0.15 |
| `up_jump_threshold`                      | 0.50              |
| `down_fast_threshold`                    | 0.30              |
| `touch_boost_enabled`                    | false             |

参数取值较为极端，但意图明确：`up_threshold = 1.0` 表示几乎不会判定为高负载，`smoothing_up = 0.01` 表示升频推进极慢，`perf_ceil = 0.15` 将上限压在 15%。

另有一个「息屏 doze」阶段，介入时间早于 scenemode：息屏后 `perf_ceil` 压至 `min(ceil, 0.30)`，`smoothing_up` 降至 0.10，`perf_floor` 设为 0.0，触摸升频关闭。

源码中保留了一处相关提示，针对上述已废弃的离线路径：prime 被下线后必须先恢复核心再 reload CLG，否则其 cpufreq policy 目录会消失，对应的 worker 将永久缺失。该结论对任何「先修改核心在线状态、再重建调频 worker」的流程均适用，因此该提示在功能废弃后仍予保留。

### 5.7 实验室（Rhine）

通过 `rhine.chr` 文件启用。机制实现位于 `src/rhine.rs`，模式定义位于 `module/config/rhine-init.yaml`。

**三种状态**：`Off`（未启用）、`On(key)`（启用了某个模式）、`ForceOff`（强制关闭）、`Bad`（文件内容非法）。

**保留字 `off`**：`is_force_off` 先剥离 ` #` 注释再做字面量比较，不交由 YAML 推断。原因是 `off` 在 YAML 1.1 中为布尔值 `false`，交由 YAML 解析会产生歧义。

**四个模式**：

| key           | 显示名   | global_mode | 关闭 FAS | 关闭 scenemode | 关闭特调 | 实际行为                                                         |
| ------------- | -------- | ----------- | -------- | -------------- | -------- | ---------------------------------------------------------------- |
| `vector`      | 矢量突破 | vector      | 是       | 是             | 是       | CPU 锁最高频（fast_lock）                                        |
| `contingency` | 危機契約 | contingency | 是       | 是             | 是       | CPU+GPU 全核最高频、performance governor、停线程迁移、后台压小核 |
| `babel`       | Babel    | babel       | 是       | 是             | 是       | 停线程迁移，后台小核、前台/顶部大核+超大核、系统进程大核         |
| `frozen`      | Frozen   | —           | —        | —              | —        | 空条目，当前无任何影响                                           |

`contingency` 和 `babel` 不走 boost 亲和（`is_boost_mode` 排除了它们），而是走 `governor_guard`（performance）+ `gpu_guard`（锁最高频）+ `fast_lock`。

**启用流程**：

1. 读磁盘 `meta.yaml`，读不到就放弃
2. 算出三个开关的生效原值
3. 写快照到 `rhine-back.chr`（含 `origin` 字段）
4. 用 `rewrite_meta_toggles` **一次性**改写 `fas_enabled`、`scenemode_enabled`、`thread_bind`。注释说明不能分两次写，否则会产生半套用的中间态
5. 设置进程内的实验室全局模式和特调禁用标志

如果第 4 步的 meta 写入失败，就删掉快照全身而退。

**还原流程**：没有快照也强制清运行时覆盖。快照非法就用内嵌默认收尾。meta 写入失败时**保留快照**，否则就再也回不去了。

**开机锁定**：锁定文件是 `chiri-labs.lock`，候选目录是 `/tmp` 和 `/dev`。选这两个是因为它们是 tmpfs，重启即消失——重启后锁不存在，正好对应「实验室不跨重启」的语义。

锁定期间不接受关闭请求，只会 `reassert` 重写状态。强制关闭（`ForceOff`）会清标记、走正常还原、把文件写回默认。`service.sh` 在开机时也会 `rm -f rhine.chr`。

### 5.8 DOWN 停摆

由 `down.chr` 控制，内容写 `down` 即进入停摆（忽略大小写，允许引号）。

停摆的含义是：**调度全部释放，只留采集和日志**。用于记录「ChiRi 不工作时设备自身的调度表现」，作为对照基线。

进入时：将当前模式记录到 `down_resume_mode`，清空 `pending_mode_after_fas`，依次释放所有 governor 与亲和设置，并将 `current_mode.chr` 写入 `"down"`。

退出时：优先取 `app_detect::last_determined_mode()` 的实时值（取不到则使用快照），按该模式重建接管，并**重置 `last_load_event`**；若不重置，刚重建的接管会被 WatchDog 判定为 stale 而立即释放。

停摆期间事件照收但不下发。例外：`ScreenStateChange` 会更新本地的 `is_screen_on` 状态——这些变量若在停摆期不吸收变化，退出停摆时监控层不会再补发（其自身持有的状态已是最新），亲和与 core_ctl 将按过期的屏幕状态应用。

WatchDog 使用 inotify 监听，掩码额外包含 `DELETE`，即删除文件同样视为解除。重试退避为 2 秒。

---

## 6. 线程摆放与核心控制

前文描述频率上限的决策，本章描述线程到核心的分配。两者共同构成完整的调度行为。

### 6.1 为什么需要手动摆线程

内核的 EAS 会自动把任务放到合适的核上，但它看到的信息有限：它不知道哪个线程是渲染线程、哪个是后台同步线程，也不知道某个应用正处于游戏场景。ChiRi 补上这部分信息，做法是：

- **关键线程绑核**：渲染线程这类对延迟敏感的线程，绑到大核或超大核
- **普通线程分散**：单核钉定，避免多个线程挤在一个核上做时间片轮转
- **后台压小核**：把后台线程限制在小核

### 6.2 前台线程的识别与枚举

前台 PID 由 `app_detect` 提供。判定条件：

```
fg_ok = cmdline 非空 且 不在亲和黑名单里
pin_fg = fg_ok 且 boost 模式 且 亮屏
```

`cmdline` 每轮读一次并缓存到 `self.fg_cmdline`，供后续复用，避免重复读 `/proc`。

线程枚举：每轮 `read_dir("/proc/{fg_pid}/task")` 一次。只有新增线程（首次见到或 `last_ticks == 0`）才读 `/proc/<tid>/stat`。

后台候选**不枚举 `/proc`**，而是直接读三个 cpuset 组的 `tasks` 节点：

- `/dev/cpuset/background/tasks`
- `/dev/cpuset/system-background/tasks`
- `/dev/cpuset/restricted/tasks`

这是个省开销的设计：后台进程可能成百上千，逐个读 `/proc` 代价太高。

### 6.3 设置亲和的系统调用

```rust
fn set_tid_affinity(tid: i32, cpus: &[usize]) -> bool
```

实现要点：

1. 用 `zeroed` 分配 `libc::cpu_set_t`。代码注释明确警告：不能用 `vec![0u8]`，因为它的对齐只有 1，强转成 `*const cpu_set_t` 不可靠
2. 用 `libc::CPU_SET` 置位
3. `max_cpu = size_of::<cpu_set_t>() * 8`。bionic 的 64 位版本是 `[u64; 16]`，支持 1024 个 CPU
4. **先 `sched_getaffinity` 读当前掩码，逐字节比较，一致就短路返回**。这个去重是从 AppOptR 学来的，能省掉大量无意义的系统调用
5. 不一致才 `sched_setaffinity(tid, size_of::<cpu_set_t>(), &mask)`

返回 true 表示成功，`ESRCH` 这类错误返回 false（线程已退出）。

### 6.4 选核算法

**核池划分**：

```
boost_pool = big ∪ prime
perf_pool  = big ∪ prime
prime_pool = prime，无 prime 的机型退化为 big
```

**打分选核**（`pick_core`）：

```
score(c) = core_utils[c] + pinned_count(c) × PINNED_WEIGHT
```

取 score 最低的核。`PINNED_WEIGHT = 0.4`。

这个权重的意义在注释里解释了：util 快照有滞后，线程刚钉上去时核心 util 还没反映它的负载。加权后能把后续线程推向其他核，避免多线程挤在同一个核上做时间片轮转。

文档与代码不一致：`affinity.rs` 模块头注释标注权重为「score = util + 钉线程数 × 0.2」，常量 `PINNED_WEIGHT` 实际取值为 0.4，以代码为准。

**带偏好的选核**（`pick_core_pref`）：主选 big，溢出才用 prime。具体是「big 里有未钉满的核就只在 big 里选，big 全部钉满才并入 prime」。

**关键线程不走打分**，走核组绑定。判定条件：

```
is_key_thread = (tid == main_tid) 或 comm ∈ KEY_THREAD_COMMS
```

`KEY_THREAD_COMMS` 是五个：

- `RenderThread`
- `GLThread`
- `GameThread`
- `UnityMain`
- `UnityGfxDeviceW`

boost 模式下若 cpuset 可用，关键线程**不写掩码**：cpuset 已将范围收窄至 `big ∪ prime`，其余交由 EAS 自调度更为合适。仅在 cpuset 不可用时写一次组掩码作为兜底。

**单核钉定的上限**：`MAX_PINS_PER_CORE = 3`。达到上限就返回 None，不再钉定。

该上限的实测依据（源码注释）：8475 运行终末地时约 180 个前台线程对应 4 个性能核，单核被钉定 30 余个线程，`psi_cpu some` 平均 28%，帧线程被辅助线程排队阻塞。达到上限后的线程留在 boost cpuset 中由 EAS 自调度。

### 6.5 迁移的防抖机制

线程迁移会打断 cache 与 TLB 的本地性，代价较高，因此设有完整的防抖机制：

| 常量                   | 值   | 作用                                                |
| ---------------------- | ---- | --------------------------------------------------- |
| `MIN_MIGRATE_INTERVAL` | 4s   | 迁移最小间隔                                        |
| `OVERLOAD_MARGIN`      | 0.15 | 重钉分数滞回：目标核 score 要比 home 核低至少这么多 |
| `RETURN_COOLDOWN`      | 16s  | 迁离某核后这段时间内禁止迁回，打破 A→B→A 振荡       |
| `REPIN_DEBOUNCE`       | 8s   | 过载重钉的防抖间隔                                  |
| `CORE_OVERLOAD_UTIL`   | 0.70 | 核心过载阈值，超过就重钉到低占用核                  |

`OVERLOAD_MARGIN` 的注释解释了滞回的必要性：缺少滞回时，0.70 阈值边缘的 util 快照噪声即可驱动无收益的迁移，「两个核互相显示略优」正是两核乒乓的直接来源。

`RETURN_COOLDOWN` 取 16 秒的原因是其与 `REPIN_DEBOUNCE`（8 秒）同拍，两倍时长足以错开乒乓周期。

### 6.6 提升与降级的阈值

该组阈值用于判定线程在普通池与性能核之间的迁移时机：

| 常量                       | 值   | 含义                       |
| -------------------------- | ---- | -------------------------- |
| `PROMOTE_UTIL_PCT`         | 25.0 | 提升阈值                   |
| `LITTLE_HIGH_WATER`        | 0.70 | 小核高水位                 |
| `LITTLE_PROMOTE_UTIL_PCT`  | 10.0 | 小核侧提升阈值             |
| `FG_BUSY_UTIL_PCT`         | 30.0 | 非关键前台线程的「忙」判定 |
| `FG_BUSY_RELEASE_UTIL_PCT` | 15.0 | 忙线程的释放水位           |
| `KEY_BIND_RELEASE_WATER`   | 0.50 | 关键线程组绑定的解除水位   |
| `BIG_HIGH_WATER`           | 0.90 | 大核高水位                 |
| `DEMOTE_UTIL_PCT`          | 5.0  | 降级阈值                   |
| `DEMOTE_STREAK`            | 3    | 降级需要的连续次数         |

提升阈值 25% 和释放水位 15% 构成滞回。注释解释了为什么不用 `DEMOTE_UTIL_PCT`（5%）当释放水位：那样 5% 到 30% 之间的中等负载线程会长期滞留在性能核，在小核高水位期间能耗反而上升，和调优目标相反。

`FG_BUSY_UTIL_PCT` 这条规则针对的是一个具体现象：8550 实测小核常有一颗被单个非关键前台线程打满（util 0.70 到 0.95），而大核平均有 3 个以上空闲、prime 几乎空转。只绑关键线程不足以解除小核饱和。

### 6.7 cpuset 与 uclamp 的操作

**boost 模式进入时**：

- 写 `top-app` 和 `foreground` 的 cpus 为 `big ∪ prime`
- 应用 uclamp（min 和 max）
- 后台线程压到 little

**boost 退出时**：

- 恢复前台组的 cpus
- 恢复 uclamp
- 后台仍然压 little

**快照恢复**（FastLock 模式）：首次进入 boost 时，`ensure_snapshot` 记录 5 个组的完整路径和 cpus，以及 `top-app` 的 `cpu.uclamp.min` 和 `cpu.uclamp.max`。

- `restore_foreground_groups` 只回写包含 `top-app` 或 `foreground` 的项
- `release` 回写全部

**后台降权**：往 `background` 和 `restricted` 的 `cpu.uclamp.max` 写百分比值（8550 是 50）。**刻意不包含 `system-background`**，因为那里是媒体、音频这类用户可感知的系统后台。

`uclamp.max` 的归属采用三态协议，通过 `Option<bool>` 区分三个所有者：

| 值            | 含义     |
| ------------- | -------- |
| `Some(true)`  | 特调所有 |
| `Some(false)` | FAS 所有 |
| `None`        | 其他     |

源码注释要求修改该部分前必须遵守此协议。

内核版本门槛：uclamp 只在 `> 5` 或 `== 5 && patch >= 3` 的内核上启用。

### 6.8 core_ctl 核心控制

代码在 `src/chiri/core_ctl.rs`。

**cluster 发现**：遍历所有 cpufreq policy，读 `policy{id}/related_cpus`（失败回退 `affected_cpus`），取第一个 CPU，节点目录是 `/sys/devices/system/cpu/cpu{first}/core_ctl`。只有能读到 `min_cpus` 的才算数。

**只改 `min_cpus` 一个参数**，其它一律不动。这是个保守的选择，避免和厂商的热管理打架

| 状态      | 行为                                          |
| --------- | --------------------------------------------- |
| Boost     | `min_cpus = cluster_size`（整簇常在线）       |
| Normal    | `min_cpus` 回写快照值                         |
| Scenemode | prime 簇逐核写 `online = 0`（**当前不可达**） |

**scenemode 的离线操作**（保留代码，但调用点恒传 `false`）：目标只包含 prime 簇，且剔除 CPU0。逐核写 `/sys/devices/system/cpu/cpuN/online = 0` 并**回读验证**。成功后记录原始值供恢复。`reassert_offline` 每 2 秒纠偏一次，防止厂商把核拉回来。

这段逻辑本身是完整的，问题在调用侧：`set_power_state` 的 scenemode 参数在 4 个调用点全是 `false`，`STATE_SCENEMODE` 分支进不去。scenemode 现在的实际行为是交还线程摆放、core_ctl 回 Normal，只压频率上限。

**静默降级**：没有 core_ctl 节点时只打一条 `corectl-unavailable` 日志，保持空表继续跑。没有 cpuset 时「独占小核」降级为只做自钉。

**恢复失败不 clear**：恢复失败的核会留在 `offlined` 列表里周期重试，直到全部恢复。

**启动清理**：`force_online_all()` 强制全部核心上线。这个操作的必要性在注释里说明了：如果上次是中途被杀，prime 离线状态下 cpufreq policy 目录会消失，对应的 worker 会永久缺失。

### 6.9 亲和黑名单

黑名单文件是 `src/chiri/affinity_blacklist.yaml`，编译期嵌入。用户和 WebUI 都不能改。

解析规则：逐行读，跳过空行和 `#` 开头的行。`re:` 前缀的条目预编译成正则，编译失败就跳过并告警。匹配时正则用 `is_match`，其余用整串相等。

三个校验点：前台 `cmdline`、后台候选的 `comm`、以及首次读到进程时缓存的 `cmdline`。

另有两项内置兜底规则：空 `cmdline`（内核线程）与以 `/` 开头（native 二进制服务）均视为黑名单。

### 6.10 其它实现细节

**`GroupBind` 枚举**：以一个枚举替代原有的两个布尔量（`group_pinned` + `busy_bound`），源码注释指出「漏改一处会静默失效」。

**`busy_window_update` 不依赖时间窗**：分片采样下同一线程两次采样可能相隔较久，因此仅做两窗防抖。

**`group_bind` 不占钉核计数**，`restore_group_mask` 也不触碰 `core_pinned`。

**cpuset 写告警键分离**：`cpuset-cpus` 和 `cpuset-restore` 用不同的告警键，否则一边成功会清掉另一边全失败的记录。

**`write_governor` 不用 `try_write_file`**：注释说 `try_write_file` 会 chmod 0444，对需要后续恢复的 sysfs 节点是毒药。

**gpu 的 SIGKILL 残留防护**：如果快照里 `orig_min == hw_max`，判定为上次被 SIGKILL 留下的残留，恢复时写硬件最低频而不是那个值。

**日志节流**：`overload_hold` 打点每 tid 半分钟一条，`place` 快照按签名去重且最少 30 秒一次。注释记录：不节流时终末地一局能刷 1.1 万行以上，占 devimp 日志的 41%。

---

## 7. 自愈与防篡改

该部分在异常处理上投入了较多代码。单独成章的原因是，其设计取向比算法本身更能反映系统的可靠性策略。

### 7.1 快照-恢复模式

所有会改动系统状态的模块都遵循同一个模式：

1. 接管前读原始值存快照
2. 改动
3. 释放时按快照写回
4. 写失败保留快照重试

涉及的模块：`fast.rs`、`governor.rs`、`gpu.rs`、`core_ctl.rs`、`affinity.rs`、`cpu_load_governor.rs`、`tuned.rs`。

**写序约束**是这套模式的通用要求：任何时刻都要保证 `min <= max`。升频先写 max 再写 min，降频先写 min 再写 max。恢复时先把 max 放宽到硬件最高，再处理 min。

### 7.2 防篡改

厂商的 perfmgr 会主动改这些 sysfs 节点。ChiRi 的应对是周期性重写：

| 模块       | 重写周期          | 方式                        |
| ---------- | ----------------- | --------------------------- |
| fast_lock  | 5s                | `write_value_force`，不去重 |
| CLG worker | 1s（超时分支）    | 重写当前频率                |
| FAS        | 30 次 apply_freqs | 强制重写                    |
| 模式文件   | 5s                | 自愈重写                    |

CLG 那条注释说明了为什么 1 秒粒度够用：「厂商篡改也是秒级」。

### 7.3 启动残留清理

| 清理项           | 做法                                                                     |
| ---------------- | ------------------------------------------------------------------------ |
| governor 残留    | `GovernorGuard::cleanup_residue()` 把所有 `performance` 写回 `schedutil` |
| core_ctl 残留    | `force_online_all()` 强制全核上线                                        |
| 旧 `config.yaml` | main 删除模块根与各 SoC 子目录下的遗留文件                               |
| staging 目录残留 | `collect_staging_dirs()` 回收上一轮 `ziped_*` 目录                       |

`cleanup_residue` 的存在说明作者考虑过「守护进程被 SIGKILL，governor 卡在 performance」这种情况——那时 CPU 会一直跑最高频。

### 7.4 双实例防护

三层：

1. **shell 层**：`service.sh` 启动前先读 `logs/watchdog.pid` 杀掉旧 WatchDog，再 `killall -9 chiri`
2. **进程层**：`flock(LOCK_EX | LOCK_NB)` 抢 `daemon.lock`
3. **文件命名层**：devimp 文件名带毫秒时间戳，两个实例会写两份不同文件（不解决冲突，但便于事后发现）

第 2 层是兜底。shell 层的清理依赖 `logs/watchdog.pid`，这个文件丢了就失效，此时旧 WatchDog 3 秒后会把新实例和旧实例并行拉起来。

### 7.5 panic 自愈

主事件循环外层包裹 `catch_unwind` 与重启循环，最多重启 5 次，每次退避 1 秒；超过后 `exit(1)` 交由 WatchDog 处理。

监控层线程由 `spawn_guarded` 包裹：panic 时落盘日志后 `exit(1)`，同样交由 WatchDog 按进程级重启；线程级重启难以保证状态一致。

### 7.6 日志系统的自愈

日志子系统的实现细节较多，以下列出关键部分：

**SelfHealingAppender**：每次写入均重新以 `create + append` 打开文件，文件被外部删除后自动重建。`rotate` 全程吞掉错误，不使用 `unwrap`。

**常驻句柄 + 巡检**：`status.csv` 使用常驻 append 句柄，每行仅 1 次 `write` 系统调用。每 16 行执行一次巡检（检查是否超过 8MB、是否被删除）。源码注释解释了 16 这一取值：常驻句柄在文件被删后指向孤儿 inode，写入不会报错，只能通过巡检发现；16 行约等于 16 秒的自愈窗口。原取值为 256，会导致长时间「无文件可读」。

**目录预算**：`logd/` 和 `devimp/` **各自独立**计量，上限 128MB，删到低于 96MB，且**目录内最新的一个文件永不删除**。

源码注释中记录了一段历史问题：原实现将两个目录合并为一个 128MB 总预算并跨目录按 mtime 删除，导致 devimp 的膨胀被计入 logd 的额度；而 logd 中归档包的 mtime 恒旧于正在写入的 devimp 文件，因此归档包被优先删除，包括启动时刚生成的那一份。

**短会话丢弃**：`logs/daemon.log` 的**首行**时间戳约等于上一轮 daemon 的启动时刻。如果距离本次启动不足 30 秒，判定为短会话，**直接清空 `logs/` 和 `devimp/`，不打包**。

原因：崩溃循环每轮仅产生少量日志，逐轮打包会将 `logd/` 填满空壳归档，淹没真正的现场数据。

三条硬约束：

1. **时区**：`daemon.log` 的时间戳是设备本地时间，所以必须用 `libc::mktime` 解读而不是 `timegm`。用错会偏一个时区（东八区偏 -8 小时），30 秒判定彻底失效
2. 只读前 256 字节取首行。`daemon.log` 可能有 50MB，绝不整读
3. 判定必须在 rename **之前**。rename 之后 `logs/` 已经换成空目录，读不到上一轮日志了

清空时 `watchdog.pid` 必须保留，否则 WebUI 的「关闭调度」会失效。

**日志重启门限**：`logs/` 或 `devimp/` 本会话累计写入达到 16MB 即 `exit(0)`， WatchDog 3 秒后拉起新进程，新进程启动时执行归档。该设计遵循「事件触发、零额外 syscall」原则：三条写日志的路径均在落盘后调用 `note_write` 记账，**刻意不遍历目录**。

约束：无 WatchDog 时（调试直跑或孤儿态）不退出，并将计数清零——退出后无人拉起，调度将永久停止。

**staging 目录遗留**：日志归档采用「rename 到同级临时目录，再交异步线程打包」的流程。若进程在打包完成前退出，已被 rename 的 `ziped_*` 目录将留在模块根：它既不在 `logd/` 也不在 `devimp/`，清理逻辑无法覆盖，后续启动也不会接管，表现为「特定条件下启动调度不打包」。

修法是在 rename **之前**扫描并回收遗留的 `ziped_*` 目录，与本次的一并打包。源码注释将其归纳为规则：「新增任何 rename 到同级临时目录再异步处理的流程都必须配同款回收，否则中断即永久丢失」。

### 7.7 配置防篡改

调优配置在编译期通过 `include_dir!` **整体嵌入二进制**，磁盘副本仅供 WebUI 展示。因此修改磁盘上的 `feature.yaml` 不会生效，必须重新编译。

`rules.yaml` 和 `src/chiri/*.yaml` 用 `include_str!` 单文件嵌入。

`meta.yaml` 是唯一的例外，为用户可修改的抬头文件，其中若干字段（日志等级、语言、FAS 开关、息屏场景模式开关、线程绑定开关、开发记录）会被读取。其解析启用 `deny_unknown_fields` 与白名单校验，非法时整体回退到内嵌默认值。

另有一处自愈逻辑 `sync_meta_snapshot`：meta 文件缺失时用嵌入原文重建；内容非法时整体覆盖并在尾部附加警告注释。

### 7.8 防环判断

配置热重载存在循环风险：重载配置会写入 meta，写入 meta 又触发监听，形成无限循环。源码中多处提到「防环判断」，具体做法是比对内容，一致时不写入。

---

## 8. 配置体系

### 8.1 文件角色划分

| 文件                                       | 位置        | 谁能改         | 用途                                            |
| ------------------------------------------ | ----------- | -------------- | ----------------------------------------------- |
| `module/config/meta.yaml`                  | 磁盘 + 嵌入 | 用户 / WebUI   | 用户可改项：日志等级、语言、各开关              |
| `module/config/feature.yaml`               | 仅嵌入      | 不能改         | 调优段，不落盘                                  |
| `module/config/{soc}/meta.yaml`            | 磁盘 + 嵌入 | 用户 / WebUI   | SoC 专属 meta                                   |
| `module/config/{soc}/feature.yaml`         | 仅嵌入      | 不能改         | SoC 专属调优段                                  |
| `module/rules.yaml`                        | 磁盘 + 嵌入 | 不能改（只读） | `global_mode` / `app_modes` / `dynamic_enabled` |
| `module/config/normal/tuned_profiles.yaml` | 仅嵌入      | 不能改         | 特调参数组                                      |
| `module/config/normal/scenemode.yaml`      | 仅嵌入      | 不能改         | 息屏场景模式参数                                |
| `module/config/normal/fas.yaml`            | 仅嵌入      | 不能改         | FAS 白名单                                      |
| `module/config/normal/fas/<名>.yaml`       | 仅嵌入      | 不能改         | 每应用 FAS 调优                                 |
| `module/config/rhine-init.yaml`            | 仅嵌入      | 不能改         | 实验室模式定义                                  |
| `src/chiri/special_tuned.yaml`             | 仅嵌入      | 不能改         | 特调白名单                                      |
| `src/chiri/affinity_blacklist.yaml`        | 仅嵌入      | 不能改         | 亲和黑名单                                      |
| `module/config/i18n/{zh,en}.ftl`           | 仅嵌入      | 不能改         | 多语言文案                                      |

「仅嵌入」意味着磁盘上可能有一份同名文件（构建时拷进去的），但运行时读的是二进制里那份。所以改了不生效。

### 8.2 加载优先级

`Config::load` 的顺序：

1. 基准 = 嵌入的 `feature.yaml`。SoC 命中就用 `{soc}/feature.yaml`，否则用根目录的
2. 叠加嵌入的 `meta.yaml` 默认值
3. 叠加磁盘 `meta.yaml`，**只覆盖写了键的字段**
4. `affinity.enabled &= thread_bind`，`core_ctl.enabled &= thread_bind`
5. `merge_tuned_profiles()` 用嵌入的 `normal/tuned_profiles.yaml` 整体覆盖 akmode 段和 `tuned_profiles` 段，逐组 normalize
6. `merge_scenemode()` 只提取 `scenemode` 段
7. `thermal.normalize()`、`affinity.normalize()`、`set_battery_options()`

配置路径的选择在 `get_config_path()`：SoC 命中且 `config/{hint}/meta.yaml` 存在就用它，否则用 `config/meta.yaml`。

### 8.3 热重载

`config_watcher` 线程用 inotify 监听**配置目录**，掩码是 `MODIFY | CLOSE_WRITE | MOVED_TO`。

监听目录而不是文件，是因为原子替换（写临时文件再 rename）会换掉 inode，盯着文件本身会失效。inotify 不支持递归，所以监听的是生效配置的**父目录**。

重载成功后置 `config_dirty` 标志。主事件循环每轮 `swap(false)` 消费这个标志，然后调对应的 `reload_config`。

有几条约定：

- **inotify 实例跨重载复用**。旧实现每轮重建实例会丢事件
- 重载必须**不影响 Doze 模式参数**
- 调度器参数要**在 100ms 内热重载**，不等模式切换
- 配置重载不能影响正在运行的 FAS 实例（FAS 配置是编译期嵌入，不参与重载）

### 8.4 必需文件断言

`build.rs` 的 `assert_required_configs` 会检查 8 个必需文件：

```
meta.yaml
feature.yaml
normal/tuned_profiles.yaml
normal/scenemode.yaml
normal/fas.yaml
rhine-init.yaml
i18n/zh.ftl
i18n/en.ftl
```

并且校验各 SoC 子目录的 `meta.yaml` 和 `feature.yaml` 成对存在。任一缺失或不成对即 `panic`，编译失败——宁可编译失败，也不静默回退到代码默认值。

### 8.5 配置结构体

`Config` 顶层字段：

| 字段                                                    | 类型                                  |
| ------------------------------------------------------- | ------------------------------------- |
| `meta`                                                  | Meta                                  |
| `function`                                              | Function（功能开关）                  |
| `io_settings`                                           | IO_Settings                           |
| `cpu_idle`                                              | CpuIdle                               |
| `reduce` / `default` / `boost` / `vector` / `scenemode` | Mode                                  |
| `scene_mode_delay_secs`                                 | u64，默认 300                         |
| `thermal`                                               | ThermalGuardConfig                    |
| `akmode`                                                | SpecialTunedConfig                    |
| `tuned_profiles`                                        | HashMap\<String, SpecialTunedConfig\> |
| `affinity`                                              | AffinityConfig                        |
| `core_ctl`                                              | CoreCtlConfig                         |

`Meta` 字段与默认值：

| 字段                | 默认   | 说明                                      |
| ------------------- | ------ | ----------------------------------------- |
| `loglevel`          | "INFO" | OFF / ERROR / WARN / INFO / DEBUG / TRACE |
| `language`          | "en"   | en / zh                                   |
| `dev_record`        | false  | 开发记录开关                              |
| `fas_enabled`       | true   | FAS 总闸                                  |
| `scenemode_enabled` | true   | 息屏场景模式总闸                          |
| `thread_bind`       | true   | 线程绑定总闸                              |
| `power_avg`         | false  | 功耗平均显示                              |
| `power_max_w`       | 12     | 功耗仪表满量程                            |
| `notify`            | true   | 系统通知                                  |
| `oplus_chg`         | false  | OPlus 充电节点                            |
| `oplus_dual_cell`   | false  | 双电芯                                    |
| `voltage_double`    | false  | 电压倍率                                  |
| `current_double`    | false  | 电流倍率                                  |
| `voltage_divisor`   | 1000000 | 电压除数（标准 ABI µV；OPlus 私有节点由安装脚本写 1000） |
| `current_divisor`   | 1000000 | 电流除数（标准 ABI µA；OPlus 私有节点由安装脚本写 1000） |
| `nofix`             | false  | 跳过覆盖外部文件                          |

`AffinityConfig` 默认值：

| 字段                        | 默认 |
| --------------------------- | ---- |
| `enabled`                   | true |
| `top_app_uclamp_min_pct`    | 0    |
| `top_app_uclamp_max_pct`    | 0    |
| `pin_foreground_threads`    | true |
| `background_uclamp_max_pct` | 50   |

`CoreCtlConfig` 默认值：`enabled = true`、`scenemode_offline = true`。

### 8.6 需要注意的一处陷阱

`CpuLoadGovernorConfig` 未启用 `deny_unknown_fields`。这意味着将 Yumi 的字段（例如 `smoothing_down`、`slow_down_scale`、`down_fast_mult`）写入 8550 的 `feature.yaml` 时会被**静默忽略**，不产生任何报错。

`module/config/feature.yaml`（根目录）为 Yumi 的配置，包含上述字段。ChiRi 的配置位于 `module/config/{soc}/feature.yaml`。两者不可混用。

---

## 9. 日志与诊断

### 9.1 三个日志文件

| 文件                                | 用途               | 轮转                     |
| ----------------------------------- | ------------------ | ------------------------ |
| `logs/daemon.log`                   | 主日志（log4rs）   | 50MB，保留 3 个备份      |
| `logs/status.csv`                   | 1 秒一行的状态宽表 | 8MB，保留 1 个备份       |
| `devimp/devimp_<包名>_<时间戳>.log` | 开发诊断日志       | 单文件 128MB，保留 20 份 |

### 9.2 daemon.log 格式

```
[2026-09-19 21:47:46] [INFO] [chiri::config] 消息内容
```

时间戳是**设备本地时间**。模块路径会剥掉重复的 crate 名前缀：`chiri::chiri::config` 变成 `chiri::config`。这是因为包名和 `src/chiri/` 子模块同名，直接打会重复。

### 9.3 status.csv 的 22 列

| #   | 列名               | 含义                                             |
| --- | ------------------ | ------------------------------------------------ |
| 1   | `timestamp`        | 本地时间 HH:MM:SS.mmm                            |
| 2   | `type`             | 行类型，目前只有 `snap`                          |
| 3   | `mode`             | 当前模式                                         |
| 4   | `package`          | 前台包名                                         |
| 5   | `charge`           | charging / discharging / full / not_charging / - |
| 6   | `screen_on`        | 0 / 1                                            |
| 7   | `batt_temp`        | 电池温度                                         |
| 8   | `cpu_temp`         | CPU 温度                                         |
| 9   | `thermal_cap_pct`  | 热压制上限百分比                                 |
| 10  | `thermal_free_pct` | 热豁免档百分比                                   |
| 11  | `clg_active`       | CLG 是否接管                                     |
| 12  | `psi_cpu_some`     | CPU 压力                                         |
| 13  | `psi_io_some`      | IO 压力                                          |
| 14  | `psi_mem_some`     | 内存压力                                         |
| 15  | `gpu_busy_pct`     | GPU 占用                                         |
| 16  | `batt_voltage_v`   | 电池电压                                         |
| 17  | `batt_current_ma`  | 电流（命名遗留，实际单位是安培）                 |
| 18  | `batt_power_w`     | 功率                                             |
| 19  | `wakeups`          | 唤醒次数增量                                     |
| 20  | `migrations`       | 迁移次数增量                                     |
| 21  | `freq_trans`       | 调频次数增量                                     |
| 22  | `fps`              | FAS 实测帧率，非 FAS 时为 `-`                    |

数值列是**全精度写入**的，用 f32 最短往返表示。取整是显示层的职责（WebUI 统一 `toFixed(1)`）。

**新增列必须追加在末尾**。WebUI 的 `STATUS_COLUMNS` 按列数过滤残缺行，往中间插入会打乱既有索引。

### 9.4 devimp 的 48 列

devimp 是开发诊断日志，按前台包名分组。文件名 `devimp_<包名>_<MMDD-HHmmss>.log`，包名段过滤为字母数字和 `. _ -`，截断到 64 字符。没有包名时用 `nopkg`。

行类型：

| type    | 含义                                 | 写入频率                        |
| ------- | ------------------------------------ | ------------------------------- |
| `tick`  | 每个决策 tick × 每个核心组的调频轨迹 | 按签名变化才写，无变化 2 秒心跳 |
| `snap`  | 1 秒环境上下文                       | 1 秒一行                        |
| `place` | 前台线程的落点核快照                 | 每 4 轮（8 秒）                 |
| `aff`   | 亲和迁移动作                         | 发生即写                        |
| `core`  | 逐核 util 与钉核计数                 | 每 4 轮（8 秒）                 |
| `tgtop` | 全系统 top 消耗者                    | 30 秒一轮，最多 5 行            |
| `event` | 模式/屏幕/热/配置/FAS 生命周期变化   | 发生即写                        |

48 列的字段名依次为 `ts`、`type`、`mode`、`screen_on`、`pid`、`package`、`tid`、`comm`、`cluster`、`core`、`from_core`、`to_core`、`util_pct`、`max_util`、`over_cores`、`under_cores`、`cur_perf`、`tgt_perf`、`cur_freq_khz`、`max_freq_khz`、`decision`、`deb_up`、`deb_down`、`reason`、`pinned`、`thermal_cap_pct`、`touch`、`psi_cpu`、`psi_io`、`psi_mem`、`gpu_busy`、`batt_v`、`batt_i`、`batt_p`、`wakeups`、`migrations`、`freq_trans`、`batt_temp`、`cpu_temp`、`clg_active`、`cpu_cur_khz`、`cpu_max_khz`、`cpu_min_khz`、`cpu_governor`、`gpu_cur_khz`、`gpu_max_khz`、`gpu_min_khz`、`gpu_governor`。

末 8 列（2026-09-21 新增，**追加在末尾**，仅 `snap` 行填充，其余行类型留 `-`）记录**内核当前实际值**，
与既有的 `cur_freq_khz` / `max_freq_khz`（调度器写入的决策值）互补：那两列是「我们写了多少」，
这 8 列是「内核现在实际是多少」。多 policy / 多 GPU 节点以 `;` 分隔，每项 `policy<id>:<值>`
或 `<设备名>:<值>`，节点读不到写 `-`。**只在 devimp 开启时采集**（每秒一次 sysfs 读，常态零开销），
且不做缓存——这些值会被内核 governor 与 TunedGovernor 随时改写，缓存只会给出过期数据。

**GPU 那 4 列当前默认关闭**（`chiri/mod.rs` 的 `GPU_SNAPSHOT_ENABLED = false`）：读 GPU 频率节点会把
GPU 从低功耗状态唤醒，8550 实测同一 playback 场景功耗因此 +47%（1.76 → 2.59 W），采集代价盖过收益；
关闭期间这 4 列恒为 `-`，CPU 那 4 列不受影响、照常每秒采集。实现保留在
`chiri/gpu.rs::devfreq_snapshot()`，确需该数据时把开关改回 true 即可（开启后还会叠加 10 s 节流）。

文件头会写入设备元信息：`module.prop` 内容、`build.prop` 解析结果、`getprop` 查询结果。源码注释中记录了一处问题：`ro.product.model` 这类跨分区属性必须通过 `getprop` 命令获取，直接读 `/system/build.prop` 会读空。

**`tgtop` 行**的列语义有复用：`pid` 是 TGID，`comm` 是 cmdline 首段，`util_pct` 是窗口运行占比，**多核并行可以超过 100%**（比如 320% 约等于 3.2 个核满载），不做 clamp。

### 9.5 日志归档

启动时 `archive_on_startup` 做四件事，顺序不能变：

1. 短会话判定（读 `daemon.log` 首行时间戳，只读 256 字节）
2. 回收遗留的 `ziped_*` staging 目录
3. 把 `logs/` 和 `devimp/` 原子 rename 为同级临时目录，名字加 `unique_staging` 去重
4. 起一个一次性子线程 `log_archiver`，串行打包

打包本身由外部脚本 `module/scripts/pack.sh` 完成。产物是**未压缩的 `.tar`**，命名去掉 `ziped_` 前缀，比如 `0918-220224.tar`。

_这里有一处注释与实际不符_：`main.rs` 的注释写的是「归档产物为 `logd/ziped_<ts>.zip`」，但实际生成的是无压缩 tar。注释是历史遗留，`logger.rs` 里另一处注释明确写了「2026-09-17 起不再由 Rust 手写 ZIP」。

打包失败时保留临时目录并写 `ARCHIVE_FAILED.txt`。此时 logger 还没初始化，打不了日志。

必须复制回 `logs/watchdog.pid`。 WatchDog 先于 daemon 启动，WebUI 靠这个文件终止 WatchDog，被归档带走会导致「关闭调度」失效。

### 9.6 开发记录（devimp）的开关

总开关是 `logger::set_devimp_active`，由主循环按 `Config.meta.dev_record` 同步。未开启时所有写入点零 IO。

写入量控制：`tick` 行按 cluster 节流，决策签名变化才写入，无变化时每 2 秒一次心跳。该优化将稳态写入量从 akmode 的每核心组 25 行/秒降至 0.5 行/秒。

devimp 另有一项 30 秒一轮的 `tgtop` 快照，用于定位待机期「小核 util 长期 60%+」的后台来源。`place` 行仅覆盖前台线程，`tgtop` 补充全系统视角。

### 9.7 锁序约定

devimp 模块包含四把锁：`DEVIMP_MODE`、`DEVIMP_FG_PKG`、`DEVIMP_TICK_STATE`、`DEVIMP_WRITER`。约定的要求是**一律不嵌套持有**。

`DEVIMP_WRITER` 是写路径的汇合点，调度、CLG worker、亲和线程都会写它。它的临界区内只做文件 IO 和自身状态修改，绝不获取其他锁。曾经在它的临界区内清 `TICK_STATE` 的代码已经全部移出。

这条规则写成了约定：「新增写入路径必须遵守，否则反向获取即成环」。

---

## 10. 外围组件

### 10.1 eBPF 探针

虽然目录叫 `yumi-ebpf`，但 ChiRi 全靠它取数据。

**帧探针**：

| 项       | 值                                                                        |
| -------- | ------------------------------------------------------------------------- |
| 类型     | uprobe                                                                    |
| 挂载点   | `libgui.so` 的 `Surface::queueBuffer`                                     |
| 回传     | RingBuf，容量 `0x8000`（32768 字节）                                      |
| 数据结构 | `FrameTimestampEvent { pid: u32, ktime_ns: u64 }`                         |
| 采集内容 | `pid = bpf_get_current_pid_tgid() >> 32`，`ktime_ns = bpf_ktime_get_ns()` |

RingBuf 的 `reserve` 失败就静默丢弃。

**CPU 探针**：

| 项              | 值                                       |
| --------------- | ---------------------------------------- |
| 类型            | tracepoint                               |
| 挂载点          | `sched/sched_switch`                     |
| 硬编码偏移      | `OFF_PREV_PID = 24`，`OFF_NEXT_PID = 56` |
| 单次 delta 上限 | 10 秒（超过丢弃）                        |

每核累计数据用 `PerCpuArray`（`max_entries = 1`），线程级用 `HashMap` 32768 项，进程级用 `HashMap` 1024 项。

**遥测探针**（ChiRi 专属，可选）：

| 探针                        | 挂载点                       | 计数器             |
| --------------------------- | ---------------------------- | ------------------ |
| `handle_sched_wakeup`       | `sched/sched_wakeup`         | `WAKEUP_COUNT`     |
| `handle_sched_migrate_task` | `sched/sched_migrate_task`   | `MIGRATE_COUNT`    |
| `handle_cpufreq_transition` | `cpufreq/cpufreq_transition` | `FREQ_TRANS_COUNT` |

三个都是 `PerCpuArray<u64>`，用户态每 2 秒读累计值取增量，发 `DaemonEvent::BpfStats`。挂载失败不影响主探针，只打一条警告。

这个「可选」的设计是必要的：不同内核编译配置下 tracepoint 未必存在，硬性要求会导致整个 eBPF 加载失败。

### 10.2 构建系统

**build.rs 的四件事**：

1. `rerun-if-changed = module/config`，目录增删触发重编译
2. `assert_required_configs` 断言必需文件
3. `build_ebpf` 编译 eBPF
4. `write_webui_assets` 生成 WebUI 资源清单

**eBPF 编译命令**：

```
cargo build --target bpfel-unknown-none -Z build-std=core --target-dir <OUT_DIR>/ebpf_target
```

`-Z build-std=core` 为 nightly 独占。项目依赖 nightly 的原因即为该编译开关，而非使用了不稳定的语言特性。

release 构建时会把 `CARGO_PROFILE_RELEASE_OPT_LEVEL` 局部覆盖为 `"2"`。原因是新版 bpf-linker 移除了 `-Oz` 和 `-Os`，只支持 `-O0` 到 `-O3`，而 workspace 根的 `opt-level = "z"` 会导致链接失败。

**bpf-linker 获取**（`ensure_bpf_linker`）按顺序尝试：

1. PATH 里已有的
2. `tools/bin` 里已装的
3. Windows 上直接返回 Err（源码依赖 `os::unix` API，无法构建）
4. `cargo install bpf-linker`，严格检查 exit code、stderr 和产物存在

第 3 步失败后，`build_ebpf` 会捕获错误并调用 `write_ebpf_stub()` 写一个 64 字节的占位文件，让 `include_bytes!` 能解析。这是为了不阻塞 rust-analyzer 和本地 `cargo check`。CI 行为不变。

**xtask**：

```
cargo xtask build              # 完整构建 + 打包 zip
cargo xtask build --no-pack    # 只组装目录，不打包
```

产物名格式是 `name-version-提交数-日期`。构建顺序：

1. 清 `output/.temp`
2. `npm run build` 构建 WebUI
3. `cargo +nightly ndk --platform 26 -t arm64-v8a build -Z build-std -r` 交叉编译
4. 拷贝 `module/` 目录
5. 删除仅嵌入的文件（`feature.yaml`、`tuned_profiles.yaml`、`scenemode.yaml`、`fas.yaml`、`rhine-init.yaml`、`normal/fas/` 目录、各 SoC 的 `feature.yaml`）
6. 拷二进制到 `core/bin/chiri`
7. 拷 `webui/dist` 到 `webroot/`
8. 打包（Deflate level 9）

第 5 步为必要步骤：这些文件已编译进二进制，不应出现在发布包中，否则会造成可修改的误解。

### 10.3 CI

`.github/workflows/build.yml`：

| 项         | 值                                                     |
| ---------- | ------------------------------------------------------ |
| 触发       | push main、tags v\*、PR main、手动                     |
| Node       | 24                                                     |
| Rust       | nightly，target `aarch64-linux-android`，带 `rust-src` |
| bpf-linker | v0.11.0，从 GitHub API 下载预编译版                    |
| NDK        | r29 + cargo-ndk                                        |
| 构建       | `cargo xtask build --no-pack`                          |

**注释剥离**：用 perl 删掉 `module/` 下 `*.sh`、`*.block`、`*.yaml`、`*.yml`、`*.ftl` 的整行 `#` 注释，保留 shebang，排除 `module/META-INF/*` 和 `module/scripts/*`。

这是为了压缩发布包体积。排除 `module/scripts/` 是因为 `pack.sh` 是对外暴露的稳定接口，注释有价值。

### 10.4 WebUI

技术栈：Svelte 5（runes 语法）、TypeScript、Vite 8、Vitest 5、js-yaml、ak-ui 1.0.0（CSS 核心）。

规模：`src/` 下 40 个源文件加 7 个测试文件，其中最大的三个是 `state.svelte.ts`（889 行）、`app.css`（567 行）、`OverviewView.svelte`（497 行）。

#### 10.4.1 分层与依赖方向

依赖是单向的，不允许回指：

```
ksu.exec → kernel/shell（可注入） → contract → data → views
```

| 层       | 职责                     | 代表文件                                                                                                                        |
| -------- | ------------------------ | ------------------------------------------------------------------------------------------------------------------------------- |
| kernel   | 唯一的 shell 执行出口    | `kernel/ksu.ts`（181 行）、`kernel/shell.ts`（49 行）                                                                           |
| contract | 文件读写、路径、错误分类 | `contract/read.ts`、`paths.ts`、`errors.ts`、`daemon.ts`、`meta.ts`、`export.ts`、`lab.ts`、`down.ts`、`power.ts`、`sources.ts` |
| data     | 解析算法与状态管理       | `data/status-csv.ts`、`daemon-log.ts`、`mode.ts`、`lab.ts`、`state.svelte.ts`                                                   |
| views    | 界面                     | 6 个视图 + 8 个组件                                                                                                             |

`kernel/shell.ts` 提供两个 runner：`ksuShell`（`live: true`）和 `inertShell`（全部 reject）。模块加载时就根据 `hasKsu()` 决定用哪个，之后可以用 `setShell()` 替换。这个 `isLive()` 是契约层所有 `unsupported-env` 判定的源头。

`ksu.exec` 的实现方式如下。使用 `chiri_exec_${Date.now()}_${seq++}` 生成全局回调名并挂载到 `globalThis`，同时**内置超时兜底**：20 秒未回调则删除该全局函数并 reject。以 `settled` 标志保证回调、超时、异常三条路径仅 settle 一次。

#### 10.4.2 错误分类

契约层的返回类型是三分支：

```
ReadResult<T> = ok | absent | failed
```

`absent` 还细分四种原因：`chiri-only`（只有 ChiRi SoC 才有这个文件）、`not-created`（还没生成）、`daemon-stopped`（守护进程没跑）、`unsupported-env`（不在设备上）。这个区分让界面能给出准确的提示，而不是笼统的「读取失败」。

文件缺失的判定靠四条正则：`/no such file/i`、`/not found/i`、`/does not exist/i`、`/no such directory/i`。命中才算 `absent`，否则是 `failed`。

#### 10.4.3 读取窗口

所有文件读取均使用 `tail -c N` 截断，不整读。原因是 `daemon.log` 可达 50MB，整读会阻塞 WebView

| 文件                            | 窗口     |
| ------------------------------- | -------- |
| 通用读取上限                    | 512KB    |
| `meta.yaml`                     | 64KB     |
| rules / 特调白名单 / FAS 白名单 | 各 256KB |
| `module.prop`                   | 8KB      |
| `daemon.log`                    | 128KB    |
| `status.csv`                    | 256KB    |
| `LiveTime.chr`                  | 64 字节  |
| lab / down / power 状态文件     | 各 4KB   |

overview 读 `status.csv` 末行时只取尾部 4096 字节。

#### 10.4.4 路径安全

`paths.ts` 是命令拼接的唯一出口。`shQuote` 实现 POSIX 单引号转义（把 `'` 换成 `'\''`）。

`isSafeConfigRel` 用于配置相对路径，拒绝空串、以 `/` 开头、含反斜杠、含 `..`，放行正则 `/^[A-Za-z0-9._/-]+$/`。`isSafeFileName` 更严，正则 `/^[A-Za-z0-9._-]+$/`，连 `/` 都不允许。

模块根目录取 `moduleInfo().moduleDir`，要求以 `/` 开头，否则回退到 `/data/adb/modules/chiri`，结果缓存。测试可以用 `setModuleRootForTest` 覆盖。

#### 10.4.5 存活判据

守护进程的存活判定通过读取 `LiveTime.chr` 实现。守护进程每 15 秒写入一次 `MM:SS` 格式的当前时间，WebUI 的容差设为 20 秒。

年龄算法的实现如下：

```
age = (now_seconds_of_hour - file_seconds_of_hour + 3600) % 3600
```

如果文件时间比当前时间超前（时钟回拨或跨小时边界），取模会落进 (1800, 3600) 这个区间，天然被判为过期，不需要额外分支。

**刻意不用 flock 判断，也绝不删 `daemon.lock`**。原因是锁文件是守护进程的单实例保护，WebUI 碰它会把守护进程搞坏。

关闭调度的命令序列：

```
cat logs/watchdog.pid  → 校验是纯数字 → kill 它
killall -9 chiri || pkill -9 chiri
rm -f logs/watchdog.pid
cmd notification post -d chiri-status
```

顺序不能反：必须先杀 WatchDog，否则杀了守护进程 3 秒后它又被拉起来。

#### 10.4.6 meta.yaml 的写入

写入需在保留用户注释的前提下逐字段替换。

`replaceTopLevelField` 用的正则：

```
/^([^:\s#]+)(\s*:\s*)(.*)$/
```

只匹配**缩进为 0 的行**（判断方式是比较 `line.length` 和 `line.trimStart().length`），所以嵌套字段里同名的键不会被误改。跳过注释行和以 `-` 开头的行。

行内注释以 `' #'`（带前导空格）为界切分，保留下来。原值如果带引号，新值也渲染成双引号，保持风格一致。

字段在文件里找不到时，`appendTopLevelField` 会把它补在文件末尾，而不是静默失败。

写入流程：先校验全部字段 → 读当前快照 → 校验不通过直接拒绝 → 逐字段做行替换 → 写 `<path>.webui.tmp` → `mv -f` 原子替换 → 回读确认。

`utf8ToBase64` 用 `TextEncoder` 加 0x8000 分块，避免长内容把调用栈撑爆。

校验规则比较宽松，只报两类硬错误：未知键、类型不符。**缺字段是合法的**（会走守护进程的默认值）。`power_max_w` 要求 `(0, 200]`，三个 divisor 要求 `(0, 1e9]`。

`name`、`author`、`nofix` 三个字段不可写，只展示。

#### 10.4.7 状态管理

`state.svelte.ts` 是全应用的中心，889 行。几个关键机制：

**并发去重**。`loadOverview` 使用一个 `overviewJob` 变量共享进行中的 promise。源码注释特别说明不能写成简单的 `if (loading) return`，否则后来的调用者会拿到空状态，导致 `isChiri` 被判为 false 而静默跳过后续逻辑。`loadLab`、`loadLogs`、`loadApps`、`loadDown` 各有相同机制。

**写入回读与回滚检测**。`commitDraft` 记录期望值，写完后回读；若 `patchKeys` 中存在任一值与期望不符，则标记 `writeReverted`。该机制对应「守护进程判定配置非法后整体重置为默认值」的情形，界面会据此提示「已恢复默认值」，而非提示写入成功。

**实验室收敛轮询**。`commitLab` 在写入之后，若 `settleMs > 0`，会以 `settleMs` 为间隔反复读取状态，直至不再处于 `force-off` 中间态或超时。`forceDisableLab` 传入 400ms，最长等待约 4 秒。

**meta 写入互斥**。`metaWritePending` 作为互斥标志，由 `setPowerAvg` 与 `setBatteryFields` 共用。`setBatteryFields` 中若 `oplus_chg` 被设为 true，会**强制清空** `voltage_double` 与 `current_double`，原因是 OPlus 私有节点与手动倍率互斥。

**功耗无值的归因**。`powerStaleWhy()` 在功耗不可读时给出具体原因：末行 `charge` 非 `discharging` 则归因于充电状态，平均模式且息屏则归因于息屏，其余情况归因为「读数不可用」。

**导出进度**。1.5 秒轮询一次，最多 160 轮（约 4 分钟）。`pollFails < 3` 才判定失败，以避免偶发的命令超时导致误判。百分比上限设为 99，以避免长时间停留在 100 的显示。

#### 10.4.8 视图

主视图四个（overview、config、apps、logs），二级视图两个（lab、battery）。hash 路由，非法值回退 overview。lab 和 battery 高亮时回落高亮 config。

**overview**：每秒自刷新。首卡展示模块名、版本、作者与守护进程状态；ChiRi 设备上右侧为功耗面板（数值加 `ak-progress` 进度条，满量程取 `power_max_w`）。模式卡执行三级去重：家族名仅在 `clg`/`lab`/`stardust` 时显示，模式 id 仅在与其显示名不同时显示。守护进程停止时不展示陈旧的 `current_mode`。另含导出面板与停止调度面板，两项危险操作均通过 `ConfirmSheet` 二次确认。

**config**：字段包括日志等级（6 档）、daemon 语言（zh/en）、`dev_record`、`fas_enabled`、`scenemode_enabled`、`thread_bind` 四个开关。`notify` 长期处于 disabled 状态，功能未完成。被实验室接管的开关会被置灰，并在提示中说明具体原因。保存需要满足 `metaValid && configState === 'ok'`。

**apps**：搜索框按包名或 label 做大小写不敏感匹配。列表项显示 label、包名（与 label 不同时）、以及三个标签（特调·回退模式 / FAS·配置名 / 模式·appMode）。规则面板展示 `yumiScheduler`、`dynamicEnabled`、`globalMode`、`ignoredApps` 和 `app_modes` 数量。

**logs**：分段控件切数据源（daemon/status）和级别（5 档）。daemon 页逐行渲染时间、级别、模块、消息，用 `data-level` 驱动配色。status 页渲染 7 列表格（时间、模式、包名、电池温度、GPU 占用、功耗、充电态）。

「跟随底部」的判定逻辑如下：滚动位置距底部 24px 以内视为在底部；一旦滑离即取消跟随，仅可通过「回到底部」按钮恢复。两个 `$effect` 依赖的是**整个数组**而非数组长度，因为日志行内容会变化而长度不变。

**lab**：四个模式卡按 `rhine-init.yaml` 的定义顺序排列。`frozen` 显示为「预留」且按钮 disabled。锁定时点关闭会弹风险确认。`onMount` 里先 `loadOverview`（为了得出 `isChiri`）再 `loadLab`，顺序不能反。

**battery**：三个面板——来源（OPlus 私有节点、双电芯）、量程（倍压倍流互斥置灰、电压电流校准）、功耗（口径开关、满量程）。

#### 10.4.9 组件

| 组件           | 行数 | 要点                                                                              |
| -------------- | ---- | --------------------------------------------------------------------------------- |
| `Panel`        | 76   | `.ak-card` 加左侧 `--signal` 色条，带 `actions` 插槽                              |
| `StateBox`     | 81   | empty / missing / error / loading 四态，loading 有脉冲动画（尊重 reduced-motion） |
| `ToggleField`  | 73   | `role=switch`，pending 时显示圆点                                                 |
| `NumberField`  | 147  | **失焦或回车才提交**；聚焦期间不被外部值覆盖；非法值保留原文并标红                |
| `Segmented`    | 56   | `role=radiogroup`，方向键循环导航，roving tabindex                                |
| `ConfirmSheet` | 95   | 原生 `<dialog>.showModal()`，busy 时拦截 Esc                                      |
| `LiveStatus`   | 29   | 三态状态芯片                                                                      |

`NumberField` 的「失焦才提交」为有意设计：若在输入过程中被外部刷新覆盖，会造成明显的操作困扰。

#### 10.4.10 数据解析算法

**status.csv**：22 列常量与守护进程的 `STATUS_HEADER` 严格对齐。解析时逐行 `split(',')`，**通过 `cells.length !== 22` 过滤残缺行**，通过 `cells[0] === 'timestamp' || cells[1] !== 'snap'` 剔除表头与非 snap 行。这也是守护进程侧强调「新增列必须追加在末尾」的原因：在中间插入会使所有行的列数判断失效。

数值转换：空或 `-` 转 null，`Number()` 结果非有限也转 null。布尔只认 `'1'`。字符串把 `-` 归一为空串。

默认保留最新 300 行，返回顺序是文件顺序（旧到新），展示方向由调用方 `reverse()`。

**daemon.log**：行格式正则

```
/^\[(\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2})\]\s*\[([A-Za-z]+)\]\s*\[([^\]]*)\]\s?([\s\S]*)$/
```

解析时使用一个 `started` 标志：首行不匹配则直接丢弃（读取窗口从文件中间截断，首行可能不完整）。匹配之后，遇到不匹配的非空行则**并入上一条的 message**，因为守护进程可能输出多行日志。

级别归一化把六档之外的值都归到 `OTHER`。级别过滤的顺序是 `['TRACE','DEBUG','INFO','WARN','ERROR','OFF','OTHER']`，用下标比较决定保留。

**模式派生**：`describeMode(id, specialModes)` 的判定顺序是——空串 → unknown；等于 `down` → down/danger；等于 `scenemode` → stardust；等于 `fas` → fas/accent；命中 CLG 表（reduce/default/boost）→ clg；命中 LAB 表（vector/contingency/babel）→ lab；在特调模式集合里 → special/accent；否则 unknown。

家族一共 7 个：`clg`、`fas`、`special`、`lab`、`down`、`stardust`、`unknown`。

**lab 状态解析**：`stripComment` 通过 `search(/\s#/)` 定位注释，即 `#` 前必须有空白才视为注释，因此 `vector#x` 不会被误判。`isForceOff` 依次剥离注释、trim、剥离成对引号，再做小写比较。`yamlScalar` 剥离一层同型引号，**引号不成对时返回 null**，以避免 `"vector'` 这类输入被误判。

`parseLabState` 的判定顺序：剥离空行与注释行 → 空则为 off → 多行则为 invalid → 先判 force-off → 再执行 yamlScalar → 命中 key 表则为 on，否则为 invalid。

`labWarnings` 包含一处细节：`force-off` 属预期中的中间态，直接返回而不报漂移警告。

#### 10.4.11 mock-shell

在无 `ksu` 环境时自动装载（`main.ts` 中 `hasKsu()` 为假即装载）。装载必须先于 `mount(App)`，因为 `router` 与 `state` 在模块求值阶段即会调用 `shell()`。

URL 参数：

| 参数       | 取值                         | 默认    |
| ---------- | ---------------------------- | ------- |
| `?soc=`    | `chiri` / `yumi`             | chiri   |
| `?state=`  | `normal` / `empty` / `error` | normal  |
| `?daemon=` | `running` / `stopped`        | running |

内部是内存文件系统（`Map` 存文件、`Set` 存目录）。`seed()` 从构建期嵌入的 `virtual:chiri-config` 播种 `module.prop`、`rules.yaml`、`meta.yaml`（chiri 用 `8550/meta.yaml`）。导出用的白名单快照会过滤掉 `re:` 行，与真实导出行为一致。

伪造数据：`fakeDaemonLog` 生成 90 行（11 条样本循环），`fakeStatusCsv` 生成 60 行（5 行模式循环，`fps` 列只有 fas 行有值）。

`liveTimeText()` **按读取时刻动态生成**心跳，`stopped` 时取 8 分钟前。这样「存活判据」这条逻辑在 mock 下也能被真正验证，而不是永远返回固定值。

命令回放用一组正则匹配：`EXISTS_CMD`、`TAIL_CMD`、`LS_CMD`、`WRITE_CMD`、`KILL_CMD`、`EXPORT_START_CMD`、`EXPORT_POLL_CMD`。`state=error` 时 TAIL/LS/WRITE 返回 `Permission denied`。未识别的命令直接 reject。

#### 10.4.12 构建与测试

`vite.config.ts` 中包含一个 `embeddedConfigPlugin`，用于生成虚拟模块 `virtual:chiri-config`：将 `module/config` 与 `src/chiri` 按目录收录，`module/rules.yaml` 与 `module/module.prop` 按单文件收录，**排除 `feature.yaml` 与 `*-example.yaml`**（前者不应向界面暴露，后者仅作文档）。开发服务器会监听 yaml 增删改并触发整页刷新。

`base: './'`，这样 WebView 用 `file://` 协议也能加载。

测试是纯逻辑断言，跑在 node 环境，共 7 个文件：

| 测试文件                | 覆盖内容                                                             |
| ----------------------- | -------------------------------------------------------------------- |
| `lab.test.ts`（229 行） | lab 状态解析全形态、**直接读 `rhine-init.yaml` 比对 `LAB_TAKEOVER`** |
| `data-parse.test.ts`    | CSV 列数常量与表头一致性、残缺行过滤、日志续行合并、级别过滤         |
| `data-model.test.ts`    | 白名单解析、模式家族映射、rules 容错                                 |
| `contract.test.ts`      | 路径安全、引号转义、字段行替换、meta 校验                            |
| `i18n.test.ts`          | zh/en 键集合全等、源码里所有 `t('key')` 都有定义                     |
| `live-time.test.ts`     | 心跳解析边界、年龄取模、容差                                         |
| `down.test.ts`          | 停摆状态解析与写入往返                                               |

`lab.test.ts` 中「直接读取 yaml 比对」的断言作用显著：它将界面中的 `LAB_TAKEOVER` 定义与 `rhine-init.yaml` 的真实内容绑定，任一侧修改而另一侧未同步即会导致测试失败。该断言用于防止「界面声明会关闭 FAS，实际未关闭」这类静默不一致。

### 10.5 analyze：CSV 分析台

`analyze/index.html` 是 2117 行的单文件页面，加 `analyze.css`（751 行）与 `ak-ui.css`（93KB，仅使用其中的 `:root` 变量与 `ak-progress`）。采用原生 ES2020，无框架、无构建，全部逻辑位于单个 `<script>` 中。

该页面用于离线分析 `status.csv`：载入日志后可查看曲线、定位异常、生成排行榜。

#### 10.5.1 功能清单

**加载**：默认通过 `fetch('status.csv', {cache: 'no-store'})` 自动载入，同时支持按钮选择文件（`accept=".csv,.tsv,.txt,text/csv"`）与全窗口拖放（以 `dragDepth` 计数处理嵌套元素的拖入与拖出）。

**图表交互**：滚轮以指针位置为锚缩放（因子 1.2），指针拖动平移（位移超过 4px 才判定为拖动，避免误触点击），双击重置，底部提供等宽平移滑块，另有放大/缩小/重置按钮。

**显示开关**：Y 轴自适应、归一化、平均值表、断裂轴、表头、文件名列、详细列名、列显示多选面板。

**筛选**：三级搜索框（AND 语义）、充放电状态下拉、亮屏下拉，均设有 150ms 防抖。

**表格**：分块加载、行点击高亮、列显隐、自绘滚动进度条。

**排行榜**：10 个榜单卡片，按包名聚合。

#### 10.5.2 CSV 解析

分隔符探测 `detectDelimiter` 在 `,`、`;`、`\t`、`|` 四个候选中选择「引号外出现次数最多」的一项。计数需感知引号状态，否则引号内的逗号会被计入。四个候选的计数均为 0 时报错。

切分使用 RFC4180 的简化状态机，支持 `""` 转义与 `\r` 丢弃。其关键在于**分片**：每次处理 256K 字符，处理完一片后检查耗时，超过 24ms 才 `await` 让出主线程。这种「按耗时而非按大小让出」的策略优于固定分片：性能较好的设备让出次数更少，性能较弱的设备让出次数更多。

编码处理上，先剥离 UTF-8 BOM；含 `\u0000` 的内容直接判定为二进制并拒绝。

解析完成后另有一项**首行数值探测**：若第一行每格均可转为数字，说明该 CSV 不含表头，此时将列名改为 `col1`、`col2`…，并把第一行并入数据。该兜底用于处理外部导出的裸数据。

行列不齐时补空或截断，并记一个 `ragged` 计数用于告警。

#### 10.5.3 时间轴

时间列的识别靠正则 `/^(ts|timestamp|time|datetime|date|时间|时刻)$/i`。

解析支持两种格式：`HH:MM`、`HH:MM:SS`、`HH:MM:SS.mmm`，以及能被 `Date.parse` 吃下的 ISO 格式。毫秒不足 3 位时补零。

**有效性门槛**：时间列的非空命中率要达到 0.8 才启用时间轴，否则回退成行号轴。这是为了防止把一个碰巧叫 `time` 的数值列当成时间。

**时钟回绕**：`toSeconds` 返回的是当天秒数，跨过 00:00 会倒退。算法是遍历时维护一个偏移量，只要出现倒退就整体加 86400：

```
while (last !== null && v + off < last) off += 86400
```

代码注释明确否定了「用半天做阈值」的写法，因为那会漏掉超过 12 小时的会话。这个 while 可以跨多天。

空值用前一个值填充。

#### 10.5.4 数值列判定

对每一列统计两个数：`total` 只计非 NA 的格子，`hit` 是能转成数字的格子数。判定条件是 `total > 0 && hit / total >= 0.8`。

NA 集合是 `{'', '-', 'NA', 'N/A', 'null', 'NULL', 'nan', 'NaN', 'none'}`。

NA 不计入分母这一处理决定了列的取舍：**偶发缺测的列仍会被保留为曲线**（缺测处断线），而整列皆为 NA 的列（例如非 FAS 场景下的 `fps` 列）因 `total = 0` 被自动排除。

每扫描 8 列让出一次主线程。若不存在任何数值列，则给出明确告警：「没有可绘制的数值列（仅显示表格）」。

#### 10.5.5 断裂轴

归档日志按会话切分，文件之间与文件内部均可能存在长时间空洞。若按真实时间绘制，空洞将占用大量横向空间。

`GAP_MIN = 60` 秒，超过就算一个空洞。空洞来源有两类：

1. **文件内空档**：同一文件相邻样本时间差超过 60 秒
2. **文件间空档**：取每个文件的时间段 `[首, 末]`，按起点排序后扫描最大右端，如果下一段的起点减去这个右端超过 60 秒，就补一个空洞

第二类是关键——它处理的是「归档把一次会话切成两个文件」造成的接缝。

所有区间排序后合并重叠部分。然后算压缩量：

```
gapSpan = 空洞总长度
R0 = max(10, gapSpan × 0.015)         // 全图约 1.5%，下限 10 秒
R  = min(R0, (b - a) × 0.45)          // 每段不超过自身长度的 45%
```

然后是两个互逆的映射函数：

**`xDisp(t)`（原始时间 → 显示坐标）**：顺序累减。`t >= b` 时累加整段压缩量；`t > a` 时累加 `(t-a) × (1-ratio)`；否则跳出。返回 `t - 累计压缩`。

**`unX(x)`（显示坐标 → 原始时间）**：`x <= ds` 时返回 `x + prev`；落在压缩段内时返回 `a + (x-ds)/ratio`；否则 `prev += shrink`。

刻度标签和悬停命中测试都走 `unX`，所以用户看到的时间是真实时间，不是被压缩过的坐标。

关掉「断裂轴」开关时，两个函数退化成恒等映射，视窗重算。

#### 10.5.6 绘图管线

**坐标系**：边距 `M = {l: 88, r: 16, t: 16, b: 30}`。绘图区尺寸取 `chart.clientWidth/Height`（缺省 900×420），并设有下限保护（`max(10, ...)`）。

**视窗**：在可见曲线上求 x 极值，经 `xDisp` 得到显示域。`clampView` 保证最小跨度为 `span/4000`（低于该值时围绕中心扩展），超出全域则贴边。

**抽稀**：`MAX_POINTS = 2600`。等距抽样，步长 `max(1, ceil(len/2600))`，并且**强制保留末点**，否则曲线尾端会缺失。抽稀结果按 `${dataVer}|${x0}|${x1}` 缓存。

**路径构建**：先计算相邻采样点时间差的中位数 `med`，取 `gapThresh = max(med × 4, 10)` 秒。时间差超过该阈值的相邻点，主实线断开（使用 `M` 而非 `L`），同时写入一段虚线连接。

被筛选掉的段单独使用 `dm` 路径（opacity 0.2），判据为段任一端所在行不可见。路径缓存键为 `${seriesKey}|${y0}|${y1}|${normalize}|${filterKey}`。

**y 轴范围**：归一化模式下恒为 `[0, 1]`。否则使用单槽缓存（键含数据版本与视窗），仅统计视窗内的可见点，上下各留 6% 余量。上下界相等时以 `|lo| × 0.05 || 1` 兜底，避免零跨度导致除零。

**断裂带标记**：每个空洞两侧各绘制一条 `dasharray 2 4` 的竖线，透明度 0.55。出视窗或显示宽度小于 3px 时跳过。

**跨文件虚线**：同名曲线按首点时间排序，相邻文件之间「前一个的末点 → 后一个的首点」只要时间递增即绘制虚线连接。此处**不设阈值**，源码注释说明曾使用 10 秒阈值，结果漏掉了 0 至 2 秒的接缝。

**悬停**：独立覆盖层 SVG，`pointer-events: none`。将 clientX 换算为数据坐标，容差为 `(视窗跨度 / 绘图宽度) × 8`。每条曲线通过二分查找定位最近点。tooltip 优先显示原始单元格文本，缺值时退回 `fmtNum` 格式化。

重绘通过 `requestAnimationFrame` 合帧，一帧仅重绘一次，鼠标快速移动时不会堆积。

**数值格式**：`fmtNum` 按量级分档——≥1e6 取整、≥1000 或 ≥10 取整或 1 位、≥1 取两位、<1 取三位，加千分位分隔，**绝不使用科学计数法**。

#### 10.5.7 排行榜

按 `package` 列聚合，缺失归到 `(无前台)`，`charge` 缺失视为放电。**每个样本计 1 秒**

| 榜单     | 聚合方式 | 数据列            | 口径                       |
| -------- | -------- | ----------------- | -------------------------- |
| 前台时长 | count    | —                 | 样本数即秒数               |
| 平均功耗 | mean     | `batt_power_w`    | 仅放电样本                 |
| 峰值功耗 | max      | `batt_power_w`    | 仅放电样本                 |
| 平均电流 | absmean  | `batt_current_ma` | 仅放电，先取绝对值再平均   |
| CPU 负载 | mean     | `psi_cpu_some`    | 均值                       |
| GPU 占用 | mean     | `gpu_busy_pct`    | 均值                       |
| CPU 温度 | mean     | `cpu_temp`        | 均值                       |
| 电池温度 | mean     | `batt_temp`       | 均值                       |
| 唤醒次数 | sum      | `wakeups`         | 累计和                     |
| 亮屏占比 | ratio    | `screen_on`       | 值为 1 的样本 / 总行 × 100 |

`count` 用全部匹配行数，其余用有效值数。排序降序后取前 8。

一个性能上的注意点：`computeRanks` 每轮全量重扫所有行，没有增量缓存。十万行数据下这是可感知的开销。

#### 10.5.8 表格

`ROW_CHUNK = 800`。渲染方式是拼 HTML 字符串后单次 `insertAdjacentHTML`——这是性能关键，逐行创建 DOM 节点会慢得多。

行身份用三个 data 属性：`data-f`（文件 id）、`data-r`（文件内行号）、`data-g`（全局行号）。

**追加而非重画**：`rowsShown` 每次加 800，非重置时只追加新块。如果每次都重画，十万行会退化成 O(n²)。

**窗口切换**：如果目标行超前超过 4000 行、早于当前渲染起点、或者已渲染跨度超过 20000 行，就直接把渲染窗口切到 `[目标-2000, 目标+800)`，而不是逐块补齐。

**筛选**：`rowVisible` 依次判充放电精确匹配、亮屏精确匹配、三级搜索（每个查询对整行**任意列**做 `toLowerCase().includes`，多个查询是 AND）。筛选开启时 `extendScanForMatches` 会持续扫块直到凑够 800 个可见行，上限 400 次扫描。

**列控制**：多文件且开启「文件名列」时前置一列，始终含「#」列，合并所有文件的表头去重，再滤掉 `hiddenCols`。列面板动态生成复选框，隐藏状态按列名跨文件记忆。

**图表与表格联动**：表格行点击设置高亮，图表会画一条竖线；图表点击采样点会跳转并高亮对应行。跳转时用 `scrollTop` 相对调整把行滚到容器垂直居中，**刻意不用 `scrollIntoView`**——那会把整个页面滚下去。

#### 10.5.9 字体子集化

页面用的中文字体做了子集裁切：用 `pyftsubset` 对 Noto Sans SC 取「本页全部字符 ∪ 可打印 ASCII ∪ `—·×→`」，得到 441 个字，每个字重约 65KB（原本单字重 1.14MB）。JetBrains Mono 只取拉丁字母、数字和标点，woff2 各 21 到 22KB。

`@font-face` 用 `local()` 回退系统字体，`font-display: swap`。

字体合规上只放 SIL OFL 1.1 许可的字体，明确禁止使用微软雅黑、SimSun、PingFang 这类系统自带字体。

#### 10.5.10 几个实现上的瑕疵

读代码时发现几处，列出来供参考：

- **`fitY` 开关实际半失效**。`yRange()` 从不读取 `state.fitY`，始终按视窗自适应；这个开关现在只在重建时触发归一化缓存清空，对绘制没有分支影响。
- **「继续载入」按钮已被 HTML 注释隐藏**，但 JS 里仍用 `if (moreBtn)` 守卫，属于保留代码。
- **`boot()` 的失败提示被整体注释掉**，自动载入 `status.csv` 失败时静默无提示，用户会以为页面坏了。
- `extendScanForMatches` 的 `guard < 400` 上限意味着极端筛选条件下最多额外扫 32 万行。

### 10.6 AppOptR：线程亲和参考实现

`AppOptR/` 是个独立项目，README 说它是「Android 应用 CPU 亲和性管理工具」。它不在 ChiRi 的构建链里，ChiRi 也没有链接它的代码。但 ChiRi 的线程摆放明显参考了它，具体是三处：`sched_getaffinity` 短路去重、cpuset 的组织方式、以及 eBPF 事件驱动的思路。

项目分两部分：`AppOpt/`（用户态，版本 2.2.6）和 `AppOpt-ebpf/`（内核态，版本 0.3.1）。只支持 Linux/Android 64 位，32 位直接 `compile_error!`。

#### 10.6.1 命令行接口

参数是手工解析的，没用 clap

| 参数        | 默认             | 约束                            |
| ----------- | ---------------- | ------------------------------- |
| `-c <file>` | `./applist.conf` | 缺参数报错退出                  |
| `-s <sec>`  | 2                | 必须 ≥1 的整数                  |
| `-b <name>` | `AppOpt`         | 不能为空、不能含 `/`            |
| `-w`        | 关闭             | 只监听 `127.0.0.1:8889`         |
| `-v`        | —                | 有 BTF 时输出会追加 `eBPF` 字样 |
| `-h`        | —                | 打印规则语法示例                |

优先级是命令行 > `./AppOpt.json` 设置文件。设置文件存 `web_enable`、`mode`、`check_interval`、`cpuset_name`、`config_file` 五项，Web 面板改动会写回它。

#### 10.6.2 规则语法

三种形态：

```
com.example.game=0-3              # 直接指定 CPU 编号
com.example.game=e-core           # 按核心类型
com.example.game {                # 块语法，按线程名
    RenderThread=6-7
}
```

也支持单行闭合 `pkg { Thread=0-5 }`，以及「单独一行包名再跟 `{`」的写法。

四种核心别名：`e-core`、`p-core`、`hp-core`、`all-core`，可以和数字范围用逗号混用。

**注释规则**包含一处细节：行首的 `#` 或 `//` 直接跳过，行内注释则要求 `#` 或 `//` **前必须为空白字符**。因此 `pkg=0-3 # 注释` 中的 `#` 为注释，而 `a#b` 中的 `#` 不是。该规则用于避免包名或线程名中可能出现的 `#` 被误切。

**校验**：包名长度上限 128（不含）、线程名上限 32（不含）；拒绝控制字符（小于 0x20 或等于 0x7f）；CPU 规格必须通过形态校验（每段是四种别名之一、纯数字、或 `数字-数字`）；解析后集合为空也拒绝。包级规则在 cpuset 可用时还会创建对应目录。

解析失败的行会累计计数并在 Web 面板暴露，未闭合的块和悬空的包名也算一次失败。

#### 10.6.3 核心类型探测

算法在 `cpuset.rs`：

1. 遍历 `/sys/devices/system/cpu/cpufreq/`，只取目录名以 `policy` 开头的
2. 读 `cpuinfo_max_freq` 作为频率，为 0 跳过
3. 读 `related_cpus` 得到关联核列表，为空跳过
4. 相同频率的 policy 合并成一组
5. 按频率升序排序
6. 第 0 组（最低频）判为 `e_core`，最后一组判为 `hp_core`，中间的判为 `p_core`

存在一个边界情况：若只有一组，该组会被归入 e-core，hp-core 为空。这在双簇机型上属正常结果。

初始化时还读 `/sys/devices/system/cpu/present` 得到在线核列表，检查 `/dev/cpuset` 是否可访问，读 `/dev/cpuset/mems`（空则默认 `"0"`），并尝试创建 `BASE_CPUSET` 目录——创建成功才置 `cpuset_enabled`。

#### 10.6.4 位图结构

```rust
const CPU_SETSIZE: usize = 1024;
const CPU_WORD_BITS: usize = 64;
const CPU_WORDS: usize = 16;

#[repr(C)]
struct CpuSet { bits: [u64; 16] }
```

编译期断言它与 `libc::cpu_set_t` 同尺寸，所以可以直接做指针转换传给 `sched_getaffinity` / `sched_setaffinity`，不需要额外拷贝。

操作有 `set`、`is_set`、`count`（逐字 `count_ones` 求和）、`or`（逐字按位或，这是规则合并的核心）、`to_range_string`（扫描位生成 `0-3,6` 这种紧凑区间串，用作 cpuset 目录名）。

`parse_cpu_ranges` 支持 `a-b` 和单值，`a > b` 时自动交换，上界 clamp 到 1023，还可以传入 present 集合过滤掉不存在的核。

#### 10.6.5 亲和应用

`affinity_set(tid, set)` 三步：

1. **去重短路**：先 `sched_getaffinity` 读当前集合，与目标完全相等就直接返回，零系统开销
2. **cpuset 追加**：如果 cpuset 可用，向 `BASE_CPUSET/tasks` 或 `BASE_CPUSET/{dir}/tasks` 以 append 方式写 tid。失败被忽略
3. **设置亲和**：`sched_setaffinity`；如果失败且 `errno == ESRCH`（线程已退出），返回成功而不是失败——线程本来就要没了，不算错误

第 1 步的去重被 ChiRi 直接沿用（`set_tid_affinity`，源码注释已注明出处）。

读取 `/proc` 的辅助函数用了栈上缓冲拼路径后 `read_at`，避免堆分配。`read_cmdline` 取首个 NUL 前的内容再按 `/` 取末段；`tid_comm` 读 `/proc/tid/comm` 到 NUL 或换行。

#### 10.6.6 规则匹配

`thread_affinity(pkg, thread)` 的判定顺序：

1. 线程名非空时，遍历所有规则找 `rule.pkg == pkg && fnmatch(rule.thread_pattern, thread)` 的
2. 多条命中时 CPU 集合逐条 `or` 合并
3. 用合并后的集合重算 cpuset 目录，保证目录与亲和性一致
4. 无线程规则命中则回退包级规则，同样 `or` 合并；cpuset 目录取第一条包级规则的，多条不唯一则清空
5. 合并后仍为空时：如果该包有线程规则，返回全核集合（避免误绑到单核）；否则返回 None 表示不处理

`fnmatch_c` 是对 POSIX `fnmatch` 的封装，带 `FNM_NOESCAPE` 标志，支持 `*` 与 `?` 通配。线程名长度达到 32 时直接判定为不匹配。**整个项目不使用正则**，匹配全部依赖 fnmatch。

`comm_to_pkg` 的兜底逻辑：comm 直接等于包名时命中；否则当 `comm.len() >= 15` 且存在包名以 comm 开头或结尾时，读取 cmdline 做精确匹配。该 15 的阈值对应 Linux comm 字段 16 字节（含结尾 NUL）的限制。

另有一层降级保护：若本次结果为包级回退，而该 tid 此前为线程规则绑定，则不覆盖（返回 true 保持原状），以防止线程规则被包级规则静默降级。

#### 10.6.7 双模式

| 维度         | eBPF 事件驱动                                  | /proc 轮询                                                     |
| ------------ | ---------------------------------------------- | -------------------------------------------------------------- |
| 数据来源     | 4 个 tracepoint + RingBuf + APPLIED_MAP        | 遍历 `/proc` 和 `/proc/pid/task`                               |
| 扫描策略     | 增量：事件触发单 tid；配置变更/启动/回退时全量 | 按需全量重建                                                   |
| 亲和同步周期 | `3 × interval` 秒                              | `5 × interval` 秒                                              |
| 事件循环     | `recv_timeout(1s)` 后 `try_recv` 排空          | `sleep(interval)`                                              |
| 进程增长处理 | 不适用                                         | 进程数增长超过 11 触发全量重扫；增长不超过 11 只置强制同步标志 |
| 失败降级     | 多种失败都回退 proc                            | 无                                                             |

模式值：0 自动、1 强制 eBPF、2 强制 proc。

自动模式的语义需留意：eBPF 初始化失败**一次**即置 `EBPF_GAVE_UP` 并放弃，此后不再重试。强制 eBPF 模式下则持续重试（间隔 30 秒）。从 proc 模式切回非 proc 模式时会清除该标记，允许重新尝试。

proc 模式下除了进程数增长，`kill(pid, 0)` 探测失败也会触发重扫（进程已死）。

#### 10.6.8 eBPF 侧

四个 tracepoint：`sched_process_fork`、`sched_process_exec`、`sched_process_exit`、`task_rename`。事件类型常量是 FORK=1、EXEC=2、RENAME=3、EXIT=4。

事件结构在内核态和用户态逐字段对齐：

```rust
struct ProcEvent {
    pid: i32,
    tid: i32,
    comm: [u8; 16],
    event_type: u32,
}
```

用户态用 `ptr::read_unaligned` 解析。

**15 字节滑动窗口白名单**是该模块的关键设计。Linux 的 `comm` 字段固定 16 字节（含结尾 NUL），长包名会被截断。用户态为每个包名生成两个键：前 15 字节、以及（如果包名超过 15 字节）末 15 字节。内核侧在 16 字节的 comm 上以 15 字节窗口滑动匹配，**只尝试两个位置**（`pos ∈ {0, 1}`）。

只滑动两个位置：comm 上限 16 字节、窗口长 15 字节，对齐位置最多 2 个，穷举即可覆盖全部情况，同时最小化 map 查询次数。

**竞态防护**：FORK 事件时，如果父线程已在 APPLIED_MAP 里，立即用 `APPLIED_MAP.insert(child_tid, 0)` 给子线程占位。这是为了防止子线程在 RENAME 事件到来之前被白名单过滤漏掉。EXEC 和 RENAME 先查自身是否 tracked，否则走白名单。EXIT 只在 tracked 时上报并移除。

**容量**：内核侧固定 `TARGET_COMM_MAP = 512`、`APPLIED_MAP = 8192`、RingBuf `256KB`。用户态会根据包名数量动态算一个更大的容量（`(包数 × 2).max(512)` 向上取 2 的幂），通过 `EbpfLoader::map_max_entries` 覆盖。白名单条目数超过容量会触发重载。

**偏移动态解析**：这个做法比 ChiRi 的硬编码偏移稳健得多。它读 tracefs 下的 `events/{类别}/{名称}/format`，解析每个 `field:` 行的 `offset:`，字段名里的 `[` 下标会被剥掉。提取 `fork.child_pid`、`fork.child_comm`、`rename.newcomm` 三个字段，注入 `OFFSETS_MAP`。

注入必须在 attach 之前完成，否则第一批事件会读到空 map。`fork_child_pid == 0` 被用作「未注入」的判据。

RingBuf 读取线程用 epoll 同时监听 ring fd 和一个 eventfd，Drop 时写 eventfd 唤醒读取线程再 join——避免线程卡在阻塞读上无法退出。

#### 10.6.9 缓存

`ProcCache` 被两个模式共用：eBPF 增量维护，proc 模式全量重建。

`TaskEntry` 存 `pid`、`pkg`、`cpus`、`cpuset_dir`、`is_thread_rule`。`task_apply` 统一了「查包名 → 算亲和 → 应用 → 回写缓存」的流程，其中「应用」这一步用函数注入——eBPF 侧会同时写 APPLIED_MAP，proc 侧是空操作。

`affinity_sync` 遍历缓存重设亲和，返回死亡 tid 列表供 eBPF 侧清理 APPLIED_MAP。

包名查找的回退链是：`comm_to_pkg` → 缓存中该 pid 条目的包名。

#### 10.6.10 规则编辑

写操作被一把全局锁串行化。

`target_scan` 先解析文件定位目标包所在的行，区分四种包级行形态和线程行的位置。`normalize_singles` 会把单行内联的 `pkg { thread=cpus }` 拆成块结构，方便后续编辑。

`rule_upsert` 的逻辑：包级规则原地改值或追加；线程规则优先改最后一条同名行并删除其余重复项；没有块就新建。遇到未闭合的块返回 `Malformed` 错误。

写盘用 `{path}.tmp` → `sync_all` → `rename` 的原子替换。写成功后 Web 端会调 `config_reload_now()` 立即热加载，不用等 inotify。

配置文件的 inotify 监听掩码是 `IN_CLOSE_WRITE | IN_DELETE_SELF | IN_MOVE_SELF`。删除或移动后要重挂监听并睡一个 interval。监听失败降级为轮询。配置文件的 mtime 用纳秒级时间戳去重，避免同一秒内的重复触发。

#### 10.6.11 Web 面板

后端是手写的 HTTP/1.1 解析，绑 `127.0.0.1:8889`，`index.html` 编译期内嵌。

安全防护有几层：请求头上限 8192 字节，body 上限 16384 字节，拒绝 `transfer-encoding`，要求 Host 是 `127.0.0.1` 或 `localhost`，要求 `Sec-Fetch-Site` 是 `none` 或 `same-origin`，POST 必须 `Content-Type: application/json`。

接口一共 9 个：

| 方法 | 路径               | 功能                               |
| ---- | ------------------ | ---------------------------------- |
| GET  | `/`、`/index.html` | 面板页面                           |
| GET  | `/api/status`      | 运行状态、模式、规则统计、核心分层 |
| GET  | `/api/rules`       | 按包分组的规则列表                 |
| GET  | `/api/config`      | 模式、间隔、cpuset 名、配置路径    |
| POST | `/api/rule`        | 新增或更新规则                     |
| POST | `/api/rule/del`    | 删除单条或整包                     |
| POST | `/api/rule/rename` | 重命名包                           |
| POST | `/api/config`      | 改设置                             |
| POST | `/api/suggest`     | 包名或线程名联想                   |

前端是单页三 Tab（状态/规则/设置）加两个全屏子页（关于/捐赠）。状态页每 3 秒轮询。

规则编辑弹窗支持「包级默认 + 多条线程规则」的树形结构一次提交，保存时先 rename、再清理删除行、最后逐条 upsert——顺序不能乱。

核心选择用 chip 切换语义核或自定义范围，`semanticVisible()` 会根据实际拓扑决定 p-core 是否显示（单簇或双簇机型上不显示）。

联想输入有 250ms 防抖，支持上下键和回车选择、Escape 关闭。联想数据源：包名来自 `/data/data` 目录枚举加 `/proc` 的 cmdline 计数，线程名来自目标包各 tid 的 comm 计数，结果截断 20 条。

服务端校验：`token_ok` 禁止控制字符和 `#`、`//`；CPU 规格要求长度小于 64 且通过形态校验；间隔限定在 1 到 3600。

#### 10.6.12 常量速查

| 值                              | 含义                                     |
| ------------------------------- | ---------------------------------------- |
| 128 / 32                        | 包名 / 线程名长度上限（不含）            |
| 1024 / 64 / 16                  | 位图的 CPU 数 / 字长 / 字数              |
| 2 秒                            | 默认检查间隔                             |
| 11                              | 进程数增长触发全量重扫的阈值             |
| 30 秒                           | eBPF 重试间隔                            |
| 1 秒                            | eBPF 事件接收超时                        |
| `3 × interval` / `5 × interval` | eBPF / proc 的亲和同步周期               |
| 15                              | comm 长度阈值，超过才走 cmdline 兜底     |
| 512 / 8192 / 256KB              | 内核侧白名单 map / APPLIED_MAP / RingBuf |
| 8889 / 127.0.0.1                | Web 端口与监听地址                       |
| 8192 / 16384                    | 请求头 / 请求体上限                      |
| 1 到 3600                       | 间隔合法区间                             |
| 20                              | 联想结果条数                             |
| `/dev/cpuset/{name}`            | cpuset 目录模板                          |
| `0o755` + `chown 0,0`           | cpuset 目录权限与属主                    |

该项目**不使用正则**，线程名匹配依赖 POSIX `fnmatch`（仅支持 `*` 与 `?`）；ChiRi 的亲和黑名单使用正则（`re:` 前缀），前者更轻量，后者更灵活。

### 10.7 安装与热更新

**customize.sh 的流程**：

1. 探测 BusyBox：`/data/adb/magisk/busybox` → `/data/adb/ksu/bin/busybox` → `/data/adb/ap/bin/busybox`
2. 判断语言：读 `persist.sys.locale`，回退 `ro.product.locale`，`grep -qi "zh"` 切中文
3. 检查热更新可用性：包内 `allowHotUpdate` 和已装模块的同名文件**内容都要是 `1`**
4. 音量键检测：`getevent -l` 后台运行，每 0.1 秒轮询共 120 轮（12 秒）。首个按下后等 1 秒确认窗口。多键误触会 abort 取消安装。超时默认按上键
5. 询问是否保留配置：上键保留，下键用包内默认覆盖
6. 热更新或全新安装

**OPlus 检测**：读 `/sys/class/oplus_chg/battery/bcc_parms`，`cut -d ',' -f 12` 非空就判为双电芯。只在「不保留配置」时执行。

**热更新流程**（这个必须小心）：

1. 先按 `logs/watchdog.pid` 杀掉 WatchDog，再 `killall -9 chiri`，`pidof chiri` 轮询最多 5 秒
2. 备份 `config/meta.yaml` 和 `rules.yaml` 为 `.bak`
3. 拷贝文件
4. 抽查 `service.sh`、`module.prop`、`allowHotUpdate`、`rules.yaml`
5. 失败就回滚 `.bak` 并 `chmod 755`
6. 末尾**必须 `abort` 结束**

最后一条与直觉相反但属必要条件：热更新必须以 `abort` 结束，否则 KernelSU 会标记为「待重启」并屏蔽 Action 按钮与 WebUI。

**卸载**：`touch .uninstalling` → 杀 WatchDog 和 chiri → 删 pid 文件 → 遍历所有 cpufreq policy，用 `scaling_available_frequencies` 的最低和最高值写回 `scaling_max_freq` 和 `scaling_min_freq`，governor 恢复 `schedutil`。

---

## 11. 已知限制与风险

本章汇总源码审查中发现的异常实现与风险点，按影响程度排列。其中部分问题在源码注释中已有记载，部分为本次审查新发现。

### 11.1 触摸升频被常量整体关闭

```rust
const TOUCH_BOOST_SUSPENDED: bool = true;
```

`on_touch` 直接返回。这意味着配置文件中的 `touch_boost_ms: 350/400/500` 与 `touch_boost_enabled: true` **当前均不生效**。文档与参数表若未标注该状态，将造成误导。

源码注释亦提到：「确认长期关闭时应改配置而非留常量」。当前处于两者之间的中间状态。

### 11.2 scenemode 的 prime 下线是死代码

`src/chiri/core_ctl.rs` 里保留着一整套 scenemode 离线逻辑：`STATE_SCENEMODE` 状态机、`scenemode_targets()` 选目标簇、逐核写 `online = 0` 并回读验证、`reassert_offline()` 周期纠偏、`restore_online()` 恢复。

但 `set_power_state(boost, scenemode)` 的第二个参数在全部 4 个调用点都是 `false`：

```
set_power_state(false, false)              // contingency / babel
set_power_state(false, false)              // scenemode 分支
set_power_state(config.core_ctl.enabled && boost, false)   // 常规路径
set_power_state(false, false)              // 停摆
```

所以 `STATE_SCENEMODE` 分支进不去，`CoreCtl.scenemode_offline` 这个配置项也就失去了作用。8550 和 8475 配 `true`、8998 配 `false`，行为完全一样。

该分支不可达不构成缺陷；`mod.rs` 的注释已明确说明「原『prime 整簇下线 + 保留核独占』已废弃」。问题在于配置文件与 `ModeInfo.md` 仍在描述旧行为，`core_ctl.rs` 的注释也在描述一条不可达的逻辑。这种「文档、配置、代码三者不一致」的状态容易对后续维护造成误导。

### 11.3 文档与代码不一致

共发现三处：

1. `affinity.rs` 的模块头注释写「score = util + 钉线程数 × 0.2」，实际常量 `PINNED_WEIGHT = 0.4`
2. `main.rs` 的注释写归档产物是 `logd/ziped_<ts>.zip`，实际是无压缩的 `.tar`
3. 配置注释与 `core_ctl.rs` 仍在描述 scenemode 下线 prime 簇的旧行为（见 11.2）

这几处不影响运行，但会让读代码的人产生错误预期。

### 11.4 eBPF 硬编码偏移的脆弱性

`sched_switch` tracepoint 的字段偏移是硬编码的：

```rust
const OFF_PREV_PID: u32 = 24;
const OFF_NEXT_PID: u32 = 56;
```

如果内核结构布局变了（不同内核版本、不同编译选项），这两个值可能失效。失效的后果是读到错误的 PID，进而污染所有 util 统计。

AppOptR 的实现更为稳健：它从 tracepoint 的 format 文件动态解析偏移。ChiRi 未采用该做法。

兜底：负载数据源失效超过 5 秒，CLG 释放接管并交还系统调频。但「读到错误数据」与「读不到数据」性质不同，前者不会触发 WatchDog。

### 11.5 单帧增量限幅的硬编码下限

`pid_jank.rs` 里的限幅逻辑有一组硬编码下限：

```
正常:   max_inc = max(max_inc_normal × scale, 0.065)
阻尼期: max_inc = max(max_inc_damped × scale, 0.040)
crit:   max_inc = max(max_inc_normal × scale × 2.5, 0.15)
```

其中 `0.065`、`0.040`、`0.15` 直接写在代码中，配置项 `max_inc_normal`（0.075）与 `max_inc_damped`（0.045）仅在乘积更大时起作用。即若将 `max_inc_normal` 配置为 0.05，实际下限仍为 0.065，配置不会完全生效。

该行为未必是有意设计，但会造成「修改配置无效」的困惑。

### 11.6 特性依赖的耦合

FAS 的多项能力依赖前台 util 数据，而前台 util 依赖 eBPF 的 `TGID_RUN_TIME` map。若该 map 因容量不足（1024 项）发生驱逐，`compute_tgid_util` 会退化到遍历 `/proc/<pid>/task` 的降级路径。降级路径设有防驱逐保护（新值小于旧值时跳过），但精度下降。

源码注释提到了该风险：`TGID_RUN_TIME` 容量取 1024 即为规避 `THREAD_RUN_TIME`（32768 项）的哈希驱逐问题。

### 11.7 双实例防护存在单点依赖

`flock` 层是可靠的，但 shell 层的清理依赖 `logs/watchdog.pid`，而 `logs/` 每次启动都会被归档 rename。归档逻辑包含「复制回 `watchdog.pid`」这一步，该步失败将导致 WebUI 的「关闭调度」失效。

源码注释已确认该依赖链，并将「必须复制回 watchdog.pid」列为硬约束。该链路存在单点依赖。

### 11.8 配置字段缺少 `deny_unknown_fields`

`CpuLoadGovernorConfig` 未启用该属性。后果是将 Yumi 的字段写入 ChiRi 的 `feature.yaml` 时会被静默忽略，不产生任何提示。鉴于 `module/config/feature.yaml`（Yumi）与 `module/config/{soc}/feature.yaml`（ChiRi）结构相近且字段名存在重叠（`up_threshold`、`down_threshold` 等），该静默行为具有实际风险。

### 11.9 日志预算清理的边界情况

`enforce_dir_limit` 的逻辑是「按 mtime 从旧删到低于目标值，但最新的一个文件永不删除」。

若单个文件超过 96MB（devimp 单文件软上限为 128MB），清理会删除其余文件而保留该超大文件，极端情况下目录将持续超出预算。该边界不构成缺陷，但需记录在案。

### 11.10 未使用的代码路径

`fas/pid.rs` 的 `update_coefficients` 与 `fas/policy_mgmt.rs` 的 `reload_rules` 均标注 `#[allow(dead_code)]`。注释说明原因为 per-app 配置为编译期嵌入，`ConfigReload` 事件不再重载 FAS。这两处是热重载的预留接口，当前未启用。

### 11.11 权限与安全边界

这个模块需要 Root 权限，能读写 sysfs、cpuset、devfreq。WebUI 通过 KernelSU 的 `ksu.exec` 执行 shell 命令，所以 WebUI 的权限等于 Root。

WebUI 侧的路径注入防护（`isSafeConfigRel` 拒绝绝对路径、`..`、反斜杠）用于防止误操作，不构成安全边界；能够访问 WebUI 者本身已具备 Root 权限。

配置的防篡改手段是「编译期嵌入」而非加密，具备重新编译能力者可修改任意内容。此为开源项目的固有边界。

---

## 12. 附录

### 12.1 关键常量速查

**时间类**

| 常量                              | 值     | 位置                         |
| --------------------------------- | ------ | ---------------------------- |
| `CLG_STALE_MAX`                   | 5s     | `chiri/mod.rs`               |
| `EVENT_POLL_MS`                   | 1000ms | `chiri/mod.rs`               |
| `THERMAL_CHECK_INTERVAL`          | 2s     | `chiri/mod.rs`               |
| `TELEMETRY_LOG_INTERVAL`          | 1s     | `chiri/mod.rs`               |
| `MODE_FILE_REWRITE_INTERVAL`      | 5s     | `chiri/mod.rs`               |
| `SCENEMODE_SAT_SECS`              | 10s    | `chiri/mod.rs`               |
| `SCENEMODE_COOLDOWN`              | 300s   | `chiri/mod.rs`               |
| `TUNED_COOLDOWN` / `FAS_COOLDOWN` | 300s   | `chiri/mod.rs`               |
| `SCHEDULER_IPC_RESTART_BACKOFF`   | 1s     | `chiri/mod.rs`               |
| `REWRITE_INTERVAL`（fast_lock）   | 5s     | `chiri/fast.rs`              |
| Worker `tick_interval`            | 1s     | `chiri/cpu_load_governor.rs` |
| `FAS_TEMP_REFRESH`                | 3s     | `chiri/fas_manager.rs`       |
| `REBALANCE_INTERVAL`              | 2s     | `chiri/affinity.rs`          |
| `MIN_MIGRATE_INTERVAL`            | 4s     | `chiri/affinity.rs`          |
| `RETURN_COOLDOWN`                 | 16s    | `chiri/affinity.rs`          |
| `REPIN_DEBOUNCE`                  | 8s     | `chiri/affinity.rs`          |
| `THREAD_STALE`                    | 30s    | `chiri/affinity.rs`          |
| PID 广播周期                      | 500ms  | `monitor/mod.rs`             |
| 应用检测轮询                      | 1500ms | `monitor/app_detect.rs`      |
| 包名防抖                          | 500ms  | `monitor/app_detect.rs`      |
| `STATS_INTERVAL`                  | 2s     | `monitor/cpu_monitor.rs`     |
| `TGTOP_INTERVAL`                  | 30s    | `monitor/cpu_monitor.rs`     |
| 遥测轮询                          | 1s     | `monitor/telemetry.rs`       |
| `BCC_PROBE_INTERVAL`              | 60s    | `monitor/telemetry.rs`       |
| 心跳 `LIVE_TIME_INTERVAL_SECS`    | 15s    | `logger.rs`                  |
| `SHORT_SESSION_SECS`              | 30s    | `logger.rs`                  |
| WatchDog 基础重启                 | 3s     | `module/service.sh`          |
| FAS 延迟退出                      | 15s    | `fas_types.rs`               |

**阈值类**

| 常量                            | 值          | 位置                       |
| ------------------------------- | ----------- | -------------------------- |
| `SCENEMODE_SAT_UTIL`            | 0.75        | `chiri/mod.rs`             |
| `THERMAL_UNPRESS_STEP`          | 0.15        | `chiri/mod.rs`             |
| `SCHEDULER_IPC_RESTART_MAX`     | 5           | `chiri/mod.rs`             |
| `OVERLOAD_MARGIN`               | 0.15        | `chiri/affinity.rs`        |
| `CORE_OVERLOAD_UTIL`            | 0.70        | `chiri/affinity.rs`        |
| `PINNED_WEIGHT`                 | 0.4         | `chiri/affinity.rs`        |
| `MAX_PINS_PER_CORE`             | 3           | `chiri/affinity.rs`        |
| `PROMOTE_UTIL_PCT`              | 25.0        | `chiri/affinity.rs`        |
| `DEMOTE_UTIL_PCT`               | 5.0         | `chiri/affinity.rs`        |
| `FG_BUSY_UTIL_PCT`              | 30.0        | `chiri/affinity.rs`        |
| `FG_BUSY_RELEASE_UTIL_PCT`      | 15.0        | `chiri/affinity.rs`        |
| `BIG_HIGH_WATER`                | 0.90        | `chiri/affinity.rs`        |
| `LITTLE_HIGH_WATER`             | 0.70        | `chiri/affinity.rs`        |
| `SCREEN_READ_FAIL_LIMIT`        | 8           | `monitor/screen_detect.rs` |
| `OFF_QUORUM`                    | 2           | `monitor/screen_detect.rs` |
| `FRAMETIME_WINDOW`              | 144         | `monitor/fps_monitor.rs`   |
| `MIN_FRAME_NS` / `MAX_FRAME_NS` | 1ms / 200ms | `monitor/fps_monitor.rs`   |

**容量类**

| 项                         | 值                 |
| -------------------------- | ------------------ |
| 事件通道 `sync_channel`    | 64                 |
| 触摸通道                   | 8                  |
| 负载通道（每 worker）      | 1                  |
| 通知通道                   | 1                  |
| `daemon.log` 轮转          | 50MB，保留 3 份    |
| `status.csv` 轮转          | 8MB，保留 1 份     |
| `devimp` 单文件            | 128MB              |
| `devimp` 保留份数          | 20                 |
| `logd` / `devimp` 目录预算 | 128MB，清理到 96MB |
| 日志重启门限               | 16MB               |
| eBPF RingBuf（帧）         | 32768 字节         |
| `THREAD_RUN_TIME` map      | 32768              |
| `TGID_RUN_TIME` map        | 1024               |
| `FpsWindow` 窗口           | 120 帧             |

### 12.2 术语对照

| 英文              | 中文         | 说明                                  |
| ----------------- | ------------ | ------------------------------------- |
| policy            | 频率策略组   | 共享一套频率控制的核心组              |
| governor          | 调速器       | 决定在允许区间内取哪个频率的内核模块  |
| schedutil         | —            | 基于调度器负载预测的调速器            |
| util              | 利用率       | 0 到 1，表示任务有多吃 CPU            |
| cpuset            | —            | 限制进程能跑在哪些核上的机制          |
| uclamp            | —            | 软性的性能诉求（min 抬升 / max 压制） |
| core_ctl          | —            | 控制每个簇至少几个核在线              |
| headroom          | 余量         | 目标值上乘的系数，用于留出性能余量    |
| perf_index / perf | 性能比       | 0 到 1 的归一化指标，映射到频率       |
| hysteresis        | 迟滞         | 故意设的死区，防阈值边缘反复横跳      |
| jank              | 卡顿         | 单帧耗时明显超预算                    |
| tracepoint        | —            | 内核预埋的静态探针点                  |
| uprobe            | —            | 用户态函数的探针点                    |
| RingBuf           | 环形缓冲     | 内核到用户态的高效数据通道            |
| PSI               | 压力停滞信息 | CPU/IO/内存被卡住的比例               |
| devfreq           | —            | 设备频率框架，GPU 用它                |
| flock             | 文件锁       | 保证单实例                            |

### 12.3 源码文件索引

| 文件                                     | 行数 | 职责                           |
| ---------------------------------------- | ---- | ------------------------------ |
| `src/main.rs`                            | 438  | 启动编排                       |
| `src/common.rs`                          | 1091 | SoC 判定、配置嵌入、白名单解析 |
| `src/logger.rs`                          | 1859 | 日志系统                       |
| `src/rhine.rs`                           | 838  | 实验室模式                     |
| `src/notify.rs`                          | 315  | 系统通知                       |
| `src/utils.rs`                           | 637  | 文件 IO、目录监听、FastWriter  |
| `src/i18n.rs`                            | 141  | 多语言                         |
| `src/down.rs`                            | 125  | 停摆状态                       |
| `src/fas_types.rs`                       | 524  | FAS 配置类型                   |
| `src/webui_asset.rs`                     | 33   | WebUI 资源还原                 |
| `src/chiri/mod.rs`                       | 2551 | 主事件循环、热保护、scenemode  |
| `src/chiri/affinity.rs`                  | 1956 | 线程亲和                       |
| `src/chiri/cpu_load_governor.rs`         | 1038 | CLG                            |
| `src/chiri/config.rs`                    | 1016 | 配置结构                       |
| `src/chiri/tuned.rs`                     | 488  | 特调                           |
| `src/chiri/core_ctl.rs`                  | 487  | 核心上下线                     |
| `src/chiri/fas_manager.rs`               | 271  | FAS 生命周期                   |
| `src/chiri/fast.rs`                      | 283  | vector 锁频                    |
| `src/chiri/governor.rs`                  | 174  | governor 接管                  |
| `src/chiri/gpu.rs`                       | 183  | GPU 锁频                       |
| `src/chiri/touch_detect.rs`              | 116  | 触摸检测                       |
| `src/chiri/scheduler.rs`                 | 138  | 系统一次性调优                 |
| `src/monitor/mod.rs`                     | 232  | 监控层编排                     |
| `src/monitor/app_detect.rs`              | 571  | 前台识别与模式判定             |
| `src/monitor/cpu_monitor.rs`             | 679  | CPU 采集                       |
| `src/monitor/fps_monitor.rs`             | 453  | 帧采集                         |
| `src/monitor/screen_detect.rs`           | 721  | 亮灭屏检测                     |
| `src/monitor/telemetry.rs`               | 377  | 遥测                           |
| `src/monitor/config.rs`                  | 62   | 规则配置类型                   |
| `src/scheduler/fas/controller.rs`        | 398  | FAS 控制器                     |
| `src/scheduler/fas/frame_pipeline.rs`    | 382  | 帧流水线                       |
| `src/scheduler/fas/gear_state.rs`        | 273  | 档位状态机                     |
| `src/scheduler/fas/pid.rs`               | 149  | PID 控制器                     |
| `src/scheduler/fas/pid_jank.rs`          | 254  | PID + Jank                     |
| `src/scheduler/fas/policy_mgmt.rs`       | 420  | policy 管理                    |
| `src/scheduler/fas/policy_controller.rs` | 203  | 单 policy 控制                 |
| `src/scheduler/fas/fps_window.rs`        | 98   | 帧窗口统计                     |
| `yumi-ebpf/src/main.rs`                  | 233  | eBPF 探针                      |

### 12.4 WebUI / analyze / AppOptR 文件清单

**`webui/src/`**（按行数降序，共 40 个文件）

| 文件                             | 行数 | 文件                            | 行数 |
| -------------------------------- | ---- | ------------------------------- | ---- |
| `state.svelte.ts`                | 889  | `contract/sources.ts`           | 93   |
| `app.css`                        | 567  | `contract/paths.ts`             | 92   |
| `views/OverviewView.svelte`      | 497  | `contract/lab.ts`               | 88   |
| `views/LogsView.svelte`          | 412  | `data/daemon-log.ts`            | 88   |
| `contract/meta.ts`               | 354  | `data/apps.ts`                  | 88   |
| `dev/mock-shell.ts`              | 335  | `contract/read.ts`              | 86   |
| `views/ConfigView.svelte`        | 304  | `components/StateBox.svelte`    | 81   |
| `i18n/locales/en.ts`             | 300  | `data/rules.ts`                 | 78   |
| `i18n/locales/zh.ts`             | 291  | `components/Panel.svelte`       | 76   |
| `views/LabView.svelte`           | 261  | `components/ToggleField.svelte` | 73   |
| `App.svelte`                     | 197  | `data/whitelists.ts`            | 57   |
| `data/lab.ts`                    | 184  | `components/Segmented.svelte`   | 56   |
| `kernel/ksu.ts`                  | 181  | `i18n/index.svelte.ts`          | 54   |
| `views/BatteryView.svelte`       | 172  | `router.svelte.ts`              | 53   |
| `views/AppsView.svelte`          | 154  | `contract/down.ts`              | 52   |
| `contract/export.ts`             | 151  | `kernel/shell.ts`               | 49   |
| `data/mode.ts`                   | 150  | `data/live-time.ts`             | 46   |
| `components/NumberField.svelte`  | 147  | `contract/errors.ts`            | 45   |
| `data/status-csv.ts`             | 125  | `data/module-info.ts`           | 44   |
| `contract/daemon.ts`             | 98   | `contract/power.ts`             | 30   |
| `components/ConfirmSheet.svelte` | 95   | `components/LiveStatus.svelte`  | 29   |
| `data/down.ts`                   | 26   | `data/power-avg.ts`             | 25   |
| `main.ts`                        | 24   |                                 |      |

**`webui/tests/`**：`lab.test.ts`（229）、`data-parse.test.ts`（154）、`data-model.test.ts`（152）、`contract.test.ts`（151）、`i18n.test.ts`（74）、`live-time.test.ts`（59）、`down.test.ts`（47）。

**`webui/` 根配置**：`vite.config.ts`（68）、`tsconfig.json`（31）、`package.json`（29）、`index.html`（17）、`vitest.config.ts`（14）、`env.d.ts`（11）、`svelte.config.js`（6）、`README.md`（70）。

**`analyze/`**：`index.html`（2117）、`analyze.css`（751）、`ak-ui.css`（93KB）、`fonts/`（4 个字体文件 + 3 个许可/说明文件）。

**`AppOptR/`**：用户态 10 个 `.rs`（`main.rs`、`config.rs`、`cpuset.rs`、`apply_affinity.rs`、`rule_match.rs`、`rule_edit.rs`、`cache.rs`、`ebpf_mode.rs`、`proc_mode.rs`、`web.rs`）、内核态 1 个 `main.rs`、Web 面板 `web/index.html`（41KB）。

### 12.5 开源许可

| 项目    | 许可    | 仓库                      |
| ------- | ------- | ------------------------- |
| yumi    | GPL-3.0 | github.com/imacte/yumi    |
| AppOptR | GPL-3.0 | gitee.com/sutoliu/AppOptR |
| ChiRi   | GPL-3.0 | github.com/Aibeto/ChiRi   |

ChiRi 自 imacte/yumi fork，保留了部分 yumi 的核心代码作为回退兜底。线程调整部分参考了 AppOptR。

字体资源另有许可：

- `analyze/fonts/LICENSE-OFL.txt`（Noto Sans SC）
- `analyze/fonts/LICENSE-OFL-jetbrains-mono.txt`（JetBrains Mono）

两者都是 SIL Open Font License。

### 12.6 覆盖范围与未覆盖的内容

本文档覆盖仓库中除 Yumi 调度本体之外的全部代码：

| 范围                                     | 覆盖情况                                           |
| ---------------------------------------- | -------------------------------------------------- |
| `src/chiri/`（14 文件）                  | 逐文件分析，算法公式完整披露                       |
| `src/monitor/`（7 文件）                 | 逐文件分析                                         |
| `src/scheduler/fas/`（8 文件）           | 逐文件分析（ChiRi 复用的 FAS 引擎）                |
| `src/` 根目录（10 文件）                 | 逐文件分析                                         |
| `yumi-ebpf/src/main.rs`                  | 探针、map、数据结构全覆盖                          |
| `module/` 脚本与配置                     | 安装、启动、卸载、热更新、全部 yaml                |
| `webui/`（40 源文件 + 7 测试）           | 分层、契约、解析算法、视图、组件、mock、构建全覆盖 |
| `analyze/`（2117 行 + CSS）              | 解析、时间轴、断裂轴、绘图、排行榜、表格全覆盖     |
| `AppOptR/`（用户态 + 内核态 + Web 面板） | 全覆盖                                             |
| `build.rs`、`xtask/`、CI                 | 全覆盖                                             |

未纳入深入分析的部分及原因：

- **`src/scheduler/` 下的 Yumi 调度本体**（`cpu_load_governor.rs`、`config.rs` 的 Yumi 分支、`mod.rs` 的接线）。该部分按项目约定为冻结代码，不影响 ChiRi 行为。文档中仅在需要对比时引用其 `smoothing_down` 系列参数。
- **`devimpbin/` 下的真机日志样本**。属数据而非代码，已确认其格式与本文描述一致（22 列 status.csv、日志归档命名、屏幕检测源等）。
- **`module/config/8475` 与 `8998` 的逐项差异**。文档列出了三个 SoC 的关键差异（CLG 防抖、uclamp 上限、scenemode_offline、IO Scheduler），未逐字段对照全部参数。
- **`.github/dependabot.yml`、`.gitignore` 等辅助配置**。与功能无关。
- **`.trae/`、`.codebuddy/`、`.workbuddy/`**。AI 辅助工具的工作目录，不属于项目代码。

### 12.7 代码问题汇总

第 11 章按影响程度列出 11 项，此处按类型重新归并，便于定位。

**功能被停用但配置仍在**

| 项                                  | 状态                                 |
| ----------------------------------- | ------------------------------------ |
| 触摸升频（`TOUCH_BOOST_SUSPENDED`） | 代码完整，被常量整体关闭             |
| scenemode 下线 prime 簇             | 代码完整，调用点恒传 `false`，不可达 |
| `CoreCtl.scenemode_offline` 配置项  | 因上一条失去作用                     |
| WebUI 的 `notify` 开关              | 界面长期 disabled                    |
| analyze 的 `fitY` 开关              | 半失效，`yRange()` 不读它            |

**文档与代码不一致**

| 位置                               | 文档说                         | 实际                |
| ---------------------------------- | ------------------------------ | ------------------- |
| `affinity.rs` 头注释               | score 权重 0.2                 | 常量是 0.4          |
| `main.rs` 注释                     | 归档产物 `.zip`                | 实际是无压缩 `.tar` |
| `chiri/mod.rs` WatchDog 注释       | 8550 default `perf_init = 1.0` | 配置里是 0.40       |
| `core_ctl.rs` 注释与 `ModeInfo.md` | scenemode 下线 prime           | 已废弃              |

**健壮性上的薄弱点**

| 项                                               | 说明                                            |
| ------------------------------------------------ | ----------------------------------------------- |
| eBPF `sched_switch` 硬编码偏移                   | 内核结构变化会静默读到错误 PID，不触发 WatchDog |
| `CpuLoadGovernorConfig` 缺 `deny_unknown_fields` | 写错字段名会被静默忽略                          |
| PID 单帧限幅的硬编码下限                         | 配置值小于下限时配置不生效                      |
| `TGID_RUN_TIME` map 容量 1024                    | 进程多时可能驱逐，降级路径精度下降              |
| 日志目录预算的边界情况                           | 单文件超预算时清理会留下超大文件                |
| analyze 自动载入失败静默                         | `boot()` 的错误提示被注释掉                     |
