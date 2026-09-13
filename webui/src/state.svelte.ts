// state.svelte.ts: [common] [overview] [config] [apps] [logs]
// 应用级状态：把契约层的三分类读取结果映射成界面可直接渲染的状态，
// 并保证「读失败」与「不适用」在界面上是不同的表达。
import {
  hasActionScript,
  hasFlock,
  probeLiveness,
  stopScheduler,
  type DaemonState
} from '@/contract/daemon'
import { readMeta, writeMetaFields, type MetaSnapshot, type WritableField } from '@/contract/meta'
import {
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
  flockAvailable = $state(true)
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
    } else if (meta.kind === 'absent') {
      this.metaSnapshot = null
      this.configState = 'missing'
      this.configError = ''
      this.configRel = ''
      this.metaPath = ''
    } else {
      this.configState = 'failed'
      this.configError = meta.error
    }
  }

  // [overview]
  async loadOverview(): Promise<void> {
    // in-flight 守卫：子组件 onMount 先于父组件执行，首次进入会被触发两次
    if (this.loading) return
    this.loading = true
    try {
      const [live, flock, modeRaw, tunedRaw, fasRaw, actionOk] = await Promise.all([
        probeLiveness(),
        hasFlock(),
        readCurrentModeRaw(),
        readSpecialTunedRaw(),
        readFasWhitelistRaw(),
        hasActionScript()
      ])

      this.flockAvailable = flock
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
      await this.loadCommon()
    } finally {
      this.loading = false
      this.ready = true
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

  // [logs]
  async loadLogs(source: 'daemon' | 'status' = 'daemon'): Promise<void> {
    this.logLoading = true
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
        }
      }
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
