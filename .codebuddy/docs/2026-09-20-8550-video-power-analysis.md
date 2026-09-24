# 8550 视频场景功耗分析（2026-09-20）

> 数据源：`devimpbin/8550/logd_0920-210959.tar.gz`
> 版本：ChiRi Canary Alpha04-Canary88（versionCode 3），机型 8550，8 核（little[0-2] / big[3-6] / prime[7]）
> 方法：devimp CSV 离线回放（2326 snap / 63379 tick / 2200 core / 375 tgtop），按模式与分钟聚合
> **本文只分析与给方案，未改动任何源码或配置。**

## [profile] 视频场景画像（playback = tv.danmaku.bili，1885 snap，跨度 2232s ≈ 37 分钟）

| 指标 | avg | p50 | p95 | max |
|---|---|---|---|---|
| 电池功率 batt_p | **2.83 W** | 2.59 W | 5.61 W | 11.22 W |
| 电池温度 | 39.7 ℃ | — | — | 41.4 ℃ |
| CPU 温度 | 53.4 ℃ | — | — | 89.6 ℃ |
| 热限频 cap | 87 % | — | — | 70 %（触发过） |
| gpu_busy | 8.6 | — | 33 | — |
| psi_cpu | 19.7 | — | — | — |
| migrations / wakeups | **557 / 采样** | — | — | ≈971 wakeups/s |

参照：同一份日志里 `default` 模式（桌面/切换）avg 4.63 W、gpu_busy 22.1、cpu_temp 68.7 ——
**视频本身不是最耗电的场景**，但它是**长稳态**场景，同样功耗占比下待机时长收益最明显，
且它当前被电池温度顶到 70% 限频，说明还有下降空间。

### 频率上限与负载（tick 行的 cur_freq_khz 是 TunedGovernor 写入的 `scaling_max_freq` 上限，非实际频率）

| cluster | 硬件 max | 上限 cap% (avg/p50/p95) | 组内最大核占用率 (avg/p50/p95) | 决策分布 |
|---|---|---|---|---|
| little (0-2) | 2016 MHz | **70.6 / 72.4 / 100** | 0.63 / 0.63 / 0.89 | down_wait 6996, hold 6750, up 6626, down 3246 |
| big (3-6) | 2803 MHz | 56.6 / 56.7 / 85.8 | 0.50 / 0.51 / 0.78 | down_wait 7550, up 7231, hold 6558, down 3602 |
| prime (7) | 3187 MHz | 35.8 / **29.2 / 83.8** | 0.27 / **0.21** / 0.76 | up 2556, hold 2192, down_wait 1895, down 1593 |

### 各核实际利用率（core 行，224 次采样）

- little core0/1/2：**58.9 / 53.7 / 54.0**（p95 99 / 100 / 98）
- big core3/4/5/6：39.0 / 33.5 / 29.9 / 30.3（p95 63~69）
- prime core7：13.1，**p50 = 0（一半时间完全空闲）**，p95 87

### 最耗 CPU 的线程（tgtop，playback）

| util% | 线程 |
|---|---|
| 42.3 | tv.danmaku.bili（主进程） |
| 29.4 | dolbycodec2（杜比音效解码） |
| 25.1 | surfaceflinger（合成） |
| 13.3 | system_server |
| 10.8 | media.hwcodec |
| 9.4 | tv.danmaku.bili:web |

## [driver] 功耗驱动的相关性（按分钟聚合，n=36，Pearson）

| X | r(batt_p, X) |
|---|---|
| **capfreq_little** | **+0.884** |
| gpu_busy | +0.851 |
| capfreq_big | +0.798 |
| cpu_temp | +0.704 |
| capfreq_prime | +0.362 |
| thermal_cap | +0.321 |

（变量间高度共线——负载上来时 gpu/频率/温度同时涨，故 r 只用于**排优先级**，不用于推算绝对值。）

## [find] 五个可下手的问题

1. **小核负载倒挂（头号驱动）**。little 三个核 util 54~59%、p95 逼近 100%，上限却给到 72%（p95 100%）；
   而 big 只有 30~39%。小核在 1.4~2.0 GHz 区间的能效明显差于大核中频，**重线程压在小核上高频跑**
   是这套数据里与功耗相关性最强的一项（r=0.884）。
