// lab.test.ts: [parse] [write]
// 实验室状态解析与写入内容。解析口径必须与守护进程 src/rhine.rs::parse_state 对齐，
// 所以这里重点覆盖「两边容易分歧」的形态：注释、引号、多行、未知 key。
import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { describe, expect, it } from 'vitest'
import { META_FIELDS } from '@/contract/meta'
import {
  LAB_ENABLEABLE,
  LAB_FORCE_OFF,
  LAB_MODE_KEYS,
  LAB_TAKEOVER,
  labFileContent,
  labWarnings,
  parseLabLock,
  parseLabState
} from '@/data/lab'

describe('rhine.chr 解析', () => {
  it('空文件与只有注释都算未启用', () => {
    expect(parseLabState('')).toEqual({ kind: 'off' })
    expect(parseLabState('\n\n')).toEqual({ kind: 'off' })
    expect(parseLabState('# 说明\n#  再来一行\n')).toEqual({ kind: 'off' })
    expect(parseLabState('  # 带缩进的注释\n')).toEqual({ kind: 'off' })
  })

  it('裸标量与带引号都认', () => {
    expect(parseLabState('vector')).toEqual({ kind: 'on', mode: 'vector' })
    expect(parseLabState('vector\n')).toEqual({ kind: 'on', mode: 'vector' })
    expect(parseLabState('"vector"')).toEqual({ kind: 'on', mode: 'vector' })
    expect(parseLabState("'vector'")).toEqual({ kind: 'on', mode: 'vector' })
    expect(parseLabState('  vector  ')).toEqual({ kind: 'on', mode: 'vector' })
  })

  it('注释与内容混排时取内容', () => {
    expect(parseLabState('# 头注释\nvector\n# 尾注释\n')).toEqual({
      kind: 'on',
      mode: 'vector'
    })
  })

  it('未知模式名与多行内容判非法', () => {
    expect(parseLabState('unknown_mode')).toMatchObject({ kind: 'invalid' })
    expect(parseLabState('vector\ncontingency')).toMatchObject({ kind: 'invalid' })
    expect(parseLabState('mode: vector')).toMatchObject({ kind: 'invalid' })
  })

  it('保留字 off 是强制关闭，不是非法内容（引号与大小写都认）', () => {
    expect(parseLabState('off')).toEqual({ kind: 'force-off' })
    expect(parseLabState('off\n')).toEqual({ kind: 'force-off' })
    expect(parseLabState('# 强制关闭\noff')).toEqual({ kind: 'force-off' })
    expect(parseLabState('"OFF"')).toEqual({ kind: 'force-off' })
  })

  it('非法时保留原文，便于界面说明读到的是什么', () => {
    const state = parseLabState('  bogus  ')
    expect(state.kind).toBe('invalid')
    if (state.kind === 'invalid') expect(state.raw).toBe('bogus')
  })
})

describe('rhine.chr 写入内容', () => {
  it('启用写模式 key 加换行，关闭写空内容', () => {
    expect(labFileContent('vector')).toBe('vector\n')
    expect(labFileContent(null)).toBe('')
  })

  it('写进去的内容能被自己解析回来（不出现写完就判非法）', () => {
    for (const key of LAB_MODE_KEYS) {
      expect(parseLabState(labFileContent(key))).toEqual({ kind: 'on', mode: key })
    }
    expect(parseLabState(labFileContent(null))).toEqual({ kind: 'off' })
  })

  it('强制关闭写保留字 off，且自己解析回来仍是强制关闭', () => {
    expect(labFileContent(LAB_FORCE_OFF)).toBe('off\n')
    expect(parseLabState(labFileContent(LAB_FORCE_OFF))).toEqual({ kind: 'force-off' })
  })
})

describe('锁定标记解析', () => {
  it('文件不存在一律视为未锁定', () => {
    expect(parseLabLock('whatever', false)).toEqual({ locked: false, mode: '', notes: [] })
  })

  it('第一行是模式名，其余 # 行是异常痕迹', () => {
    const lock = parseLabLock('vector\n# rhine.chr 内容非法\n# 又来一条\n', true)
    expect(lock.locked).toBe(true)
    expect(lock.mode).toBe('vector')
    expect(lock.notes).toHaveLength(2)
  })

  it('第一行不是已知模式时 mode 为空串，但文件存在就算锁定', () => {
    const lock = parseLabLock('bogus\n', true)
    expect(lock.locked).toBe(true)
    expect(lock.mode).toBe('')
  })

  it('空文件（只有标记本身）算锁定、无模式、无痕迹', () => {
    expect(parseLabLock('', true)).toEqual({ locked: true, mode: '', notes: [] })
  })
})

