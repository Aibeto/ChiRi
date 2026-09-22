// sources.ts: [device] [files] [logs]
// 各接触点的原始读取入口（不解析，解析见 src/data/*）。
// 缺失语义：白名单类文件只有 ChiRi 机型会产生（→ chiri-only）；日志/快照类
// 是「尚未产生」（→ not-created）；真正的失败一律 failed，绝不吞成空值。
import { REL, absOf, shQuote } from './paths'
import { listDir, readTail, readText } from './read'
import { readActiveConfigRel } from './meta'
import type { ReadResult } from './errors'
import { absent, failed, ok, shellError } from './errors'
import { isLive, run } from '@/kernel/shell'

// [device]
/** 设备形态：非 ChiRi 机型与读不到生效配置时都是 unknown */
export type DeviceKind = 'chiri' | 'unknown'

let deviceCache: DeviceKind | null = null

/**
 * 是否 ChiRi 专属机型：以 active_config.chr 是否指向 SoC 子目录（含 '/'）为判据
 * （与 daemon 的 is_chiri_soc 同结果：只有 ChiRi 的生效配置在 config/{soc}/ 下）。
 * 仅用于界面呈现「不适用」等文案，不作为权限或写入判断依据。
 */
export async function deviceKind(): Promise<DeviceKind> {
  if (deviceCache !== null) return deviceCache
  const rel = await readActiveConfigRel()
  deviceCache = rel.kind !== 'ok' || !rel.value.includes('/') ? 'unknown' : 'chiri'
  return deviceCache
}


// [files]
/** current_mode.chr：无换行、无空格；取值由 determine_mode 决定，可能是未注册的模式名 */
export async function readCurrentModeRaw(): Promise<ReadResult<string>> {
  const r = await readText(absOf('currentMode'), 'not-created', 256)
  if (r.kind !== 'ok') return r
  return ok(r.value.trim())
}

/** rules.yaml（磁盘展示副本，只读，无写入口） */
export function readRulesRaw(): Promise<ReadResult<string>> {
  return readText(absOf('rules'), 'not-created', 256 * 1024)
}

/** special_tuned.yaml：仅精确包名条目；只有 ChiRi 机型且 daemon 启动过才有 */
export function readSpecialTunedRaw(): Promise<ReadResult<string>> {
  return readText(absOf('specialTuned'), 'chiri-only', 256 * 1024)
}

/** fas_whitelist.yaml：每行「包名:配置名」 */
export function readFasWhitelistRaw(): Promise<ReadResult<string>> {
  return readText(absOf('fasWhitelist'), 'chiri-only', 256 * 1024)
}

/** module.prop：模块名/版本/作者等 */
export function readModulePropRaw(): Promise<ReadResult<string>> {
  return readText(absOf('moduleProp'), 'not-created', 8 * 1024)
}

// [logs]
/** 本次运行的守护进程日志尾部（单文件上限 50MB，禁止整读） */
export function readDaemonLogTail(bytes = 128 * 1024): Promise<ReadResult<string>> {
  return readTail(absOf('daemonLog'), bytes, 'not-created')
}

/** 每秒状态快照尾部（8MB 上限；仅 ChiRi 产生） */
export function readStatusCsvTail(bytes = 256 * 1024): Promise<ReadResult<string>> {
  return readTail(absOf('statusCsv'), bytes, 'not-created')
}

/** devimp 目录文件列表（daemon 启动即建目录，空目录合法；仅 ChiRi 有内容） */
export function listDevimp(): Promise<ReadResult<string[]>> {
  return listDir(absOf('devimpDir'), 'not-created')
}

/** logd 归档包列表（只有发生过归档才存在） */
export function listLogd(): Promise<ReadResult<string[]>> {
  return listDir(absOf('logdDir'), 'not-created')
}

/**
 * 删除历史归档：整目录删掉 logd/（历次重启的归档包）与 devimp/（诊断文件）。
 * 删目录比逐个删文件干净：devimp 由 daemon 启动时重建，logd 下次归档自动出现；
 * 当前会话的 logs/（daemon.log / status.csv）不在此列，不会被碰到。
 */
export async function clearArchives(): Promise<ReadResult<true>> {
  if (!isLive()) return absent<true>('unsupported-env')
  const { errno, stderr } = await run(
    `rm -rf ${shQuote(absOf('logdDir'))} && echo done`
  )
  return errno === 0 ? ok(true) : failed<true>(shellError(errno, stderr))
}

export { REL }
