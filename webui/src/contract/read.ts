// read.ts: [text] [list] [exists] [classify]
// 读取原语：一切读取都限量（daemon.log 单文件 50MB、status.csv 8MB，禁止整读），
// 并把「文件不存在」与「真实失败」分开返回。
import { run, isLive } from '@/kernel/shell'
import { shQuote } from './paths'
import { absent, failed, ok, shellError, type AbsentReason, type ReadResult } from './errors'

/** 单次读取的字节上限：小文件一次读全，大文件自动退化为尾部窗口 */
export const READ_WINDOW_BYTES = 512 * 1024

// [classify]
const MISSING_PATTERNS = [/no such file/i, /not found/i, /does not exist/i, /no such directory/i]

function looksMissing(stderr: string): boolean {
  return MISSING_PATTERNS.some(re => re.test(stderr))
}

function envUnsupported<T>(): ReadResult<T> {
  return absent<T>('unsupported-env')
}

// [text]
/**
 * 读取文件内容（限量窗口）。`tail -c` 对小文件返回全部、对大文件只返回尾部，
 * 天然避免整文件装载；窗口从中间开始时可能截断首行，调用方按行解析即可容忍。
 */
export async function readText(
  path: string,
  missing: AbsentReason = 'not-created',
  maxBytes: number = READ_WINDOW_BYTES
): Promise<ReadResult<string>> {
  if (!isLive()) return envUnsupported<string>()
  try {
    // 不带 `--`：部分 toybox/busybox 版本不认该选项，路径恒为绝对路径也用不到它
    const { errno, stdout, stderr } = await run(`tail -c ${maxBytes} ${shQuote(path)}`)
    if (errno === 0) return ok(stdout)
    if (looksMissing(stderr)) return absent<string>(missing)
    return failed<string>(shellError(errno, stderr))
  } catch (e) {
    return failed<string>(e instanceof Error ? e.message : String(e))
  }
}

/** 读取文件尾部固定字节（日志页用），语义同 readText，但显式表达“取最近一段” */
export async function readTail(
  path: string,
  maxBytes: number,
  missing: AbsentReason = 'not-created'
): Promise<ReadResult<string>> {
  return readText(path, missing, maxBytes)
}

// [list]
/** 列举目录内容（不含隐藏文件）；目录不存在 → absent */
export async function listDir(
  path: string,
  missing: AbsentReason = 'not-created'
): Promise<ReadResult<string[]>> {
  if (!isLive()) return envUnsupported<string[]>()
  try {
    const { errno, stdout, stderr } = await run(`ls -1 ${shQuote(path)}`)
    if (errno === 0) {
      const entries = stdout
        .split('\n')
        .map(line => line.trim())
        .filter(line => line.length > 0)
      return ok(entries)
    }
    if (looksMissing(stderr)) return absent<string[]>(missing)
    return failed<string[]>(shellError(errno, stderr))
  } catch (e) {
    return failed<string[]>(e instanceof Error ? e.message : String(e))
  }
}

// [exists]
/** 路径存在性（文件或目录），不做错误分类——探测语义本身就是二值 */
export async function exists(path: string): Promise<boolean> {
  if (!isLive()) return false
  try {
    const { errno, stdout } = await run(`[ -e ${shQuote(path)} ] && echo 1 || echo 0`)
    return errno === 0 && stdout.trim() === '1'
  } catch {
    return false
  }
}
