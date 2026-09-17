// ksu.ts: [env] [exec] [ui] [packages] [module]
// KernelSU WebView JS 桥的 TypeScript 封装：ksu.* 由原生注入，不同管理器版本可能缺 API，
// 所有调用都做能力探测，缺失时降级而不是抛异常（契约层的 Absent/Failed 由上层判定）。

interface KsuGlobal {
  exec?: (cmd: string, opts: string, cb: string) => void
  toast?: (msg: string) => void
  listPackages?: (type: string) => string
  getPackagesInfo?: (pkgs: string) => string
  moduleInfo?: () => string
  fullScreen?: (on: boolean) => void
  enableEdgeToEdge?: (on: boolean) => void
  /** 关闭 WebUI 宿主（管理器注入；旧版本可能没有） */
  exit?: () => void
}

export interface ExecResult {
  errno: number
  stdout: string
  stderr: string
}

export interface PackageInfo {
  packageName: string
  versionName?: string
  versionCode?: number
  appLabel?: string
  isSystem?: boolean
  uid?: number
}

export interface ModuleInfo {
  id?: string
  name?: string
  version?: string
  versionCode?: number
  author?: string
  description?: string
  /** 模块在设备上的目录（KernelSU 提供，缺失时由契约层回退默认路径） */
  moduleDir?: string
}

// [env]
function ksu(): KsuGlobal | null {
  const g = globalThis as { ksu?: KsuGlobal }
  return g.ksu ?? null
}

/** 是否运行在 KernelSU/Magisk 管理器的 WebView 内（dev 浏览器为 false） */
export function hasKsu(): boolean {
  return ksu() !== null
}

// [exec]
let callbackSeq = 0

/**
 * 执行 shell 命令。无 ksu 环境（浏览器 dev）时抛错，由调用方决定回退策略。
 * 注意：命令字符串由契约层负责转义/编码，本层不做任何拼接。
 * 带超时兜底：原生回调丢失（WebView 被杀等）时不能让调用方的 loading 永久卡死。
 */
export function exec(command: string, timeoutMs = 20000): Promise<ExecResult> {
  const api = ksu()
  if (!api?.exec) {
    return Promise.reject(new Error('ksu.exec unavailable'))
  }
  return new Promise<ExecResult>((resolve, reject) => {
    const name = `chiri_exec_${Date.now()}_${callbackSeq++}`
    const store = globalThis as unknown as Record<string, unknown>
    let settled = false

    const timer = setTimeout(() => {
      if (settled) return
      settled = true
      delete store[name]
      reject(new Error(`命令执行超时(${timeoutMs}ms)`))
    }, timeoutMs)

    store[name] = (errno: number, stdout: string, stderr: string) => {
      if (settled) return
      settled = true
      clearTimeout(timer)
      delete store[name]
      resolve({ errno, stdout: stdout ?? '', stderr: stderr ?? '' })
    }

    try {
      api.exec!(command, JSON.stringify({}), name)
    } catch (e) {
      if (settled) return
      settled = true
      clearTimeout(timer)
      delete store[name]
      reject(e instanceof Error ? e : new Error(String(e)))
    }
  })
}

// [ui]
export function toast(message: string): void {
  const api = ksu()
  if (api?.toast) {
    try {
      api.toast(message)
      return
    } catch {
      /* 落到 console */
    }
  }
  console.info(`[toast] ${message}`)
}

export function enableEdgeToEdge(on: boolean): void {
  const api = ksu()
  try {
    api?.enableEdgeToEdge?.(on)
  } catch {
    /* 老版本无此 API：忽略 */
  }
}

export function fullScreen(on: boolean): void {
  const api = ksu()
  try {
    api?.fullScreen?.(on)
  } catch {
    /* 老版本无此 API：忽略 */
  }
}

/**
 * 关闭 WebUI：管理器注入了 exit 时走原生关闭并返回 true；
 * 未注入（旧版管理器 / 外部浏览器）返回 false，由调用方兜底
 * （WebView 里 window.close() 通常无效，只做历史后退会变成「返回上一页」）。
 */
export function exitApp(): boolean {
  const api = ksu()
  if (!api?.exit) return false
  try {
    api.exit()
    return true
  } catch {
    return false
  }
}

// [packages]
export function listPackages(type: 'user' | 'system' | 'all' = 'user'): string[] {
  const api = ksu()
  if (!api?.listPackages) return []
  try {
    const raw = api.listPackages(type)
    const parsed: unknown = JSON.parse(raw)
    return Array.isArray(parsed) ? parsed.filter((p): p is string => typeof p === 'string') : []
  } catch {
    return []
  }
}

export function getPackagesInfo(packages: string[]): PackageInfo[] {
  const api = ksu()
  if (!api?.getPackagesInfo) return []
  try {
    const parsed: unknown = JSON.parse(api.getPackagesInfo(JSON.stringify(packages)))
    return Array.isArray(parsed) ? (parsed as PackageInfo[]) : []
  } catch {
    return []
  }
}

// [module]
export function moduleInfo(): ModuleInfo | null {
  const api = ksu()
  if (!api?.moduleInfo) return null
  try {
    const parsed: unknown = JSON.parse(api.moduleInfo())
    return parsed && typeof parsed === 'object' ? (parsed as ModuleInfo) : null
  } catch {
    return null
  }
}
