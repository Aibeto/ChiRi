// state.svelte.ts: [common] [overview] [config] [apps] [logs]
// 应用级状态：把契约层的三分类读取结果映射成界面可直接渲染的状态，
// 并保证「读失败」与「不适用」在界面上是不同的表达。
import {
  hasActionScript,
  probeLiveness,
  stopScheduler,
  type DaemonState
} from '@/contract/daemon'
import { readDown, writeDown } from '@/contract/down'
import { readPowerAvg } from '@/contract/power'
import { pollExport, startExport } from '@/contract/export'
import { readLab, writeLabMode } from '@/contract/lab'
import { readMeta, writeMetaFields, type MetaSnapshot, type WritableField } from '@/contract/meta'
import {
  clearArchives,
  deviceKind,
  listDevimp,
  listLogd,
  readCurrentModeRaw,
  readDaemonLogTail,
  readFasWhitelistRaw,
  readModulePropRaw,
  readRulesRaw,
  readSpecialTunedRaw,
  readStatusCsvTail,
  type DeviceKind
} from '@/contract/sources'
import { fetchInstalledPackages, buildAppEntries, filterApps, type AppEntry } from '@/data/apps'
import { parseDaemonLog, type LogLine } from '@/data/daemon-log'
import {
  LAB_FORCE_OFF,
  LAB_MODE_KEYS,
  LAB_TAKEOVER,
  labWarnings,
  type LabLock,
  type LabModeKey,
  type LabState,
  type LabWarningCode,
  type LabWriteTarget
} from '@/data/lab'
import { describeMode, type ModeInfo } from '@/data/mode'
import { EMPTY_MODULE_PROP, parseModuleProp, type ModuleProp } from '@/data/module-info'
import { EMPTY_RULES, parseRules, type RulesInfo } from '@/data/rules'
import { parseStatusCsv, type StatusRow } from '@/data/status-csv'
import {
  parseFasWhitelist,
  parseSpecialTuned,
  specialModeSet,
  type SpecialTunedEntry
} from '@/data/whitelists'
import { t } from '@/i18n/index.svelte'
import { toast } from '@/kernel/ksu'

/** 只读文件的界面态：缺失（不适用/尚未生成）与失败要分开 */
export type FileState = 'ok' | 'missing' | 'failed'

class AppStore {
  // [common]
  ready = $state(false)
  loading = $state(false)
  /** 当前生效配置相对路径（active_config.chr） */
  configRel = $state('')
  /** 生效配置绝对路径 */
  metaPath = $state('')
  configState = $state<FileState>('ok')
  configError = $state('')
  deviceKind = $state<DeviceKind>('unknown')
  moduleProp = $state<ModuleProp>(EMPTY_MODULE_PROP)

  // [overview]
  daemonState = $state<DaemonState>('unknown')
  daemonError = $state('')
  currentMode = $state('')
  modeInfo = $state<ModeInfo>(describeMode(''))
  modeMissing = $state(false)
  /** current_mode.chr 读取「真实失败」——与「尚未生成」分开，界面上排查方向不同 */
  modeError = $state('')
  /** 特调 / FAS 白名单读取真实失败 */
  whitelistError = $state('')
  actionAvailable = $state(false)
  metaValid = $state(true)
  metaProblems = $state<string[]>([])
  metaSnapshot = $state<MetaSnapshot | null>(null)
  specialCount = $state(0)
  fasCount = $state(0)
  stopping = $state(false)

  // [config]
  saving = $state(false)
  writeError = $state('')
  writeReverted = $state(false)
  /** 待保存的改动（页面上未提交的草稿） */
  draft = $state<Partial<Record<WritableField, string | boolean>>>({})

  // [lab]
  /** rhine.chr 的解析结果（off / on / invalid），未读到前保持 off */
  labState = $state<LabState>({ kind: 'off' })
  /** rhine.chr 绝对路径 */
  labPath = $state('')
  /** 锁定标记（tmpfs 上）：存在就表示本次开机后启用过，运行时关不掉 */
  labLock = $state<LabLock>({ locked: false, mode: '', notes: [] })
  /** rhine-back.chr 是否存在（原值快照 = 还原能力） */
  labHasBackup = $state(false)
  labError = $state('')
  labLoading = $state(false)
  /** 正在写入（启用/关闭）——期间禁用按钮，避免重复提交 */
  labPending = $state(false)

  // [apps]
  apps = $state<AppEntry[]>([])
  appKeyword = $state('')
  scanning = $state(false)
  appsState = $state<FileState>('ok')
  rules = $state<RulesInfo>(EMPTY_RULES)

