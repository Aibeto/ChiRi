// daemon.ts: [liveness] [watchdog] [stop] [recover]
// 守护进程存活判据与生命周期操作。硬规则（见计划文档与项目记忆）：
// - 存活判据只能用 daemon.lock 的 flock 探测，pidof 是弱信号；
// - 探测必须「取锁后立刻释放」，否则 daemon 下次启动抢不到锁会静默退出；
// - 绝不删除 daemon.lock（flock 是 inode 级，删除会让新实例在旧 inode 之外加锁成功 → 双实例）；
// - 关闭调度必须先杀看门狗再杀主进程，否则看门狗 3s 后会把 daemon 拉回来。
import { absOf, shQuote } from './paths'
import { exists, readText } from './read'
import { isLive, run } from '@/kernel/shell'
import { absent, failed, ok, shellError, type ReadResult } from './errors'

// [liveness]
export type DaemonState =
  /** 锁被持有 = 有活跃实例 */
  | 'running'
  /** 能拿到锁 = 没有实例在跑 */
  | 'stopped'
  /** 无法判定（无 flock applet 或环境不支持）——界面如实显示，不退回 pidof */
  | 'unknown'

let flockAvailable: boolean | null = null

/** flock applet 可用性（Android 由 toybox/busybox 提供，缺失时无法判定存活） */
export async function hasFlock(): Promise<boolean> {
  if (flockAvailable !== null) return flockAvailable
  if (!isLive()) return (flockAvailable = false)
  try {
    const { stdout } = await run('command -v flock >/dev/null 2>&1 && echo 1 || echo 0')
    flockAvailable = stdout.trim() === '1'
  } catch {
    flockAvailable = false
  }
  return flockAvailable
}

/** 仅测试用：注入 flock 可用性 */
export function setFlockAvailableForTest(v: boolean | null): void {
  flockAvailable = v
}

export async function probeLiveness(): Promise<ReadResult<DaemonState>> {
  if (!isLive()) return absent<DaemonState>('unsupported-env')
  if (!(await hasFlock())) return ok<DaemonState>('unknown')
  try {
    // 带命令形式（flock -n FILE true）：命令结束即释放锁；不带命令会从 stdin 持续持锁
    const { errno, stdout, stderr } = await run(
      `flock -n ${shQuote(absOf('daemonLock'))} true >/dev/null 2>&1 && echo FREE || echo HELD`
    )
    if (errno !== 0) return failed<DaemonState>(shellError(errno, stderr))
    const verdict = stdout.trim()
    if (verdict === 'FREE') return ok<DaemonState>('stopped')
    if (verdict === 'HELD') return ok<DaemonState>('running')
    return ok<DaemonState>('unknown')
  } catch (e) {
    return failed<DaemonState>(e instanceof Error ? e.message : String(e))
  }
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
 * 关闭调度：先按 pidfile 杀看门狗 → 再杀 yumi → 删 pidfile。
 * 注意副作用（由界面负责提示）：daemon 是被信号杀死、没有还原逻辑，
 * fast 模式的锁频会残留到卸载脚本执行；恢复只能点模块 Action 或重启设备。
 */
export async function stopScheduler(): Promise<ReadResult<true>> {
  if (!isLive()) return absent<true>('unsupported-env')
  const pidFile = shQuote(absOf('watchdogPid'))
  const cmd =
    `p=$(cat ${pidFile} 2>/dev/null); ` +
    `case "$p" in ''|0|*[!0-9]*) ;; *) kill "$p" 2>/dev/null ;; esac; ` +
    `killall -9 yumi 2>/dev/null || pkill -9 yumi 2>/dev/null; ` +
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
