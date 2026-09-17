// power.ts: [read]
// PowerAVG.chr 只读契约（模块根）。文件由守护进程写（每次 1s 采样）、WebUI 只读
// 展示：不存在（daemon 未跑过 / Yumi 无此功能）与空文件统一归一为「无值」
// （watt = null），界面显示 ———不是错误；只有环境不可用才报 failed。
import { absOf } from './paths'
import { readText } from './read'
import { ok, type ReadResult } from './errors'
import { parsePowerAvgWatt } from '@/data/power-avg'

/** 单行数字，4KB 足够 */
const POWER_AVG_READ_BYTES = 4 * 1024

export interface PowerAvgSnapshot {
  /** PowerAVG.chr 绝对路径 */
  path: string
  /** 当前留存值（W）；文件缺失/为空/内容非法时为 null */
  watt: number | null
}

export async function readPowerAvg(): Promise<ReadResult<PowerAvgSnapshot>> {
  const path = absOf('powerAvg')
  const text = await readText(path, 'not-created', POWER_AVG_READ_BYTES)
  if (text.kind === 'failed') return text
  // 缺失（absent）与空内容同口径：无值，不报缺失（守护进程跑起来就会写）
  return ok({
    path,
    watt: parsePowerAvgWatt(text.kind === 'ok' ? text.value : '')
  })
}