describe('运行配置一致性告警', () => {
  const on = (mode: 'vector' | 'contingency') => ({ kind: 'on', mode }) as const
  const off = { kind: 'off' } as const
  const locked = (mode = 'vector') => ({ locked: true, mode, notes: [] })

  it('未锁定时只报内容非法，不报「漂移」（空文件本来就是正常态）', () => {
    const idle = { locked: false, mode: '', notes: [] }
    expect(labWarnings({ lock: idle, state: off, hasBackup: false })).toEqual([])
    expect(labWarnings({ lock: idle, state: { kind: 'invalid', raw: 'x' }, hasBackup: true })).toEqual([
      'state-invalid'
    ])
  })

  it('锁定中但 rhine.chr 被清空 → lock-drifted', () => {
    expect(labWarnings({ lock: locked(), state: off, hasBackup: true })).toEqual(['lock-drifted'])
  })

  it('锁定中 rhine.chr 被改成另一个模式 → lock-drifted', () => {
    expect(labWarnings({ lock: locked('vector'), state: on('contingency'), hasBackup: true })).toEqual([
      'lock-drifted'
    ])
  })

  it('锁定中快照丢了 → no-backup；两个问题可以同时报', () => {
    expect(labWarnings({ lock: locked(), state: off, hasBackup: false })).toEqual([
      'lock-drifted',
      'no-backup'
    ])
    expect(labWarnings({ lock: locked(), state: on('vector'), hasBackup: false })).toEqual([
      'no-backup'
    ])
  })

  it('一切都对时没有告警', () => {
    expect(labWarnings({ lock: locked(), state: on('vector'), hasBackup: true })).toEqual([])
  })

  it('文件写着 off（强制关闭请求）不报漂移与缺快照：那是预期中间态', () => {
    expect(
      labWarnings({ lock: locked(), state: { kind: 'force-off' }, hasBackup: false })
    ).toEqual([])
  })
})

describe('实验室接管的 meta 开关', () => {
  it('每个已定义模式都有一条，且只列 meta.yaml 里存在的字段', () => {
    for (const key of LAB_MODE_KEYS) {
      expect(LAB_TAKEOVER[key]).toBeDefined()
      for (const field of LAB_TAKEOVER[key]) {
        expect(META_FIELDS as readonly string[]).toContain(field)
      }
    }
    expect(Object.keys(LAB_TAKEOVER).sort()).toEqual([...LAB_MODE_KEYS].sort())
  })

  /**
   * 读 daemon 的实验室模式定义（module/config/rhine-init.yaml），得到「模式 key → 影响项字段」。
   * 只做够用的顶层解析：顶层 `key:` 开一个模式，其下缩进一级的 `field:` 是影响项，
   * `key: {}`（预留条目）解析为空列表。
   */
  function readRhineInit(): Record<string, string[]> {
    const text = readFileSync(join(process.cwd(), '..', 'module/config/rhine-init.yaml'), 'utf8')
    const defs: Record<string, string[]> = {}
    let current = ''
    for (const raw of text.split('\n')) {
      const line = raw.replace(/\s#.*$/, '').trimEnd()
      if (line === '') continue
      const top = /^([A-Za-z0-9_-]+):/.exec(line)
      if (top) {
        current = top[1]
        defs[current] = []
        continue
      }
      const item = /^[ \t]+([A-Za-z0-9_-]+):/.exec(line)
      if (item && current) defs[current].push(item[1])
    }
    return defs
  }

  // 刚性断言（故意做成绊线）：直接读 daemon 的 rhine-init.yaml 比对，**不写死字面量**——
  // 写死的话改了 yaml 也不会变红，「与 rhine-init.yaml 同步」就成了一句空话。
  // 只比 meta.yaml 里有的字段：global_mode / special_tuned 不是 meta 字段，界面管不着。
  it('接管表与 rhine-init.yaml 的影响项同步', () => {
    const defs = readRhineInit()
    expect(Object.keys(defs).sort()).toEqual([...LAB_MODE_KEYS].sort())
    for (const key of LAB_MODE_KEYS) {
      const metaFields = defs[key].filter(f => (META_FIELDS as readonly string[]).includes(f))
      expect([...LAB_TAKEOVER[key]].sort()).toEqual(metaFields.sort())
    }
  })

  it('contingency/babel 与 vector 接管相同的 meta 开关（与 rhine-init 一致）', () => {
    expect(LAB_TAKEOVER.contingency.length).toBeGreaterThan(0)
    expect([...LAB_TAKEOVER.contingency].sort()).toEqual([...LAB_TAKEOVER.vector].sort())
    expect([...LAB_TAKEOVER.babel].sort()).toEqual([...LAB_TAKEOVER.vector].sort())
  })
})

describe('与守护进程解析口径对齐', () => {
  it('行内注释：模式名按 YAML 剥（生效），保留字也在解析前剥', () => {
    expect(parseLabState('vector # 临时切一下')).toEqual({ kind: 'on', mode: 'vector' })
    expect(parseLabState('off # 强制关闭')).toEqual({ kind: 'force-off' })
    // `#` 前没有空白不算注释：整串是标量，daemon 判 Bad
    expect(parseLabState('vector#x')).toEqual({ kind: 'invalid', raw: 'vector#x' })
  })

  it('引号按 daemon 的两条规则分别处理', () => {
    // 模式名走 YAML 标量：成对引号剥一层、不 trim 内侧
    expect(parseLabState('"vector"')).toEqual({ kind: 'on', mode: 'vector' })
    expect(parseLabState('" vector "')).toEqual({ kind: 'invalid', raw: '" vector "' })
    // 保留字走 trim_matches（逐边剥）；不成对引号的模式名 = YAML 解析失败 → 非法
    expect(parseLabState('"off\'')).toEqual({ kind: 'force-off' })
    expect(parseLabState('"vector\'')).toEqual({ kind: 'invalid', raw: '"vector\'' })
  })
})

describe('可启用模式', () => {
  // 2026-09-22：frozen（待春归）已实现（锁最低频 + 停亲和/迁移/日志），从预留转为可启用
  it('vector/contingency/babel/frozen 都给启用入口', () => {
    expect(LAB_ENABLEABLE).toEqual(
      expect.arrayContaining(['vector', 'contingency', 'babel', 'frozen'])
    )
  })

  it('可启用模式都在已定义的 key 里', () => {
    for (const key of LAB_ENABLEABLE) expect(LAB_MODE_KEYS).toContain(key)
  })
})