  // [logs]
  logLines = $state<LogLine[]>([])
  logState = $state<FileState>('missing')
  logError = $state('')
  statusRows = $state<StatusRow[]>([])
  statusState = $state<FileState>('missing')
  statusError = $state('')
  devimpFiles = $state<string[]>([])
  logdFiles = $state<string[]>([])
  /** devimp / logd 目录列举的真实失败（目录不存在是正常缺失，不算失败） */
  dirError = $state('')
  logLoading = $state(false)
  /** 自刷新失败提示的节流标志：失败 toast 一次，成功后复位（每秒轮询不能刷屏） */
  refreshFailNotified = false

  /** 自刷新失败时的 toast（同一次失败串只提示一次） */
  notifyRefreshFail(): void {
    if (this.refreshFailNotified) return
    this.refreshFailNotified = true
    toast(t('state.refreshFailed'))
  }

  // [derived]
  /** 只有明确判定为 ChiRi 机型时才为真（无法判定时不为真，界面据此显示“无法判定”） */
  get isChiri(): boolean {
    return this.deviceKind === 'chiri'
  }

  get filteredApps(): AppEntry[] {
    return filterApps(this.apps, this.appKeyword)
  }

  get statusRowsNewestFirst(): StatusRow[] {
    return [...this.statusRows].reverse()
  }

  // [common]
  /** 设备信息与生效配置（所有页面都需要） */
  async loadCommon(): Promise<void> {
    const [kind, propRaw, meta] = await Promise.all([
      deviceKind(),
      readModulePropRaw(),
      readMeta()
    ])
    this.deviceKind = kind
    if (propRaw.kind === 'ok') this.moduleProp = parseModuleProp(propRaw.value)

    if (meta.kind === 'ok') {
      this.metaSnapshot = meta.value
      this.metaPath = meta.value.path
      this.configRel = meta.value.rel
      this.metaValid = meta.value.valid
      this.metaProblems = meta.value.problems
      this.configState = 'ok'
      this.configError = ''
      this.refreshFailNotified = false
    } else if (meta.kind === 'absent') {
      this.metaSnapshot = null
      this.configState = 'missing'
      this.configError = ''
      this.configRel = ''
      this.metaPath = ''
    } else {
      this.configState = 'failed'
      this.configError = meta.error
      this.notifyRefreshFail()
    }
  }

  // [overview]
  /** 在飞的 overview 请求：并发调用共享同一个 promise */
  private overviewJob: Promise<void> | null = null

  async loadOverview(): Promise<void> {
    // 并发调用共享在飞请求（子组件 onMount 先于父组件执行，首次进入会被触发两次）。
    // 不能简单 `if (this.loading) return`：后来者会立刻拿到「还没加载完」的空状态，
    // 例如 isChiri 仍是 false → 依赖它的页面会静默跳过（接管置灰就是这么失效的）
    if (this.overviewJob) return this.overviewJob
    this.loading = true
    this.overviewJob = (async () => {
      try {
      const [live, modeRaw, tunedRaw, fasRaw, actionOk, powerAvg] = await Promise.all([
        probeLiveness(),
        readCurrentModeRaw(),
        readSpecialTunedRaw(),
        readFasWhitelistRaw(),
        hasActionScript(),
        readPowerAvg()
      ])

      if (live.kind === 'ok') {
        this.daemonState = live.value
        this.daemonError = ''
      } else if (live.kind === 'absent') {
        this.daemonState = 'unknown'
        this.daemonError = t('state.unsupportedEnv')
      } else {
        this.daemonState = 'unknown'
        this.daemonError = live.error
      }

      this.modeMissing = modeRaw.kind === 'absent'
      this.modeError = modeRaw.kind === 'failed' ? modeRaw.error : ''
      const mode = modeRaw.kind === 'ok' ? modeRaw.value : ''

      this.whitelistError =
        tunedRaw.kind === 'failed'
          ? tunedRaw.error
          : fasRaw.kind === 'failed'
            ? fasRaw.error
            : ''
      const special: Map<string, SpecialTunedEntry> =
        tunedRaw.kind === 'ok' ? parseSpecialTuned(tunedRaw.value) : new Map()
      const fas: Map<string, string> =
        fasRaw.kind === 'ok' ? parseFasWhitelist(fasRaw.value) : new Map()
      const modes = specialModeSet(special)
      this.specialCount = special.size
      this.fasCount = fas.size

      this.currentMode = mode
      this.modeInfo = describeMode(mode, modes)
      this.actionAvailable = actionOk
      // 功耗参考/平均值（W）：缺失/为空/非法统一按「无值」显示 —，不打断其它数据
      this.powerAvgWatt = powerAvg.kind === 'ok' ? powerAvg.value.watt : null
      this.powerAvgMissing = powerAvg.kind === 'ok' ? powerAvg.value.missing : false
      // 没有取值时把「为什么没有」一并算出来：界面只显示 — 会让人以为是坏的
      this.powerStaleReason = this.powerAvgWatt === null ? await this.powerStaleWhy() : ''
      // 当前功耗（status.csv 末行 batt_power_w）：模式卡右侧「当前功耗」块的数据源；
      // 文件缺失/无行/读数缺失一律 null（显示 —，不打断其它数据）
      if (this.isChiri) {
        const tail = await readStatusCsvTail(4096)
        const rows = tail.kind === 'ok' ? parseStatusCsv(tail.value) : []
        this.powerNowWatt = rows[rows.length - 1]?.battPower ?? null
      }
      await this.loadCommon()
      } finally {
        this.loading = false
        this.ready = true
      }
    })()
    try {
      await this.overviewJob
    } finally {
      this.overviewJob = null
    }
  }

