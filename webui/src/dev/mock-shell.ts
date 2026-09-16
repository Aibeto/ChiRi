// mock-shell.ts: [scenario] [fs] [seed] [exec] [install]
// 无 KernelSU 环境（浏览器 dev / 自动化走查）下的设备替身：
// 用构建期嵌入的仓库配置 + 生成的假日志构成一个内存文件系统，并按契约层实际发出的
// 命令形态回放。URL 参数可切换设备形态，便于走查空态/错误态：
//   ?soc=chiri|yumi   设备是否 ChiRi 专属机型（影响白名单与 status.csv 是否存在）
//   ?state=normal|empty|error   正常 / 守护进程从未启动 / 读取真实失败
//   ?daemon=running|stopped     存活探测结果（影响“关闭调度”演示）
import embedded from 'virtual:chiri-config'
import { load as loadYaml } from 'js-yaml'
import { setShell, type ExecResult, type ShellRunner } from '@/kernel/shell'

const FILES = embedded.files
const ROOT = '/data/adb/modules/chiri'

// [scenario]
function param(name: string, fallback: string): string {
  try {
    const search = globalThis.location?.search ?? ''
    return new URLSearchParams(search).get(name) ?? fallback
  } catch {
    return fallback
  }
}

const soc = param('soc', 'chiri')
const state = param('state', 'normal')
const isChiri = soc === 'chiri'
let daemonRunning = param('daemon', 'running') !== 'stopped'
const readFails = state === 'error'
const empty = state === 'empty'

// [fs]
const files = new Map<string, string>()
const dirs = new Set<string>()

function abs(rel: string): string {
  return `${ROOT}/${rel}`
}

function put(rel: string, content: string): void {
  files.set(abs(rel), content)
}

function mkdir(rel: string): void {
  dirs.add(abs(rel))
}

function listDir(path: string): string[] | null {
  const prefix = path.endsWith('/') ? path : `${path}/`
  if (!dirs.has(path) && ![...files.keys()].some(k => k.startsWith(prefix)) && path !== ROOT) {
    return null
  }
  const out = new Set<string>()
  for (const key of [...files.keys(), ...dirs]) {
    if (!key.startsWith(prefix)) continue
    const rest = key.slice(prefix.length)
    if (!rest) continue
    out.add(rest.includes('/') ? rest.slice(0, rest.indexOf('/')) : rest)
  }
  return [...out].sort()
}

// [seed]
function two(n: number): string {
  return String(n).padStart(2, '0')
}

function stamp(offsetMs: number): string {
  const d = new Date(Date.now() + offsetMs)
  return `${d.getFullYear()}-${two(d.getMonth() + 1)}-${two(d.getDate())} ${two(d.getHours())}:${two(d.getMinutes())}:${two(d.getSeconds())}`
}

function clock(offsetMs: number): string {
  const d = new Date(Date.now() + offsetMs)
  return `${two(d.getHours())}:${two(d.getMinutes())}:${two(d.getSeconds())}.000`
}

function fakeDaemonLog(): string {
  const samples: Array<[string, string, string]> = [
    ['INFO', 'yumi::main', 'yumi-module-starting 调度开始启动'],
    ['INFO', 'yumi::main', 'main-chiri-scheduler-selected 选择 ChiRi 调度'],
    ['INFO', 'yumi::monitor::app_detect', 'app-detect-init 前台检测已就绪'],
    ['DEBUG', 'yumi::chiri', 'scheduler-event-mode-change pkg=com.tencent.tmgp.sgame old=balance new=performance'],
    ['INFO', 'yumi::chiri', 'scheduler-clg-init mode=performance'],
    ['DEBUG', 'yumi::logger', 'status-log-snapshot 写入成功'],
    ['WARN', 'yumi::chiri::thermal', 'thermal-cap 温度接近软限，已压制性能上限'],
    ['INFO', 'yumi::chiri::fas_manager', 'fas instance activated pkg=com.miHoYo.Yuanshen'],
    ['ERROR', 'yumi::chiri::fas', 'fas policy load failed: missing embedded config'],
    ['DEBUG', 'yumi::chiri::clg', 'clg-tick mode=performance target=2200000'],
    ['INFO', 'yumi::logger', 'log-archive-submitted zip=logd/ziped_0913-120000.zip']
  ]
  const lines: string[] = []
  for (let i = 0; i < 90; i++) {
    const s = samples[i % samples.length]
    lines.push(`[${stamp(-90000 + i * 1000)}] [${s[0]}] [${s[1]}] ${s[2]}`)
  }
  return lines.join('\n') + '\n'
}

