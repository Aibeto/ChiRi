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
    <header class="panel__head ak-card__header u-between">
      <div class="panel__titles">
        <h2 class="ak-card__title">{title}</h2>
        {#if desc}
          <p class="ak-card__description">{desc}</p>
        {/if}
      </div>
      {#if actions}
        <div class="panel__actions u-row-tight">{@render actions()}</div>
      {/if}
    </header>
  {/if}
  <div class="ak-card__content">
    {@render children?.()}
  </div>
</section>

<style>
  /* 表面（边框/底色/切角）与排版来自官方 .ak-card（深色适配在 app.css [ak-adapt]），
     这里只保留项目扩展：左侧信号条与头部骨架修正 */
  .panel {
    position: relative;
  }

  .panel[data-signal]::before {
    position: absolute;
    top: 0;
    bottom: 0;
    left: 0;
    width: 3px;
    content: '';
    background: var(--signal);
  }

  /* .u-between 默认居中；标题可能多行，这里保持顶部对齐 */
  .panel__head {
    align-items: flex-start;
  }

  .panel__titles {
    min-width: 0;
  }

  .panel__actions {
    flex: 0 0 auto;
  }

  /* 无标题/操作的面板：内容区没有官方 .ak-card__header 的间距可借 */
  .ak-card__content:first-child {
    margin-top: 0;
  }
</style>
