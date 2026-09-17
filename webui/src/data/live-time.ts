// live-time.ts: [parse]
// LiveTime.chr 解析（模块根）。守护进程每 15s 写一次**本地时间** `MM:SS`
// （见 src/logger.rs::write_live_time），WebUI 用它与本机时间比差判定调度存活：
// 差值 > LIVE_TIME_TOLERANCE_SECONDS 即视为已关闭（文件缺失同判）。

/** 文件名（与守护进程 logger.rs::LIVE_TIME_CHR 对齐） */
export const LIVE_TIME_FILE = 'LiveTime.chr'

/** 心跳容差（秒）：守护进程 15s 写一次，容差留 5s 余量 */
export const LIVE_TIME_TOLERANCE_SECONDS = 20

/**
 * 解析 `MM:SS` 为「小时内秒数」（0..3599）：取首个非空行，分/秒都必须在 00-59
 * （守护进程写的是 `{:02}:{:02}`）。无有效行 / 格式非法返回 null。
 */
export function parseLiveTimeSeconds(text: string): number | null {
  for (const raw of text.split('\n')) {
    const line = raw.trim()
    if (!line) continue
    const m = /^(\d{1,2}):([0-5]\d)$/.exec(line)
    if (!m) return null
    const minutes = Number(m[1])
    if (minutes > 59) return null
    return minutes * 60 + Number(m[2])
  }
  return null
}

/**
 * 心跳年龄（秒）：文件时间与本机时间的差，按 1 小时取模（文件只有分秒、无小时）。
 * 取模后落在 (1800, 3600) 的区间表示文件时间「跑在本机前面」（时钟回拨或上一小时的
 * 旧值）——同样是过期，直接以 >1800 的大值参与比较即可判为不新鲜。
 */
export function liveTimeAgeSeconds(fileSeconds: number, nowSeconds: number): number {
  return (nowSeconds - fileSeconds + 3600) % 3600
}

/** 心跳是否新鲜（年龄 ≤ 容差）——即守护进程正在写心跳 */
export function isLiveTimeFresh(fileSeconds: number, nowSeconds: number): boolean {
  return liveTimeAgeSeconds(fileSeconds, nowSeconds) <= LIVE_TIME_TOLERANCE_SECONDS
}

/** 本机当前时间的「小时内秒数」（与心跳文件同口径） */
export function nowSecondsOfHour(date: Date = new Date()): number {
  return date.getMinutes() * 60 + date.getSeconds()
}