  async refreshCommon(): Promise<void> {
    await this.loadCommon()
  }

  // [config]
  /** 草稿：合并多个字段后一次提交（每次写入都会触发守护进程全量热重载） */
  setDraft(field: WritableField, value: string | boolean): void {
    this.draft = { ...this.draft, [field]: value }
    this.writeError = ''
  }

  discardDraft(): void {
    this.draft = {}
    this.writeError = ''
    this.writeReverted = false
  }

  get hasDraft(): boolean {
    return Object.keys(this.draft).length > 0
  }

  async commitDraft(): Promise<boolean> {
    if (!this.hasDraft || this.saving) return false
    this.saving = true
    this.writeError = ''
    this.writeReverted = false
    try {
      // 期望值 = 写入前的字段值 + 本次草稿；回读后若有字段不等于期望值，
      // 说明守护进程判定文件非法并整体重置（界面必须如实说“已恢复默认值”）
      const patchKeys = Object.keys(this.draft) as WritableField[]
      const expected: Record<string, unknown> = {}
      for (const key of patchKeys) expected[key] = this.draft[key]

      const result = await writeMetaFields(this.draft)
      if (result.kind !== 'ok') {
        // 失败要分清楚：真实失败、文件尚未生成、当前环境无法访问设备
        this.writeError =
          result.kind === 'failed'
            ? result.error
            : result.reason === 'unsupported-env'
              ? t('state.unsupportedEnv')
              : t('config.missing')
        toast(this.writeError)
        return false
      }
      this.metaSnapshot = result.value
      this.metaValid = result.value.valid
      this.metaProblems = result.value.problems
      this.metaPath = result.value.path
      this.configRel = result.value.rel
      this.draft = {}
      this.writeReverted = patchKeys.some(key => result.value.values[key] !== expected[key])
      toast(t('config.saved'))
      return true
    } finally {
      this.saving = false
    }
  }

  // [apps]
  async loadApps(rescan = false): Promise<void> {
    // in-flight 守卫：连点「重新扫描」时避免并发两组扫描互相复位标志
    if (this.scanning) return
    this.scanning = true
    try {
      const [packages, rulesRaw, tunedRaw, fasRaw] = await Promise.all([
        fetchInstalledPackages(),
        readRulesRaw(),
        readSpecialTunedRaw(),
        readFasWhitelistRaw()
      ])
      this.rules =
        rulesRaw.kind === 'ok'
          ? parseRules(rulesRaw.value)
          : rulesRaw.kind === 'failed'
            ? { ...EMPTY_RULES, problem: rulesRaw.error }
            : { ...EMPTY_RULES, problem: t('logs.missing') }
      const special = tunedRaw.kind === 'ok' ? parseSpecialTuned(tunedRaw.value) : new Map()
      const fas = fasRaw.kind === 'ok' ? parseFasWhitelist(fasRaw.value) : new Map()
      this.appsState = packages.length > 0 ? 'ok' : 'missing'
      this.apps = buildAppEntries(packages, {
        specialTuned: special,
        fasWhitelist: fas,
        appModes: this.rules.appModes
      })
      if (rescan) toast(t('apps.scanned'))
    } finally {
      this.scanning = false
    }
  }

  // [lab]
  /** 当前启用的实验室模式（未启用 / 内容非法时为 null） */
  get labMode(): LabModeKey | null {
    return this.labState.kind === 'on' ? this.labState.mode : null
  }

