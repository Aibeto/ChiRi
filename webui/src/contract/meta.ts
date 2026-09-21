// meta.ts: [fields] [active] [read] [validate] [write]
// meta.yaml 是唯一可写配置。守护进程侧规则（src/common.rs::parse_disk_meta +
// sync_meta_snapshot）：**字段全部可选**（缺省 = 沿用二进制内嵌默认，与 rhine 影响项
// 同一「缺省 = 不变更」语义；老文件/精简文件都合法，缺字段不再拒绝写入）、
// 拒绝未知键、出现即校验类型（布尔只能是
// YAML 字面量 true/false），任一异常 → 整个文件被内嵌默认覆盖（用户其他键一起丢）。
// 因此写入策略是「单次读-改-写 + 顶层行替换」，只动目标字段、保留注释与其他键。
import { load as loadYaml } from 'js-yaml'
import { configAbs, absOf, isSafeConfigRel, shQuote } from './paths'
import { isLive, run } from '@/kernel/shell'
import { readText } from './read'
import { absent, failed, ok, shellError, type ReadResult } from './errors'

// [fields]
/** 已知字段全集（= 守护进程允许出现的键） */
export const META_FIELDS = [
  'name',
  'author',
  'language',
  'loglevel',
  'dev_record',
  'fas_enabled',
  'scenemode_enabled',
  'thread_bind',
  'powerbase_enabled',
  'power_avg',
  'notify',
  'oplus_chg',
  'oplus_dual_cell',
  'voltage_double',
  'current_double',
  'voltage_divisor',
  'current_divisor',
  /** 旧键（曾把电压/电流校准合成一个）：daemon 仍接收，等价于只设电压校准 */
  'unit_divisor',
  'nofix',
  'power_max_w'
] as const
export type MetaField = (typeof META_FIELDS)[number]

/** 允许 WebUI 修改的字段（name/author 仅展示：daemon 不消费，改了也不影响行为） */
export const WRITABLE_FIELDS = [
  'language',
  'loglevel',
  'dev_record',
  'fas_enabled',
  'scenemode_enabled',
  'thread_bind',
  'powerbase_enabled',
  'power_avg',
  'notify',
  'oplus_chg',
  'oplus_dual_cell',
  'voltage_double',
  'current_double',
  'voltage_divisor',
  'current_divisor',
  'power_max_w'
] as const
export type WritableField = (typeof WRITABLE_FIELDS)[number]

export const LOG_LEVELS = ['OFF', 'ERROR', 'WARN', 'INFO', 'DEBUG', 'TRACE'] as const
export type LogLevel = (typeof LOG_LEVELS)[number]
export const LANGUAGES = ['zh', 'en'] as const
export type Language = (typeof LANGUAGES)[number]

export interface MetaSnapshot {
  /** 生效 meta.yaml 相对 config/ 的路径（active_config.chr 内容） */
  rel: string
  /** 绝对路径 */
  path: string
  /** 原始文本（写回时按行替换） */
  raw: string
  /** YAML 解析结果（可能为空对象） */
  values: Record<string, unknown>
  /** 是否满足守护进程的严格校验 */
  valid: boolean
  /** 校验不通过的说明（界面如实展示） */
  problems: string[]
}

// [active]
/**
 * 读取生效配置相对路径（active_config.chr）：daemon 启动时写入、无换行、
 * 形如 `meta.yaml` 或 `8550/meta.yaml`；只在启动时写，停跑后可能是陈旧值。
 */
export async function readActiveConfigRel(): Promise<ReadResult<string>> {
  const r = await readText(absOf('activeConfig'), 'not-created', 512)
  if (r.kind !== 'ok') return r
  const rel = r.value.trim()
  if (!rel) return failed<string>('active_config.chr 为空')
  if (!isSafeConfigRel(rel)) return failed<string>(`active_config.chr 内容非法：${rel}`)
  return ok(rel)
}

// [read]
export async function readMeta(): Promise<ReadResult<MetaSnapshot>> {
  const rel = await readActiveConfigRel()
  if (rel.kind !== 'ok') return rel
  const path = configAbs(rel.value)
  const text = await readText(path, 'not-created', 64 * 1024)
  if (text.kind !== 'ok') return text

  const problems: string[] = []
  let values: Record<string, unknown> = {}
  try {
    const parsed = loadYaml(text.value)
    if (parsed && typeof parsed === 'object' && !Array.isArray(parsed)) {
      values = parsed as Record<string, unknown>
    } else {
      problems.push('文件内容不是 YAML 映射结构')
    }
  } catch (e) {
    problems.push(`YAML 解析失败：${e instanceof Error ? e.message : String(e)}`)
  }
  problems.push(...validateMeta(values))

  return ok({
    rel: rel.value,
    path,
    raw: text.value,
    values,
    valid: problems.length === 0,
    problems
  })
}

