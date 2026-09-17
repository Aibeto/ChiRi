// power-avg.ts: [parse]
// PowerAVG.chr 解析（模块根）。守护进程 src/logger.rs::power_avg_update 每 1s 采样
// 写一行数字（保留两位小数），**仅电池放电时计入**（插电/充满/未充电时跳过，文件
// 保留上次放电得出的值）。口径由 meta.yaml 的 `power_avg` 决定：
// false = 参考值（与上次取半递推，偏近期）、true = 累计平均值（等权全史）——
// 文件内容只存值、不存口径，界面按 meta 的开关标注。

/** 文件名（与守护进程 logger.rs::POWER_AVG_CHR 对齐） */
export const POWER_AVG_FILE = 'PowerAVG.chr'

/**
 * 解析文件内容为瓦特值：取第一行有效内容（忽略空行与注释）转为数字。
 * 文件不存在 / 为空 / 内容非法 / 数值越界时返回 null（界面显示 —）。
 */
export function parsePowerAvgWatt(text: string): number | null {
  const first = text
    .split('\n')
    .map(line => line.trim())
    .find(line => line.length > 0 && !line.startsWith('#'))
  if (first === undefined) return null
  const value = Number(first)
  if (!Number.isFinite(value) || value < 0) return null
  return value
}
