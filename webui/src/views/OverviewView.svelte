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
  // 调度未运行（daemonState=stopped）时不把 current_mode.chr 的陈旧值当「当前模式」
  // 展示：文件里的旧值不代表现在，卡片只陈述「调度未运行」（家族/模式名/id 都不显示）。
  // 尚无记录（文件缺失）同口径显示「尚未产生模式记录」，与实验室页一致。
  const modeIdle = $derived(app.daemonState === "stopped");
  const modeName = $derived(
    modeIdle ? t("mode.unknown.stopped")
    : app.modeMissing ? t("mode.unknown.missing")
    : t(app.modeInfo.labelKey),
  );
  // 模式家族（CLG/特调/实验室/停摆/FAS），与详细模式分开显示；未运行/无记录时不显示
  const familyLabel = $derived(
    modeIdle || app.modeMissing ? "" : t(`mode.family.${app.modeInfo.kind}`),
  );
  const modeId = $derived(
    modeIdle || app.modeMissing ? "—" : app.modeInfo.id || "—",
  );
  const modeSignal = $derived(modeIdle ? "info" : app.modeInfo.signal);
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
        <!-- 耗电情况：PowerAVG.chr（daemon 每 1s 采样写入），口径随 meta.power_avg；
             量程 = meta.power_max_w（默认 12W），进度条直接复用官方 ak-progress 原语 -->
        <div class="power">
          <p class="u-note">
            {app.powerAvgUsesAverage ? t("overview.power.avg") : t("overview.power.ref")}
          </p>
          <p class="power__value u-mono">
            {app.powerAvgWatt === null
              ? "—"
              : `${app.powerAvgWatt.toFixed(2)} ${t("unit.watt")}`}
          </p>
          <div
            class="ak-progress__track power__bar"
            role="progressbar"
            aria-valuemin="0"
            aria-valuemax="100"
            aria-valuenow={Math.round(powerPercent)}
            style={`--ak-progress-value: ${powerPercent.toFixed(1)}%`}
          >
            <span class="ak-progress__fill"></span>
          </div>
        </div>
      {/if}
    </div>
    {#if app.powerAvgMissing}
      <p class="u-note u-danger u-mt-2">{t("overview.power.missing")}</p>
    {/if}
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
    signal={modeSignal}
  >
    <div class="mode" data-signal={modeSignal}>
      <div class="mode__main">
        {#if familyLabel}
          <p class="mode__family">{familyLabel}</p>
        {/if}
        <p class="mode__name">{modeName}</p>
        <p class="mode__id u-mono">{modeId}</p>
      </div>
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

  /* 耗电读数：口径标签 + 数值 + 官方量程条，右栏紧凑排列（替代原八角仪表盘） */
  .power {
    display: grid;
    justify-items: end;
    align-content: center;
    gap: 0.3rem;
    min-width: 8.5rem;
  }

  .power__value {
    margin: 0;
    font-size: 1.125rem;
    font-weight: 700;
    line-height: 1.1;
  }

  .power__bar {
    width: 100%;
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
