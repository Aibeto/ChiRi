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

<!-- 视觉底座来自 ak-ui 的 ak-notice 原语（信号条 + 切角 + 层级排版）；
     深色适配在 app.css 的 ak-adapt 段，本组件只管结构 -->
<div
  class="state ak-notice"
  class:ak-notice--danger={kind === 'error'}
  class:ak-notice--warning={kind === 'missing'}
  data-kind={kind}
  role={kind === 'error' ? 'alert' : undefined}
>
  <div class="state__text ak-notice__body">
    <p class="ak-notice__title">{message}</p>
    {#if detail}
      <p class="state__detail ak-notice__message u-mono">{detail}</p>
    {/if}
  </div>
  {#if action}
    <div class="state__action">{@render action()}</div>
  {/if}
</div>

<style>
  /* 骨架/排版来自官方 .ak-notice（深色适配在 app.css [ak-adapt]）；
     这里只保留信号色分支、加载脉冲与两点结构修正 */
  /* 中性状态（empty/loading）信号条用信息色；error/missing 由 ak-notice--danger/--warning 管 */
  .state[data-kind='empty'],
  .state[data-kind='loading'] {
    --ak-notice-signal: var(--ak-signal-info);
  }

  .state[data-kind='loading']::before {
    animation: state-pulse 1.4s ease-in-out infinite;
  }

  .state__text {
    min-width: 0;
  }

  /* 详情是路径/标识符，可能很长：必须允许断行 */
  .state__detail {
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
    .state[data-kind='loading']::before {
      animation: none;
    }
  }
</style>
