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
  // 尚无模式记录（current_mode.chr 缺失）时不显示家族与「未知」模式名——避免出现
  // 「UNKNOWN / 未知」这种看起来像模式值的占位，与实验室页同口径
  const modeName = $derived(
    app.modeMissing ? t("mode.unknown.missing") : t(app.modeInfo.labelKey),
  );
  // 模式家族（CLG/特调/实验室/停摆/FAS），与详细模式分开显示
  const familyLabel = $derived(
    app.modeMissing ? "" : t(`mode.family.${app.modeInfo.kind}`),
  );
  const deviceLabel = $derived(
    app.deviceKind === "chiri" ? t("overview.device.chiri")
    : app.deviceKind === "yumi" ? t("overview.device.yumi")
    : t("overview.device.unknown"),
  );

  // 仪表盘进度：功耗 / 满量程（meta.power_max_w，默认 12W），夹在 0~100%
  const powerPercent = $derived.by(() => {
    const w = app.powerAvgWatt;
    if (w === null) return 0;
    return Math.min(100, Math.max(0, (w / app.powerMaxWatt) * 100));
  });

  const configValue = $derived.by(() => {
    if (app.configState === "ok") return app.configRel;
    if (app.configState === "missing") return t("logs.missing");
    return t("state.failed");
  });

  onMount(() => {
    void app.loadOverview();
    // 每秒自刷新数据（不重载页面）；loadOverview 有在飞共享，轮询不会堆积
    const timer = setInterval(() => {
      void app.loadOverview();
    }, 1000);
    return () => clearInterval(timer);
  });

  async function confirmStop() {
    await app.stopDaemon();
    confirmOpen = false;
  }
</script>

<div class="u-stack">
  <Panel signal={app.daemonState === "running" ? "success" : "action"}>
    <div class="daemon-card">
      <div class="daemon">
        <p class="daemon__name u-mono">{app.moduleProp.name || t("app.title")}</p>
        <p class="u-note u-mono">
          {app.moduleProp.version || "—"}{app.moduleProp.versionCode ?
            ` (${app.moduleProp.versionCode})`
          : ""}
        </p>
        {#if app.moduleProp.author}
          <p class="u-note">
            {t("overview.module.author")}：{app.moduleProp.author}
          </p>
        {/if}
        <p class="u-note">{daemonDetail}</p>
      </div>
      {#if app.isChiri}
        <!-- 耗电情况：PowerAVG.chr（daemon 每 1s 采样写入），口径随 meta.power_avg。
             八角读数板直接复用官方 ak-gauge（去进度环的适配见 app.css） -->
        <div class="ak-gauge" style={`--ak-gauge-value: ${powerPercent.toFixed(1)}%`}>
          <div class="ak-gauge__content">
            <span class="ak-gauge__label">
              {app.powerAvgUsesAverage ? t("overview.power.avg") : t("overview.power.ref")}
            </span>
            <span class="ak-gauge__value">
              {app.powerAvgWatt === null ? "—" : app.powerAvgWatt.toFixed(2)}
            </span>
            <span class="ak-gauge__unit">{t("unit.watt")}</span>
          </div>
        </div>
      {/if}
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
        {#if familyLabel}
          <p class="mode__family">{familyLabel}</p>
        {/if}
        <p class="mode__name">{modeName}</p>
        <p class="mode__id u-mono">{app.modeInfo.id || "—"}</p>
      </div>
      {#if app.currentMode && app.daemonState === "stopped"}
        <p class="mode__stale u-note">{t("overview.mode.stale")}</p>
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
    <ul class="export__notice u-notice">
      <li>{t("overview.export.notice.session")}</li>
      <li>{t("overview.export.notice.busy")}</li>
    </ul>
    {#if app.exportPhase === "done"}
      <p class="export__msg export__done u-note u-mt-3">
        {t("overview.export.saved", { path: app.exportTarget })}
      </p>
    {/if}
    {#if app.exportError}
      <p class="export__msg u-note u-mt-3 u-danger">{app.exportError}</p>
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
      <!-- 进度区整块用官方 ak-progress（标题行 + 斜纹刻度轨道），进度值走官方
           --ak-progress-value 变量驱动填充 -->
      <div class="ak-progress u-mt-3" style={`--ak-progress-value: ${app.exportPercent}%`}>
        <div class="ak-progress__header">
          <span>
            {app.exportTotal > 0
              ? t("overview.export.progress", {
                  mb: app.exportMb,
                  done: app.exportDone,
                  total: app.exportTotal
                })
              : t("overview.export.preparing")}
          </span>
          <span class="ak-progress__value">{app.exportPercent}%</span>
        </div>
        <div
          class="ak-progress__track"
          role="progressbar"
          aria-valuemin="0"
          aria-valuemax="100"
          aria-valuenow={app.exportPercent}
        >
          <span class="ak-progress__fill"></span>
        </div>
      </div>
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
    <p class="u-note u-mt-3">
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
  /* 第一张卡片：左侧设备/守护进程信息，右侧耗电读数板（平行两栏） */
  .daemon-card {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    gap: var(--ak-space-4);
    align-items: center;
  }

  .daemon {
    display: grid;
    gap: 0.15rem;
    min-width: 0;
  }

  .daemon__name {
    margin: 0;
    font-size: 1.125rem;
    font-weight: 700;
    letter-spacing: 0.02em;
  }

  .mode {
    display: grid;
    gap: var(--ak-space-2);
  }

  .mode__main {
    display: grid;
    gap: 0.2rem;
  }

  /* 信号色由全局 [data-signal] 映射提供（与 Panel 同一份） */
  .mode__name {
    margin: 0;
    color: var(--signal, var(--ak-text-primary));
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

  /* 文字样式来自 .u-note，这里只补暖色竖条与内边距 */
  .mode__stale {
    padding: var(--ak-space-2) var(--ak-space-3);
    border-left: 2px solid var(--ak-signal-action);
  }

  .mode__problems {
    margin: 0;
    padding-left: var(--ak-space-4);
    color: var(--ak-signal-danger);
    font-size: 0.75rem;
    line-height: 1.6;
  }

  /* 提示清单主体来自全局 .u-notice，这里只补与下方按钮的间距 */
  .export__notice {
    margin-bottom: var(--ak-space-3);
  }

  /* 产物路径可能很长：允许断行，避免把卡片撑宽（文字样式来自 .u-note） */
  .export__msg {
    word-break: break-all;
  }

  .export__done {
    color: var(--ak-signal-success);
  }
</style>