// [validate]
function normalizeScalar(v: unknown): string {
  if (typeof v !== 'string') return ''
  // 复刻 daemon common.rs::unquote 的语义：只剥一层「成对且同型」的引号，
  // 不做逐边剥离（`INFO'` 这类畸形值 daemon 判非法，这里必须同样判非法，
  // 否则会出现「界面说已保存、守护进程随后整体重置」的体验裂缝）
  const s = v.trim()
  if (s.length >= 2) {
    const head = s[0]
    const tail = s[s.length - 1]
    if ((head === '"' && tail === '"') || (head === "'" && tail === "'")) {
      return s.slice(1, -1).trim()
    }
  }
  return s
}

/** 复刻守护进程 parse_disk_meta 的校验口径（大小写不敏感去引号后比对） */
export function validateMeta(values: Record<string, unknown>): string[] {
  const problems: string[] = []
  // **字段全部可选**（缺省 = 沿用二进制内嵌默认，daemon 侧同语义）：缺字段不是问题，
  // 只剩「未知键」与「类型不符」两类硬错误
  const known = new Set<string>(META_FIELDS)
  for (const k of Object.keys(values)) {
    if (!known.has(k)) problems.push(`存在未知字段 ${k}`)
  }

  const name = normalizeScalar(values.name)
  const author = normalizeScalar(values.author)
  if ('name' in values && !name) problems.push('name 不能为空')
  if ('author' in values && !author) problems.push('author 不能为空')

  if ('language' in values) {
    const lang = normalizeScalar(values.language).toLowerCase()
    if (!(LANGUAGES as readonly string[]).includes(lang)) problems.push('language 只能是 zh 或 en')
  }
  if ('loglevel' in values) {
    const lv = normalizeScalar(values.loglevel).toUpperCase()
    if (!(LOG_LEVELS as readonly string[]).includes(lv)) {
      problems.push(`loglevel 只能是 ${LOG_LEVELS.join('/')}`)
    }
  }
  for (const f of [
    'dev_record',
    'fas_enabled',
    'scenemode_enabled',
    'thread_bind',
    'powerbase_enabled',
    'power_avg',
    'notify',
    'oplus_chg',
    'oplus_dual_cell',
    'voltage_double',
    'current_double'
  ] as const) {
    if (f in values && typeof values[f] !== 'boolean') {
      problems.push(`${f} 必须是布尔值 true/false`)
    }
  }

  // 出现即校验类型（serde 类型不符会让整个文件判非法、被内嵌默认覆盖）；
  // 数值范围不限报——守护进程对越界的 power_max_w 只回退默认值，界面同口径
  if ('nofix' in values && typeof values.nofix !== 'boolean') {
    problems.push('nofix 必须是布尔值 true/false')
  }
  if (
    'power_max_w' in values &&
    (typeof values.power_max_w !== 'number' || !Number.isFinite(values.power_max_w))
  ) {
    problems.push('power_max_w 必须是数字')
  }
  for (const f of ['unit_divisor', 'voltage_divisor', 'current_divisor'] as const) {
    if (f in values && (typeof values[f] !== 'number' || !Number.isFinite(values[f]))) {
      problems.push(`${f} 必须是数字`)
    }
  }
  return problems
}

/** 写入前的单字段值校验（返回错误描述，null 表示通过） */
export function validateFieldValue(
  field: WritableField,
  value: string | boolean | number
): string | null {
  switch (field) {
    case 'language':
      return (LANGUAGES as readonly string[]).includes(String(value)) ? null : '语言只能是 zh 或 en'
    case 'loglevel':
      return (LOG_LEVELS as readonly string[]).includes(String(value))
        ? null
        : `日志等级只能是 ${LOG_LEVELS.join('/')}`
    case 'power_max_w': {
      const n = Number(value)
      return Number.isFinite(n) && n > 0 && n <= 200
        ? null
        : '满量程必须是 0~200 之间的数字（W）'
    }
    case 'voltage_divisor':
    case 'current_divisor': {
      const n = Number(value)
      return Number.isFinite(n) && n > 0 && n <= 1e9
        ? null
        : '校准值必须是大于 0 的数字（默认 1000）'
    }
    case 'dev_record':
    case 'fas_enabled':
    case 'scenemode_enabled':
    case 'thread_bind':
    case 'powerbase_enabled':
    case 'power_avg':
    case 'notify':
    case 'oplus_chg':
    case 'oplus_dual_cell':
    case 'voltage_double':
    case 'current_double':
      return typeof value === 'boolean' ? null : `${field} 必须是布尔值`
  }
}