2. **prime 空转 + 尖峰推高**。core7 p50 = 0（一半时间空闲）却维持 29% 的上限，尖峰又把上限推到
   p95 84%——`util_smoothing 0.35` 已滤掉一部分，但残留尖峰仍能把上限顶到 80%+，
   而超大核空转与高频是最贵的漏电来源。
3. **电池温度顶到软限频**。batt_temp 41.3~41.4 ℃ ≥ `batt_soft_temp_c: 41` → 触发 `soft_perf_cap 0.70`。
   日志里 cap=70% 的那十几分钟功耗确实低（0.7~3.8 W），但那是**被动降频**，代价是性能；
   把功耗本身降下来才是不触发限频的正路。
4. **GPU 与功耗强相关（r=0.851），但 ChiRi 没有 GPU 省电手段**。现有 `src/chiri/gpu.rs` 只做
   「contingency 锁到硬件最高频」，方向相反；视频合成并不需要峰值 GPU 频率。
5. **迁移偏多（≈557/s）**。`module/config/8550/feature.yaml` 把 `sched_migration_cost_ns` 从内核默认
   500000 降到 200000（更愿意跨核均衡）。这对**响应型**负载有利，对视频这类**稳态**负载是反的：
   稳态下应该让任务留在原地，减少 cache 失效与搬迁开销。

## [plan] 优化方案（按「先纯配置、后代码」排序）

### V1 — playback 的 `headroom` 1.05 → 0.95（纯配置，先做这个）

`module/config/normal/tuned_profiles.yaml:29-35`：

```yaml
playback:
  headroom: 1.05      # → 0.95（建议先试 1.0，再 0.95）
  perf_floor: 0.0
  hysteresis: 0.04
  down_hold_ms: 100
  util_smoothing: 0.35
  boost_affinity: false
```

公式 `上限比例 = clamp(组内最大核占用率 × headroom, floor, 1.0)`，按当前 util 估算：

| cluster | 现在 cap p50 | headroom 1.0 | headroom 0.95 |
|---|---|---|---|
| little | 72.4 % | ~63 % | ~60 % |
| big | 56.7 % | ~51 % | ~48 % |
| prime | 29.2 % | ~21 % | ~20 % |

- **预期**：0.2~0.5 W（相关性外推不可直接当收益，必须实测）
- **风险**：压太狠会掉帧/解码卡顿。视频是硬解（media.hwcodec / dolbycodec2），CPU 侧余量通常比想象大，
  但仍需真机确认
- **回退**：卡顿或功耗无改善即回退 1.05

### V2 — `util_smoothing` 0.35 → 0.25（纯配置，低风险）

进一步滤掉弹幕/进度条尖峰，目标是压住 prime 的 p95（84% → 约 70%）。
视频是稳态场景，对决策响应延迟不敏感，风险低。可与 V1 同批做。

### V3 — 给 `SpecialTunedConfig` 加 per-cluster 覆盖（代码，中风险）

现状：三个 cluster 共用同一组 headroom/floor。但数据显示三簇的问题完全不同
（little 过载、big 合理、prime 空转），一刀切必然顾此失彼。

建议新增（示例）：

```yaml
playback:
  headroom: 1.0
  per_cluster:
    little: { headroom: 0.9, perf_ceil: 0.65 }
    prime:  { headroom: 0.8, perf_ceil: 0.5 }
```

需要改：`src/chiri/config.rs`（`SpecialTunedConfig` + normalize）、`src/chiri/tuned.rs`（按 cluster 取参数）。
注意 `tuned.rs` 目前**没有 perf_ceil**（CLG 有），加之前先确认语义与 CLG 对齐。

### V4 — playback 场景的 GPU 频率上限（代码，高收益 / 高风险）

给 `src/chiri/gpu.rs` 增加「按场景设上限」能力（现在只有锁最高频一种）。
gpu_busy 与功耗 r=0.851 且视频场景利用率不高（avg 8.6），限到硬件最高频的 50~60% 有空间。
**必须先做掉帧验证**（用 fps 列 + 主观），掉帧即回退。

### V5 — playback 期间调高 `sched_migration_cost_ns`（代码，小收益）

在 TunedGovernor 接管 playback 时把 `sched_migration_cost_ns` 从 200000 提到 400000~500000，
退出时按快照恢复（复用 `restore` 机制）。只在该场景生效，不动其它场景的响应性。

