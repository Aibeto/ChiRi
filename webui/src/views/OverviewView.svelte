<script lang="ts">
  // OverviewView.svelte: [status] [mode] [info] [stop]
  import { onMount } from "svelte";
  import Panel from "@/components/Panel.svelte";
  import ConfirmSheet from "@/components/ConfirmSheet.svelte";
  import StateBox from "@/components/StateBox.svelte";
  import { t } from "@/i18n/index.svelte";
  import { app } from "@/state.svelte";

  let confirmOpen = $state(false);

  const daemonDetail = $derived(
    app.daemonError ||
      (app.daemonState === "unknown" && !app.flockAvailable ?
        t("daemon.unknown.detail")
      : t(`daemon.${app.daemonState}.detail`)),
  );
  const modeName = $derived(t(app.modeInfo.labelKey));
  // 无记录与“模式名未知”是两件事，描述文案要分开
  const modeDesc = $derived(
    // descKey 为空（CLG 档当前不带描述）时跳过描述行，而不是渲染一行空白
    app.modeMissing
      ? t("mode.unknown.missing")
      : app.modeInfo.descKey
        ? t(app.modeInfo.descKey)
        : "",
  );
  const deviceLabel = $derived(
    app.deviceKind === "chiri" ? t("overview.device.chiri")
    : app.deviceKind === "yumi" ? t("overview.device.yumi")
    : t("overview.device.unknown"),
  );

  const configValue = $derived.by(() => {
    if (app.configState === "ok") return app.configRel;
    if (app.configState === "missing") return t("logs.missing");
    return t("state.failed");
  });

  onMount(() => {
    void app.loadOverview();
  });

  async function confirmStop() {
    await app.stopDaemon();
    confirmOpen = false;
  }
</script>

