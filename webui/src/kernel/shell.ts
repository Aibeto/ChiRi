// shell.ts: [contract] [injection] [run]
// 唯一的命令执行出口。真实实现走 KernelSU exec；dev / 单测可注入替代实现，
// 这是“契约层与数据层可脱离真机断言”的前提。
import { exec as ksuExec, hasKsu } from './ksu'

export interface ExecResult {
  errno: number
  stdout: string
  stderr: string
}

export interface ShellRunner {
  exec(command: string): Promise<ExecResult>
  /** 是否具备真实设备执行能力（dev/单测为 false） */
  readonly live: boolean
}

// [contract]
export const ksuShell: ShellRunner = {
  live: true,
  exec: ksuExec
}

/** 无设备环境下的兜底：所有命令按“环境不支持”失败，由契约层转成 absent('unsupported-env') */
export const inertShell: ShellRunner = {
  live: false,
  exec: () => Promise.reject(new Error('shell unavailable'))
}

// [injection]
let current: ShellRunner = hasKsu() ? ksuShell : inertShell

export function setShell(runner: ShellRunner): void {
  current = runner
}

export function shell(): ShellRunner {
  return current
}

/** 当前是否具备真实执行能力 */
export function isLive(): boolean {
  return current.live
}

// [run]
export function run(command: string): Promise<ExecResult> {
  return current.exec(command)
}