function fakeStatusCsv(): string {
  const header =
    'timestamp,type,mode,package,charge,screen_on,batt_temp,cpu_temp,thermal_cap_pct,thermal_free_pct,' +
    'clg_active,psi_cpu_some,psi_io_some,psi_mem_some,gpu_busy_pct,batt_voltage_v,batt_current_ma,' +
    'batt_power_w,wakeups,migrations,freq_trans,fps'
  const modes = ['balance', 'performance', 'balance', 'fas', 'balance']
  const pkgs = ['com.tencent.mm', 'com.tencent.tmgp.sgame', '', 'com.miHoYo.Yuanshen', 'com.tencent.mm']
  const rows: string[] = [header]
  for (let i = 0; i < 60; i++) {
    const idx = i % 5
    rows.push(
      [
        clock(-60000 + i * 1000),
        'snap',
        modes[idx],
        pkgs[idx],
        i % 3 === 0 ? 'charging' : 'discharging',
        i % 7 === 0 ? '0' : '1',
        (35 + (i % 8) * 0.4).toFixed(1),
        (42 + (i % 10) * 0.5).toFixed(1),
        '80',
        '72',
        modes[idx] === 'balance' ? '1' : '1',
        (i % 5) * 1.3,
        (i % 4) * 0.8,
        (i % 3) * 0.4,
        (20 + (i % 6) * 5).toFixed(0),
        '4.35',
        (i % 5 === 0 ? 1200 : -450).toString(),
        (i % 5 === 0 ? 5.2 : -1.95).toFixed(2),
        (200 + i * 3).toString(),
        (5 + (i % 9)).toString(),
        (900 + i * 2).toString(),
        // fps 预留列：仅 fas 模式有实测值，其余为 '-'
        modes[idx] === 'fas' ? (58 + (i % 5) * 0.6).toFixed(1) : '-'
      ].join(',')
    )
  }
  return rows.join('\n') + '\n'
}

function specialTunedExport(): string {
  const raw = FILES['src/chiri/special_tuned.yaml'] ?? ''
  return raw
    .split('\n')
    .filter(line => {
      const t = line.trim()
      return t && !t.startsWith('#') && !t.startsWith('re:')
    })
    .map(line => line.trim())
    .join('\n')
    .concat('\n')
}

function fasWhitelistExport(): string {
  const raw = FILES['module/config/normal/fas.yaml'] ?? ''
  try {
    const parsed = loadYaml(raw) as { fas?: { apps?: Record<string, string> } } | undefined
    const apps = parsed?.fas?.apps ?? {}
    return Object.entries(apps)
      .map(([pkg, cfg]) => `${pkg}:${cfg}`)
      .join('\n')
      .concat('\n')
  } catch {
    return ''
  }
}

function seed(): void {
  mkdir('logs')
  mkdir('devimp')
  mkdir('logd')

  put('module.prop', FILES['module/module.prop'] ?? '')
  put('rules.yaml', FILES['module/rules.yaml'] ?? '')
  put('daemon.lock', '')
  put('action.sh', '#!/system/bin/sh\n')

  const metaRel = isChiri ? 'config/8550/meta.yaml' : 'config/meta.yaml'
  put('active_config.chr', isChiri ? '8550/meta.yaml' : 'meta.yaml')
  put(metaRel, FILES[`module/config/${isChiri ? '8550/' : ''}meta.yaml`] ?? '')

  if (isChiri) {
    put('special_tuned.yaml', specialTunedExport())
    put('fas_whitelist.yaml', fasWhitelistExport())
  }

  if (empty) return

  put('current_mode.chr', 'balance')
  put('logs/watchdog.pid', '12345\n')
  put('logs/daemon.log', fakeDaemonLog())
  if (isChiri) put('logs/status.csv', fakeStatusCsv())
  put('devimp/devimp_com.tencent.tmgp.sgame_0913-120000.log', '# ts-column=local format_now\n')
  put('logd/ziped_0913-120000.zip', 'PK\u0003\u0004mock')
}

