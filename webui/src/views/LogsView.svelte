<script lang="ts">
  // LogsView.svelte: [source] [terminal] [snapshot]
  // 两类数据源都只读尾部窗口（daemon.log 单文件上限 50MB、status.csv 8MB），
  // 界面上明确写出「最近一段」，并把归档位置讲清楚。
  import { onMount } from 'svelte'
  import Panel from '@/components/Panel.svelte'
  import Segmented from '@/components/Segmented.svelte'
  import StateBox from '@/components/StateBox.svelte'
  import { filterByLevel, type LogLevelName } from '@/data/daemon-log'
  import { t } from '@/i18n/index.svelte'
  import { app } from '@/state.svelte'

  let source = $state<'daemon' | 'status'>('daemon')
  let level = $state<LogLevelName>('TRACE')

  const sourceItems = $derived([
    { id: 'daemon', label: t('logs.source.daemon') },
    { id: 'status', label: t('logs.source.status') }
  ])

  const levelItems = $derived([
    { id: 'TRACE', label: t('logs.level.all') },
    { id: 'DEBUG', label: 'DEBUG' },
    { id: 'INFO', label: 'INFO' },
    { id: 'WARN', label: 'WARN' },
    { id: 'ERROR', label: 'ERROR' }
  ])

  const visible = $derived(filterByLevel(app.logLines, level))

  function fmt(value: number | null, digits: number, suffix = ''): string {
    return value === null ? '—' : `${value.toFixed(digits)}${suffix}`
  }

  async function pick(id: string) {
    source = id === 'status' ? 'status' : 'daemon'
    // 切换数据源视为重新进入视图：恢复跟随（进入即看最新）
    followDaemon = true
    followStatus = true
    await app.loadLogs(source)
  }

  /** charge 列固定短词 → 文案键（未知/缺测显示 —） */
  const CHARGE_KEYS: Record<string, string> = {
    charging: 'logs.charge.charging',
    discharging: 'logs.charge.discharging',
    full: 'logs.charge.full',
    not_charging: 'logs.charge.not_charging'
  }

  function chargeLabel(value: string): string {
    const key = CHARGE_KEYS[value]
    return key ? t(key) : '—'
  }

  // [follow] 两个子滚动窗口共用「跟随底部」逻辑：新数据到达时若处于跟随态则
  // 自动滚到底；用户一旦滑离底部即退出跟随（滚回底也不会自动恢复）——只能点
  // 「回到底部」恢复。避免用户回看历史时被每秒刷新反复拽到底部。
  let daemonBody = $state<HTMLDivElement>()
  let snapshotBody = $state<HTMLDivElement>()
  let followDaemon = $state(true)
  let followStatus = $state(true)

  /** 距底 ≤ 24px 视为「在底部」：程序性滚底也会触发 scroll 事件，不能被当成用户滑动 */
  function atBottom(el: HTMLElement): boolean {
    return el.scrollHeight - el.scrollTop - el.clientHeight <= 24
  }

  function onDaemonScroll() {
    if (daemonBody && !atBottom(daemonBody)) followDaemon = false
  }

  function onStatusScroll() {
    if (snapshotBody && !atBottom(snapshotBody)) followStatus = false
  }

  /** 回到底部并恢复跟随（滑动后唯一的恢复途径） */
  function backToBottom() {
    if (source === 'daemon') {
      followDaemon = true
      if (daemonBody) daemonBody.scrollTop = daemonBody.scrollHeight
    } else {
      followStatus = true
      if (snapshotBody) snapshotBody.scrollTop = snapshotBody.scrollHeight
    }
  }

  // 每秒自刷新数据（不重载页面）；loadLogs 内部有在飞守卫，轮询不会堆积
  onMount(() => {
    void app.loadLogs(source)
    const timer = setInterval(() => {
      void app.loadLogs(source)
    }, 1000)
    return () => clearInterval(timer)
  })

  // 跟随滚底：$effect 在 DOM 更新后运行。依赖**整个数组**而不是长度——尾部窗口
  // 行数可能恒定（滚动窗口），只有内容替换时也必须滚到底
  $effect(() => {
    void visible
    if (followDaemon && daemonBody) daemonBody.scrollTop = daemonBody.scrollHeight
  })

  $effect(() => {
    void app.statusRows
    if (followStatus && snapshotBody) snapshotBody.scrollTop = snapshotBody.scrollHeight
  })
</script>

