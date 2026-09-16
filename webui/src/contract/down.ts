// down.ts: [read] [write]
// DOWN 停摆契约（down.chr）。文件对外暴露、支持手改，读写的唯一权威是它本身。
// 与 lab 同款：文件不存在 = 正常调度（不是错误，守护进程会补建模板）；
// 解除停摆写空内容而不是删文件——「文件在但为空」对用户排查更清楚。
import { absOf, shQuote } from './paths'
import { isLive, run } from '@/kernel/shell'
import { absent, failed, ok, shellError, type ReadResult } from './errors'
import { readText } from './read'
import { utf8ToBase64 } from './meta'
import { downFileContent, parseDown } from '@/data/down'

/** down.chr 单行内容，4KB 足够 */
const DOWN_READ_BYTES = 4 * 1024

export interface DownSnapshot {
  /** down.chr 绝对路径 */
  path: string
  /** 调度是否处于停摆 */
  active: boolean
}

// [read]
export async function readDown(): Promise<ReadResult<DownSnapshot>> {
  const path = absOf('down')
  const text = await readText(path, 'not-created', DOWN_READ_BYTES)
  if (text.kind === 'failed') return text
  // 缺失按「未停摆」处理：守护进程启动时会补建，这里报缺失只会误导
  return ok({ path, active: parseDown(text.kind === 'ok' ? text.value : '') })
}

// [write]
/**
 * 写停摆状态：`active` true 写保留字 down、false 写空内容。
 * 与配置页同款——先落同目录临时文件再原子替换，避免守护进程读到半截内容。
 */
export async function writeDown(active: boolean): Promise<ReadResult<DownSnapshot>> {
  if (!isLive()) return absent<DownSnapshot>('unsupported-env')
  const path = absOf('down')
  const tmp = `${path}.webui.tmp`
  const b64 = utf8ToBase64(downFileContent(active))
  const cmd =
    `printf '%s' ${shQuote(b64)} | base64 -d > ${shQuote(tmp)} && ` +
    `mv -f ${shQuote(tmp)} ${shQuote(path)} || { rm -f ${shQuote(tmp)}; exit 1; }`
  try {
    const { errno, stderr } = await run(cmd)
    if (errno !== 0) return failed<DownSnapshot>(shellError(errno, stderr))
  } catch (e) {
    return failed<DownSnapshot>(e instanceof Error ? e.message : String(e))
  }
  // 写后回读：以文件实际内容为准
  return readDown()
}
