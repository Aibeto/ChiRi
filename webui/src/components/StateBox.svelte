<script lang="ts">
  // StateBox.svelte: [states]
  // 空 / 缺失 / 失败 / 加载四种占位：状态用文字 + 信号条表达，不依赖颜色单独区分。
  import type { Snippet } from 'svelte'

  let {
    kind = 'empty',
    message = '',
    detail = '',
    action
  } = $props<{
    kind?: 'empty' | 'missing' | 'error' | 'loading'
    message?: string
    detail?: string
    action?: Snippet
  }>()
</script>

<div class="state" data-kind={kind} role={kind === 'error' ? 'alert' : undefined}>
  <span class="state__bar"></span>
  <div class="state__text">
    <p class="state__message">{message}</p>
    {#if detail}
      <p class="state__detail u-mono">{detail}</p>
    {/if}
  </div>
  {#if action}
    <div class="state__action">{@render action()}</div>
  {/if}
</div>

<style>
  .state {
    display: grid;
    grid-template-columns: 3px minmax(0, 1fr) auto;
    align-items: center;
    gap: var(--ak-space-3);
    padding: var(--ak-space-3) var(--ak-space-3) var(--ak-space-3) 0;
    border: var(--ak-line-hairline) dashed var(--ak-surface-raised);
    background: var(--ak-surface-muted);
  }

  .state__bar {
    align-self: stretch;
    background: var(--ak-signal-disabled);
  }

  .state[data-kind='error'] .state__bar {
    background: var(--ak-signal-danger);
  }

  .state[data-kind='missing'] .state__bar {
    background: var(--ak-signal-action);
  }

  .state[data-kind='loading'] .state__bar {
    background: var(--ak-signal-info);
    animation: state-pulse 1.4s ease-in-out infinite;
  }

  .state__text {
    min-width: 0;
  }

  .state__message {
    margin: 0;
    font-size: 0.8125rem;
    line-height: 1.5;
  }

  .state__detail {
    margin: 0.25rem 0 0;
    color: var(--ak-text-secondary);
    font-size: 0.6875rem;
    line-height: 1.5;
    overflow-wrap: anywhere;
  }

  .state__action {
    flex: 0 0 auto;
  }

  @keyframes state-pulse {
    0%,
    100% {
      opacity: 0.5;
    }

    50% {
      opacity: 1;
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .state[data-kind='loading'] .state__bar {
      animation: none;
    }
  }
</style>
