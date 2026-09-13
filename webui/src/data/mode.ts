// mode.ts: [catalog] [derive]
// 模式派生。事实来源（src/monitor/app_detect.rs::determine_mode）：
//   fas（白名单命中且应用配置可解析）→ 特调白名单 fallback → app_modes → global_mode。
// 只有 powersave/balance/performance/fast 在 daemon 里注册为 CLG 档（config.rs::get_mode）；
// 特调模式名由 special_tuned.yaml 的 modes 定义（当前仅 akmode）；
// determine_mode 不做注册校验，因此 current_mode.chr 可能出现未注册的字面值 → 归为 unknown。

// [catalog]
export type ModeKind = 'clg' | 'fas' | 'special' | 'unknown'
/** 语义信号（UI 映射到 --ak-signal-*，不用裸色值） */
export type ModeSignal = 'info' | 'success' | 'action' | 'danger' | 'accent'

export interface ModeInfo {
  /** 原始模式名（current_mode.chr 的内容） */
  id: string
  kind: ModeKind
  signal: ModeSignal
  /** i18n 键（缺失时界面回退显示原始 id） */
  labelKey: string
  descKey: string
}

const CLG_CATALOG: Record<string, { signal: ModeSignal; labelKey: string; descKey: string }> = {
  powersave: { signal: 'success', labelKey: 'mode.powersave', descKey: 'mode.powersave.desc' },
  balance: { signal: 'info', labelKey: 'mode.balance', descKey: 'mode.balance.desc' },
  performance: { signal: 'action', labelKey: 'mode.performance', descKey: 'mode.performance.desc' },
  fast: { signal: 'danger', labelKey: 'mode.fast', descKey: 'mode.fast.desc' }
}

// [derive]
/**
 * 由 current_mode 值与特调模式集合派生展示信息。
 * `specialModes` 来自 special_tuned.yaml 的 modes 并集——注意该文件只导出精确条目，
 * 正则条目对应的特调模式在 UI 侧不可知，因此这里只能覆盖「已配置」的部分。
 */
export function describeMode(id: string, specialModes?: ReadonlySet<string>): ModeInfo {
  const mode = id.trim()
  if (!mode) {
    return {
      id: '',
      kind: 'unknown',
      signal: 'info',
      labelKey: 'mode.unknown',
      descKey: 'mode.unknown.desc'
    }
  }
  if (mode === 'fas') {
    return {
      id: mode,
      kind: 'fas',
      signal: 'accent',
      labelKey: 'mode.fas',
      descKey: 'mode.fas.desc'
    }
  }
  const clg = CLG_CATALOG[mode]
  if (clg) {
    return { id: mode, kind: 'clg', signal: clg.signal, labelKey: clg.labelKey, descKey: clg.descKey }
  }
  if (specialModes?.has(mode)) {
    return {
      id: mode,
      kind: 'special',
      signal: 'accent',
      labelKey: 'mode.special',
      descKey: 'mode.special.desc'
    }
  }
  return {
    id: mode,
    kind: 'unknown',
    signal: 'info',
    labelKey: 'mode.unknown',
    descKey: 'mode.unknown.desc'
  }
}

/** CLG 四档（配置页与规则页展示用） */
export const CLG_MODE_IDS = ['powersave', 'balance', 'performance', 'fast'] as const
