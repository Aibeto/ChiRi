<script lang="ts">
  // BatteryView.svelte: [header] [source] [scale] [power]
  // 电池读数二级页（配置 → 电池读数）：电流/电压/功率的读取来源、双电芯、单位换算与
  // 功耗显示口径。全部直写 meta.yaml（不走草稿，写后回读），与 daemon 的热重载对齐。
  import NumberField from '@/components/NumberField.svelte'
  import Panel from '@/components/Panel.svelte'
  import StateBox from '@/components/StateBox.svelte'
  import ToggleField from '@/components/ToggleField.svelte'
  import { t } from '@/i18n/index.svelte'
  import { go } from '@/router.svelte'
  import { app } from '@/state.svelte'

  /** 任何一笔 meta.yaml 直写进行中（口径与电池字段共用一把锁，见 state 侧注释） */
  const busy = $derived(app.metaWritePending)
  /** 私有节点打开时倍压/倍流置灰（daemon 侧也只认私有节点，互斥） */
  const oplus = $derived(app.oplusChg)
  const canWrite = $derived(app.metaValid && !busy)

  /**
   * 开关 OPlus 私有节点：打开时同一笔写入里把倍电压/倍电流清掉。
   * 分开写会让中间态（私有开 + 倍压还开着）被热重载读到一次，值会翻倍。
   */
  function toggleOplus(next: boolean): void {
    void app.setBatteryFields(
      next
        ? { oplus_chg: true, voltage_double: false, current_double: false }
        : { oplus_chg: false }
    )
  }
</script>

<div class="u-stack">
  <header class="bar u-row">
    <button type="button" class="ak-button btn btn--ghost bar__back" onclick={() => go('config')}>
      <span class="bar__arrow" aria-hidden="true">←</span>
      {t('battery.action.back')}
    </button>
    <p class="bar__title">{t('battery.title')}</p>
  </header>

  {#if !app.isChiri}
    <!-- 遥测线程只在 ChiRi SoC 上启动：这些开关在 Yumi 上没有消费方，如实说明 -->
    <StateBox kind="missing" message={t('state.notApplicable')} detail={t('state.chiriOnly')} />
  {/if}

  {#if app.battError}
    <StateBox kind="error" message={t('state.failed')} detail={app.battError} />
  {/if}

  {#if !app.metaValid}
    <!-- meta.yaml 有非法项时写开关被禁用：说明原因，否则「点了没反应」无从排查 -->
    <StateBox
      kind="error"
      message={t('battery.metaInvalid')}
      detail={app.metaProblems.join(' · ')}
    />
  {/if}

  {#if app.isChiri}
  <Panel signal="action" title={t('battery.source.title')}>
    <div class="ak-form-stack">
      <ToggleField
        label={t('battery.oplusChg')}
        hint={t('battery.oplusChg.hint')}
        checked={oplus}
        disabled={!canWrite}
        onchange={toggleOplus}
      />
      <ToggleField
        label={t('battery.oplusDualCell')}
        hint={t('battery.oplusDualCell.hint')}
        checked={app.oplusDualCell}
        disabled={!canWrite || !oplus}
        onchange={next => app.setBatteryFields({ oplus_dual_cell: next })}
      />
    </div>
  </Panel>

  <Panel signal="action" title={t('battery.scale.title')}>
    <div class="ak-form-stack">
      <!-- 私有节点打开时显示为「未开」：daemon 只认私有节点，这两项此刻确实不生效
           （手改 meta 把它们留着也一样），下面那行说明给出来 -->
      <ToggleField
        label={t('battery.voltageDouble')}
        hint={t('battery.voltageDouble.hint')}
        checked={app.voltageDouble && !oplus}
        disabled={!canWrite || oplus}
        onchange={next => app.setBatteryFields({ voltage_double: next })}
      />
      <ToggleField
        label={t('battery.currentDouble')}
        hint={t('battery.currentDouble.hint')}
        checked={app.currentDouble && !oplus}
        disabled={!canWrite || oplus}
        onchange={next => app.setBatteryFields({ current_double: next })}
      />
      {#if oplus}
        <p class="u-note">{t('battery.double.mutex')}</p>
      {/if}
      <NumberField
        label={t('battery.unitDivisor')}
        hint={t('battery.unitDivisor.hint')}
        value={app.unitDivisor}
        min={0.001}
        max={1000000000}
        disabled={!canWrite}
        onchange={next => app.setBatteryFields({ unit_divisor: next })}
      />
    </div>
  </Panel>

  <Panel signal="action" title={t('battery.power.title')}>
    <div class="ak-form-stack">
      <ToggleField
        label={t('config.powerAvg')}
        hint={t('config.powerAvg.hint')}
        checked={app.powerAvgUsesAverage}
        disabled={!canWrite}
        onchange={next => app.setPowerAvg(next)}
      />
      {#if app.powerAvgError}
        <p class="u-note u-danger u-mt-2">{app.powerAvgError}</p>
      {/if}
      <NumberField
        label={t('battery.powerMax')}
        hint={t('battery.powerMax.hint')}
        value={app.powerMaxWatt}
        min={1}
        max={200}
        unit="W"
        disabled={!canWrite}
        onchange={next => app.setBatteryFields({ power_max_w: next })}
      />
    </div>
  </Panel>
  {/if}
</div>

<style>
  .bar {
    align-items: center;
    gap: 0.5rem;
  }

  .bar__back {
    display: inline-flex;
    align-items: center;
    gap: 0.3rem;
  }

  .bar__arrow {
    font-size: 1rem;
    line-height: 1;
  }

  .bar__title {
    margin: 0;
    color: var(--ak-text-primary);
    font-size: 0.9375rem;
    font-weight: 700;
  }
</style>