<div class="u-stack">
  <!-- 副标题已按需求注释（2026-09-17）：desc={t('logs.window')} -->
  <Panel title={t('logs.title')}>

    <Segmented items={sourceItems} value={source} label={t('logs.source')} onselect={pick} />

    {#if source === 'daemon'}
      <div class="ak-field u-mt-3">
        <span class="ak-label">{t('logs.level')}</span>
        <Segmented items={levelItems} value={level} onselect={id => (level = id as LogLevelName)} />
      </div>
    {/if}
  </Panel>

  {#if source === 'daemon'}
    {#if app.logState === 'failed'}
      <StateBox kind="error" message={t('state.failed')} detail={app.logError} />
    {:else if app.logState === 'missing'}
      <StateBox kind="missing" message={t('logs.missing')} detail={t('logs.archive')} />
    {:else}
      <section class="terminal" data-ak-ui="terminal" aria-label={t('logs.source.daemon')}>
        <header class="terminal__bar u-between">
          <span class="terminal__path u-mono">logs/daemon.log</span>
          <span class="terminal__count u-mono">{t('logs.lines', { n: visible.length })}</span>
        </header>
        <div
          class="terminal__body u-scroll"
          bind:this={daemonBody}
          onscroll={onDaemonScroll}
        >
          {#each visible as line, index (index)}
            <p class="log" data-level={line.level}>
              <span class="log__time u-mono">{line.time}</span>
              <span class="log__level u-mono">{line.level}</span>
              <span class="log__module u-mono">{line.module}</span>
              <span class="log__message">{line.message}</span>
            </p>
          {/each}
        </div>
      </section>
    {/if}

    {#if app.dirError}
      <StateBox kind="error" message={t('state.failed')} detail={app.dirError} />
    {/if}

    {#if app.logdFiles.length > 0}
      <!-- 副标题已按需求注释（2026-09-17）：desc={t('logs.archive')} -->
      <Panel title="logd/">
        <ul class="files files--scroll u-list-reset u-scroll">
          {#each app.logdFiles as file (file)}
            <li class="files__item u-mono">{file}</li>
          {/each}
        </ul>
      </Panel>
    {/if}

    {#if app.devimpFiles.length > 0}
      <!-- 副标题已按需求注释（2026-09-17）：desc={t('config.devRecord.hint')} -->
      <Panel title="devimp/">
        <ul class="files files--scroll u-list-reset u-scroll">
          {#each app.devimpFiles as file (file)}
            <li class="files__item u-mono">{file}</li>
          {/each}
        </ul>
      </Panel>
    {/if}
  {:else}
    {#if app.statusState === 'failed'}
      <StateBox kind="error" message={t('state.failed')} detail={app.statusError} />
    {:else if app.statusState === 'missing'}
      <StateBox
        kind="missing"
        message={app.isChiri ? t('logs.missing') : t('state.notApplicable')}
        detail={app.isChiri ? t('state.daemonStopped') : t('state.chiriOnly')}
      />
    {:else}
      <div
        class="snapshot-wrap u-scroll"
        bind:this={snapshotBody}
        onscroll={onStatusScroll}
      >
        <table class="snapshot" aria-label={t('logs.source.status')}>
          <thead>
            <tr class="snapshot__row snapshot__row--head u-mono">
              <th scope="col">{t('logs.status.times')}</th>
              <th scope="col">{t('logs.status.mode')}</th>
              <th scope="col">{t('logs.status.pkg')}</th>
              <th scope="col">{t('logs.status.batt')}</th>
              <th scope="col">{t('logs.status.load')}</th>
              <th scope="col">{t('logs.status.power')}</th>
              <th scope="col">{t('logs.status.charge')}</th>
            </tr>
          </thead>
          <tbody>
            <!-- 时间升序（旧上新下），与 daemon 终端一致：新行从底部进入并跟随 -->
            {#each app.statusRows as row, index (index)}
              <tr class="snapshot__row u-mono">
                <td>{row.timestamp}</td>
                <td>{row.mode || '—'}</td>
                <td>{row.pkg || '—'}</td>
                <td>{fmt(row.battTemp, 1, t('unit.celsius'))}</td>
                <td>{fmt(row.gpuBusy, 0, t('unit.percent'))}</td>
                <td>{fmt(row.battPower, 2, t('unit.watt'))}</td>
                <td>{chargeLabel(row.charge)}</td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    {/if}
  {/if}

  <!-- 子滚动窗口右下角回到底部（daemon 终端与状态快照共用；滑动后唯一恢复途径） -->
  {#if source === 'daemon' ? app.logState === 'ok' : app.statusState === 'ok'}
    <button
      type="button"
      class="to-bottom"
      aria-label={t('action.toBottom')}
      onclick={backToBottom}
    >
      ↓
    </button>
  {/if}
</div>

<style>
  .terminal {
    border: var(--ak-line-hairline) solid var(--ak-surface-raised);
    /* 日志终端的沉浸深底（比 canvas 更深）：无对应语义 token 的组件级变量，
       集中在此定义、不散落色值 */
    --terminal-surface: #0a0c0e;
    background: var(--terminal-surface);
  }

  /* 两端布局来自 .u-between，这里只管分隔线与底 */
  .terminal__bar {
    padding: var(--ak-space-2) var(--ak-space-3);
    border-bottom: var(--ak-line-hairline) solid var(--ak-surface-raised);
    background: var(--ak-surface-panel);
  }

  .terminal__path,
  .terminal__count {
    color: var(--ak-text-secondary);
    font-size: 0.6875rem;
  }

  /* 滚动行为来自 .u-scroll */
  .terminal__body {
    max-height: 60vh;
    padding: var(--ak-space-3);
  }

  .log {
    display: grid;
    /* 全列按内容自适应 + 不换行：长消息靠容器横向滑动查看 */
    grid-template-columns: auto auto auto minmax(0, max-content);
    gap: var(--ak-space-2);
    margin: 0 0 0.15rem;
    font-size: 0.6875rem;
    line-height: 1.55;
    white-space: nowrap;
  }

  .log__time {
    color: var(--ak-text-secondary);
  }

  .log__level {
    min-width: 3.4rem;
    color: var(--ak-text-secondary);
    font-weight: 700;
    letter-spacing: 0.06em;
  }

  .log__module {
    color: var(--ak-text-secondary);
  }

  .log__message {
    color: var(--ak-text-primary);
  }

  .log[data-level='ERROR'] .log__level,
  .log[data-level='ERROR'] .log__message {
    color: var(--ak-signal-danger);
  }

  .log[data-level='WARN'] .log__level,
  .log[data-level='WARN'] .log__message {
    color: var(--ak-signal-action);
  }

  .log[data-level='INFO'] .log__level {
    color: var(--ak-signal-info);
  }

  .log[data-level='DEBUG'] .log__level,
  .log[data-level='DEBUG'] .log__message {
    color: var(--ak-text-secondary);
  }

  .snapshot-wrap {
    /* 列宽随数据自适应（table auto 布局）；与 daemon 终端同款的子滚动窗口：
       限高 + 双轴滑动（配合「跟随底部」），滚动行为来自 .u-scroll */
    max-width: 100%;
    max-height: 60vh;
    border: var(--ak-line-hairline) solid var(--ak-surface-raised);
    background: var(--ak-surface-panel);
  }

  .snapshot {
    width: 100%;
    border-collapse: collapse;
  }

  .snapshot__row {
    white-space: nowrap;
    border-bottom: var(--ak-line-hairline) solid var(--ak-surface-raised);
    font-size: 0.6875rem;
    line-height: 1.5;
  }

  /* 内距必须落在 th/td 上：tr 的 padding 不参与表格布局（被忽略，列会糊在一起）；
   * 全部左对齐 */
  .snapshot th,
  .snapshot td {
    padding: var(--ak-space-2) var(--ak-space-3);
    text-align: left;
  }

  .snapshot__row--head {
    color: var(--ak-text-secondary);
    font-weight: 700;
    letter-spacing: 0.06em;
  }

  /* 粘性表头必须落在 th 上（tr 上的 sticky + 背景在滚动时不覆盖数据行） */
  .snapshot__row--head th {
    position: sticky;
    top: 0;
    z-index: 1;
    background: var(--ak-surface-raised);
  }

  /* 列表重置来自 .u-list-reset，滚动来自 .u-scroll */
  .files {
    display: grid;
    gap: 0.25rem;
  }

  /* 历史文件可能很多：限高 */
  .files--scroll {
    max-height: 12rem;
  }

  .files__item {
    color: var(--ak-text-secondary);
    font-size: 0.6875rem;
    line-height: 1.6;
    overflow-wrap: anywhere;
  }

  /* 右下角回到底部（避开底部导航） */
  .to-bottom {
    position: fixed;
    right: var(--ak-space-4);
    bottom: calc(var(--ak-space-6) + 4rem);
    z-index: 20;
    width: 2.75rem;
    height: 2.75rem;
    border: var(--ak-line-hairline) solid var(--ak-surface-raised);
    border-radius: 999px;
    background: var(--ak-surface-panel);
    color: var(--ak-text-primary);
    font-size: 1rem;
    box-shadow: var(--ak-shadow-panel);
  }
</style>
