// lab.ts: [keys] [takeover] [parse] [lock]
// 实验室（rhine）状态解析。rhine.chr 内部是 YAML 标量：内容就是模式 key，
// 空文件或只有注释行 = 未启用。
//
// 口径必须与守护进程 src/rhine.rs::parse_state 一致，否则会出现「界面说已启用、
// 守护进程判非法并重置回未启用」的裂缝——那种状态下用户看到的和实际跑的不一样。
// 两边都做三件事：剥空行与 # 注释、只接受单行标量、key 必须在内置定义里。

// [keys]
/**
 * rhine-init.yaml 里已定义的实验室模式。key 取的是它实际做的事（`vector` =
 * 全局模式切 vector），界面上的名字走 i18n `lab.mode.*`，两边不要混用。
 * 新增模式要同时改这里、locale、以及 rhine-init.yaml。
 */
export const LAB_MODE_KEYS = ['vector', 'contingency', 'babel', 'frozen'] as const
export type LabModeKey = (typeof LAB_MODE_KEYS)[number]

/**
 * 界面上放开启用的模式。frozen 仍是预留条目（rhine-init.yaml 空映射，启用无效果），
 * 不给入口避免点了没反应。
 */
export const LAB_ENABLEABLE: readonly LabModeKey[] = ['vector', 'contingency', 'babel']

// [takeover]
/**
 * 各实验室模式会**接管**的 meta 开关（字段名与 meta.yaml 一致）。被接管的项在配置页里
 * 置灰不可切换：实验室正按自己的定义写它，锁定期间手改会被守护进程的 `reassert` 拉回去，
 * 草稿保存出去只会「看起来生效」。
 *
 * 与 `rhine-init.yaml` 的影响项一一对应，**必须同步改**（WebUI 按约定不读那个文件，与
 * `LAB_MODE_KEYS` 一样硬编码）。`tests/lab.test.ts` 里有一条针对它的刚性断言兜底：
 * 改了 rhine-init.yaml 就得同步这张表，否则测试会红。
 */
export const LAB_TAKEOVER: Record<LabModeKey, readonly string[]> = {
  // 与 rhine-init.yaml 的各模式影响项一致：实验室期间 FAS/息屏场景模式被关闭
  // （global_mode/special_tuned 不是 meta 字段，不在接管范围内）
  vector: ['fas_enabled', 'scenemode_enabled'],
  contingency: ['fas_enabled', 'scenemode_enabled'],
  babel: ['fas_enabled', 'scenemode_enabled'],
  frozen: []
}

// [parse]
/**
 * 保留字：强制关闭。写进 rhine.chr 即表示「无视『关闭需重启』的风险」——守护进程
 * 会清掉锁定标记、按快照还原原值，随后把本文件写回未启用。与「写空内容」的区别
 * 就在锁定期间：空内容会被挡回去，`off` 不会。
 * 必须与守护进程 `src/rhine.rs::is_force_off` 同口径（忽略大小写、允许带引号）。
 */
export const LAB_FORCE_OFF = 'off'

export type LabState =
  | { kind: 'off' }
  | { kind: 'on'; mode: LabModeKey }
  /** 文件里写着保留字 off：强制关闭请求（守护进程收敛后会把文件写回未启用） */
  | { kind: 'force-off' }
  /** 内容不是合法标量或写了未定义的模式名——守护进程会把它重置掉 */
  | { kind: 'invalid'; raw: string }

/**
 * 剥 YAML 行内注释：`#` 前必须是行首或空白才算注释（`vector#x` 里 `#` 是标量的一部分）。
 * daemon 的模式名走 serde_yaml、注释由它剥；这里必须自己剥，否则 `vector # 注释` 会被
 * 判成非法，而守护进程其实已经把它当 vector 生效了。
 */
