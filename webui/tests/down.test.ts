// down.test.ts: [parse] [write]
// DOWN 停摆状态的解析口径必须与守护进程 src/down.rs::read_down 对齐，
// 所以重点覆盖「两边容易分歧」的形态：注释、引号、大小写、其它写法。
import { describe, expect, it } from 'vitest'
import { DOWN_WORD, downFileContent, parseDown } from '@/data/down'

describe('down.chr 解析', () => {
  it('空文件与只有注释都算未停摆', () => {
    expect(parseDown('')).toBe(false)
    expect(parseDown('\n\n')).toBe(false)
    expect(parseDown('# 说明\n# 再来一行\n')).toBe(false)
  })

  it('写了 down 就是停摆（大小写与引号都认）', () => {
    expect(parseDown('down')).toBe(true)
    expect(parseDown('down\n')).toBe(true)
    expect(parseDown('  DOWN  ')).toBe(true)
    expect(parseDown('"down"')).toBe(true)
    expect(parseDown("'down'")).toBe(true)
    expect(DOWN_WORD).toBe('down')
  })

  it('注释与内容混排时取内容', () => {
    expect(parseDown('# 头注释\ndown\n# 尾注释\n')).toBe(true)
    expect(parseDown('# 头注释\noff\n')).toBe(false)
  })

  it('别的写法一律算未停摆（不认近似值）', () => {
    expect(parseDown('fastmode')).toBe(false)
    expect(parseDown('downdown')).toBe(false)
    expect(parseDown('down disabled')).toBe(false)
    // 引号内侧的空白：daemon 剥引号后用 trim_matches、不再 trim 内侧，这里也必须判否
    expect(parseDown('" down "')).toBe(false)
  })
})

describe('down.chr 写入内容', () => {
  it('开启写 down，解除写空内容', () => {
    expect(downFileContent(true)).toBe('down\n')
    expect(downFileContent(false)).toBe('')
  })

  it('写进去的内容能被自己解析回来（不出现写完就判错）', () => {
    expect(parseDown(downFileContent(true))).toBe(true)
    expect(parseDown(downFileContent(false))).toBe(false)
  })
})
