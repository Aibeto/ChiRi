---
name: devimp-log-analysis
description: 解析与判定 ChiRi 设备的 devimp 日志包（devimpbin 下 logd_*.tar.gz），做功耗/调度回归分析、FAS 与热限频验证、场景画像、调参 A/B。触发词：devimp、日志包、logd_、功耗分析、FAS 验证、帧率档位、迁移率、热限频、场景特调评估。
---

> **双路径镜像说明**（2026-09-23 修订）：本 skill 的**正式位置是 `.agents/skills/devimp-log-analysis/`**
> —— Agent Skills 开放标准（agentskills.io）的通用目录，Cursor 与 CodeBuddy 均可直接加载
> （Cursor 官方识别 `.agents/skills/` 与 `.cursor/skills/`，故无需再维护 `.cursor/` 副本）。
> `.codebuddy/skills/devimp-log-analysis/` 是 CodeBuddy 项目级的**逐字镜像**。**两份 SKILL.md 与
> `scripts/analyze.py` 必须一致**——改任何一份后必须同步另一份（可直接整目录复制）。

> 修订记录：v1（0923 初版）曾把 skill 放 `.cursor/skills/` 并当作正式位置，同日改为 `.agents/skills/`
> 为正式、`.codebuddy/skills/` 为镜像。

# devimp 日志包分析（ChiRi 专用）

设备端 Magisk 模块在「导出日志」时打包 `logd_<MMDD-HHMMSS>.tar.gz`，内含
`devimp_<包名>_<MMDD-HHMMSS>.log`、`status.csv`、`daemon.log`（可能还有内层 tar）。
本 skill 固化的是已重复验证过的分析流程与**判读口径**——数字很容易读错，先读「判定要点」再下结论。

## 一、五步流程

### 1. 解压（含内层 tar）

```powershell
$w="$env:TEMP\an0922\<tag>"; New-Item -ItemType Directory -Force -Path $w | Out-Null
tar -xzf devimpbin\<soc>\logd_XXXX-XXXXXX.tar.gz -C $w
Get-ChildItem $w -Recurse -Filter *.tar | ForEach-Object { tar -xf $_.FullName -C $_.DirectoryName }
```

### 2. 现场判定（**目录名不可信**；先定版再比数据）

| 判什么 | 怎么看 |
|---|---|
| **模块版本** | devimp 文件头三行：`# module=ChiRi Canary <ver> (versionCode N)`；`daemon.log` 的 `[Main] 模块版本:` 行（A06 起有）。**同一 tar 内出现多值 = 混版本，必须报警** |
| **机型/系统** | 同处文件头：`# soc=… board=… model=…` 与 `# android= kernel=`。**机型或系统不同 → 功耗绝对值不可比**（实例：logd_0922-042407 = Canary92 / PHB110 / A16；logd_0922-130643 = Canary88 / MEIZU 21 Note / A15，同目录同天但不可比） |
| 设备族 | devimp 文件名的包名：`miui.*`/`quicksearchbox` = 小米系；`coloros.*`/`heytap.*`/`salt.music` = OPlus 系 |
| schema | devimp 首行表头：无 `cpu_cur_khz` = **40 列旧**（Canary92 前）；含 `cpu_cur_khz` 含 `from_core` = **48 列**（Canary92 期）；含 `cpu_cur_khz` 不含 `from_core` = **44 列新**（拆分版） |
| 版本行为特征 | `daemon.log` 启动段：`已导出 N 个 FAS 白名单条目`、`PowerBase`/`meta.yaml fields invalid` 等 |

### 3. 选组（包内常混入多段旧日志）

按文件名 `_MMDD-HHMMSS` 排序取**最新一簇**；`status.csv` 尾行时间戳佐证。
daemon 可能中途重启过（启动序列日志为界）→ 只取重启后的连续段。

### 4. 跑聚合脚本

```powershell
python .agents\skills\devimp-log-analysis\scripts\analyze.py <解压目录> [--since MMDD-HHMMSS]
# .codebuddy\skills\... 下是逐字镜像，二者等价；改脚本记得两边同步
```

输出：按 `mode×package` 的 n / P_avg / p50 / p95 / battT / cpuT / cap / gpu / psi / mig
+ little/big/prime 的 cap(avg/p95)+util + tgtop 线程占比 + 行类型分布。
充放电方向自动判定（双向试探取合理功耗侧），不确定时会显式提示。

### 5. 判定与对比

- 先答「与什么比」：**同场景、同版本、同 dev_record 状态、同亮度/音量**；跨包必须先核对版本与设备
- 结论模板：场景表 → 与基线差值 → 归因（调度侧/热/GPU/环境）→ 是否可动 ChiRi 参数

## 二、列索引速查

### 44 列（2026-09-23 起重写版）