function stripComment(line: string): string {
  const at = line.search(/\s#/)
  return at >= 0 ? line.slice(0, at) : line
}

/**
 * 保留字判定：daemon 的 `is_force_off` 在 **YAML 解析之前**做字面量比较
 * （剥行内注释 → trim → 逐边剥引号 → 忽略大小写），所以这里不能套用 YAML 标量那套。
 */
function isForceOff(line: string): boolean {
  return stripComment(line)
    .trim()
    .replace(/^["']+|["']+$/g, '')
    .toLowerCase() === LAB_FORCE_OFF
}

/**
 * 模式名按 daemon 的 serde_yaml 口径取字符串标量：剥注释 → trim → **成对同型引号**
 * 剥一层（不 trim 内侧）。不成对的引号（`"vector'`）YAML 解析会失败、daemon 判非法，
 * 这里也必须判非法——否则又会出现「界面说已启用、守护进程判非法并重置」的裂缝。
 */
function yamlScalar(line: string): string | null {
  const s = stripComment(line).trim()
  const head = s.charAt(0)
  const tail = s.charAt(s.length - 1)
  if (s.length >= 2 && ((head === '"' && tail === '"') || (head === "'" && tail === "'"))) {
    return s.slice(1, -1)
  }
  if (head === '"' || head === "'" || tail === '"' || tail === "'") return null
  return s
}

export function parseLabState(text: string): LabState {
  const body = text
    .split('\n')
    .map(line => line.trim())
    .filter(line => line.length > 0 && !line.startsWith('#'))
  if (body.length === 0) return { kind: 'off' }
  const raw = body.join('\n')
  // 多行内容解析不成字符串标量，守护进程同样判非法
  if (body.length > 1) return { kind: 'invalid', raw }
  const line = body[0]
  // 保留字先判：它既不是模式 key 也不算非法内容（daemon 也是先做字面量比较，顺序不能反）
  if (isForceOff(line)) return { kind: 'force-off' }
  const key = yamlScalar(line)
  return key && (LAB_MODE_KEYS as readonly string[]).includes(key)
    ? { kind: 'on', mode: key as LabModeKey }
    : { kind: 'invalid', raw }
}

/** 可写进 rhine.chr 的三种目标：模式 key / 保留字 off / null（关闭，写空内容） */
export type LabWriteTarget = LabModeKey | typeof LAB_FORCE_OFF | null

/** 写入内容：启用写模式 key，未启用写空内容（与「正常模式下应为空」一致） */
export function labFileContent(mode: LabWriteTarget): string {
  return mode ? `${mode}\n` : ''
}

// [lock]
/**
 * 锁定标记：实验室启用后在 tmpfs 上落一个标记，存在即表示「本次开机后启用过」——
 * 运行时关不掉，只能重启设备（标记随重启消失）。
 * 文件名与候选目录必须与守护进程 `src/rhine.rs` 的 `LOCK_NAME` / `LOCK_DIRS` 一致，
 * 否则界面会说「未锁定」而设备其实锁着。
 */
export const LAB_LOCK_NAME = 'chiri-labs.lock'
export const LAB_LOCK_DIRS = ['/tmp', '/dev'] as const

export interface LabLock {
  /** 标记存在 */
  locked: boolean
  /** 标记第一行记录的模式（空串 = 没写或不是已知模式） */
  mode: string
  /** 异常痕迹（`#` 开头的行）：非空即表示检测到运行配置被改动过 */
  notes: string[]
}

/** 解析标记文件内容。`exists` 为 false 时（文件不存在）一律视为未锁定。 */
export function parseLabLock(text: string, exists: boolean): LabLock {
  if (!exists) return { locked: false, mode: '', notes: [] }
  const lines = text.split('\n')
  const first = (lines[0] ?? '').trim()
  const mode = (LAB_MODE_KEYS as readonly string[]).includes(first) ? first : ''
  const notes = lines
    .slice(1)
    .map(l => l.trim())
    .filter(l => l.startsWith('#'))
    .map(l => l.slice(1).trim())
    .filter(l => l.length > 0)
  return { locked: true, mode, notes }
}

/**
 * 运行配置一致性问题的稳定代号（界面层映射成 i18n 文案，便于单测）。
 * 这些检查只在**已锁定**时才有意义——未锁定时 rhine.chr 为空本来就是正常态。
 */
export type LabWarningCode =
  /** rhine.chr 内容不是合法标量：被手改坏过，守护进程会重置它 */
  | 'state-invalid'
  /** 锁定中但 rhine.chr 不是标记里那个模式：文件被清空或改成了别的写法 */
  | 'lock-drifted'
  /** 锁定中却没有 rhine-back.chr：原值快照丢了，关闭后不一定能完整还原 */
  | 'no-backup'

export function labWarnings(input: {
  lock: LabLock
  state: LabState
  hasBackup: boolean
}): LabWarningCode[] {
  const { lock, state, hasBackup } = input
  const out: LabWarningCode[] = []
  if (state.kind === 'invalid') out.push('state-invalid')
  if (!lock.locked) return out
  // 文件里写着 off = 用户刚请求强制关闭：锁定与快照都正在被守护进程收掉，
  // 此时「不一致」「没快照」都是预期中间态，报出来只会吓人一跳
  if (state.kind === 'force-off') return out
  if (state.kind !== 'on' || (lock.mode && state.mode !== lock.mode)) out.push('lock-drifted')
  if (!hasBackup) out.push('no-backup')
  return out
}
