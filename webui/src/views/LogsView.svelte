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
    await app.loadLogs(source)
  }

  onMount(() => {
    void app.loadLogs(source)
  })
</script>

<div class="stack">
  <Panel title={t('logs.title')} desc={t('logs.window')}>
    {#snippet actions()}
      <button
        type="button"
        class="ak-button btn"
        disabled={app.logLoading}
        onclick={() => app.loadLogs(source)}
      >
        {app.logLoading ? t('state.loading') : t('action.refresh')}
      </button>
    {/snippet}

    <Segmented items={sourceItems} value={source} label={t('logs.source')} onselect={pick} />

    {#if source === 'daemon'}
      <div class="level">
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
        <header class="terminal__bar">
          <span class="terminal__path u-mono">logs/daemon.log</span>
          <span class="terminal__count u-mono">{t('logs.lines', { n: visible.length })}</span>
        </header>
        <div class="terminal__body">
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
      <Panel title="logd/" desc={t('logs.archive')}>
        <ul class="files">
          {#each app.logdFiles as file (file)}
            <li class="files__item u-mono">{file}</li>
          {/each}
        </ul>
      </Panel>
    {/if}

    {#if app.devimpFiles.length > 0}
      <Panel title="devimp/" desc={t('config.devRecord.hint')}>
        <ul class="files">
          {#each app.devimpFiles.slice(-8) as file (file)}
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
      <section class="snapshot" aria-label={t('logs.source.status')}>
        <div class="snapshot__row snapshot__row--head u-mono">
          <span>{t('logs.status.times')}</span>
          <span>{t('logs.status.mode')}</span>
          <span>{t('logs.status.pkg')}</span>
          <span>{t('logs.status.batt')}</span>
          <span>{t('logs.status.load')}</span>
        </div>
        {#each app.statusRowsNewestFirst as row, index (index)}
          <div class="snapshot__row u-mono">
            <span>{row.timestamp}</span>
            <span>{row.mode || '—'}</span>
            <span class="u-truncate">{row.pkg || '—'}</span>
            <span>{fmt(row.battTemp, 1, t('unit.celsius'))}</span>
            <span>{fmt(row.gpuBusy, 0, t('unit.percent'))}</span>
          </div>
        {/each}
      </section>
    {/if}
  {/if}
</div>

<style>
  .stack {
    display: grid;
    gap: var(--ak-space-4);
  }

  .level {
    display: grid;
    gap: 0.5rem;
    margin-top: var(--ak-space-3);
  }

  .terminal {
    border: var(--ak-line-hairline) solid var(--ak-surface-raised);
    background: #0a0c0e;
  }

  .terminal__bar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--ak-space-3);
    padding: var(--ak-space-2) var(--ak-space-3);
    border-bottom: var(--ak-line-hairline) solid var(--ak-surface-raised);
    background: var(--ak-surface-panel);
  }

  .terminal__path,
  .terminal__count {
    color: var(--ak-text-secondary);
    font-size: 0.6875rem;
  }

  .terminal__body {
    max-height: 60vh;
    padding: var(--ak-space-3);
    overflow: auto;
    overscroll-behavior: contain;
  }

  .log {
    display: grid;
    grid-template-columns: auto auto minmax(0, 9rem) minmax(0, 1fr);
    gap: var(--ak-space-2);
    margin: 0 0 0.15rem;
    font-size: 0.6875rem;
    line-height: 1.55;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
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
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
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

  .snapshot {
    border: var(--ak-line-hairline) solid var(--ak-surface-raised);
    background: var(--ak-surface-panel);
    overflow: auto;
  }

  .snapshot__row {
    display: grid;
    grid-template-columns: 6.5rem 5.5rem minmax(6rem, 1fr) 5rem 4rem;
    gap: var(--ak-space-2);
    padding: var(--ak-space-2) var(--ak-space-3);
    border-bottom: var(--ak-line-hairline) solid var(--ak-surface-raised);
    font-size: 0.6875rem;
    line-height: 1.5;
  }

  .snapshot__row--head {
    position: sticky;
    top: 0;
    color: var(--ak-text-secondary);
    background: var(--ak-surface-raised);
    font-weight: 700;
    letter-spacing: 0.06em;
  }

  .files {
    display: grid;
    gap: 0.25rem;
    margin: 0;
    padding: 0;
    list-style: none;
  }

  .files__item {
    color: var(--ak-text-secondary);
    font-size: 0.6875rem;
    line-height: 1.6;
    overflow-wrap: anywhere;
  }

  /* 窄屏重排而不是让终端/表格横向滚动：模块列信息优先级最低，先让位 */
  @media (max-width: 30rem) {
    .log {
      grid-template-columns: auto auto minmax(0, 1fr);
    }

    .log__module {
      display: none;
    }

    .snapshot__row {
      grid-template-columns: 6rem 4.5rem minmax(0, 1fr) 4.5rem;
    }

    .snapshot__row > span:nth-child(5) {
      display: none;
    }
  }
</style>
