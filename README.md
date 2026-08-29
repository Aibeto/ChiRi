[Read this document in English](README.en.md)

> English support is currently suspended

# ChiRi 调度

**古法（？） CPU 调度：eBPF + Rust + FAS 帧感知调度（未启用） + schedutil 调速器**

---

## 项目介绍

**ChiRi**是一个以日用流畅度为主的Android CPU调度，以日用流畅度为主，其次是轻度游戏
低功耗并不是主要目标，但并不代表极差的续航水平
中高负载日用场景尚可，游戏差点意思（
请记住，这不是一个以游戏性能为主的调度。

### 主要特性

- **CLG** - 基于 eBPF 实时负载数据的自适应调频，替代传统内核调速器。
- **WebUI** - 无需安装额外 App，通过浏览器即可管理调度配置
- **场景特调** - ？！特调？！
- **热更新** - 无需重启设备即可更新调度，详见[如何不重启设备更新调度](/mdocs/updateWithoutRestart.md)

## 环境要求

Android 8.0 (API 26) 及以上 ARM64 (AArch64) 并拥有 Root 权限
内核支持 eBPF（需要 `CONFIG_BPF`、`CONFIG_BPF_SYSCALL` 等内核选项）和 schedutil 调速器

## 性能模式

ChiRi 提供6种性能模式：

| 模式              | 描述                     | 启用场景         |
| :---------------- | :----------------------- | :--------------- |
| **scenemode**     | 树懒模式，尽可能降低功耗 | 暂不支持手动启用 |
| **powersave**     | 降低响应，延长续航       | 待机、轻度使用   |
| **balance**       | 自适应调频，平衡         | 万金油           |
| **performance**   | 高响应配置，性能优先     | 中高负载         |
| **fast**          | 最大性能释放             | 不服跑个分       |
| **FAS（未启用）** | 未完成                   | 中负载游戏场景   |

---

### 调度核心

ChiRi CLG 调度核心，使用白名单适配soc
[已适配soc列表](/mdocs/socList.md)
没有你的soc？请提交issue

#### 核心特性

- **高性能 Rust 实现**: 极低的系统资源占用，运行功耗极低。
- **eBPF 内核级监控**: 通过 `sched_switch` tracepoint 精确采集每核心 CPU 利用率和线程运行时间；通过 `queueBuffer` uprobe 零开销捕获渲染帧间隔。
- **实时配置监听**: 支持配置文件（`config.yaml`）和规则文件（`rules.yaml`）热重载，切换模式无需重启。
- **内置 FAS 引擎**: PID 控制器驱动的帧感知调度，支持自动容量权重探测、per-app 配置、CPU 利用率辅助调频。
- **CLG 负载调速器**: 基于 eBPF 实时负载的自适应调频，替代内核原生调速器。
- **多语言国际化**: 基于 Fluent 的 i18n 系统，支持中英文日志输出。

## 性能优化建议

### 日常使用

1.  **balance** - 自适应调频，适用于大部分场景
2.  **特调balance** - 详见各特调说明

### 游戏优化

在 FAS 功能完成之前，建议使用 balance 模式，性能不足时切换到 performance 模式
请谨慎使用fast模式，此模式下 CPU 频率可能会锁定为最大

## 故障排除

### 常见问题

## 项目统计

<div align="center">

## Star History

<a href="https://www.star-history.com/?repos=Aibeto%2FChiRi&type=date&legend=top-left">
 <picture>
   <source media="(prefers-color-scheme: dark)" srcset="https://api.star-history.com/chart?repos=Aibeto/ChiRi&type=date&theme=dark&legend=top-left&sealed_token=tf3zEomCrJwH8mUjjoPJ4AYEGRMg4j2ikzBb69MYPk8hz7_LJPCxNNlSn_EzPeOCmXuuudIcf4hXzvAheF8cIHNIzUjGPZ0odO4AEGoNdkeQbOA5kRfoHg" />
   <source media="(prefers-color-scheme: light)" srcset="https://api.star-history.com/chart?repos=Aibeto/ChiRi&type=date&legend=top-left&sealed_token=tf3zEomCrJwH8mUjjoPJ4AYEGRMg4j2ikzBb69MYPk8hz7_LJPCxNNlSn_EzPeOCmXuuudIcf4hXzvAheF8cIHNIzUjGPZ0odO4AEGoNdkeQbOA5kRfoHg" />
   <img alt="Star History Chart" src="https://api.star-history.com/chart?repos=Aibeto/ChiRi&type=date&legend=top-left&sealed_token=tf3zEomCrJwH8mUjjoPJ4AYEGRMg4j2ikzBb69MYPk8hz7_LJPCxNNlSn_EzPeOCmXuuudIcf4hXzvAheF8cIHNIzUjGPZ0odO4AEGoNdkeQbOA5kRfoHg" />
 </picture>
</a>

</div>

## 联系方式

- **GitHub Issues** - [项目问题和建议](https://github.com/Aibeto/ChiRi/issues)

---

<div align="center">

<sub>ChiRi - 千漓</sub>
<sub>Based on imacte/yumi</sub>

</div>
