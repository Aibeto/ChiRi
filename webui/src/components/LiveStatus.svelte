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

<!-- 视觉全部来自官方 .ak-status（深色芯片 + 脉冲信号点，层级适配在 app.css [ak-adapt]）；
     三态用修饰类 + 文字双重表达 -->
<div
  class="ak-status"
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