<div class="stack">
  <Panel signal={app.daemonState === "running" ? "success" : "action"}>
    {#snippet actions()}
      <button
        type="button"
        class="ak-button btn"
        disabled={app.loading}
        onclick={() => app.loadOverview()}
      >
        {app.loading ? t("state.loading") : t("action.refresh")}
      </button>
    {/snippet}
    <div class="daemon">
      <p class="daemon__name u-mono">{app.moduleProp.name || t("app.title")}</p>
      <p class="daemon__meta u-mono">
        {app.moduleProp.version || "—"}{app.moduleProp.versionCode ?
          ` (${app.moduleProp.versionCode})`
        : ""}
      </p>
      {#if app.moduleProp.author}
        <p class="daemon__meta">
          {t("overview.module.author")}：{app.moduleProp.author}
        </p>
      {/if}
      <p class="daemon__meta">{daemonDetail}</p>
    </div>
  </Panel>

  {#if app.loading && !app.ready}
    <StateBox kind="loading" message={t("state.loading")} />
  {:else if app.configState === "failed"}
    <StateBox
      kind="error"
      message={t("state.failed")}
      detail={app.configError}
    />
  {/if}

  <Panel
    title={t("overview.mode")}
    // desc={t("overview.mode.observed")}
    signal={app.modeInfo.signal}
  >
    <div class="mode" data-signal={app.modeInfo.signal}>
      <div class="mode__main">
        <p class="mode__name">{modeName}</p>
        <p class="mode__id u-mono">{app.modeInfo.id || "—"}</p>
        {#if modeDesc}
          <p class="mode__desc">{modeDesc}</p>
        {/if}
      </div>
      {#if app.currentMode && app.daemonState === "stopped"}
        <p class="mode__stale">{t("overview.mode.stale")}</p>
      {/if}
      {#if app.modeError}
        <StateBox
          kind="error"
          message={t("state.failed")}
          detail={app.modeError}
        />
      {/if}
      {#if app.metaProblems.length > 0}
        <ul class="mode__problems">
          {#each app.metaProblems as problem, index (index)}
            <li>{problem}</li>
          {/each}
        </ul>
      {/if}
    </div>
  </Panel>

  <Panel title={t("overview.config")}>
    <div class="kv">
      <span class="kv__key">{t("overview.config")}</span>
      <span class="kv__value u-mono">{configValue}</span>
    </div>
    <div class="kv">
      <span class="kv__key">{t("overview.device")}</span>
      <span class="kv__value">{deviceLabel}</span>
    </div>
    {#if app.deviceKind === "chiri"}
      <div class="kv">
        <span class="kv__key">{t("apps.tag.special")}</span>
        <span class="kv__value u-mono">{app.specialCount}</span>
      </div>
      <div class="kv">
        <span class="kv__key">{t("apps.tag.fas")}</span>
        <span class="kv__value u-mono">{app.fasCount}</span>
      </div>
    {:else}
      <div class="kv">
        <span class="kv__key"
          >{t("apps.tag.special")} / {t("apps.tag.fas")}</span
        >
        <span class="kv__value u-muted">
          {app.deviceKind === "yumi" ?
            t("state.notApplicable")
          : t("overview.device.unknown")}
        </span>
      </div>
    {/if}
    {#if app.whitelistError}
      <StateBox
        kind="error"
        message={t("state.failed")}
        detail={app.whitelistError}
      />
    {/if}
    {#if app.metaPath}
      <div class="kv">
        <span class="kv__key">meta.yaml</span>
        <span class="kv__value u-mono">{app.metaPath}</span>
      </div>
    {/if}
  </Panel>

  <Panel title={t("overview.export.title")} desc={t("overview.export.desc")}>
    <ul class="export__notice">
      <li>{t("overview.export.notice.session")}</li>
      <li>{t("overview.export.notice.busy")}</li>
    </ul>
    {#if app.exportPhase === "done"}
      <p class="export__done">{t("overview.export.saved", { path: app.exportTarget })}</p>
    {/if}
    {#if app.exportError}
      <p class="export__error">{app.exportError}</p>
    {/if}
    <button
      type="button"
      class="ak-button btn btn--primary btn--block"
      disabled={app.exportPhase === "running"}
      onclick={() => void app.startExport()}
    >
      {app.exportPhase === "running" ? t("overview.export.running") : t("overview.export.action")}
    </button>
    {#if app.exportPhase === "running"}
      <div
        class="export__bar"
        role="progressbar"
        aria-valuemin="0"
        aria-valuemax="100"
        aria-valuenow={app.exportPercent}
      >
        <span class="export__fill" style={`width: ${app.exportPercent}%`}></span>
      </div>
      <p class="export__meta">
        {app.exportTotal > 0
          ? t("overview.export.progress", {
              mb: app.exportMb,
              done: app.exportDone,
              total: app.exportTotal
            })
          : t("overview.export.preparing")}
      </p>
    {/if}
  </Panel>

  <Panel
    title={t("overview.stop")}
    // desc={t("overview.stop.desc")}
    signal="danger"
  >
    <button
      type="button"
      class="ak-button btn btn--danger btn--block"
      disabled={app.stopping || app.daemonState !== "running"}
      onclick={() => (confirmOpen = true)}
    >
      {app.stopping ? t("state.loading") : t("overview.stop")}
    </button>
    <p class="stop__hint">
      {app.actionAvailable ?
        t("overview.stop.confirm.recover")
      : t("overview.stop.recover.reboot")}
    </p>
  </Panel>
</div>

<ConfirmSheet
  open={confirmOpen}
  danger
  busy={app.stopping}
  title={t("overview.stop.confirm.title")}
  message={t("overview.stop.confirm.message")}
  notes={[
    t("overview.stop.confirm.effects"),
    t("overview.stop.confirm.recover"),
  ]}
  confirmText={t("overview.stop")}
  cancelText={t("action.cancel")}
  onconfirm={confirmStop}
  ondismiss={() => (confirmOpen = false)}
/>

<style>
  .stack {
    display: grid;
    gap: var(--ak-space-4);
  }

  .daemon {
    display: grid;
    gap: 0.15rem;
  }

  .daemon__name {
    margin: 0;
    font-size: 1.125rem;
    font-weight: 700;
    letter-spacing: 0.02em;
  }

  .daemon__meta {
    margin: 0;
    color: var(--ak-text-secondary);
    font-size: 0.75rem;
    line-height: 1.5;
  }

  .mode {
    display: grid;
    gap: var(--ak-space-2);
  }

  .mode__main {
    display: grid;
    gap: 0.2rem;
  }

  .mode[data-signal="success"] {
    --mode-signal: var(--ak-signal-success);
  }

  .mode[data-signal="action"] {
    --mode-signal: var(--ak-signal-action);
  }

  .mode[data-signal="danger"] {
    --mode-signal: var(--ak-signal-danger);
  }

  .mode[data-signal="accent"] {
    --mode-signal: var(--ak-signal-accent);
  }

  .mode[data-signal="info"] {
    --mode-signal: var(--ak-signal-info);
  }

  .mode__name {
    margin: 0;
    color: var(--mode-signal, var(--ak-text-primary));
    font-size: 1.5rem;
    font-weight: 700;
    letter-spacing: 0.01em;
    line-height: 1.2;
  }

  .mode__id {
    margin: 0;
    color: var(--ak-text-secondary);
    font-size: 0.6875rem;
    letter-spacing: 0.08em;
  }

  .mode__desc {
    margin: 0;
    color: var(--ak-text-secondary);
    font-size: 0.8125rem;
  }

  .mode__stale {
    margin: 0;
    padding: var(--ak-space-2) var(--ak-space-3);
    border-left: 2px solid var(--ak-signal-action);
    color: var(--ak-text-secondary);
    font-size: 0.75rem;
  }

  .mode__problems {
    margin: 0;
    padding-left: var(--ak-space-4);
    color: var(--ak-signal-danger);
    font-size: 0.75rem;
    line-height: 1.6;
  }

  /* 导出卡片的两条提示：暖色竖条 + 次要文字，与实验室页的提示块同风格 */
  .export__notice {
    display: grid;
    gap: 0.4rem;
    margin: 0 0 var(--ak-space-3);
    padding: 0;
    list-style: none;
  }

  .export__notice li {
    position: relative;
    padding-left: var(--ak-space-3);
    color: var(--ak-text-secondary);
    font-size: 0.75rem;
    line-height: 1.6;
  }

  .export__notice li::before {
    position: absolute;
    top: 0.45em;
    left: 0;
    width: 2px;
    height: 0.8em;
    content: "";
    background: var(--ak-signal-action);
  }

  /* 产物路径可能很长，必须允许断行，否则会把卡片撑宽 */
  .export__done,
  .export__error {
    margin: var(--ak-space-3) 0 0;
    font-size: 0.72rem;
    line-height: 1.5;
    word-break: break-all;
  }

  .export__done {
    color: var(--ak-signal-success);
  }

  /* 进度条：细条 + 信号色，不做圆角（与 ak-ui 的直角语言一致） */
  .export__bar {
    height: 4px;
    margin-top: var(--ak-space-3);
    overflow: hidden;
    background: var(--ak-surface-raised);
  }

  .export__fill {
    display: block;
    height: 100%;
    background: var(--ak-signal-action);
    transition: width 0.4s ease;
  }

  .export__meta {
    margin: 0.4rem 0 0;
    color: var(--ak-text-secondary);
    font-size: 0.72rem;
    line-height: 1.5;
  }

  .export__error {
    color: var(--ak-signal-danger);
  }

  .stop__hint {
    margin: var(--ak-space-3) 0 0;
    color: var(--ak-text-secondary);
    font-size: 0.75rem;
    line-height: 1.5;
  }
</style>