  /** rhine.chr 读到的内容不是合法标量——守护进程会把它重置为未启用 */
  get labInvalid(): boolean {
    return this.labState.kind === 'invalid'
  }

  /** 锁定中：关闭实验室需要重启设备，界面直接禁用关闭入口 */
  get labLocked(): boolean {
    return this.labLock.locked
  }

  /**
   * 当前实验室模式接管的 meta 开关（配置页据此置灰不可切换）；实验室未启用时为空。
   * 优先看 rhine.chr 里的模式；文件读不出模式（内容非法 / 强制关闭中间态 / 读失败）
   * 但**锁定标记里还记着模式**时按标记推导——那一刻守护进程正是按标记里的模式在
   * reassert，置灰一松，用户改的开关马上会被写回去。
   */
  get labTakeover(): readonly string[] {
    const fromFile = this.labMode
    const fromLock = (LAB_MODE_KEYS as readonly string[]).includes(this.labLock.mode)
      ? (this.labLock.mode as LabModeKey)
      : null
    const key = fromFile ?? fromLock
    return key ? LAB_TAKEOVER[key] : []
  }

  /** 运行配置一致性问题的代号（界面映射成文案） */
  get labWarnings(): LabWarningCode[] {
    return labWarnings({
      lock: this.labLock,
      state: this.labState,
      hasBackup: this.labHasBackup
    })
  }

  /** 把契约层读回来的快照写进状态（loadLab 与强制关闭的收敛轮询共用） */
  private applyLabSnapshot(snap: {
    state: LabState
    path: string
    lock: LabLock
    hasBackup: boolean
  }): void {
    this.labState = snap.state
    this.labPath = snap.path
    this.labLock = snap.lock
    this.labHasBackup = snap.hasBackup
  }

  /** 在飞的 lab 读请求：并发调用共享同一个 promise（理由同 loadOverview） */
  private labJob: Promise<void> | null = null

  async loadLab(): Promise<void> {
    if (this.labJob) return this.labJob
    this.labLoading = true
    this.labJob = (async () => {
      try {
        const result = await readLab()
        if (result.kind === 'ok') {
          this.applyLabSnapshot(result.value)
          this.labError = ''
        } else {
          // 文件不存在按未启用处理（契约层已归一），走到这里只剩环境不可用
          this.labError = result.kind === 'failed' ? result.error : t('state.unsupportedEnv')
        }
      } finally {
        this.labLoading = false
      }
    })()
    try {
      await this.labJob
    } finally {
      this.labJob = null
    }
  }

  /** 启用（传 key）或关闭（传 null）实验室。写后回读，状态以文件实际内容为准。 */
  async setLabMode(mode: LabModeKey | null): Promise<void> {
    // 锁定期间关不掉：守护进程会把 rhine.chr 原样写回去，这里先挡住，别让界面假装成功。
    // 确实要在锁定时关掉，走下面的 forceDisableLab（写保留字 off）。
    if (mode === null && this.labLocked) {
      this.labError = t('lab.lock.denied')
      toast(this.labError)
      return
    }
    await this.commitLab(mode, mode ? t('lab.toast.enabled') : t('lab.toast.disabled'))
  }

  /**
   * 强制关闭实验室（无视「关闭需重启」的风险）：写保留字 off。
   * 守护进程收到后会清掉锁定标记、按快照还原原值、把 rhine.chr 写回未启用，
   * 也就是「恢复原地调度」。
   */
  async forceDisableLab(): Promise<void> {
    // 等待窗口交给 commitLab：它必须落在 pending 区间内，否则这段时间按钮会复活、
    // 和守护进程的收敛抢着写 rhine.chr
    await this.commitLab(LAB_FORCE_OFF, t('lab.toast.forceOff'), 400)
  }

