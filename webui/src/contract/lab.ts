// lab.ts: [read] [write]
// 实验室（rhine.chr）契约。这个文件对外暴露、支持手改，读写的唯一权威是它本身，
// 界面不缓存也不推测守护进程有没有套用成功。
//
// 三条与 meta.yaml 不同、容易被写错的地方：
//  1. 文件不存在 = 合法未启用，不是「不适用」也不是失败。守护进程启动时会补建它。
//  2. 写入只需一个模式 key（或空、或保留字 off），没有「多字段一起提交」的语义，
//     因此不做草稿。
//  3. 关闭实验室要写空内容而不是删文件——「文件在但为空」和「文件被删」对界面
//     是同一件事，但对用户排查来说前者更清楚。
//  4. 锁定期间关闭会被守护进程挡回，要真的关掉得写保留字 off（强制关闭）：
//     写空内容 ≠ 强制关闭，这是两个不同的请求。
import { absOf, shQuote } from './paths'
import { isLive, run } from '@/kernel/shell'
import { absent, failed, ok, shellError, type ReadResult } from './errors'
import { exists, readText } from './read'
import { utf8ToBase64 } from './meta'
import {
  LAB_LOCK_DIRS,
  LAB_LOCK_NAME,
  labFileContent,
  parseLabLock,
  parseLabState,
  type LabLock,
  type LabState,
  type LabWriteTarget
} from '@/data/lab'

/** rhine.chr 单行内容，4KB 足够，防止读到异常大的文件 */
const LAB_READ_BYTES = 4 * 1024

export interface LabSnapshot {
  /** rhine.chr 绝对路径 */
  path: string
  state: LabState
  /** 锁定标记：存在就表示「本次开机后启用过」，运行时关不掉 */
  lock: LabLock
  /** rhine-back.chr 是否存在（原值快照 = 还原能力） */
  hasBackup: boolean
}

// [read]
/** 按候选目录顺序找锁定标记，都没有就是未锁定（顺序与守护进程一致） */
async function readLock(): Promise<LabLock> {
  for (const dir of LAB_LOCK_DIRS) {
    const r = await readText(`${dir}/${LAB_LOCK_NAME}`, 'not-created', LAB_READ_BYTES)
    if (r.kind === 'ok') return parseLabLock(r.value, true)
  }
  return parseLabLock('', false)
}

export async function readLab(): Promise<ReadResult<LabSnapshot>> {
  const path = absOf('rhine')
  const [text, lock, hasBackup] = await Promise.all([
    readText(path, 'not-created', LAB_READ_BYTES),
    readLock(),
    exists(absOf('rhineBack'))
  ])
  if (text.kind === 'failed') return text
  // 缺失按未启用处理：守护进程启动时就会补建，这里报「缺失」只会误导
  const raw = text.kind === 'ok' ? text.value : ''
  return ok({ path, state: parseLabState(raw), lock, hasBackup })
}

// [write]
/**
 * 写入实验室状态：`mode` 为 null = 关闭（写空内容）、为保留字 `off` = 强制关闭
 * （见 data/lab.ts::LAB_FORCE_OFF）、否则是模式 key。
 * 与配置页同款写法——先落同目录临时文件再原子替换，避免守护进程读到半截内容；
 * 临时文件后缀 `.webui.tmp` 与配置页保持一致。
 */
export async function writeLabMode(mode: LabWriteTarget): Promise<ReadResult<LabSnapshot>> {
  if (!isLive()) return absent<LabSnapshot>('unsupported-env')
  const path = absOf('rhine')
  const tmp = `${path}.webui.tmp`
  const b64 = utf8ToBase64(labFileContent(mode))
  const cmd =
    `printf '%s' ${shQuote(b64)} | base64 -d > ${shQuote(tmp)} && ` +
    `mv -f ${shQuote(tmp)} ${shQuote(path)} || { rm -f ${shQuote(tmp)}; exit 1; }`
  try {
    const { errno, stderr } = await run(cmd)
    if (errno !== 0) return failed<LabSnapshot>(shellError(errno, stderr))
  } catch (e) {
    return failed<LabSnapshot>(e instanceof Error ? e.message : String(e))
  }
  // 写后回读：以文件实际内容为准，不回报「我以为写进去的值」
  return readLab()
}
