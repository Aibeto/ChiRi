// rules.ts: [types] [parse]
// rules.yaml 的磁盘副本是 daemon 启动时复制出的**展示副本**（运行时一律读二进制内嵌值），
// 因此 WebUI 只读展示；`fas_rules` 段随包文件不含，解析时不能假设存在。
// 字段来源：src/monitor/config.rs::RulesConfig。
import { load as loadYaml } from 'js-yaml'

// [types]
export interface RulesInfo {
  /** 全局调度总开关 */
  yumiScheduler: boolean
  /** 动态模式总开关（false 时前台永远用 global_mode） */
  dynamicEnabled: boolean
  /** 全局模式（dynamic_enabled=false 或应用未命中时的取值） */
  globalMode: string
  /** 应用 → 模式（模块随附维护，WebUI 只读展示） */
  appModes: Record<string, string>
  /** 被跳过的包名（含模糊匹配：pkg.contains(ignored)） */
  ignoredApps: string[]
  /** 解析是否成功 */
  ok: boolean
  /** 解析失败说明 */
  problem?: string
}

export const EMPTY_RULES: RulesInfo = {
  yumiScheduler: true,
  dynamicEnabled: true,
  globalMode: 'balance',
  appModes: {},
  ignoredApps: [],
  ok: false
}

// [parse]
function asBool(v: unknown, fallback: boolean): boolean {
  return typeof v === 'boolean' ? v : fallback
}

function asString(v: unknown, fallback: string): string {
  if (typeof v === 'string' && v.trim()) return v.trim()
  if (typeof v === 'number') return String(v)
  return fallback
}

function asStringMap(v: unknown): Record<string, string> {
  if (!v || typeof v !== 'object' || Array.isArray(v)) return {}
  const out: Record<string, string> = {}
  for (const [k, val] of Object.entries(v as Record<string, unknown>)) {
    if (typeof val === 'string' && val.trim()) out[k] = val.trim()
    else if (typeof val === 'number') out[k] = String(val)
  }
  return out
}

function asStringList(v: unknown): string[] {
  if (!Array.isArray(v)) return []
  return v.filter((x): x is string => typeof x === 'string' && x.trim().length > 0)
}

export function parseRules(text: string): RulesInfo {
  try {
    const parsed = loadYaml(text)
    if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) {
      return { ...EMPTY_RULES, problem: '文件内容不是 YAML 映射结构' }
    }
    const obj = parsed as Record<string, unknown>
    return {
      yumiScheduler: asBool(obj.yumi_scheduler, true),
      dynamicEnabled: asBool(obj.dynamic_enabled, true),
      globalMode: asString(obj.global_mode, 'balance'),
      appModes: asStringMap(obj.app_modes),
      ignoredApps: asStringList(obj.ignored_apps),
      ok: true
    }
  } catch (e) {
    return { ...EMPTY_RULES, problem: e instanceof Error ? e.message : String(e) }
  }
}
