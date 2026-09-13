// data-model.test.ts: [whitelists] [rules] [module-prop] [mode] [apps]
import { describe, expect, it } from 'vitest'
import { parseFasWhitelist, parseSpecialTuned, specialModeSet } from '@/data/whitelists'
import { parseRules } from '@/data/rules'
import { parseModuleProp } from '@/data/module-info'
import { CLG_MODE_IDS, describeMode } from '@/data/mode'
import { buildAppEntries, filterApps } from '@/data/apps'

describe('白名单解析', () => {
  it('special_tuned.yaml 每行「包名:模式列表:回退模式」', () => {
    const text = [
      '# 注释行',
      'com.hypergryph.arknights:akmode:akmode',
      'com.example.multi:powersave,akmode:powersave',
      'com.example.nofallback:akmode'
    ].join('\n')
    const map = parseSpecialTuned(text)
    expect(map.get('com.hypergryph.arknights')).toEqual({ modes: ['akmode'], fallback: 'akmode' })
    expect(map.get('com.example.multi')?.modes).toEqual(['powersave', 'akmode'])
    // 缺回退模式时取模式列表首项（与守护进程导出语义一致）
    expect(map.get('com.example.nofallback')?.fallback).toBe('akmode')
    expect(map.has('# 注释行')).toBe(false)
  })

  it('特调模式并集用于判定 current_mode 是否为特调', () => {
    const map = parseSpecialTuned('com.a:akmode:akmode\ncom.b:x,y:x\n')
    expect([...specialModeSet(map)].sort()).toEqual(['akmode', 'x', 'y'])
  })

  it('fas_whitelist.yaml 每行「包名:配置名」', () => {
    const map = parseFasWhitelist('# c\ncom.miHoYo.Yuanshen:endfield\ncom.tencent.tmgp.sgame:pubg\n')
    expect(map.get('com.miHoYo.Yuanshen')).toBe('endfield')
    expect(map.size).toBe(2)
  })

  it('残缺行被忽略', () => {
    expect(parseFasWhitelist('nocolon\n:empty\npkg:\n').size).toBe(0)
    expect(parseSpecialTuned('pkg:\n:pkg:pkg\n').size).toBe(0)
  })
})

describe('rules.yaml 解析', () => {
  it('读取全部只读展示字段', () => {
    const info = parseRules(
      [
        'yumi_scheduler: true',
        'dynamic_enabled: true',
        'global_mode: "balance"',
        'app_modes:',
        '  com.tencent.tmgp.sgame: performance',
        'ignored_apps:',
        '  - com.android.systemui'
      ].join('\n')
    )
    expect(info.ok).toBe(true)
    expect(info.globalMode).toBe('balance')
    expect(info.appModes['com.tencent.tmgp.sgame']).toBe('performance')
    expect(info.ignoredApps).toEqual(['com.android.systemui'])
  })

  it('缺字段时按守护进程默认值（dynamic_enabled/global_mode/yumi_scheduler 均为真值）', () => {
    const info = parseRules('app_modes: {}\n')
    expect(info.ok).toBe(true)
    expect(info.yumiScheduler).toBe(true)
    expect(info.dynamicEnabled).toBe(true)
    expect(info.globalMode).toBe('balance')
  })

  it('非法 YAML 返回 ok=false 并带原因', () => {
    const info = parseRules('[[[')
    expect(info.ok).toBe(false)
    expect(info.problem).toBeTruthy()
  })
})

describe('module.prop 解析', () => {
  it('KEY=VALUE 逐行解析', () => {
    const prop = parseModuleProp(
      ['id=chiri', 'name=ChiRi', 'version=Alpha02', 'versionCode=2', 'author=Aibeto', '# c'].join('\n')
    )
    expect(prop).toEqual({
      id: 'chiri',
      name: 'ChiRi',
      version: 'Alpha02',
      versionCode: '2',
      author: 'Aibeto',
      description: ''
    })
  })
})

describe('模式派生', () => {
  const special = new Set(['akmode'])

  it('CLG 四档识别为 clg', () => {
    for (const id of CLG_MODE_IDS) {
      expect(describeMode(id, special).kind).toBe('clg')
    }
  })

  it('fas 与特调模式分别归类（特调需命中所见模式集合）', () => {
    expect(describeMode('fas', special).kind).toBe('fas')
    expect(describeMode('akmode', special).kind).toBe('special')
    // 正则条目对应的特调模式 UI 不可见 → 归为未知，而不是猜成特调
    expect(describeMode('secretmode', special).kind).toBe('unknown')
  })

  it('空值与未注册值都归为 unknown 且不抛错', () => {
    expect(describeMode('', special).kind).toBe('unknown')
    expect(describeMode('turbo', special).kind).toBe('unknown')
  })

  it('四档信号色互相可区分', () => {
    const signals = CLG_MODE_IDS.map(id => describeMode(id).signal)
    expect(new Set(signals).size).toBe(signals.length)
  })
})

describe('应用标签合成与过滤', () => {
  const apps = [
    { pkg: 'com.hypergryph.arknights', label: '明日方舟' },
    { pkg: 'com.miHoYo.Yuanshen', label: '原神' },
    { pkg: 'com.tencent.mm', label: '微信' }
  ]
  const ctx = {
    specialTuned: parseSpecialTuned('com.hypergryph.arknights:akmode:akmode\n'),
    fasWhitelist: parseFasWhitelist('com.miHoYo.Yuanshen:endfield\n'),
    appModes: { 'com.tencent.mm': 'balance' }
  }

  it('只对命中的包打标签，未命中的不臆造', () => {
    const entries = buildAppEntries(apps, ctx)
    expect(entries[0].special?.fallback).toBe('akmode')
    expect(entries[0].fasConfig).toBeUndefined()
    expect(entries[1].fasConfig).toBe('endfield')
    expect(entries[1].special).toBeUndefined()
    expect(entries[2].appMode).toBe('balance')
  })

  it('按包名或应用名即时过滤', () => {
    const entries = buildAppEntries(apps, ctx)
    expect(filterApps(entries, '微信').map(e => e.pkg)).toEqual(['com.tencent.mm'])
    expect(filterApps(entries, 'MIHOYO').map(e => e.pkg)).toEqual(['com.miHoYo.Yuanshen'])
    expect(filterApps(entries, '  ')).toHaveLength(3)
  })
})
