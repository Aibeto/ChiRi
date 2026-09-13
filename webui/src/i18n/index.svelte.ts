// index.svelte.ts: [locale] [t]
// 界面语言与守护进程的 meta.language 解耦：界面语言存浏览器本地，仅影响 UI 文案；
// meta.language 只决定 daemon 自身日志/提示语言（在配置页单独设置）。
import zh from './locales/zh'
import en from './locales/en'

export type Locale = 'zh' | 'en'
export const LOCALES: Locale[] = ['zh', 'en']
const STORAGE_KEY = 'chiri_ui_lang'

const DICTS: Record<Locale, Record<string, string>> = { zh, en }

function initialLocale(): Locale {
  try {
    const saved = localStorage.getItem(STORAGE_KEY)
    if (saved === 'zh' || saved === 'en') return saved
    const nav = navigator.language?.toLowerCase() ?? ''
    return nav.startsWith('zh') ? 'zh' : 'en'
  } catch {
    return 'zh'
  }
}

// [locale]
export const i18n = $state({ locale: initialLocale() })

export function setLocale(next: Locale): void {
  i18n.locale = next
  try {
    localStorage.setItem(STORAGE_KEY, next)
  } catch {
    /* 隐私模式等场景忽略 */
  }
}

export function toggleLocale(): void {
  setLocale(i18n.locale === 'zh' ? 'en' : 'zh')
}

// [t]
/**
 * 取文案：缺失键回退到中文，再回退到键名本身（便于发现漏翻但不会让界面空白）。
 * 支持 `{name}` 形式的简单插值。
 */
export function t(key: string, params?: Record<string, string | number>): string {
  const dict = DICTS[i18n.locale] ?? DICTS.zh
  let text = dict[key] ?? DICTS.zh[key] ?? key
  if (params) {
    for (const [k, v] of Object.entries(params)) {
      text = text.replaceAll(`{${k}}`, String(v))
    }
  }
  return text
}
