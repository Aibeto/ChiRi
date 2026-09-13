<script lang="ts">
  // Panel.svelte: [layout] [signal]
  // 面板容器：复用 ak-ui 的 .ak-card 结构类，深色表面与切角几何由项目类提供
  // （ak-ui 的 .ak-card 本身是透明 + 阴影，不适合深色大块布局）。
  import type { Snippet } from 'svelte'

  let {
    title = '',
    desc = '',
    signal = '',
    actions,
    children
  } = $props<{
    title?: string
    desc?: string
    /** 语义信号：info / action / danger / success / accent（映射 --ak-signal-*） */
    signal?: string
    actions?: Snippet
    children?: Snippet
  }>()
</script>

<section class="panel ak-card" data-signal={signal || undefined}>
  {#if title || actions}
    <header class="panel__head ak-card__header">
      <div class="panel__titles">
        <h2 class="panel__title ak-card__title">{title}</h2>
        {#if desc}
          <p class="panel__desc ak-card__description">{desc}</p>
        {/if}
      </div>
      {#if actions}
        <div class="panel__actions">{@render actions()}</div>
      {/if}
    </header>
  {/if}
  <div class="panel__body ak-card__content">
    {@render children?.()}
  </div>
</section>

<style>
  .panel {
    display: block;
    position: relative;
    box-sizing: border-box;
    padding: var(--ak-space-4);
    border: var(--ak-line-hairline) solid var(--ak-surface-raised);
    background: var(--ak-surface-panel);
    box-shadow: none;
    clip-path: polygon(
      0 0,
      calc(100% - var(--ak-cut-md)) 0,
      100% var(--ak-cut-md),
      100% 100%,
      0 100%
    );
  }

  .panel[data-signal] {
    --panel-signal: var(--ak-signal-info);
  }

  .panel[data-signal='action'] {
    --panel-signal: var(--ak-signal-action);
  }

  .panel[data-signal='danger'] {
    --panel-signal: var(--ak-signal-danger);
  }

  .panel[data-signal='success'] {
    --panel-signal: var(--ak-signal-success);
  }

  .panel[data-signal='accent'] {
    --panel-signal: var(--ak-signal-accent);
  }

  .panel[data-signal]::before {
    position: absolute;
    top: 0;
    bottom: 0;
    left: 0;
    width: 3px;
    content: '';
    background: var(--panel-signal);
  }

  .panel__head {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: var(--ak-space-3);
  }

  .panel__titles {
    min-width: 0;
  }

  .panel__title {
    margin: 0;
    font-family: var(--ak-font-sans);
    font-size: 1rem;
    font-weight: 600;
    letter-spacing: 0.02em;
    line-height: 1.4;
  }

  .panel__desc {
    margin: 0.25rem 0 0;
    color: var(--ak-text-secondary);
    font-size: 0.75rem;
    line-height: 1.5;
    opacity: 1;
  }

  .panel__actions {
    display: flex;
    flex: 0 0 auto;
    gap: var(--ak-space-2);
  }

  .panel__body {
    margin-top: var(--ak-space-4);
  }

  .panel__body:first-child {
    margin-top: 0;
  }
</style>
