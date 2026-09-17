// export.ts: [start] [poll]
// 导出历史 devimp 日志：把 devimp/ 打成 tar.xz 放到 /sdcard/Download。
//
// 三条硬约束（都来自真机现实）：
//  1. **后台执行**：几百 MB 的日志要压几十秒到几分钟，前台等 ksu exec 可能被桥的超时
//     掐断（也让人以为界面卡死）。命令自己 fork 到后台、结束写标记文件，前端只轮询。
//  2. **xz 不可用要能退**：toybox 的 tar 未必带 -J，设备上一般也没有独立 xz。
//     按 tar -J → busybox tar -J → gzip 逐级退，退到 gzip 时产物是另一个文件名，
//     由 UI 如实告知用户。
//  3. **本次运行的文件必须排除**：daemon 正在写它的尾部，打进包里只有半截。
//     判据是 mtime 最新的那个 *.log（WebUI 拿不到 daemon 的内部状态），
//     用 find 生成排除后的列表交给 tar -T，避开各版本 tar 的 --exclude 语义差异。
import { absOf, shQuote } from './paths'
import { isLive, run } from '@/kernel/shell'
import { absent, failed, ok, shellError, type ReadResult } from './errors'

/** 导出目录（外部可见，不需要 root 也能拿到） */
const DOWNLOAD_DIR = '/sdcard/Download'
/**
 * 排除列表与失败标记都放 tmpfs：/dev 在 Android 上必有，/tmp 不一定。
 * 失败标记刻意不放 /sdcard：`mkdir -p /sdcard/Download` 失败时那里根本写不进去，
 * 前端就只能等到超时才报错。
 */
const LIST_FILE = '/dev/chiri_export.list'
const FAIL_FILE = '/dev/chiri_export.fail'
/** tar -v 的逐文件输出（每处理一个一行），前端据此算进度 */
const PROGRESS_FILE = '/dev/chiri_export.progress'
/** 待打包文件总数（脚本先用 wc -l 算好，前端当作百分比的分母） */
const TOTAL_FILE = '/dev/chiri_export.total'

export interface ExportJob {
  /** tar.xz 目标路径 */
  target: string
  /** gzip 回退产物（设备不支持 xz 时出现） */
  fallback: string
  /** 失败标记（内容为退出码）：3 = 进不去 devimp，4 = 三种打包都失败，5 = 没有历史日志 */
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
export async function startDevimpExport(): Promise<ReadResult<ExportJob>> {
  if (!isLive()) return absent<ExportJob>('unsupported-env')
  const base = `${DOWNLOAD_DIR}/devimp_${stamp()}.tar`
  const job: ExportJob = {
    target: `${base}.xz`,
    fallback: `${base}.gz`,
    failFlag: FAIL_FILE
  }
  const devimp = absOf('devimpDir')
  // 后台脚本：先把可能残留的产物清掉，再按 xz → busybox xz → gzip 逐级打包。
  // 用 find 生成「除本次运行文件之外」的列表：CUR 是 mtime 最新的 *.log。
  // 注：/sdcard/Download 是 Android 自带的下载目录，**不创建它**。真撞上不可写（未挂载、
  // 只读），三种 tar 都会失败并落到脚本末尾 `exit 4` 的兜底，前端 1.5 秒内就能报出来，
  // 不会干等到超时。
  const script = [
    `D=${shQuote(devimp)}`,
    `O=${shQuote(job.target)}`,
    `G=${shQuote(job.fallback)}`,
    `F=${shQuote(job.failFlag)}`,
    `L=${shQuote(LIST_FILE)}`,
    `P=${shQuote(PROGRESS_FILE)}`,
    `N=${shQuote(TOTAL_FILE)}`,
    `rm -f "$O" "$G" "$O.part" "$G.part" "$F" "$P" "$N"`,
    `cd "$D" || { echo 3 > "$F"; exit 3; }`,
    `CUR=$(ls -t *.log 2>/dev/null | head -n1)`,
    `find . -maxdepth 1 -type f ! -name "$CUR" > "$L"`,
    `[ -s "$L" ] || { echo 5 > "$F"; exit 5; }`,
    `wc -l < "$L" > "$N"`,
    // -v 每处理一个文件输出一行（进度来源），stderr 一并收进来好让真错误也有迹可循；
    // 换回退方案前清空，避免三轮的行数叠加成 >100%
    `tar -cvJf "$O.part" -T "$L" >> "$P" 2>&1 && mv -f "$O.part" "$O" && exit 0`,
    `: > "$P"`,
    `busybox tar -cvJf "$O.part" -T "$L" >> "$P" 2>&1 && mv -f "$O.part" "$O" && exit 0`,
    `: > "$P"`,
    `tar -cvzf "$G.part" -T "$L" >> "$P" 2>&1 && mv -f "$G.part" "$G" && exit 0`,
    `echo 4 > "$F"`
  ].join('; ')
  // nohup + 重定向：让 ksu exec 立刻返回，不跟着等打包
  const cmd = `nohup sh -c ${shQuote(script)} >/dev/null 2>&1 & echo started`
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
export type ExportPhase = 'running' | 'done-xz' | 'done-gz' | 'empty' | 'no-devimp' | 'failed'

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
  const part = `${job.target}.part`
  const cmd =
    `[ -f ${shQuote(job.target)} ] && echo s:xz; ` +
    `[ -f ${shQuote(job.fallback)} ] && echo s:gz; ` +
    `[ -f ${shQuote(job.failFlag)} ] && echo "s:fail:$(cat ${shQuote(job.failFlag)} 2>/dev/null)"; ` +
    `echo "b:$([ -f ${shQuote(part)} ] && wc -c < ${shQuote(part)} || echo 0)"; ` +
    `echo "d:$(wc -l < ${shQuote(PROGRESS_FILE)} 2>/dev/null)"; ` +
    `echo "t:$(cat ${shQuote(TOTAL_FILE)} 2>/dev/null)"; ` +
    `exit 0`
  try {
    const { errno, stdout, stderr } = await run(cmd)
    // 带操作上下文：调用方（轮询循环）据此区分「探测挂了」与「打包失败」
    if (errno !== 0) return failed<ExportProbe>(`查询导出进度失败：${shellError(errno, stderr)}`)
    const out = stdout
    const num = (prefix: string): number => {
      const m = new RegExp(`^${prefix}:(\\d+)`, 'm').exec(out)
      return m ? Number(m[1]) : 0
    }
    const progress: ExportProgress = { done: num('d'), total: num('t'), bytes: num('b') }
    if (/^s:xz$/m.test(out)) return ok({ phase: 'done-xz', progress })
    if (/^s:gz$/m.test(out)) return ok({ phase: 'done-gz', progress })
    const fail = /^s:fail:(\d+)/m.exec(out)
    if (fail) {
      const phase = fail[1] === '5' ? 'empty' : fail[1] === '3' ? 'no-devimp' : 'failed'
      return ok({ phase, progress })
    }
    return ok({ phase: 'running', progress })
  } catch (e) {
    return failed<ExportProbe>(e instanceof Error ? e.message : String(e))
  }
}