// [write]
/**
 * 顶层行替换：仅匹配缩进为 0 的 `键: 值` 行，保留键名大小写、分隔空白与行内注释；
 * 原值带引号时统一渲染为双引号（daemon 只做去引号比较，两种写法都合法）。
 * 返回 null 表示未找到该字段（调用方应放弃写入，而不是整文件重排）。
 */
export function replaceTopLevelField(
  content: string,
  field: string,
  value: string | boolean | number
): string | null {
  const lines = content.split('\n')
  const want = field.toLowerCase()
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i]
    const trimmed = line.trimStart()
    if (!trimmed || trimmed.startsWith('#') || trimmed.startsWith('-')) continue
    if (line.length !== trimmed.length) continue // 仅顶层
    const m = /^([^:\s#]+)(\s*:\s*)(.*)$/.exec(trimmed)
    if (!m || m[1].toLowerCase() !== want) continue

    const rest = m[3]
    const commentIdx = rest.indexOf(' #')
    const comment = commentIdx >= 0 ? rest.slice(commentIdx) : ''
    const oldValue = (commentIdx >= 0 ? rest.slice(0, commentIdx) : rest).trim()
    const quoted = /^["']/.test(oldValue)
    const rendered =
      typeof value === 'boolean' || typeof value === 'number'
        ? String(value)
        : quoted
          ? `"${value}"`
          : String(value)
    lines[i] = `${m[1]}${m[2]}${rendered}${comment}`
    return lines.join('\n')
  }
  return null
}

/** 顶层追加：可选字段（power_max_w / nofix）在旧版文件里可能整行缺失——缺失合法，补在文件尾 */
export function appendTopLevelField(
  content: string,
  field: string,
  value: string | boolean | number
): string {
  const tail = content.length === 0 || content.endsWith('\n') ? '' : '\n'
  return `${content}${tail}${field}: ${String(value)}\n`
}

export function utf8ToBase64(input: string): string {
  const bytes = new TextEncoder().encode(input)
  let bin = ''
  const CHUNK = 0x8000
  for (let i = 0; i < bytes.length; i += CHUNK) {
    bin += String.fromCharCode(...bytes.subarray(i, i + CHUNK))
  }
  return btoa(bin)
}

/**
 * 单次读-改-写事务：一次写入完成所有字段改动（每次写入都会触发 daemon 全量热重载，
 * 且 tmp+rename 会产生两个 inotify 事件，故绝不做多次写入）。
 * 临时文件后缀固定为 `.webui.tmp`，与 daemon 自身的 `<name>.tmp` 区分，避免互踩。
 */
export async function writeMetaFields(
  patch: Partial<Record<WritableField, string | boolean | number>>
): Promise<ReadResult<MetaSnapshot>> {
  if (!isLive()) return absent<MetaSnapshot>('unsupported-env')

  const keys = Object.keys(patch) as WritableField[]
  if (keys.length === 0) return failed<MetaSnapshot>('没有需要写入的字段')
  for (const key of keys) {
    const problem = validateFieldValue(key, patch[key] as string | boolean | number)
    if (problem) return failed<MetaSnapshot>(problem)
  }

  const snapshot = await readMeta()
  if (snapshot.kind !== 'ok') return snapshot
  if (!snapshot.value.valid) {
    return failed<MetaSnapshot>(
      '当前 meta.yaml 不符合守护进程校验规则，已拒绝写入（等待守护进程自愈后再试）'
    )
  }

  let content = snapshot.value.raw
  for (const key of keys) {
    const value = patch[key] as string | boolean | number
    let next = replaceTopLevelField(content, key, value)
    if (next === null) {
      // 字段缺失本身合法（缺省 = 沿用内嵌默认，daemon 侧同语义）：直接补在文件尾
      next = appendTopLevelField(content, key, value)
    }
    content = next
  }

  const path = snapshot.value.path
  const tmp = `${path}.webui.tmp`
  const b64 = utf8ToBase64(content)
  // 先写同目录临时文件再原子替换：避免守护进程 inotify 读到半截内容
  const cmd =
    `printf '%s' ${shQuote(b64)} | base64 -d > ${shQuote(tmp)} && ` +
    `mv -f ${shQuote(tmp)} ${shQuote(path)} || { rm -f ${shQuote(tmp)}; exit 1; }`

  try {
    const { errno, stderr } = await run(cmd)
    if (errno !== 0) return failed<MetaSnapshot>(shellError(errno, stderr))
  } catch (e) {
    return failed<MetaSnapshot>(e instanceof Error ? e.message : String(e))
  }

  // 写后回读：daemon 可能已用内嵌默认覆盖（此时界面必须呈现实际值而非期望值）
  return readMeta()
}
