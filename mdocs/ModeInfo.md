# 模式信息 / Mode Info

调度模式分三套：CLG 档位、特调（Special Tuning）、实验室（Rhine）。另有 `fas` 与 `down` 两个值会出现在 `current_mode.chr`。

模式值出现的位置：

- `current_mode.chr`：守护进程写的当前模式，WebUI 状态页读它（daemon 停止后是陈旧值）
- `daemon.log` / devimp 日志的 mode 列
- 界面显示名走 i18n（WebUI `mode.*`，daemon 日志 `[Tuned]` 等前缀）

前台模式选择顺序（`src/monitor/app_detect.rs` 的 `determine_mode`）：

1. FAS 白名单命中且应用配置可用 → `fas`
2. 特调白名单命中 → 该条目的回退模式
3. `rules.yaml` 的 `app_modes`
4. 兜底 `global_mode`（实验室启用期间被其覆盖）

## CLG 档位 / CLG Gears

按负载分档调频。每个负载 tick（ChiRi 160ms）用平滑占用比对升降频阈值，调整各核心组的性能上限（`perf_ceil` 封顶、`headroom` 给余量）；触摸时短时升频。实现是 CLG，在 `src/chiri/cpu_load_governor.rs`。

由 `rules.yaml` 的 `global_mode` / `app_modes` 指定。id 与显示名相同，描述走 i18n `mode.*.desc`。

| 字段                       | reduce      | default    | boost      |
| -------------------------- | ----------- | ---------- | ---------- |
| up_threshold               | 0.90        | 0.72       | 0.55       |
| down_threshold             | 0.50        | 0.55       | 0.40       |
| smoothing_up               | 0.50        | 0.75       | 0.95       |
| up / down_rate_limit_ticks | 2 / 2       | 2 / 3      | 1 / 5      |
| headroom_factor            | 1.05        | 1.20       | 1.40       |
| perf_floor / perf_ceil     | 0.05 / 0.60 | 0.10 / 1.0 | 0.40 / 1.0 |
| touch_boost_ms             | 350         | 400        | 500        |

（上表为 8550 实测值，其余防抖/微调字段见配置文件；`vector` 档不读这套参数，运行时由 `fast_lock` 锁 min=max=硬件最高频，段里的值只是占位）

- 参数位置：`module/config/<soc>/feature.yaml`（8550/8475/8998 各一份），共享默认在 `module/config/feature.yaml`
- 生效方式：编译期嵌入，改完要重新编译；`meta.yaml` 的热重载不涉及档位参数
- 与特调的关系：前台命中特调白名单时 CLG 释放让位，退出特调后交还

## 特调 / Special Tuning

面向特定用途的连续控制。前台命中白名单就接管：先 write schedutil、min 压到硬件最低，之后每个负载 tick（40ms，Monitor 层按激活标志切采样率）按「组内最大核心占用 × headroom」算出 scaling_max_freq 上限。

- 升频立即执行（响应性优先）；降频带 hysteresis 死区，且目标须持续 `down_hold_ms` 才下调
- 负载可做 EMA 平滑（`util_smoothing`）：视频/轻载这类抖动场景滤掉尖峰，游戏保持原始值
- 看门狗：负载事件中断 5 秒（`CLG_STALE_MAX`，eBPF 源失效）就释放接管、恢复原 governor/min/max
- 接管失败（无可用 cluster）进入 300 秒冷却，期间由 CLG 接管
- 退出前台释放接管，频率与 governor 按快照恢复

白名单在 `src/chiri/special_tuned.yaml`（编译期嵌入），参数组在 `module/config/normal/tuned_profiles.yaml`。

| 字段           | akmode                                        | playback                                                   |
| -------------- | --------------------------------------------- | ---------------------------------------------------------- |
| 用途           | 游戏（明日方舟）                              | 视频播放稳态                                               |
| 触发           | arknights 国服/日服 + `re:(?i)arknights` 兜底 | B 站 `tv.danmaku.bili` + `re:(?i)(bilibili\|danmaku\.bili)` |
| headroom       | 1.15                                          | 1.05                                                       |
| perf_floor     | 0.0                                           | 0.0                                                        |
| hysteresis     | 0.03                                          | 0.04                                                       |
| down_hold_ms   | 100                                           | 100                                                        |
| util_smoothing | 1.0（不平滑）                                 | 0.35                                                       |
| boost_affinity | true                                          | false                                                      |

特调只给长稳态场景做。daily 组（桌面/聊天/资讯等 17 个应用）2026-09-17 晚上机实测后移除：高频短交互收益低、侵入高。

字段含义：

| 字段           | 作用                                                                                                      |
| -------------- | --------------------------------------------------------------------------------------------------------- |
| headroom       | 目标上限 = 组内最大占用 × 该值                                                                            |
| perf_floor     | 目标比例下限（0 = 空闲可到硬件最低）                                                                      |
| hysteresis     | 变更死区（比例，乘以硬件最高频）                                                                          |
| down_hold_ms   | 降频保持时长                                                                                              |
| util_smoothing | 决策负载 EMA 系数，1.0 = 不平滑                                                                           |
| boost_affinity | 是否走 boost 亲和：true 收窄 cpuset 到 big+prime 且 core_ctl 保大核常在线；省电型设 false（保持普通亲和） |