### V6 — 不建议主动改的项

- `batt_soft_temp_c: 41` → 42/43：确实能减少限频触发，但属于安全阈值，收益/风险比不划算
- `dolbycodec2` 29.4%、`surfaceflinger` 25.1%：分别是 app 行为与系统合成，ChiRi 无直接手段
  （除非给非关键线程做 uclamp 降权，但会牵连前台体验）

## [verify] 验证方法

同一场景（同一个视频、同亮度、同音量、同网络状态）录两份 devimp，各 ≥10 分钟：

1. `batt_p` 的 avg / p50（主指标）
2. `batt_temp` 与 `cap%`（是否不再触发 70% 限频）
3. `capfreq_little` / `capfreq_big` / `capfreq_prime`（上限是否按预期下移）
4. `fps` 列 + 主观流畅度（**一票否决项**：掉帧即回退）
5. 顺便看 `migrations` 是否随 V5 下降

判定：功耗降 ≥0.2 W 且无卡顿 → 采纳；否则回退到上一档。

## [done] 已落地（2026-09-20 晚，按用户确认执行 V1 / V2 / V3 / V5；V4 GPU 未做）

**参数全部走 special 可调字段，没有写死任何数值**：

| 层 | 位置 | 改动 |
|---|---|---|
| 配置 | `module/config/normal/tuned_profiles.yaml` | playback：headroom 1.05→**0.95**；util_smoothing 0.35→**0.25**；新增 `perf_ceil: 1.0`；`per_cluster`：little `{headroom 0.90, perf_ceil 0.65}`、prime `{headroom 0.80, perf_ceil 0.50}`；`migration_cost_ns: 400000` |
| 配置 | `module/config/normal/tuned_profiles-example.yaml` | 同步字段说明书（perf_ceil / per_cluster / migration_cost_ns 的语义与默认值） |
| 代码 | `src/chiri/config.rs` | `SpecialTunedConfig` 新增 `perf_ceil`、`per_cluster: HashMap<String, ClusterTunedOverride>`、`migration_cost_ns: Option<u64>`；新增 `ClusterTunedOverride` / `EffectiveTuned` / `for_cluster()`；**headroom clamp 下限 1.0 → 0.5**（不改这行，0.95 会被静默抬回 1.0，该参数在省电方向等于失效） |
| 代码 | `src/chiri/tuned.rs` | 决策改为按组取 `for_cluster(core_name)` 的有效参数；`target_ratio` 用 `clamp(perf_floor, perf_ceil)`；新增 `apply_migration_cost` / `restore_migration_cost`（接管时写、release 按快照恢复，未配置则完全不碰该节点） |

**等价性保证**：不写 `per_cluster`、不写 `perf_ceil`、不写 `migration_cost_ns` 时，
参数与行为**逐位等同改动前**（默认 1.0 / 空表 / None）；`akmode` 与其它未列覆盖的模式不受影响。
`cargo +nightly check -p chiri --target aarch64-linux-android` 通过。

**预期效果**（按日志 util 估算，非实测）：

| cluster | 调整前 cap p50 | 调整后估算 | 主要手段 |
|---|---|---|---|
| little | 72.4 % | 0.63 × 0.90 = 0.567 → 约 57 % | per_cluster headroom |
| big | 56.7 % | 0.51 × 0.95 = 0.485 → 约 48 % | 模式级 headroom |
| prime | 29.2 %（p95 84 %） | 0.21 × 0.80 = 0.168 → 约 17 %；尖峰 0.76×0.8=0.61 被 `perf_ceil 0.50` 截断 | headroom + 天花板 |

**验证（真机，一票否决制）**：同视频、同亮度/音量/网络，录 devimp ≥10 分钟 →
1. `batt_p` avg / p50（主指标，目标降 ≥0.2 W）
2. `batt_temp` 与 `cap%`（目标：不再落进 70 % 软限频区）
3. `capfreq_little / big / prime` 是否按上表下移
4. **fps 与主观卡顿 —— 掉帧即回退**
5. `migrations` 是否随 `migration_cost_ns` 下降

回退路径：先把 headroom 回 1.0（只保留 per_cluster 对 little/prime 的压制），仍卡则整体回 1.05 / 0.35。