| 列 | 名字 | 列 | 名字 |
|---|---|---|---|
| 0-3 | ts,type,mode,screen_on | 22-26 | touch,psi_cpu,psi_io,psi_mem,gpu_busy |
| 4-9 | pid,package,tid,comm,cluster,core | 27-29 | batt_v,batt_i,batt_p |
| 10-12 | **max_util**,over_cores,under_cores | 30-32 | wakeups,migrations,freq_trans |
| 13-16 | cur_perf,tgt_perf,**cur_freq_khz,max_freq_khz** | 33-35 | batt_temp,cpu_temp,clg_active |
| 17-21 | decision,deb_up,deb_down,reason,thermal_cap_pct | 36-43 | cpu_cur_khz,cpu_max_khz,cpu_min_khz,cpu_governor,gpu_*×4 |

### 48 列（旧，仍会遇到）

列 10-12 = `from_core,to_core,util_pct`，13 = `max_util`，18/19 = cur/max_freq，25 = `pinned`，
`thermal_cap_pct` 在 25/列为 21（44 列）——**其余列在 44 列版统一左移 3，末 8 列左移 4**。

### 行与文件

- 行类型：`tick`（决策轨迹，按签名变化写）/ `snap`（1s 环境）/ `tgtop`（30s，列 7=comm、12=占用率、13=累计 ms）/ `event`
- **44 列版起**：`place`/`aff`/`core` 三类行迁出到同目录独立 aff 文件（`AFF_HEADER` 1 列）；main 文件里没有它们

## 三、判定要点（最易读错的地方）

- **`cur_freq_khz`/`max_freq_khz` 是调度器写入的 scaling_max 决策值，不是实际频率**；实际值看 snap 的 `cpu_cur_khz`。比率 = cap%
- `max_util`：CLG 行 = 平滑前原始 util；tuned（akmode/playback）行 = 平滑后决策负载——离线二次平滑前先区分来源
- `migrations`/`wakeups` = BpfStats **2s 增量**（除 2 得每秒；数值每 2s 才变一次）
- 迁移率量级：视频稳态 ≈300/s；亮屏 UI（surfaceflinger+system_server 主导）2500~6000/s——**禁止跨场景对比**
- `thermal_cap_pct`：对照 feature.yaml 的 `soft_perf_cap`（8550=0.85、8475/8998=0.70）；
  `free_above` 决定 32.8℃~41℃（8550）间的渐进压制带；41℃ 触发、38℃ 解除（hysteresis 3）
- 充放电：**不要用电流符号判定**（厂商方向不一）；status.csv 有 `charge` 列（权威）。
  devimp snap 无 charge 列时用脚本的双向启发式，并在结论里注明。
  脚本口径：**严格排除 `batt_i == 0`**（无电流/数据缺失视为非放电样本，会略抬 P_avg）；
  输出 `[!] 两侧功率都合理` 时说明该机型方向需人工确认——结合 status.csv 的 charge 列人工核对一次
- 模式语义：`clg_active=1` = CLG 在接管；`mode=fas` = FAS 接管；特调模式（playback/akmode）= tuned 接管；
  PowerBase 开启时替换 CLG（日志有 powerbase-activated）
- `freq_trans` 恒 0 而非 `-` = 内核无 cpufreq tracepoint（探针没挂上），不是「频率从未切换」
- FAS 验证看 `daemon.log` 的 `fas-gear-switch` / `fas-low-perf-upgrade` 与 snap 的 `mode=fas`

## 四、基线参考（会随固件变化，仅作量级对照）

| 场景 | 量级 |
|---|---|
| 待机（息屏/无前台） | 0.4~0.8 W |
| 视频播放（bili，playback 特调后） | 2.1~2.8 W |
| 音乐播放（本地/在线的 media 场景） | 2.2~3.4 W（GPU 高则偏上） |
| 亮屏 UI（抖音/搜索/桌面） | 1.9~2.4 W |
| 聊天（QQ/微信） | 3.0~4.0 W（热态更高） |
| MOBA（王者，FAS 接管后） | 3.0~4.7 W |
| 重负载游戏 | 7 W+ |

## 五、已知坑

1. **目录名不可信**——`devimpbin/8550/` 里出现过 MIUI 8475 设备的包；用包名特征 + SoC 参数形态判
2. **包内混装旧日志**——永远按时间戳筛最新组再下结论
3. **status.csv 可能只有几十行**（daemon 刚重启）——此时只用 devimp；daemon.log 的启动序列是重启界标
4. **统计输出勿截断后计数**——`Select-Object -Last N` 会让「文件数/总改动」失真（踩过）
5. PowerShell 内嵌 python 易被引号/`$` 转义吃掉——**一律写成 .py 文件再跑**；
   pyyaml 在部分环境损坏 → 校验 yaml 用 `node` + `webui/node_modules/js-yaml`
6. 热态日志（battT 长期 40+℃）会整体抬高功耗——评估调参需凉机对照
7. 对功耗结论保持怀疑：先确认「是不是同一台设备、同一个版本、同一类场景」
