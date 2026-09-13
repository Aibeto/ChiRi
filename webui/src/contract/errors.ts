// errors.ts: [types] [constructors] [helpers]
// 读取三分类：把「正常读到的空值」「契约允许的缺失」「真实失败」分开表达。
// 契约要求（见 .workbuddy/docs/webui-refactor-plan.md）：第三种必须在界面上报错，
// 不允许像旧实现那样一律吞成空值。

export type ReadResult<T> =
  | { kind: 'ok'; value: T }
  | { kind: 'absent'; reason: AbsentReason }
  | { kind: 'failed'; error: string }

/** 契约允许的缺失原因（界面按“不适用”呈现，而不是错误） */
export type AbsentReason =
  /** 该能力仅特定机型产生（status.csv / devimp / 特调 / FAS 白名单） */
  | 'chiri-only'
  /** 文件尚未生成（守护进程未启动过、日志未产生） */
  | 'not-created'
  /** 守护进程未运行，数据为上一轮残留而已清理 */
  | 'daemon-stopped'
  /** 运行环境不具备该能力（非 KernelSU WebView） */
  | 'unsupported-env'

// [constructors]
export function ok<T>(value: T): ReadResult<T> {
  return { kind: 'ok', value }
}

export function absent<T>(reason: AbsentReason): ReadResult<T> {
  return { kind: 'absent', reason }
}

export function failed<T>(error: string): ReadResult<T> {
  return { kind: 'failed', error }
}

// [helpers]
export function isOk<T>(r: ReadResult<T>): r is { kind: 'ok'; value: T } {
  return r.kind === 'ok'
}

// [shell-error]
/** shell 执行失败（errno !== 0）时转成统一错误文案 */
export function shellError(errno: number, stderr: string): string {
  const detail = stderr.trim()
  return detail ? `命令失败(${errno}): ${detail}` : `命令失败(${errno})`
}