  /**
   * 写 rhine.chr 并刷新相关状态（启用/关闭/强制关闭三个入口共用）。
   * `settleMs` > 0 时写后多等一会儿再回读：守护进程处理 off 要走「清标记 → 还原 meta
   * → 写回未启用」，立刻读会停在中间态；等待期间 pending 保持 true。
   */
  async commitLab(mode: LabWriteTarget, done: string, settleMs = 0): Promise<void> {
    if (this.labPending) return
    this.labPending = true
    this.labError = ''
    try {
      const result = await writeLabMode(mode)
      if (result.kind !== 'ok') {
        this.labError = result.kind === 'failed' ? result.error : t('state.unsupportedEnv')
        toast(this.labError)
        return
      }
      this.labState = result.value.state
      this.labPath = result.value.path
      this.labLock = result.value.lock
      this.labHasBackup = result.value.hasBackup
      // 被实验室接管的开关若还留着草稿，保存出去也会被守护进程按定义写回去——
      // 直接清掉，别让「待提交」标记在配置页骗人
      for (const field of this.labTakeover) delete this.draft[field as WritableField]
      // 实验室会改写 meta.yaml 的 fas/scenemode/thread_bind 开关，配置页那份快照已过期
      await this.refreshCommon()
      if (settleMs > 0) {
        // 等守护进程把 off 收敛完（清标记 → 还原 meta → 写回未启用）：固定等一次不够，
        // 慢文件系统上可能还没收敛，界面就会停在「已提交强制关闭」、按钮还会复活去和
        // 守护进程抢写 rhine.chr。这里直接读文件（绕过 labJob 守卫）轮询到不再是
        // force-off 或超时为止。
        const deadline = Date.now() + settleMs * 10
        for (;;) {
          await new Promise(resolve => setTimeout(resolve, settleMs))
          const snap = await readLab()
          if (snap.kind === 'ok') {
            this.applyLabSnapshot(snap.value)
            if (snap.value.state.kind !== 'force-off') break
          }
          if (Date.now() >= deadline) break
        }
        await this.refreshCommon()
      }
      toast(done)
    } finally {
      this.labPending = false
    }
  }

  // [down]
  /** DOWN 停摆：调度关停全部调度功能（CLG/特调/FAS/锁频/线程摆放/core_ctl），只留采集与日志 */
  downActive = $state(false)
  /** down.chr 绝对路径 */
  downPath = $state('')
  downError = $state('')
  /** 正在写入（切换期间禁用开关，避免重复提交） */
  downPending = $state(false)

  // [powerAvg]
  /** 功耗参考/平均值（W）：null = PowerAVG.chr 缺失/为空/非法（显示 —） */
  powerAvgWatt = $state<number | null>(null)
  /** 口径开关写入中（高级设置直写 meta.yaml） */
  /** 文件缺失/为空（daemon 未运行过）：界面红字提示，不静默显示 — */
  powerAvgMissing = $state(false)
  /** 没有取到值时的原因文案（空 = 说不清，界面回退到 overview.power.missing） */
  powerStaleReason = $state('')
  powerAvgPending = $state(false)
  powerAvgError = $state('')
  /** PowerBase 开关写入中的状态（高级设置） */
  powerbasePending = $state(false)
  powerbaseError = $state('')
  /** 当前功耗（W，status.csv 最后一行的 batt_power_w，1s 采样）：null = 无行/无读数，显示 — */
  powerNowWatt = $state<number | null>(null)

  /** 功耗口径：是否使用累计平均值（meta.yaml `power_avg`，默认 false = 参考值） */
  get powerAvgUsesAverage(): boolean {
    return this.metaSnapshot?.values?.power_avg === true
  }

  /** PowerBase 是否开启（meta.yaml `powerbase_enabled`，默认 false = 由 CLG 调频） */
  get powerbaseEnabled(): boolean {
    return this.metaSnapshot?.values?.powerbase_enabled === true
  }

  // [powerMax]
  /** 耗电仪表盘满量程（W）：meta.yaml 可选字段 `power_max_w`，缺省/越界/非法一律按 12。
   *  只在 meta.yaml 手改（高级设置不提供输入框），此处仅供仪表盘换算读取 */
  get powerMaxWatt(): number {
    const v = this.metaSnapshot?.values?.power_max_w
    return typeof v === 'number' && Number.isFinite(v) && v > 0 && v <= 200 ? v : 12
  }

  /**
   * PowerAVG 没取到值时，说明原因（空串 = 说不清，界面回退到 overview.power.missing）。
   * 判据来自 status.csv 最后一行（daemon 每秒一行，含 charge 与 screen_on）：
   *   ① 非放电（充电 / 充满 / 未充电 / 状态不可识别）→ 按口径不取样；
   *   ② 平均模式且息屏 → 不取样（平均口径＝亮屏放电）；
   *   ③ 放电且屏幕条件满足却仍为空 → 电压/电流读数不可用（或 daemon 还没写过一行）。
   */
  private async powerStaleWhy(): Promise<string> {
    const tail = await readStatusCsvTail(4096)
    if (tail.kind !== 'ok') return ''
    const rows = parseStatusCsv(tail.value)
    const last = rows[rows.length - 1]
    if (!last) return ''
    if (last.charge !== 'discharging') {
      const known = ['charging', 'full', 'not_charging']
      const name = known.includes(last.charge) ? t(`logs.charge.${last.charge}`) : last.charge
      return t('overview.power.reason.charge', { state: name })
    }
    if (this.powerAvgUsesAverage && !last.screenOn) return t('overview.power.reason.screen')
    return t('overview.power.reason.noreading')
  }

