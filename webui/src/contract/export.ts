// export.ts: [start] [poll]
// 导出历史归档：把 logd/（历次重启的日志归档）打成 tar.gz 放到 /sdcard/Download。
// 打包交给外部脚本 scripts/pack.sh（对外暴露的稳定接口，与守护进程的启动归档
// 共用同一份、构建流程不得修改）：先 tar 再 gzip，完成后删除中间 .tar。
// 压缩格式 2026-09-18 由 xz 改回 gzip（用户要求：xz 在设备上太慢）。
//
// 两条硬约束（都来自真机现实）：
//  1. **后台执行**：归档可能有几百 MB，压缩要花些时间（gzip 数十秒级），前台等
//     ksu exec 可能被桥的超时掐断（也让人以为界面卡死）。命令自己 fork 到后台、
//     结束写产物/标记文件，前端只轮询。
//  2. **gzip 不可用要能退**：设备可能没有 gzip，此时保留未压缩 .tar 作为产物
//     （tar 本身完整可解，压不动就不压），由 UI 告知。
import { absOf, shQuote } from './paths'
import { isLive, run } from '@/kernel/shell'
import { absent, failed, ok, shellError, type ReadResult } from './errors'

/** 导出目录（外部可见，不需要 root 也能拿到） */
const DOWNLOAD_DIR = '/sdcard/Download'
/**
 * 失败标记放 tmpfs：/dev 在 Android 上必有，/tmp 不一定。刻意不放 /sdcard：
 * 那里不可写（未挂载/只读）时标记根本写不进去，前端只能等到超时才报错。
 */
const FAIL_FILE = '/dev/chiri_export.fail'
/** tar -v 的逐文件输出（每处理一个一行），前端据此算进度 */
const PROGRESS_FILE = '/dev/chiri_export.progress'
/** 待打包文件总数（脚本先用 wc -l 算好，前端当作百分比的分母） */
const TOTAL_FILE = '/dev/chiri_export.total'

export interface ExportJob {
  /** tar.gz 目标路径 */
  target: string
  /** 未压缩回退产物（设备不支持 gzip 时保留的 .tar） */
  fallback: string
  /** 失败标记（内容为退出码）：3 = logd 目录不可进，4 = 打包/压缩失败，5 = 没有历史归档 */
  failFlag: string
}

/** 本地时间戳 MMDD-HHmmss，人眼可辨，与归档命名同风格 */
function stamp(now = new Date()): string {
  const p = (n: number) => String(n).padStart(2, '0')
  return (
    `${p(now.getMonth() + 1)}${p(now.getDate())}-` +
    `${p(now.getHours())}${p(now.getMinutes())}${p(now.getSeconds())}`
  )
}

// [start]
/**
 * 启动后台打包并立刻返回。真正的完成状态由 pollExport 轮询三个标记文件得出。
 * 目录不存在 / 只剩本次运行的文件等错误由后台脚本写进 failFlag，前端读退出码映射文案。
 */
export async function startExport(): Promise<ReadResult<ExportJob>> {
  if (!isLive()) return absent<ExportJob>('unsupported-env')
  const base = `${DOWNLOAD_DIR}/logd_${stamp()}.tar`
  const job: ExportJob = {
    target: `${base}.gz`,
    fallback: base,
    failFlag: FAIL_FILE
  }
  // 打包交给外部脚本（与守护进程启动归档共用同一份，见 module/scripts/pack.sh）：
  // 先 tar 再 gzip，完成后删除中间 .tar；设备没有 gzip 时保留未压缩 .tar 作为产物。
  // 注：/sdcard/Download 是 Android 自带的下载目录，**不创建它**——真撞上不可写
  // 会落到脚本的失败标记，前端 1.5 秒内就能报出来，不会干等到超时。
  const cmd =
    `nohup sh ${shQuote(absOf('packSh'))} export ${shQuote(absOf('logdDir'))} ` +
    `${shQuote(base)} ${shQuote(FAIL_FILE)} ${shQuote(PROGRESS_FILE)} ${shQuote(TOTAL_FILE)} ` +
    `>/dev/null 2>&1 & echo started`
  try {
    const { errno, stderr } = await run(cmd)
    // 带操作上下文：裸的「命令失败(1)」无法定位是启动挂了还是桥的瞬时错误
    if (errno !== 0) return failed<ExportJob>(`启动导出失败：${shellError(errno, stderr)}`)
  } catch (e) {
    return failed<ExportJob>(`启动导出失败：${e instanceof Error ? e.message : String(e)}`)
  }
  return ok(job)
}

