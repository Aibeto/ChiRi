// contract.test.ts: [paths] [meta-write] [meta-validate]
import { describe, expect, it } from 'vitest'
import { isSafeConfigRel, isSafeFileName, shQuote } from '@/contract/paths'
import {
  LOG_LEVELS,
  replaceTopLevelField,
  validateFieldValue,
  validateMeta
} from '@/contract/meta'

describe('paths 安全校验', () => {
  it('接受守护进程实际写出的两种 active_config 形态', () => {
    expect(isSafeConfigRel('meta.yaml')).toBe(true)
    expect(isSafeConfigRel('8550/meta.yaml')).toBe(true)
  })

  it('拒绝路径穿越、绝对路径与空串', () => {
    expect(isSafeConfigRel('')).toBe(false)
    expect(isSafeConfigRel('../meta.yaml')).toBe(false)
    expect(isSafeConfigRel('/data/adb/modules/chiri/config/meta.yaml')).toBe(false)
    expect(isSafeConfigRel('8550\\meta.yaml')).toBe(false)
    expect(isSafeConfigRel('8550/../meta.yaml')).toBe(false)
  })

  it('文件名校验不允许目录分隔', () => {
    expect(isSafeFileName('devimp_a.b_0913-120000.log')).toBe(true)
    expect(isSafeFileName('a/b')).toBe(false)
    expect(isSafeFileName('..')).toBe(false)
  })

  it('shell 单引号转义后可安全嵌入命令', () => {
    expect(shQuote('/data/adb/modules/chiri/logs/daemon.log')).toBe(
      "'/data/adb/modules/chiri/logs/daemon.log'"
    )
    expect(shQuote("a'b")).toBe("'a'\\''b'")
  })
})

describe('meta.yaml 顶层行改写', () => {
  const sample = [
    '# 头部注释',
    'name: "default_config"',
    'author: "yuki"',
    'language: "zh"',
    'loglevel: "INFO"',
    'dev_record: false # 保留我',
    'fas_enabled: true',
    'scenemode_enabled: true',
    'thread_bind: true',
    'nested:',
    '  dev_record: true'
  ].join('\n')

  it('保留引号风格与行内注释', () => {
    const out = replaceTopLevelField(sample, 'dev_record', true)
    expect(out).toContain('dev_record: true # 保留我')
    expect(out).toContain('name: "default_config"')
  })

  it('字符串字段沿用原有引号风格', () => {
    const out = replaceTopLevelField(sample, 'loglevel', 'DEBUG')
    expect(out).toContain('loglevel: "DEBUG"')
  })

  it('不触碰缩进的嵌套同名字段', () => {
    const out = replaceTopLevelField(sample, 'dev_record', false)
    expect(out).toContain('\n  dev_record: true')
  })

  it('字段不存在时返回 null（调用方必须放弃整文件重写）', () => {
    expect(replaceTopLevelField(sample, 'not_exist', 'x')).toBeNull()
  })

  it('大小写不敏感匹配键名', () => {
    const out = replaceTopLevelField(sample, 'LogLevel', 'WARN')
    expect(out).toContain('loglevel: "WARN"')
  })
})

describe('meta.yaml 校验（复刻守护进程口径）', () => {
  const valid = {
    name: 'config_8550',
    author: 'ChiRi',
    language: 'zh',
    loglevel: 'INFO',
    dev_record: true,
    fas_enabled: true,
    scenemode_enabled: false,
    thread_bind: true,
    power_avg: false
  }

  it('合法文件无问题', () => {
    expect(validateMeta(valid)).toEqual([])
  })

  it('缺字段合法（缺省 = 沿用内嵌默认）、未知字段仍判非法', () => {
    // 字段全部可选：老文件/精简文件不再判非法（见 src/common.rs::parse_disk_meta）
    const missing = { ...valid } as Record<string, unknown>
    delete missing.author
    delete missing.power_avg
    expect(validateMeta(missing)).toEqual([])

    expect(validateMeta({ ...valid, extra: 1 }).some(p => p.includes('extra'))).toBe(true)
  })

  it('loglevel 超出六档、language 非 zh/en、开关非布尔都会报错', () => {
    expect(validateMeta({ ...valid, loglevel: 'VERBOSE' }).length).toBe(1)
    expect(validateMeta({ ...valid, language: 'ja' }).length).toBe(1)
    expect(validateMeta({ ...valid, dev_record: 'true' }).length).toBe(1)
  })

  it('带引号的值按去引号后比较（与 daemon 一致）', () => {
    expect(validateMeta({ ...valid, loglevel: '"debug"', language: "'zh'" })).toEqual([])
  })

  it('写入前的单字段校验', () => {
    expect(validateFieldValue('loglevel', 'TRACE')).toBeNull()
    expect(validateFieldValue('loglevel', 'quiet')).not.toBeNull()
    expect(validateFieldValue('language', 'en')).toBeNull()
    expect(validateFieldValue('language', 'fr')).not.toBeNull()
    expect(validateFieldValue('dev_record', true)).toBeNull()
    expect(validateFieldValue('dev_record', 'yes' as unknown as string)).not.toBeNull()
    expect(validateFieldValue('unit_divisor', 1000)).toBeNull()
    expect(validateFieldValue('unit_divisor', 0)).not.toBeNull()
    expect(validateFieldValue('unit_divisor', Number.NaN)).not.toBeNull()
    expect(validateFieldValue('oplus_chg', true)).toBeNull()
    expect(validateFieldValue('oplus_chg', 1 as unknown as boolean)).not.toBeNull()
  })

  it('电池读数字段：布尔类型与单位校准的数字类型', () => {
    expect(
      validateMeta({ ...valid, oplus_chg: true, oplus_dual_cell: false, unit_divisor: 1000 })
    ).toEqual([])
    // 开关写成字符串、校准值写成字符串 → 各报一条（daemon 侧同样判非法）
    expect(validateMeta({ ...valid, current_double: 'true' }).length).toBe(1)
    expect(validateMeta({ ...valid, unit_divisor: '1000' }).length).toBe(1)
  })

  it('日志等级枚举与守护进程一致', () => {
    expect([...LOG_LEVELS]).toEqual(['OFF', 'ERROR', 'WARN', 'INFO', 'DEBUG', 'TRACE'])
  })
})
