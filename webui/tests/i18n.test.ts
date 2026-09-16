// i18n.test.ts: [parity] [static] [dynamic]
// 文案防回归：中英键集合一致，源码用到的键都有定义（t() 找不到键会直接显示键名，
// 这类问题在界面上很容易被忽略）。
import { readFileSync, readdirSync, statSync } from 'node:fs'
import { join, relative } from 'node:path'
import { describe, expect, it } from 'vitest'
import en from '@/i18n/locales/en'
import zh from '@/i18n/locales/zh'

const SRC = join(process.cwd(), 'src')

function walk(dir: string, out: string[] = []): string[] {
  for (const entry of readdirSync(dir)) {
    const full = join(dir, entry)
    if (statSync(full).isDirectory()) walk(full, out)
    else if (/\.(svelte|ts)$/.test(entry)) out.push(full)
  }
  return out
}

describe('文案完整性', () => {
  it('中英文键集合完全一致', () => {
    expect(Object.keys(zh).sort()).toEqual(Object.keys(en).sort())
  })

  it('源码里静态使用的键都有定义', () => {
    const used = new Set<string>()
    // 只匹配独立的 t('key')：排除 put( / get( / split( 这类以 t 结尾的函数名
    const pattern = /(?:^|[^\w$])t\(\s*'([^']+)'/g
    for (const file of walk(SRC)) {
      const text = readFileSync(file, 'utf8')
      for (const match of text.matchAll(pattern)) used.add(match[1])
    }
    const missing = [...used].filter(key => !(key in zh))
    expect(missing, `缺定义的键：${missing.join(', ')}`).toEqual([])
  })

  it('动态拼接的键按枚举组合都存在', () => {
    for (const state of ['running', 'stopped', 'unknown']) {
      expect(`daemon.${state}` in zh).toBe(true)
      expect(`daemon.${state}.detail` in zh).toBe(true)
    }
    for (const id of ['overview', 'config', 'apps', 'logs']) {
      expect(`nav.${id}` in zh).toBe(true)
    }
    for (const lang of ['zh', 'en']) expect(`lang.${lang}` in zh).toBe(true)
    for (const mode of [
      'reduce',
      'default',
      'boost',
      'vector',
      'down',
      'fas',
      'special',
      'unknown'
    ]) {
      expect(`mode.${mode}` in zh).toBe(true)
      expect(`mode.${mode}.desc` in zh).toBe(true)
    }
  })

  it('每个文件都能被遍历到（防止测试自身失效）', () => {
    const files = walk(SRC).map(f => relative(SRC, f))
    expect(files.length).toBeGreaterThan(10)
    expect(files.some(f => f.endsWith('App.svelte'))).toBe(true)
  })
})