// [poll]
export type ExportPhase = 'running' | 'done-gz' | 'done-tar' | 'empty' | 'no-logd' | 'failed'

export interface ExportProgress {
  /** 已开始打包的文件数（tar -v 的行数） */
  done: number
  /** 待打包文件总数（0 = 脚本还没算出来） */
  total: number
  /** 已写入归档的字节数（`.part` 大小，随压缩持续增长） */
  bytes: number
}

export interface ExportProbe {
  phase: ExportPhase
  progress: ExportProgress
}

/**
 * 一轮探测：产物出现 = 完成、失败标记出现 = 按退出码分类、都没有 = 还在压。
 * 顺带把进度三个数一起读回来——多一次 exec 往返只为读进度不划算。
 */
export async function pollExport(job: ExportJob): Promise<ReadResult<ExportProbe>> {
  if (!isLive()) return absent<ExportProbe>('unsupported-env')
  const tarPath = job.fallback
  const part = `${tarPath}.part`
  const mode = `${job.failFlag}.mode`
  // 字节进度 = .part（打包中）+ .tar（打包完成/无 gzip）+ .tar.gz（压缩完成）之和，
  // 同一时刻至多一个存在（脚本 mv 与 gzip 就地删除保证）
  const cmd =
    `[ -f ${shQuote(job.target)} ] && echo s:gz; ` +
    `[ -f ${shQuote(job.failFlag)} ] && echo "s:fail:$(cat ${shQuote(job.failFlag)} 2>/dev/null)"; ` +
    `[ -f ${shQuote(tarPath)} ] && [ ! -f ${shQuote(mode)} ] && echo s:tar; ` +
    `b=$(wc -c < ${shQuote(part)} 2>/dev/null || echo 0); ` +
    `b=$((b + $(wc -c < ${shQuote(tarPath)} 2>/dev/null || echo 0))); ` +
    `b=$((b + $(wc -c < ${shQuote(job.target)} 2>/dev/null || echo 0))); ` +
    `echo "b:$b"; ` +
    `echo "d:$(wc -l < ${shQuote(PROGRESS_FILE)} 2>/dev/null)"; ` +
    `echo "t:$(cat ${shQuote(TOTAL_FILE)} 2>/dev/null)"; ` +
    `exit 0`
  try {
    const { errno, stdout, stderr } = await run(cmd)
    // 带操作上下文：调用方（轮询循环）据此区分「探测挂了」与「打包失败」
    if (errno !== 0) return failed<ExportProbe>(`查询导出进度失败：${shellError(errno, stderr)}`)
    const out = stdout
    const num = (prefix: string): number => {
      // \s* 容忍 wc 系工具在 stdin 计数上的前导空格（GNU 多输入时会有 padding）
      const m = new RegExp(`^${prefix}:\\s*(\\d+)`, 'm').exec(out)
      return m ? Number(m[1]) : 0
    }
    const progress: ExportProgress = { done: num('d'), total: num('t'), bytes: num('b') }
    if (/^s:gz$/m.test(out)) return ok({ phase: 'done-gz', progress })
    if (/^s:tar$/m.test(out)) return ok({ phase: 'done-tar', progress })
    const fail = /^s:fail:(\d+)/m.exec(out)
    if (fail) {
      const phase = fail[1] === '5' ? 'empty' : fail[1] === '3' ? 'no-logd' : 'failed'
      return ok({ phase, progress })
    }
    return ok({ phase: 'running', progress })
  } catch (e) {
    return failed<ExportProbe>(e instanceof Error ? e.message : String(e))
  }
}