白名单格式：每行 `匹配器:模式列表(逗号分隔):回退模式`。精确包名优先（文件内顺序），未命中再按 `re:` 正则条目；实验室内关闭全部特调期间，整条体系失效。正则条目不会出现在 WebUI 导出的快照里。

新增一个特调模式：

1. `special_tuned.yaml` 加条目（模式列表写新模式名）
2. `normal/tuned_profiles.yaml` 的 `tuned_profiles` 段加同名参数组；不配参数组会回退 `akmode` 缺省段，启动时打 `tuned-profile-missing` 告警
3. 不需要改代码；界面里新模式显示为「特调」

## FAS

FAS（帧感知调度）面向白名单游戏：帧事件驱动 per-policy 锁频（min=max），配合 PID 与 Jank 统计调优，governor 切 performance。失去前台后有 15 秒延迟期（`deactivate_delay_secs`），期内切回无缝续期，到期才释放、交回 CLG。FAS 期间 CLG / 特调 / vector 全部暂停，温控与触摸升频豁免。

- 白名单：`module/config/normal/fas.yaml`（`包名: 配置名`）
- 每应用配置：`module/config/normal/fas/<配置名>.yaml`（字段说明见 `fas-example.yaml`），未写字段取 `src/fas_types.rs` 默认值
- 当前条目：终末地 `com.hypergryph.endfield → endfield`
- 新增游戏两步：`fas.yaml` 加一行 + 新建 `fas/<配置名>.yaml`，无需改代码

## 实验室 / Lab（Rhine）

用 `rhine.chr` 启用（手改或 WebUI 进入）。影响项定义在 `module/config/rhine-init.yaml`，启用时按条目改写其它调度模式，原值备份到 `rhine-back.chr`；关闭实验室或重启设备按备份还原。本次开机启用过会锁定，普通关闭被拒，只能重启或确认走强制关闭（清标记、快照还原，运行时残留由还原后的模式接手）。

| key           | 显示名                          | 影响项                                                                                                        |
| ------------- | ------------------------------- | ------------------------------------------------------------------------------------------------------------- |
| `vector`      | Vector Breakthrough / 矢量突破  | global_mode=vector；关 FAS、息屏场景模式、全部特调                                                            |
| `contingency` | Contingency Contract / 危機契約 | global_mode=contingency；同上开关项。生效行为：CPU+GPU 全核最高频、performance 调速器、停线程迁移、后台压小核 |
| `babel`       | Babel                           | global_mode=babel；同上开关项。生效行为：停线程迁移，后台小核、前台/顶部大核+超大核、系统进程大核             |
| `frozen`      | Frozen                          | 预留，无影响项，不给启用入口                                                                                  |

影响项还有通用的 `thread_bind`（false = 线程摆放交还系统）。新增实验模式：`rhine-init.yaml` 追加条目，同时改 WebUI 的模式列表与文案（`webui/src/data/lab.ts`、locales）；`off` 是保留字（强制关闭），不能当模式 key。

## 其他模式值

- `fas`：FAS 会话进行中的当前模式（见上）
- `down`：DOWN 停摆。调度全部释放、只留采集与日志，由 `down.chr` 控制；用于记录「ChiRi 不工作时」设备自身的调度情况。退出停摆按进入前的模式重新接管
- `scenemode` 不是模式值：独立的息屏调度轴。息屏超过 `scene_mode_delay_secs`（8550 为 300 秒）后进入：频率上限压到 15%，core_ctl 可下线 prime 簇省漏电；亮屏按快照恢复。总闸是 `meta.yaml` 的 `scenemode_enabled`

## 相关文件 / Files

| 文件                                              | 作用                                                                |
| ------------------------------------------------- | ------------------------------------------------------------------- |
| `module/config/<soc>/feature.yaml`                | CLG 各档参数、亲和、core_ctl、温控（8550/8475/8998）                |
| `module/config/normal/tuned_profiles.yaml`        | 特调参数组（缺省段 akmode + tuned_profiles 段）                     |
| `src/chiri/special_tuned.yaml`                    | 特调白名单（格式 `匹配器:模式列表:回退模式`）                       |
| `module/config/rhine-init.yaml`                   | 实验室模式定义与影响项                                              |
| `module/config/normal/fas.yaml` + `fas/<名>.yaml` | FAS 白名单与每应用调优                                              |
| `module/config/normal/scenemode.yaml`             | 息屏场景模式参数                                                    |
| `module/rules.yaml`                               | `global_mode` / `app_modes` / `dynamic_enabled`（编译期嵌入，只读） |
| `module/config/meta.yaml`                         | 用户可改项：日志等级、语言、FAS/息屏场景模式/线程调整开关           |