  // [batteryRead]
  /** 电池读数选项（meta.yaml 的 5 个字段，电池读数二级页直写）：读快照，写入走 setBatteryFields */
  get oplusChg(): boolean {
    return this.metaSnapshot?.values?.oplus_chg === true
  }
  get oplusDualCell(): boolean {
    return this.metaSnapshot?.values?.oplus_dual_cell === true
  }
  get voltageDouble(): boolean {
    return this.metaSnapshot?.values?.voltage_double === true
  }
  get currentDouble(): boolean {
    return this.metaSnapshot?.values?.current_double === true
  }
  /** 电压校准除数：缺省/非法一律按 1000000（标准 Android ABI µV 口径，与 daemon
   * DEFAULT_UNIT_DIVISOR 同值）；OPlus 私有节点报 mV，安装脚本会写成 1000。
   * 旧键 unit_divisor 作为兜底 */
  get voltageDivisor(): number {
    const values = this.metaSnapshot?.values
    for (const v of [values?.voltage_divisor, values?.unit_divisor]) {
      if (typeof v === 'number' && Number.isFinite(v) && v > 0) return v
    }
    return 1_000_000
  }
  /** 电流校准除数：缺省/非法一律按 1000000（标准节点 µA → A；batt_power_w 按安培消费） */
  get currentDivisor(): number {
    const v = this.metaSnapshot?.values?.current_divisor
    return typeof v === 'number' && Number.isFinite(v) && v > 0 ? v : 1_000_000
  }
  /** 电池读数页写入中（直写 meta.yaml） */
  battPending = $state(false)
  /**
   * meta.yaml 直写互斥：写路径是「读-改-写」，两笔并发会各自读旧内容、后一笔把前一笔的
   * 字段覆盖掉（丢字段）。页面上的直写入口（setBatteryFields / setPowerAvg）共用这一把锁。
   */
  metaWritePending = $state(false)
  battError = $state('')

  async loadDown(): Promise<void> {
    if (this.downPending) return
    const result = await readDown()
    if (result.kind === 'ok') {
      this.downActive = result.value.active
      this.downPath = result.value.path
      this.downError = ''
    } else {
      // 文件不存在按未停摆处理（契约层已归一），走到这里只剩环境不可用
      this.downError = result.kind === 'failed' ? result.error : t('state.unsupportedEnv')
    }
  }

  /** 开启/解除 DOWN 停摆。写后回读，状态以 down.chr 实际内容为准 */
  async setDown(active: boolean): Promise<void> {
    if (this.downPending) return
    this.downPending = true
    this.downError = ''
    try {
      const result = await writeDown(active)
      if (result.kind !== 'ok') {
        this.downError = result.kind === 'failed' ? result.error : t('state.unsupportedEnv')
        toast(this.downError)
        return
      }
      this.downActive = result.value.active
      this.downPath = result.value.path
      toast(active ? t('config.down.on') : t('config.down.off'))
      // 停摆会改 current_mode.chr（写 down / 删掉让自愈写回），总览页那份快照已过期
      await this.loadOverview()
    } finally {
      this.downPending = false
    }
  }

  /**
   * 高级设置：功耗口径开关——直写 meta.yaml 的 `power_avg`（不走草稿，立即热重载）。
   * 关闭（false，默认）= 参考值（旧值×10 与新值按 10:1 加权递推，偏历史，含息屏）；
   * 开启 = 累计平均值（等权全史，仅亮屏放电样本）。
   * 写后回读，界面以实际落盘内容为准。
   */
  async setPowerAvg(useAverage: boolean): Promise<void> {
    if (this.powerAvgPending || this.metaWritePending) return
    this.powerAvgPending = true
    this.metaWritePending = true
    this.powerAvgError = ''
    try {
      const result = await writeMetaFields({ power_avg: useAverage })
      if (result.kind !== 'ok') {
        this.powerAvgError =
          result.kind === 'failed' ? result.error : t('state.unsupportedEnv')
        toast(this.powerAvgError)
        return
      }
      this.metaSnapshot = result.value
      this.metaValid = result.value.valid
      this.metaProblems = result.value.problems
      this.metaPath = result.value.path
      toast(useAverage ? t('overview.power.avg') : t('overview.power.ref'))
    } finally {
      this.powerAvgPending = false
      this.metaWritePending = false
    }
  }

