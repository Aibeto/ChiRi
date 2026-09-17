// live-time.test.ts: [parse] [freshness]
import { describe, expect, it } from 'vitest'
import {
  LIVE_TIME_TOLERANCE_SECONDS,
  isLiveTimeFresh,
  liveTimeAgeSeconds,
  nowSecondsOfHour,
  parseLiveTimeSeconds
} from '@/data/live-time'

describe('LiveTime.chr 解析', () => {
  it('解析 MM:SS 为小时内秒数', () => {
    expect(parseLiveTimeSeconds('07:42\n')).toBe(7 * 60 + 42)
    expect(parseLiveTimeSeconds('00:00')).toBe(0)
    expect(parseLiveTimeSeconds('59:59')).toBe(3599)
  })

  it('取首个非空行（忽略前后空行与后续内容）', () => {
    expect(parseLiveTimeSeconds('\n\n12:34\n99:99\n')).toBe(12 * 60 + 34)
  })

  it('格式非法 / 空内容返回 null', () => {
    expect(parseLiveTimeSeconds('')).toBeNull()
    expect(parseLiveTimeSeconds('\n')).toBeNull()
    expect(parseLiveTimeSeconds('12')).toBeNull()
    expect(parseLiveTimeSeconds('12:60')).toBeNull()
    expect(parseLiveTimeSeconds('60:00')).toBeNull()
    expect(parseLiveTimeSeconds('12:34:56')).toBeNull()
  })
})

describe('心跳新鲜度', () => {
  it('年龄按 1 小时取模（跨小时边界）', () => {
    expect(liveTimeAgeSeconds(100, 130)).toBe(30)
    // 59:40 写、00:10 读 → 30s
    expect(liveTimeAgeSeconds(59 * 60 + 40, 10)).toBe(30)
    // 本机时间早于文件时间 → 取模后落到 (1800, 3600) 的过期区间
    expect(liveTimeAgeSeconds(10, 59 * 60 + 40)).toBe(3570)
  })

  it('容差 20s：内为新鲜，超过即过期', () => {
    expect(LIVE_TIME_TOLERANCE_SECONDS).toBe(20)
    expect(isLiveTimeFresh(100, 100)).toBe(true)
    expect(isLiveTimeFresh(100, 100 + LIVE_TIME_TOLERANCE_SECONDS)).toBe(true)
    expect(isLiveTimeFresh(100, 100 + LIVE_TIME_TOLERANCE_SECONDS + 1)).toBe(false)
  })

  it('跨小时边界仍能判定（59:55 写、00:05 读 = 10s → 新鲜）', () => {
    expect(isLiveTimeFresh(59 * 60 + 55, 5)).toBe(true)
  })

  it('文件时间跑在本机前面（时钟回拨 / 上一小时旧值）判过期', () => {
    expect(isLiveTimeFresh(10, 59 * 60 + 40)).toBe(false)
  })

  it('nowSecondsOfHour 取当前分秒', () => {
    expect(nowSecondsOfHour(new Date(2026, 8, 18, 13, 5, 9))).toBe(5 * 60 + 9)
  })
})
