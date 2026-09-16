// status-csv.ts: [columns] [types] [parse]
// logs/status.csv 契约（src/logger.rs STATUS_HEADER / status_log_snapshot）：
//   22 列、首行是表头、type 恒为 snap；timestamp 是**设备本地时间** HH:MM:SS.mmm（无日期）；
//   缺测值统一为 '-'；screen_on / clg_active 用 0/1 表示。
//   fps 是预留列：仅 FAS 激活且帧窗口有样本时为实测值，其余为 '-'。
// 该文件仅 ChiRi 机型的调度线程产生，读取一律取尾部窗口。

// [columns]
export const STATUS_COLUMNS = [
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
] as const

export const STATUS_COLUMN_COUNT = STATUS_COLUMNS.length

// [types]
export interface StatusRow {
  /** 设备本地时间 HH:MM:SS.mmm（无日期，跨天需靠归档名/行序推断） */
  timestamp: string
  mode: string
  /** 前台包名，启动初期可能为空串 */
  pkg: string
  /** charging / discharging / full / not_charging / '-' */
  charge: string
  screenOn: boolean
  battTemp: number | null
  cpuTemp: number | null
  /** 热保护上限百分比 */
  thermalCap: number | null
  thermalFree: number | null
  clgActive: boolean
  psiCpu: number | null
  psiIo: number | null
  psiMem: number | null
  gpuBusy: number | null
  battVoltage: number | null
  battCurrent: number | null
  battPower: number | null
  wakeups: number | null
  migrations: number | null
  freqTrans: number | null
  /** FAS 实测帧率；未启动 FAS 时为 null（CSV 里是 '-'） */
  fps: number | null
}

// [parse]
const NA = '-'

function num(raw: string | undefined): number | null {
  if (raw === undefined) return null
  const v = raw.trim()
  if (!v || v === NA) return null
  const n = Number(v)
  return Number.isFinite(n) ? n : null
}

function bool(raw: string | undefined): boolean {
  return raw?.trim() === '1'
}

function str(raw: string | undefined): string {
  const v = (raw ?? '').trim()
  return v === NA ? '' : v
}

/**
 * 解析 CSV 文本为快照行。窗口可能从行中间开始（tail -c），
 * 因此按「字段数必须等于 22」过滤残缺行；表头行按首列名识别。
 * 返回按时间顺序（文件顺序）的数组，由调用方决定展示方向。
 */
export function parseStatusCsv(text: string, maxRows = 300): StatusRow[] {
  const rows: StatusRow[] = []
  const lines = text.split('\n')
  for (const line of lines) {
    const raw = line.trim()
    if (!raw) continue
    const cells = raw.split(',')
    if (cells.length !== STATUS_COLUMN_COUNT) continue
    if (cells[0] === 'timestamp' || cells[1] !== 'snap') continue
    rows.push({
      timestamp: str(cells[0]),
      mode: str(cells[2]),
      pkg: str(cells[3]),
      charge: str(cells[4]),
      screenOn: bool(cells[5]),
      battTemp: num(cells[6]),
      cpuTemp: num(cells[7]),
      thermalCap: num(cells[8]),
      thermalFree: num(cells[9]),
      clgActive: bool(cells[10]),
      psiCpu: num(cells[11]),
      psiIo: num(cells[12]),
      psiMem: num(cells[13]),
      gpuBusy: num(cells[14]),
      battVoltage: num(cells[15]),
      battCurrent: num(cells[16]),
      battPower: num(cells[17]),
      wakeups: num(cells[18]),
      migrations: num(cells[19]),
      freqTrans: num(cells[20]),
      fps: num(cells[21])
    })
  }
  return rows.length > maxRows ? rows.slice(rows.length - maxRows) : rows
}
