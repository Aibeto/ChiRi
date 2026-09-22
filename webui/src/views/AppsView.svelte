<script lang="ts">
  // AppsView.svelte: [search] [list] [rules]
  // 应用性能模式由模块随附的 rules.yaml 决定（WebUI 无写入口），
  // 因此这里只按契约事实打标签：特调命中、FAS 白名单命中、现存 app_modes。
  import { onMount } from "svelte";
  import Panel from "@/components/Panel.svelte";
  import StateBox from "@/components/StateBox.svelte";
  import { t } from "@/i18n/index.svelte";
  import { app } from "@/state.svelte";

  const shown = $derived(app.filteredApps.length);
  const total = $derived(app.apps.length);

  onMount(() => {
    if (app.apps.length === 0) void app.loadApps();
  });
</script>

<div class="u-stack">
  <Panel title={t("apps.title")} desc={t("apps.count", { n: total })}>
    {#snippet actions()}
      <button
        type="button"
        class="ak-button btn"
        disabled={app.scanning}
        onclick={() => app.loadApps(true)}
      >
        {app.scanning ? t("action.scanning") : t("action.rescan")}
      </button>
    {/snippet}

    <label class="ak-field">
      <span class="ak-label">{t("apps.search")}</span>
      <input
        class="ak-input"
        type="search"
        placeholder={t("apps.search")}
        bind:value={app.appKeyword}
      />
    </label>

    {#if shown !== total}
      <p class="u-note u-mt-2 u-mono">{t("apps.filtered", { shown, total })}</p>
    {/if}
  </Panel>

  {#if app.appsState === "missing" && total === 0}
    <StateBox
      kind="empty"
      message={t("apps.empty")}
      detail={t("apps.empty.detail")}
    />
  {:else if total === 0}
    <StateBox kind="loading" message={t("state.loading")} />
  {:else if shown === 0}
    <StateBox kind="empty" message={t("apps.empty")} />
  {:else}
    <ul class="apps u-list-reset">
      {#each app.filteredApps as entry (entry.pkg)}
        <li class="apps__item">
          <div class="apps__head">
            <p class="apps__label u-truncate">{entry.label}</p>
            {#if entry.label !== entry.pkg}
              <p class="apps__pkg u-mono u-truncate">{entry.pkg}</p>
            {/if}
          </div>
          <div class="ak-tag-group apps__tags">
            {#if entry.special}
              <span class="ak-tag ak-tag--advanced">
                {t("apps.tag.special")} · {entry.special.fallback}
              </span>
            {/if}
            {#if entry.fasConfig}
              <span class="ak-tag">{t("apps.tag.fas")} · {entry.fasConfig}</span
              >
            {/if}
            {#if entry.appMode}
              <span class="ak-tag ak-tag--neutral"
                >{t("apps.tag.mode")} · {entry.appMode}</span
              >
            {/if}
          </div>
        </li>
      {/each}
    </ul>
  {/if}

  <Panel title={t("apps.rules")} desc={t("apps.rules.hint")}>
    {#if !app.rules.ok}
      <StateBox
        kind="error"
        message={t("state.failed")}
        detail={app.rules.problem}
      />
    {:else}
      <div class="kv">
        <span class="kv__key">{t("apps.rules.dynamic")}</span>
        <span class="kv__value u-mono"
          >{app.rules.dynamicEnabled ? "true" : "false"}</span
        >
      </div>
      <div class="kv">
        <span class="kv__key">{t("apps.rules.globalMode")}</span>
        <span class="kv__value u-mono">{app.rules.globalMode}</span>
      </div>
      <div class="kv">
        <span class="kv__key">{t("apps.rules.ignored")}</span>
        <span class="kv__value u-mono">
          {app.rules.ignoredApps.length ?
            app.rules.ignoredApps.join("、")
          : "—"}
        </span>
      </div>
      <div class="kv">
        <span class="kv__key">app_modes</span>
        <span class="kv__value u-mono">
          {Object.keys(app.rules.appModes).length || "—"}
        </span>
      </div>
    {/if}
  </Panel>
</div>

<style>
  /* 列表重置来自 .u-list-reset，这里只做 1px 缝隙的分隔线效果 */
  .apps {
    display: grid;
    gap: 1px;
    background: var(--ak-surface-raised);
  }

  .apps__item {
    display: grid;
    gap: var(--ak-space-2);
    padding: var(--ak-space-3) var(--ak-space-4);
    background: var(--ak-surface-panel);
  }

  .apps__head {
    display: grid;
    gap: 0.1rem;
    min-width: 0;
  }

  .apps__label {
    margin: 0;
    font-size: 0.875rem;
    font-weight: 600;
    line-height: 1.4;
  }

  .apps__pkg {
    margin: 0;
    color: var(--ak-text-secondary);
    font-size: 0.6875rem;
    line-height: 1.4;
  }

  .apps__tags {
    gap: var(--ak-space-2);
  }
</style>
