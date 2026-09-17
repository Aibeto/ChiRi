// daemon.ts: [liveness] [watchdog] [stop] [recover]
// 守护进程存活判据与生命周期操作。硬规则（见计划文档与项目记忆）：
// - 存活判据 = 心跳文件 LiveTime.chr（daemon 每 15s 写一次本地时间 MM:SS）：
//   与本机时间比对，超过容差（20s）判「已停止」。此前用 flock 探测 daemon.lock，
//   依赖 toybox 是否带 flock applet，缺失时只能显示「无法判定」；
// - daemon.lock 仍由 daemon 自己持有（单实例锁，模块根、不随 logs/ 归档）：WebUI
//   不再探测、也绝不删除——flock 是 inode 级，删除会让新实例在旧 inode 之外加锁成功；
// - 关闭调度必须先杀看门狗再杀主进程，否则看门狗 3s 后会把 daemon 拉回来。
import { absOf, shQuote } from './paths'
import { exists, readText } from './read'
import {
  LIVE_TIME_FILE,
  isLiveTimeFresh,
  nowSecondsOfHour,
  parseLiveTimeSeconds
} from '@/data/live-time'
import { isLive, run } from '@/kernel/shell'
import { absent, failed, ok, shellError, type ReadResult } from './errors'

// [liveness]
export type DaemonState =
  /** 心跳新鲜 = 有活跃实例 */
  | 'running'
  /** 心跳过期或文件缺失 = 没有实例在跑 */
  | 'stopped'
  /** 无法判定（环境不支持 / 读失败 / 内容非法）——界面如实显示，不猜 */
  | 'unknown'

/** 心跳文件极小（`MM:SS\n`，64 字节足够） */
const LIVE_TIME_READ_BYTES = 64

/**
 * 探测存活：读心跳文件并与本机时间比对。
 * - 文件缺失 → stopped（没有任何实例在写心跳）
 * - 内容非法 → failed（读到了但不可用：界面报错，不谎报「已停止」）
 */
export async function probeLiveness(): Promise<ReadResult<DaemonState>> {
  if (!isLive()) return absent<DaemonState>('unsupported-env')
  const r = await readText(absOf('liveTime'), 'not-created', LIVE_TIME_READ_BYTES)
  if (r.kind === 'absent') return ok<DaemonState>('stopped')
  if (r.kind !== 'ok') return r
  const fileSeconds = parseLiveTimeSeconds(r.value)
  if (fileSeconds === null) {
    return failed<DaemonState>(`${LIVE_TIME_FILE} 内容非法：${r.value.trim() || '(空)'}`)
  }
  const fresh = isLiveTimeFresh(fileSeconds, nowSecondsOfHour())
  return ok<DaemonState>(fresh ? 'running' : 'stopped')
}

// [watchdog]
/**
 * 看门狗 PID（契约保留 API：关闭调度在 shell 内联读取，此函数供后续界面展示/诊断用）。
 * 注意两种写入格式并存：sh 看门狗写 `"<pid>\n"`，
 * daemon 自愈（ensure_watchdog_pid_file）写 `"<pid>"` 无换行 → 必须 trim。
 */
export async function readWatchdogPid(): Promise<ReadResult<number>> {
  const r = await readText(absOf('watchdogPid'), 'not-created', 256)
  if (r.kind !== 'ok') return r
  const raw = r.value.trim()
  if (!/^\d+$/.test(raw)) return failed<number>(`watchdog.pid 内容非法：${raw || '(空)'}`)
  return ok(Number(raw))
}

// [stop]
/**
 * 关闭调度：先按 pidfile 杀看门狗 → 再杀 chiri → 删 pidfile。
 * 注意副作用（由界面负责提示）：daemon 是被信号杀死、没有还原逻辑，
 * fast 模式的锁频会残留到卸载脚本执行；恢复只能点模块 Action 或重启设备。
 */
export async function stopScheduler(): Promise<ReadResult<true>> {
  if (!isLive()) return absent<true>('unsupported-env')
  const pidFile = shQuote(absOf('watchdogPid'))
  const cmd =
    `p=$(cat ${pidFile} 2>/dev/null); ` +
    `case "$p" in ''|0|*[!0-9]*) ;; *) kill "$p" 2>/dev/null ;; esac; ` +
    `killall -9 chiri 2>/dev/null || pkill -9 chiri 2>/dev/null; ` +
    `rm -f ${pidFile}; sleep 1; echo done`
  try {
    const { errno, stderr } = await run(cmd)
    if (errno !== 0) return failed<true>(shellError(errno, stderr))
    return ok(true)
  } catch (e) {
    return failed<true>(e instanceof Error ? e.message : String(e))
  }
}

// [recover]
/** 恢复入口只是模块 Action（WebUI 内不能自己 nohup 拉起：ksu.exec 会清理进程组） */
export async function hasActionScript(): Promise<boolean> {
  return exists(absOf('actionSh'))
}
