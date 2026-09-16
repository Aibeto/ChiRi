// data-parse.test.ts: [status-csv] [daemon-log]
import { describe, expect, it } from 'vitest'
import { STATUS_COLUMN_COUNT, parseStatusCsv } from '@/data/status-csv'
import { filterByLevel, parseDaemonLog } from '@/data/daemon-log'

const HEADER =
  'timestamp,type,mode,package,charge,screen_on,batt_temp,cpu_temp,thermal_cap_pct,thermal_free_pct,' +
  'clg_active,psi_cpu_some,psi_io_some,psi_mem_some,gpu_busy_pct,batt_voltage_v,batt_current_ma,' +
  'batt_power_w,wakeups,migrations,freq_trans,fps'

function row(overrides: Partial<Record<string, string>> = {}): string {
  const base: Record<string, string> = {
    timestamp: '12:00:01.000',
    type: 'snap',
    mode: 'default',
    package: 'com.tencent.mm',
    charge: 'discharging',
    screen_on: '1',
    batt_temp: '35.5',
    cpu_temp: '45.2',
    thermal_cap_pct: '80',
    thermal_free_pct: '72',
    clg_active: '1',
    psi_cpu_some: '1.20',
    psi_io_some: '0.40',
    psi_mem_some: '0.10',
    gpu_busy_pct: '25',
    batt_voltage_v: '4.350',
    batt_current_ma: '-450',
    batt_power_w: '-1.95',
    wakeups: '210',
    migrations: '6',
    freq_trans: '902',
    fps: '-'
  }
  return [
    'timestamp',
    'type',
    'mode',
    'package',
    'charge',
    'screen_on',
    'batt_temp',
    'cpu_temp',
    'thermal_cap_pct',
    'thermal_free_pct',
    'clg_active',
    'psi_cpu_some',
    'psi_io_some',
    'psi_mem_some',
    'gpu_busy_pct',
    'batt_voltage_v',
    'batt_current_ma',
    'batt_power_w',
    'wakeups',
    'migrations',
    'freq_trans',
    'fps'
  ]
    .map(col => overrides[col] ?? base[col])
    .join(',')
}

describe('status.csv 解析', () => {
  it('列数常量与守护进程表头一致（22 列）', () => {
    expect(STATUS_COLUMN_COUNT).toBe(22)
    expect(HEADER.split(',')).toHaveLength(22)
  })

  it('跳过表头并按列名取值', () => {
    const rows = parseStatusCsv([HEADER, row()].join('\n'))
    expect(rows).toHaveLength(1)
    const r = rows[0]
    expect(r.timestamp).toBe('12:00:01.000')
    expect(r.mode).toBe('default')
    expect(r.pkg).toBe('com.tencent.mm')
    expect(r.screenOn).toBe(true)
    expect(r.clgActive).toBe(true)
    expect(r.battTemp).toBeCloseTo(35.5)
    expect(r.thermalCap).toBe(80)
    expect(r.battPower).toBeCloseTo(-1.95)
    // FAS 未启动：fps 预留列为 '-' → null
    expect(r.fps).toBeNull()
  })

  it('FAS 激活时 fps 列取实测值', () => {
    const rows = parseStatusCsv([HEADER, row({ mode: 'fas', fps: '59.8' })].join('\n'))
    expect(rows[0].fps).toBeCloseTo(59.8)
  })

  it('缺测占位为 null，空包名保留空串', () => {
    const rows = parseStatusCsv(
      [
        HEADER,
        row({ batt_temp: '-', cpu_temp: '-', package: '', screen_on: '0', charge: '-' })
      ].join('\n')
    )
    expect(rows[0].battTemp).toBeNull()
    expect(rows[0].cpuTemp).toBeNull()
    expect(rows[0].pkg).toBe('')
    expect(rows[0].screenOn).toBe(false)
    expect(rows[0].charge).toBe('')
  })

  it('窗口截断产生的残缺行与非 snap 行都被过滤', () => {
    const partial = '12:00:02.000,snap,default'
    const other = row({ type: 'fg' })
    const rows = parseStatusCsv([partial, other, row()].join('\n'))
    expect(rows).toHaveLength(1)
  })

  it('超过上限时保留最新的若干行', () => {
    const lines = [HEADER]
    for (let i = 0; i < 30; i++) lines.push(row({ timestamp: `12:00:${String(i).padStart(2, '0')}.000` }))
    const rows = parseStatusCsv(lines.join('\n'), 5)
    expect(rows).toHaveLength(5)
    expect(rows[4].timestamp).toBe('12:00:29.000')
  })
})

describe('daemon.log 解析', () => {
  const text = [
    '12:00:00 被截断的残行',
    '[2026-09-13 12:00:00] [INFO] [yumi::main] 启动完成',
    '[2026-09-13 12:00:01] [ERROR] [yumi::chiri] 出错了',
    '    详细堆栈续行',
    '[2026-09-13 12:00:02] [DEBUG] [yumi::logger] tick',
    '[2026-09-13 12:00:03] [WEIRD] [yumi::x] 未知级别'
  ].join('\n')

  it('解析时间/级别/模块/消息，并丢弃窗口开头的残行', () => {
    const lines = parseDaemonLog(text)
    expect(lines).toHaveLength(4)
    expect(lines[0].time).toBe('2026-09-13 12:00:00')
    expect(lines[0].level).toBe('INFO')
    expect(lines[0].module).toBe('yumi::main')
  })

  it('多行消息并入上一条', () => {
    const lines = parseDaemonLog(text)
    expect(lines[1].message).toContain('详细堆栈续行')
  })

  it('未知级别归为 OTHER', () => {
    const lines = parseDaemonLog(text)
    expect(lines[3].level).toBe('OTHER')
  })

  it('级别过滤按严重度阈值保留', () => {
    const lines = parseDaemonLog(text)
    const warnUp = filterByLevel(lines, 'WARN')
    expect(warnUp.map(l => l.level)).toEqual(['ERROR', 'OTHER'])
  })
})