  /**
   * 高级设置：PowerBase 开关——直写 meta.yaml 的 `powerbase_enabled`（不走草稿，立即热重载）。
   * 开启后原本由 CLG 接管的档位改由 PowerBase 以放电功耗为指标调频；模式名、规则与
   * 界面等外部接口保持不变（只换实现）。写后回读，界面以实际落盘内容为准。
   */
  async setPowerbase(enabled: boolean): Promise<void> {
    if (this.powerbasePending || this.metaWritePending) return
    this.powerbasePending = true
    this.metaWritePending = true
    this.powerbaseError = ''
    try {
      const result = await writeMetaFields({ powerbase_enabled: enabled })
      if (result.kind !== 'ok') {
        this.powerbaseError =
          result.kind === 'failed' ? result.error : t('state.unsupportedEnv')
        toast(this.powerbaseError)
        return
      }
      this.metaSnapshot = result.value
      this.metaValid = result.value.valid
      this.metaProblems = result.value.problems
      this.metaPath = result.value.path
      toast(enabled ? t('config.powerbase') : t('config.advanced'))
    } finally {
      this.powerbasePending = false
      this.metaWritePending = false
    }
  }

  /**
   * 电池读数页：直写 meta.yaml（不走草稿，立即热重载）。
   * 开启 OPlus 私有节点时把倍电压/倍电流一并清掉——两者互斥（daemon 侧也只认私有节点）；
   * 界面同时置灰这两项，这里再兜一次，避免留一个「开着但不生效」的开关。
   * 写后回读，界面以实际落盘内容为准。
   */
  async setBatteryFields(
    patch: Partial<{
      oplus_chg: boolean
      oplus_dual_cell: boolean
      voltage_double: boolean
      current_double: boolean
      voltage_divisor: number
      current_divisor: number
      power_max_w: number
    }>
  ): Promise<void> {
    if (this.battPending || this.metaWritePending) return
    this.battPending = true
    this.metaWritePending = true
    this.battError = ''
    try {
      // 强制清空排在展开之后：调用方就算多传 voltage_double: true，也写不出
      // 「私有节点开 + 倍压开」这种互斥组合（daemon 侧还会再判一次）
      const fields =
        patch.oplus_chg === true
          ? { ...patch, voltage_double: false, current_double: false }
          : patch
      const result = await writeMetaFields(fields)
      if (result.kind !== 'ok') {
        this.battError = result.kind === 'failed' ? result.error : t('state.unsupportedEnv')
        toast(this.battError)
        return
      }
      this.metaSnapshot = result.value
      this.metaValid = result.value.valid
      this.metaProblems = result.value.problems
      this.metaPath = result.value.path
    } finally {
      this.battPending = false
      this.metaWritePending = false
    }
  }

  // [export]
  /** 导出阶段：running 期间禁用按钮，done/failed 显示结果 */
  exportPhase = $state<'idle' | 'running' | 'done' | 'failed'>('idle')
  /** 成功后的产物绝对路径（设备不支持 gzip 时是未压缩 .tar） */
  exportTarget = $state('')
  exportError = $state('')
  /** 进度：已处理文件数 / 总数（来自 tar -v 行数）、已写入归档字节数 */
  exportDone = $state(0)
  exportTotal = $state(0)
  exportBytes = $state(0)

  /** 进度百分比：封顶 99，100 留给完成态。分母是文件数，文件少时粒度偏粗 */
  get exportPercent(): number {
    if (this.exportTotal <= 0) return 0
    return Math.min(99, Math.round((this.exportDone / this.exportTotal) * 100))
  }

  /** 已写入的兆字节（1 位小数） */
  get exportMb(): string {
    return (this.exportBytes / 1048576).toFixed(1)
  }

