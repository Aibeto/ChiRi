// paths.ts: [root] [rel] [abs] [cargo] [safety]
// 设备侧路径的唯一来源：模块根优先取 KernelSU moduleInfo().moduleDir，回退约定路径。
import { moduleInfo } from '@/kernel/ksu'

// [root]
const DEFAULT_MODULE_DIR = '/data/adb/modules/chiri'
let cachedRoot: string | null = null

/** 模块在设备上的绝对目录（去掉尾部斜杠，保证可直接拼接） */
export function moduleRoot(): string {
  if (cachedRoot) return cachedRoot
  const dir = moduleInfo()?.moduleDir?.trim()
  cachedRoot = dir && dir.startsWith('/') ? dir.replace(/\/+$/, '') : DEFAULT_MODULE_DIR
  return cachedRoot
}

/** 仅测试用：注入根目录，避免依赖 ksu 环境 */
export function setModuleRootForTest(root: string | null): void {
  cachedRoot = root
}

// [rel]
/** 与守护进程约定的相对路径（见计划文档“接触点”表） */
export const REL = {
  daemonLock: 'daemon.lock',
  activeConfig: 'active_config.chr',
  currentMode: 'current_mode.chr',
  rules: 'rules.yaml',
  specialTuned: 'special_tuned.yaml',
  fasWhitelist: 'fas_whitelist.yaml',
  /** 实验室状态（对外暴露，可手改；空/只有注释 = 未启用） */
  rhine: 'rhine.chr',
  /** 实验室原值快照，只在启用期间存在（界面只读展示，写入方是守护进程） */
  rhineBack: 'rhine-back.chr',
  /** DOWN 停摆状态（对外暴露，可手改；写了 down = 调度停摆） */
  down: 'down.chr',
  moduleProp: 'module.prop',
  actionSh: 'action.sh',
  daemonLog: 'logs/daemon.log',
  statusCsv: 'logs/status.csv',
  statusCsvBak: 'logs/status.csv.1',
  watchdogPid: 'logs/watchdog.pid',
  devimpDir: 'devimp',
  logdDir: 'logd'
} as const

export type RelKey = keyof typeof REL

// [abs]
/** 相对模块根拼绝对路径 */
export function abs(relPath: string): string {
  return `${moduleRoot()}/${relPath}`
}

/** 接触点快捷取绝对路径 */
export function absOf(key: RelKey): string {
  return abs(REL[key])
}

/** 生效配置文件的绝对路径：active_config.chr 内容相对 config/ 目录 */
export function configAbs(relFromConfig: string): string {
  return `${moduleRoot()}/config/${relFromConfig}`
}

// [cargo]
/** POSIX 单引号转义：命令拼接的唯一出口，禁止裸拼用户/文件内容 */
export function shQuote(value: string): string {
  return `'${value.replace(/'/g, `'\\''`)}'`
}

// [safety]
/**
 * active_config.chr 内容校验：必须是不含上溯的相对路径（可含一层 SoC 子目录）。
 * 拒绝绝对路径、`..`、空串与反斜杠，防路径注入。
 */
export function isSafeConfigRel(p: string): boolean {
  if (!p) return false
  if (p.startsWith('/') || p.includes('\\') || p.includes('..')) return false
  return /^[A-Za-z0-9._/-]+$/.test(p)
}

/** 仅允许文件名（无目录分隔）的场景（devimp / logd 目录项） */
export function isSafeFileName(name: string): boolean {
  if (!name || name.includes('/') || name.includes('\\') || name.includes('..')) return false
  return /^[A-Za-z0-9._-]+$/.test(name)
}
