<script lang="ts">
  // LiveStatus.svelte: [status]
  // 复用 ak-ui 的状态原语（.ak-status 自带深色底 + 脉冲信号点），
  // 三种存活态用文字 + 修饰类双重表达（不只靠颜色）。
  let {
    state,
    label,
    detail = ''
  } = $props<{
    state: 'running' | 'stopped' | 'unknown'
    label: string
    detail?: string
  }>()
</script>

<div
  class="ak-status status"
  class:ak-status--offline={state === 'stopped'}
  class:ak-status--warning={state === 'unknown'}
  role="status"
>
  <span class="ak-status__signal"></span>
  <span class="ak-status__label">{label}</span>
  {#if detail}
    <span class="ak-status__detail">{detail}</span>
  {/if}
</div>

<style>
  .status {
    min-height: auto;
    padding: var(--ak-space-2) var(--ak-space-3);
    background: var(--ak-surface-raised);
  }
</style>
