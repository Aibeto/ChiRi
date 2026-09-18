<script lang="ts">
  // ConfigView.svelte: [header] [fields] [commit]
  // meta.yaml 的可写字段见 contract/meta.ts::WRITABLE_FIELDS；每次写入都会触发守护进程全量热重载，
  // 因此改动先落草稿、由「保存」一次性提交（单次读-改-写）。
  import { onMount } from 'svelte'
  import Panel from '@/components/Panel.svelte'
  import StateBox from '@/components/StateBox.svelte'
  import ToggleField from '@/components/ToggleField.svelte'
  import { LOG_LEVELS, LANGUAGES, type WritableField } from '@/contract/meta'
  import { t } from '@/i18n/index.svelte'
  import { go } from '@/router.svelte'
  import { app } from '@/state.svelte'

  const values = $derived((app.metaSnapshot?.values ?? {}) as Record<string, unknown>)

  function raw(field: string): string {
    const v = values[field]
    return typeof v === 'string' ? v.replace(/^["']|["']$/g, '').trim() : ''
  }

  function value(field: WritableField, fallback: string | boolean): string | boolean {
    const draft = app.draft[field]
    if (draft !== undefined) return draft
    const current = values[field]
    if (typeof current === 'boolean') return current
    if (typeof current === 'string') {
      // 守护进程允许带引号的标量，界面按去引号后的值比较
      const bare = current.replace(/^["']|["']$/g, '').trim()
      return field === 'loglevel' ? bare.toUpperCase() : bare
    }
    return fallback
  }

  const loglevel = $derived(String(value('loglevel', 'INFO')).toUpperCase())
  const daemonLang = $derived(String(value('language', 'zh')).toLowerCase())
  const devRecord = $derived(Boolean(value('dev_record', false)))
  const fasEnabled = $derived(Boolean(value('fas_enabled', true)))
  const scenemodeEnabled = $derived(Boolean(value('scenemode_enabled', true)))
  const threadBind = $derived(Boolean(value('thread_bind', true)))
  const notifyOn = $derived(Boolean(value('notify', true)))

  /** 实验室接管的开关：置灰不可切换（守护进程正按实验室定义写它，手改也会被写回去） */
  const takeover = $derived(new Set(app.labTakeover))

  /** 被接管时在提示里点名原因——只看到「灰了」却不知道为什么，最容易被当成 bug */
  function hint(field: WritableField, base: string): string {
    if (!takeover.has(field)) return base
    const mode = app.labMode ? t(`lab.mode.${app.labMode}`) : ''
    return `${base} · ${t('config.labTakenOver', { mode })}`
  }

  onMount(() => {
    // 顺序不能颠倒：isChiri 由 loadOverview 得出，冷启动直接落在本页时它还是 false，
    // 先判就会漏掉 loadLab —— 实验室接管的开关不会置灰，用户改完还会被守护进程写回去
    void (async () => {
      if (!app.ready) await app.loadOverview()
      // 实验室入口要显示当前有没有启用，进配置页时顺手读一次
      if (app.isChiri) await app.loadLab()
    })()
    // DOWN 停摆开关（高级设置）显示的是 down.chr 的实际内容
    void app.loadDown()
  })
</script>

<div class="u-stack">
  {#if app.configState === 'failed'}
    <StateBox kind="error" message={t('state.failed')} detail={app.configError} />
  {:else if app.configState === 'missing'}
    <StateBox
      kind="missing"
      message={t('logs.missing')}
      detail={t('config.readonly.hint')}
    />
  {/if}

  {#if app.metaSnapshot && !app.metaValid}
    <StateBox
      kind="error"
      message={t('overview.config.invalid')}
      detail={app.metaProblems.join('；')}
    />
  {/if}

  <Panel title={t('config.header')} desc={app.configRel || undefined}>
    <div class="kv">
      <span class="kv__key">{t('config.name')}</span>
      <span class="kv__value">{raw('name') || '—'}</span>
    </div>
    <div class="kv">
      <span class="kv__key">{t('config.author')}</span>
      <span class="kv__value">{raw('author') || '—'}</span>
    </div>
    <div class="kv">
      <span class="kv__key">{t('config.language')}</span>
      <span class="kv__value u-mono">{raw('language') || '—'}</span>
    </div>
  </Panel>

  <Panel title={t('config.writable')} desc={t('config.readonly.hint')}>
    <div class="ak-form-stack">
      <label class="ak-field">
        <span class="ak-label">{t('config.loglevel')}</span>
        <select
          class="ak-select"
          value={loglevel}
          disabled={!app.metaValid}
          onchange={event =>
            app.setDraft('loglevel', (event.currentTarget as HTMLSelectElement).value)}
        >
          {#each LOG_LEVELS as level (level)}
            <option value={level} selected={level === loglevel}>{level}</option>
          {/each}
        </select>
      </label>

      <label class="ak-field">
        <span class="ak-label">{t('config.daemonLanguage')}</span>
        <select
          class="ak-select"
          value={daemonLang}
          disabled={!app.metaValid}
          onchange={event =>
            app.setDraft('language', (event.currentTarget as HTMLSelectElement).value)}
        >
          {#each LANGUAGES as lang (lang)}
            <option value={lang} selected={lang === daemonLang}>{t(`lang.${lang}`)}</option>
          {/each}
        </select>
        <span class="ak-field__hint">{t('config.daemonLanguage.hint')}</span>
      </label>

      <!-- 2026-09-17 副标题按需求注释：hint={t('config.devRecord.hint')} -->
      <ToggleField
        label={t('config.devRecord')}
        checked={devRecord}
        pending={app.draft.dev_record !== undefined}
        disabled={!app.metaValid}
        onchange={next => app.setDraft('dev_record', next)}
      />

      <ToggleField
        label={t('config.fasEnabled')}
        hint={hint('fas_enabled', t('config.fasEnabled.hint'))}
        checked={fasEnabled}
        pending={app.draft.fas_enabled !== undefined}
        disabled={!app.metaValid || takeover.has('fas_enabled')}
        onchange={next => app.setDraft('fas_enabled', next)}
      />

      <ToggleField
        label={t('config.scenemodeEnabled')}
        hint={hint('scenemode_enabled', t('config.scenemodeEnabled.hint'))}
        checked={scenemodeEnabled}
        pending={app.draft.scenemode_enabled !== undefined}
        disabled={!app.metaValid || takeover.has('scenemode_enabled')}
        onchange={next => app.setDraft('scenemode_enabled', next)}
      />

      <ToggleField
        label={t('config.threadBind')}
        hint={hint('thread_bind', t('config.threadBind.hint'))}
        checked={threadBind}
        pending={app.draft.thread_bind !== undefined}
        disabled={!app.metaValid || takeover.has('thread_bind')}
        onchange={next => app.setDraft('thread_bind', next)}
      />

      <ToggleField
        label={t('config.notify')}
        hint={t('config.notify.hint')}
        checked={notifyOn}
        pending={app.draft.notify !== undefined}
        disabled={!app.metaValid}
        onchange={next => app.setDraft('notify', next)}
      />
    </div>

    {#if app.writeReverted}
      <StateBox kind="missing" message={t('config.reverted')} />
    {/if}

    <div class="commit">
      <p class="commit__state" role="status">
        {#if app.writeError}
          <span class="u-danger">{app.writeError}</span>
        {:else if app.hasDraft}
          {t('config.dirty')}
        {:else}
          <span class="u-muted">{t('config.noChanges')}</span>
        {/if}
      </p>
      <div class="u-actions">
        <button
          type="button"
          class="ak-button btn btn--ghost"
          disabled={!app.hasDraft || app.saving}
          onclick={() => app.discardDraft()}
        >
          {t('action.cancel')}
        </button>
        <button
          type="button"
          class="ak-button btn btn--primary"
          disabled={!app.hasDraft || app.saving || !app.metaValid || app.configState !== 'ok'}
          onclick={() => app.commitDraft()}
        >
          {app.saving ? t('state.loading') : t('action.confirm')}
        </button>
      </div>
    </div>
  </Panel>

  <!-- 高级设置：直接写调度进程的状态文件，不经过 meta.yaml 草稿/保存那套。
       ak-form-stack 强制一行一个控件——ak-choice 是 inline-grid，不套栅格会挤成一行多个 -->
  <Panel signal="action" title={t('config.advanced')} desc={t('config.advanced.hint')}>
    <div class="ak-form-stack">
      <ToggleField
        label={t('config.down')}
        hint={t('config.down.hint')}
        checked={app.downActive}
        disabled={app.downPending}
        onchange={next => app.setDown(next)}
      />
      {#if app.downError}
        <p class="u-note u-danger u-mt-2">{app.downError}</p>
      {/if}
      <!-- 功耗口径：直写 meta.yaml 的 power_avg（不走草稿，立即热重载） -->
      <ToggleField
        label={t('config.powerAvg')}
        hint={t('config.powerAvg.hint')}
        checked={app.powerAvgUsesAverage}
        disabled={app.powerAvgPending || !app.metaValid}
        onchange={next => app.setPowerAvg(next)}
      />
      {#if app.powerAvgError}
        <p class="u-note u-danger u-mt-2">{app.powerAvgError}</p>
      {/if}
    </div>
  </Panel>

  {#if app.isChiri}
    <!-- 实验室：二级页面入口。只显示「有没有启用」这一个事实，具体在实验室页里管 -->
    <Panel
      signal={app.labMode ? 'danger' : ''}
      title={t('lab.entry')}
      desc={t('lab.entry.hint')}
    >
      <div class="u-between">
        <p class="entry__state u-note" data-on={app.labMode ? 'yes' : undefined}>
          {app.labMode
            ? t('lab.entry.on', { mode: t(`lab.mode.${app.labMode}`) })
            : t('lab.entry.off')}
        </p>
        <button
          type="button"
          class="ak-button btn btn--ghost entry__open"
          onclick={() => go('lab')}
        >
          {t('lab.entry.open')}
        </button>
      </div>
    </Panel>
  {/if}
</div>

<style>
  /* 入口状态文字默认走 .u-note 的次要色，启用时换危险色 */
  .entry__state[data-on='yes'] {
    color: var(--ak-signal-danger);
  }

  .entry__open {
    flex: 0 0 auto;
  }

  .commit {
    display: grid;
    gap: var(--ak-space-3);
    margin-top: var(--ak-space-4);
    padding-top: var(--ak-space-3);
    border-top: var(--ak-line-hairline) solid var(--ak-surface-raised);
  }

  /* 有意保留正文色：与「未改动」态的 .u-muted 拉开对比（不用 .u-note） */
  .commit__state {
    margin: 0;
    font-size: 0.75rem;
    line-height: 1.5;
  }
</style>