  /**
   * 导出历史归档到 /sdcard/Download。
   * 打包在设备上后台跑（gzip 压几百 MB 也要一会儿），这里只轮询产物，期间界面照常可用。
   */
  async startExport(): Promise<void> {
    if (this.exportPhase === 'running') return
    this.exportPhase = 'running'
    this.exportError = ''
    this.exportTarget = ''
    this.exportDone = 0
    this.exportTotal = 0
    this.exportBytes = 0
    const started = await startExport()
    if (started.kind !== 'ok') {
      this.exportPhase = 'failed'
      this.exportError =
        started.kind === 'failed'
          ? `${started.error} · ${t('overview.export.failed.hint')}`
          : t('state.unsupportedEnv')
      return
    }
    const job = started.value
    // 1.5s 一轮，最多 160 轮（4 分钟）：gzip -6 压几百 MB 通常几十秒到一两分钟量级
    let pollFails = 0
    for (let i = 0; i < 160; i++) {
      await new Promise(resolve => setTimeout(resolve, 1500))
      const probed = await pollExport(job)
      if (probed.kind !== 'ok') {
        // 单次探测失败多半是桥的瞬时错误（授权、回调丢失、WebView 切后台）——
        // 后台 tar 多半还在跑，连续 3 次才判死，别让一次「命令失败(1)」错杀导出
        pollFails++
        if (pollFails < 3) continue
        this.exportPhase = 'failed'
        const detail = probed.kind === 'failed' ? probed.error : t('state.unsupportedEnv')
        this.exportError = `${detail} · ${t('overview.export.failed.hint')}`
        return
      }
      pollFails = 0
      this.exportDone = probed.value.progress.done
      this.exportTotal = probed.value.progress.total
      this.exportBytes = probed.value.progress.bytes
      const phase = probed.value.phase
      if (phase === 'running') continue
      if (phase === 'done-gz' || phase === 'done-tar') {
        this.exportPhase = 'done'
        this.exportTarget = phase === 'done-gz' ? job.target : job.fallback
        toast(
          phase === 'done-gz' ? t('overview.export.done') : t('overview.export.doneTar')
        )
        return
      }
      this.exportPhase = 'failed'
      this.exportError =
        phase === 'empty'
          ? t('overview.export.empty')
          : phase === 'no-logd'
            ? t('overview.export.noDir')
            : t('overview.export.fail')
      return
    }
    this.exportPhase = 'failed'
    this.exportError = t('overview.export.timeout')
  }

  // [logs]
  /** 历史归档删除中（防重复点击） */
  archivePending = $state(false)
  /** 删除归档的真实失败（环境不可用 / rm 报错） */
  archiveError = $state('')

  /** 删除历史归档（logd/ 与 devimp/）：成功后刷新日志页的列表 */
  async deleteArchives(): Promise<void> {
    if (this.archivePending) return
    this.archivePending = true
    this.archiveError = ''
    try {
      const r = await clearArchives()
      if (r.kind !== 'ok') {
        this.archiveError = r.kind === 'failed' ? r.error : t('state.unsupportedEnv')
        return
      }
      await this.loadLogs()
    } finally {
      this.archivePending = false
    }
  }

  async loadLogs(source: 'daemon' | 'status' = 'daemon'): Promise<void> {
    if (this.logLoading) return // 自刷新轮询防堆积
    this.logLoading = true
    let failed = false
    try {
      if (source === 'daemon') {
        const [log, devimp, logd] = await Promise.all([
          readDaemonLogTail(),
          listDevimp(),
          listLogd()
        ])
        if (log.kind === 'ok') {
          this.logLines = parseDaemonLog(log.value)
          this.logState = 'ok'
          this.logError = ''
        } else if (log.kind === 'absent') {
          this.logLines = []
          this.logState = 'missing'
          this.logError = ''
        } else {
          this.logLines = []
          this.logState = 'failed'
          this.logError = log.error
          this.notifyRefreshFail()
        }
        this.dirError =
          devimp.kind === 'failed' ? devimp.error : logd.kind === 'failed' ? logd.error : ''
        this.devimpFiles = devimp.kind === 'ok' ? devimp.value : []
        this.logdFiles = logd.kind === 'ok' ? logd.value : []
      } else {
        const csv = await readStatusCsvTail()
        if (csv.kind === 'ok') {
          this.statusRows = parseStatusCsv(csv.value)
          this.statusState = this.statusRows.length > 0 ? 'ok' : 'missing'
          this.statusError = ''
        } else if (csv.kind === 'absent') {
          this.statusRows = []
          this.statusState = 'missing'
          this.statusError = ''
        } else {
          this.statusRows = []
          this.statusState = 'failed'
          this.statusError = csv.error
          this.notifyRefreshFail()
        }
      }
      if (!failed) this.refreshFailNotified = false
    } finally {
      this.logLoading = false
    }
  }

  // [actions]
  async stopDaemon(): Promise<void> {
    this.stopping = true
    try {
      const result = await stopScheduler()
      if (result.kind === 'ok') {
        toast(t('overview.stop.done'))
        // 关闭后重新探测：状态以实际观测为准，不凭假设
        await this.loadOverview()
      } else {
        const message = result.kind === 'failed' ? result.error : t('overview.stop.unavailable')
        toast(t('overview.stop.failed'))
        this.daemonError = message
      }
    } finally {
      this.stopping = false
    }
  }
}

export const app = new AppStore()
