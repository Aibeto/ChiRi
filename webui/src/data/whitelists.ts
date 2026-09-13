// whitelists.ts: [special] [fas]
// 两个 daemon 导出文件的行格式（src/main.rs:156-179）：
//   special_tuned.yaml : `包名:模式列表(逗号分隔):优先回退模式`（仅精确包名条目，
//                        `re:` 正则条目不导出——所以 UI 看到的不是特调全集）
//   fas_whitelist.yaml : `包名:配置名`
// 两文件都只在 ChiRi 机型 + daemon 成功启动过时存在，且启动后不再自愈重写。

// [special]
export interface SpecialTunedEntry {
  /** 该应用可用的特调模式（daemon 侧 modes 列表） */
  modes: string[]
  /** 用户未显式配置该应用时采用的模式 */
  fallback: string
}

export function parseSpecialTuned(text: string): Map<string, SpecialTunedEntry> {
  const out = new Map<string, SpecialTunedEntry>()
  for (const line of text.split('\n')) {
    const raw = line.trim()
    if (!raw || raw.startsWith('#')) continue
    const parts = raw.split(':')
    if (parts.length < 2) continue
    const pkg = parts[0].trim()
    const modes = parts[1]
      .split(',')
      .map(s => s.trim())
      .filter(Boolean)
    if (!pkg || modes.length === 0) continue
    const fallback = (parts[2] ?? '').trim() || modes[0]
    out.set(pkg, { modes, fallback })
  }
  return out
}

/** 所有精确条目的特调模式并集（界面用来判定 current_mode 是否为特调） */
export function specialModeSet(entries: Map<string, SpecialTunedEntry>): Set<string> {
  const set = new Set<string>()
  for (const entry of entries.values()) {
    for (const m of entry.modes) set.add(m)
  }
  return set
}

// [fas]
export function parseFasWhitelist(text: string): Map<string, string> {
  const out = new Map<string, string>()
  for (const line of text.split('\n')) {
    const raw = line.trim()
    if (!raw || raw.startsWith('#')) continue
    const idx = raw.indexOf(':')
    if (idx <= 0) continue
    const pkg = raw.slice(0, idx).trim()
    const cfg = raw.slice(idx + 1).trim()
    if (pkg && cfg) out.set(pkg, cfg)
  }
  return out
}
