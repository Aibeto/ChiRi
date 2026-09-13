<script lang="ts">
  // ConfigView.svelte: [header] [fields] [commit]
  // meta.yaml 只有 5 个字段可写；每次写入都会触发守护进程全量热重载，
  // 因此改动先落草稿、由「保存」一次性提交（单次读-改-写）。
  import { onMount } from 'svelte'
  import Panel from '@/components/Panel.svelte'
  import StateBox from '@/components/StateBox.svelte'
  import ToggleField from '@/components/ToggleField.svelte'
  import { LOG_LEVELS, LANGUAGES, type WritableField } from '@/contract/meta'
  import { t } from '@/i18n/index.svelte'
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

  onMount(() => {
    if (!app.ready) void app.loadOverview()
  })
</script>

<div class="stack">
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
    <div class="fields">
      <label class="field">
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

      <label class="field">
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

      <ToggleField
        label={t('config.devRecord')}
        hint={t('config.devRecord.hint')}
        checked={devRecord}
        pending={app.draft.dev_record !== undefined}
        disabled={!app.metaValid}
        onchange={next => app.setDraft('dev_record', next)}
      />

      <ToggleField
        label={t('config.fasEnabled')}
        hint={t('config.fasEnabled.hint')}
        checked={fasEnabled}
        pending={app.draft.fas_enabled !== undefined}
        disabled={!app.metaValid}
        onchange={next => app.setDraft('fas_enabled', next)}
      />

      <ToggleField
        label={t('config.scenemodeEnabled')}
        hint={t('config.scenemodeEnabled.hint')}
        checked={scenemodeEnabled}
        pending={app.draft.scenemode_enabled !== undefined}
        disabled={!app.metaValid}
        onchange={next => app.setDraft('scenemode_enabled', next)}
      />
    </div>

    {#if app.writeReverted}
      <StateBox kind="missing" message={t('config.reverted')} />
    {/if}

    <div class="commit">
      <p class="commit__state" role="status">
        {#if app.writeError}
          <span class="commit__error">{app.writeError}</span>
        {:else if app.hasDraft}
          {t('config.dirty')}
        {:else}
          <span class="u-muted">{t('config.noChanges')}</span>
        {/if}
      </p>
      <div class="commit__actions">
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
</div>

<style>
  .stack {
    display: grid;
    gap: var(--ak-space-4);
  }

  .fields {
    display: grid;
    gap: var(--ak-space-4);
  }

  .field {
    display: grid;
    gap: 0.5rem;
  }

  .commit {
    display: grid;
    gap: var(--ak-space-3);
    margin-top: var(--ak-space-4);
    padding-top: var(--ak-space-3);
    border-top: var(--ak-line-hairline) solid var(--ak-surface-raised);
  }

  .commit__state {
    margin: 0;
    font-size: 0.75rem;
    line-height: 1.5;
  }

  .commit__error {
    color: var(--ak-signal-danger);
  }

  .commit__actions {
    display: flex;
    justify-content: flex-end;
    gap: var(--ak-space-3);
  }
</style>
