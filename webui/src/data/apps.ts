// apps.ts: [types] [fetch] [build] [filter]
// 应用列表与标签合成。标签只陈述「已配置」：
//   - 特调：包名命中 special_tuned.yaml 精确条目（正则条目 UI 不可见）
//   - FAS：包名命中 fas_whitelist.yaml（真正是否生效取决于白名单应用配置能否解析，UI 不可知）
//   - 模式：rules.yaml 现存 app_modes 的只读展示
import { getPackagesInfo, hasKsu, listPackages } from '@/kernel/ksu'
import { isLive, run } from '@/kernel/shell'
import type { SpecialTunedEntry } from './whitelists'

// [types]
export interface InstalledApp {
  pkg: string
  label: string
}

export interface AppEntry extends InstalledApp {
  special?: SpecialTunedEntry
  fasConfig?: string
  appMode?: string
}

export interface AppTagContext {
  specialTuned: Map<string, SpecialTunedEntry>
  fasWhitelist: Map<string, string>
  appModes: Record<string, string>
}

// [fetch]
/**
 * 安装的应用（第三方）。优先 KernelSU 原生 bridge，失败回退 `pm list packages -3`；
 * 应用名获取失败时静默降级为包名（不影响主流程）。
 */
export async function fetchInstalledPackages(): Promise<InstalledApp[]> {
  let pkgs: string[] = []
  if (hasKsu()) {
    pkgs = listPackages('user')
  }
  if (pkgs.length === 0 && isLive()) {
    try {
      const { errno, stdout } = await run('pm list packages -3')
      if (errno === 0) {
        pkgs = stdout
          .split('\n')
          .map(l => l.replace(/^package:/, '').trim())
          .filter(Boolean)
      }
    } catch {
      /* 保持空列表 */
    }
  }

  const labels = new Map<string, string>()
  if (pkgs.length > 0 && hasKsu()) {
    try {
      for (const info of getPackagesInfo(pkgs)) {
        if (info.packageName && info.appLabel) labels.set(info.packageName, info.appLabel)
      }
    } catch {
      /* 降级为包名 */
    }
  }

  return [...new Set(pkgs)].sort().map(pkg => ({ pkg, label: labels.get(pkg) ?? pkg }))
}

// [build]
export function buildAppEntries(apps: InstalledApp[], ctx: AppTagContext): AppEntry[] {
  return apps.map(app => {
    const entry: AppEntry = { ...app }
    const special = ctx.specialTuned.get(app.pkg)
    if (special) entry.special = special
    const fas = ctx.fasWhitelist.get(app.pkg)
    if (fas) entry.fasConfig = fas
    const mode = ctx.appModes[app.pkg]
    if (mode) entry.appMode = mode
    return entry
  })
}

// [filter]
/** 支持应用名与包名的即时过滤（大小写不敏感） */
export function filterApps(entries: AppEntry[], keyword: string): AppEntry[] {
  const kw = keyword.trim().toLowerCase()
  if (!kw) return entries
  return entries.filter(
    e => e.pkg.toLowerCase().includes(kw) || e.label.toLowerCase().includes(kw)
  )
}
