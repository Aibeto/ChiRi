// mode.ts: [catalog] [derive]
import { DOWN_WORD } from '@/data/down'
// 模式派生。事实来源（src/monitor/app_detect.rs::determine_mode）：
//   fas（白名单命中且应用配置可解析）→ 特调白名单 fallback → app_modes → global_mode。
// 只有 reduce/default/boost/vector 在 daemon 里注册为 CLG 档（config.rs::get_mode）；
// 特调模式名由 special_tuned.yaml 的 modes 定义（当前仅 akmode）；
// determine_mode 不做注册校验，因此 current_mode.chr 可能出现未注册的字面值 → 归为 unknown。

// [catalog]
/**
 * 模式家族（2026-09-17 重构，仅概念分组，不产生新的模式值）：
 * - CLG：reduce/default/boost（兜底档，current_mode 直接是档名）
 * - stardust：scenemode/down（保守性使用或手动开启；scenemode 是独立息屏轴，
 *   down 是停摆布尔，都不作为 current_mode 档位出现——down 除外，见 describeMode）
 * - rhine：vector/contingency/babel（仅实验室，rhine.chr 驱动 global_mode 覆盖）
 */
export type ModeKind = 'clg' | 'fas' | 'special' | 'lab' | 'down' | 'unknown'
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

const CLG_CATALOG: Record<string, {
  signal: ModeSignal; labelKey: string; descKey: string
}> = {
  reduce: {
    signal: 'success', labelKey: 'mode.reduce', descKey: 'mode.reduce.desc'
  },
  default: {
    signal: 'info', labelKey: 'mode.default', descKey: 'mode.default.desc'
  },
  boost: {
    signal: 'action', labelKey: 'mode.boost', descKey: 'mode.boost.desc'
  }
}

/**
 * rhine 家族（仅实验室）：vector/contingency/babel。vector 虽保留在 CLG_MODE_IDS
 * 的展示列表里（档位概念沿用），但运行时它走 fast_lock 硬锁、不读 CLG 参数，
 * 语义归 rhine 家族，故从 CLG_CATALOG 移到这里。
 */
const LAB_CATALOG: Record<string, {
  signal: ModeSignal; labelKey: string; descKey: string
}> = {
  vector: {
    signal: 'danger', labelKey: 'mode.vector', descKey: 'mode.vector.desc'
  },
  contingency: {
    signal: 'danger', labelKey: 'mode.contingency', descKey: 'mode.contingency.desc'
  },
  babel: {
    signal: 'accent', labelKey: 'mode.babel', descKey: 'mode.babel.desc'
  }
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
  // DOWN 停摆（2026-09-16）：不是调度档位，是「调度不工作」本身——判据在 down.chr
  // （见 data/down.ts），同时会被写进 current_mode.chr。显示名就是 id，不分语言
  if (mode === DOWN_WORD) {
    return {
      id: mode,
      kind: 'down',
      signal: 'danger',
      labelKey: 'mode.down',
      descKey: 'mode.down.desc'
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
  const lab = LAB_CATALOG[mode]
  if (lab) {
    return { id: mode, kind: 'lab', signal: lab.signal, labelKey: lab.labelKey, descKey: lab.descKey }
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
export const CLG_MODE_IDS = ['reduce', 'default', 'boost', 'vector'] as const