// [exec]
function ok(stdout: string): ExecResult {
  return { errno: 0, stdout, stderr: '' }
}

function missing(): ExecResult {
  return { errno: 1, stdout: '', stderr: 'cat: No such file or directory' }
}

function denied(): ExecResult {
  return { errno: 1, stdout: '', stderr: 'cat: Permission denied' }
}

function unquote(raw: string): string {
  const t = raw.trim()
  if (t.length >= 2 && t.startsWith("'") && t.endsWith("'")) {
    return t.slice(1, -1).replace(/'\\''/g, "'")
  }
  return t
}

function b64decode(input: string): string {
  const bin = atob(input)
  const bytes = Uint8Array.from(bin, c => c.charCodeAt(0))
  return new TextDecoder().decode(bytes)
}

const FLOCK_CMD = /^command -v flock/
const FLOCK_PROBE = /^flock -n (\S+) true/
const EXISTS_CMD = /^\[ -e (.+) \] && echo 1 \|\| echo 0$/
const TAIL_CMD = /^tail -c (\d+) (.+)$/
const LS_CMD = /^ls -1 (.+)$/
const WRITE_CMD = /^printf '%s' (\S+) \| base64 -d > (.+) && mv -f (.+) (.+) \|\| \{ rm -f (.+); exit 1; \}$/
const KILL_CMD = /killall -9 yumi/

function handle(cmd: string): ExecResult | null {
  if (FLOCK_CMD.test(cmd)) return ok('1\n')

  if (FLOCK_PROBE.test(cmd)) {
    return ok(daemonRunning ? 'HELD\n' : 'FREE\n')
  }

  const exists = EXISTS_CMD.exec(cmd)
  if (exists) {
    const path = unquote(exists[1])
    return ok(files.has(path) || dirs.has(path) ? '1\n' : '0\n')
  }

  if (readFails) {
    if (TAIL_CMD.test(cmd) || LS_CMD.test(cmd) || WRITE_CMD.test(cmd)) return denied()
  }

  const tail = TAIL_CMD.exec(cmd)
  if (tail) {
    const limit = Number(tail[1])
    const path = unquote(tail[2])
    const content = files.get(path)
    if (content === undefined) return missing()
    return ok(content.slice(-limit))
  }

  const ls = LS_CMD.exec(cmd)
  if (ls) {
    const entries = listDir(unquote(ls[1]))
    if (entries === null) {
      return { errno: 1, stdout: '', stderr: 'ls: No such file or directory' }
    }
    return ok(entries.length ? entries.join('\n') + '\n' : '')
  }

  const write = WRITE_CMD.exec(cmd)
  if (write) {
    const tmp = unquote(write[2])
    const target = unquote(write[4])
    try {
      files.set(target, b64decode(unquote(write[1])))
      files.delete(tmp)
      return ok('')
    } catch {
      return { errno: 1, stdout: '', stderr: 'base64: invalid input' }
    }
  }

  if (KILL_CMD.test(cmd)) {
    daemonRunning = false
    files.delete(abs('logs/watchdog.pid'))
    return ok('done\n')
  }

  if (/^pm list packages/.test(cmd)) {
    return ok(
      [
        'package:com.tencent.mm',
        'package:com.tencent.tmgp.sgame',
        'package:com.miHoYo.Yuanshen',
        'package:com.hypergryph.arknights'
      ].join('\n') + '\n'
    )
  }

  return null
}

export const mockShell: ShellRunner = {
  live: true,
  exec(command: string): Promise<ExecResult> {
    const result = handle(command)
    if (result) return Promise.resolve(result)
    // 未识别的命令按“环境不支持”处理，避免 mock 静默给出错误语义
    return Promise.reject(new Error(`mock: 未识别的命令 -> ${command}`))
  }
}

// [install]
let installed = false

export function installMockShell(): void {
  if (installed) return
  installed = true
  seed()
  setShell(mockShell)
}
