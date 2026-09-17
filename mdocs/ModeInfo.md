# 模式信息 / Mode Info

分三套：CLG 档位、特调（Special Tuning）、实验室（Rhine）。另有 `fas` 与 `down` 两个值会出现在 `current_mode.chr`。

前台模式选择顺序（`src/monitor/app_detect.rs` 的 `determine_mode`）：

1. FAS 白名单命中且应用配置可用 → `fas`
2. 特调白名单命中 → 该条目的回退模式
3. `rules.yaml` 的 `app_modes`
4. 兜底 `global_mode`（实验室启用期间被其覆盖）

## CLG 档位 / CLG Gears

按负载分档调频：升/降频阈值加性能上限，实现是 CLG（`src/chiri/cpu_load_governor.rs`）。由 `rules.yaml` 的 `global_mode` / `app_modes` 指定。id 与显示名相同，描述走 i18n `mode.*.desc`。

| id        | 用途               | 8550 参数要点                                     |
| --------- | ------------------ | ------------------------------------------------- |
| `reduce`  | 省电，优先续航     | 升频阈值 0.90，上限 0.60                          |
| `default` | 日常均衡，兜底档   | 升频 0.72，上限 1.0                               |
| `boost`   | 性能优先，放开上限 | 升频 0.55，headroom 1.40                          |
| `vector`  | 全核最高频硬锁     | 不读 CLG 参数，由 `fast_lock` 锁 min=max=硬件最高 |

参数在 `module/config/<soc>/feature.yaml`，共享默认在 `module/config/feature.yaml`。

## 特调 / Special Tuning

面向特定用途的连续控制。每个负载 tick（40ms）按「组内最大核心占用 × headroom」算出频率上限：升频立即执行，降频带 hysteresis 死区和 `down_hold_ms` 防抖。前台命中白名单就接管，CLG 让位；退出前台交还。

白名单：`src/chiri/special_tuned.yaml`（编译期嵌入，格式 `匹配器:模式列表:回退模式`）。
参数组：`module/config/normal/tuned_profiles.yaml`（键名与模式名同名；缺省段 `akmode` 兼作未注册模式的回退）。

| id         | 用途                     | 触发                                                                                                   | 参数要点                                   |
| ---------- | ------------------------ | ------------------------------------------------------------------------------------------------------ | ------------------------------------------ |
| `akmode`   | 游戏（明日方舟）         | arknights 国服/日服 + `re:(?i)arknights` 兜底                                                          | headroom 1.15，无平滑，boost 亲和开        |
| `playback` | 视频播放稳态             | B 站 `tv.danmaku.bili` + `re:(?i)(bilibili\|danmaku\.bili)`                                            | headroom 1.0，平滑 0.35，亲和关            |
| `daily`    | 桌面/社交/资讯等轻载交互 | 17 个精确包（桌面、QQ、通义、飞书、酷安、Edge、MT、trim、网易云、DeepSeek 等）+ `re:(?i)launcher` 兜底 | headroom 1.0，floor 0.10，平滑 0.5，亲和关 |

参数字段：

| 字段                          | 作用                                                                     |
| ----------------------------- | ------------------------------------------------------------------------ |
| `headroom`                    | 目标上限 = 组内最大占用 × 该值                                           |
| `perf_floor`                  | 目标比例下限（0 = 空闲可到硬件最低）                                     |
| `util_smoothing`              | 决策负载的 EMA 系数（1.0 = 不平滑）                                      |
| `boost_affinity`              | 是否走 boost 亲和（收窄 cpuset + core_ctl 保大核常在线）；省电型设 false |
| `hysteresis` / `down_hold_ms` | 降频死区与保持时长                                                       |

## 实验室 / Lab（Rhine）

用 `rhine.chr` 启用（手改或 WebUI）。重启设备自动关闭、改动还原；本次开机启用过会锁定，关闭需要重启，或确认走强制关闭。影响项定义在 `module/config/rhine-init.yaml`。

| id            | 显示名                          | 用途                                                     |
| ------------- | ------------------------------- | -------------------------------------------------------- |
| `vector`      | Vector Breakthrough / 矢量突破  | 全局模式切 `vector` 档，关 FAS、息屏场景模式与全部特调   |
| `contingency` | Contingency Contract / 危機契約 | CPU+GPU 全核最高频，performance 调速器，停线程迁移       |
| `babel`       | Babel                           | 规整化亲和：后台小核、前台/顶部大核+超大核、系统进程大核 |
| `frozen`      | Frozen                          | 预留，未实现，不给启用入口                               |

## 其他模式值

- `fas`：FAS 帧感知调度（白名单游戏，如终末地 `com.hypergryph.endfield`）。按帧数据调频，配置在 `module/config/normal/fas.yaml` 和 `fas/<配置名>.yaml`
- `down`：DOWN 停摆。调度全部释放，只留采集与日志，由 `down.chr` 控制
- `scenemode` 不是模式值：它是独立的息屏调度轴（息屏超时后压上限），不参与模式判定
