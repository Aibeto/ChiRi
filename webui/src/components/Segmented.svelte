<script lang="ts">
  // Segmented.svelte: [switch] [keyboard]
  // 单选切换：用 radiogroup/radio 语义（互斥选择，而非 tab 页），
  // 补方向键导航以满足键盘可达性。
  type SegmentedItem = { id: string; label: string }

  let {
    items,
    value,
    label = '',
    scroll = false,
    onselect
  } = $props<{
    items: SegmentedItem[]
    value: string
    label?: string
    /** 项多 / 窄屏时整条左右滑动（项按内容宽度排列，不再压缩）——样式见 app.css [ak-adapt] */
    scroll?: boolean
    onselect: (id: string) => void
  }>()

  function onKeydown(event: KeyboardEvent) {
    if (event.key !== 'ArrowRight' && event.key !== 'ArrowLeft') return
    const current = items.findIndex((item: SegmentedItem) => item.id === value)
    if (current < 0) return
    const step = event.key === 'ArrowRight' ? 1 : -1
    const next = items[(current + step + items.length) % items.length]
    event.preventDefault()
    onselect(next.id)
  }
</script>

<!-- 视觉/尺寸/响应式全部来自官方 .ak-segmented（深色终端切换器）；
     aria-checked 是 radiogroup 语义，由 app.css [ak-adapt] 映射到官方选中态。
     tabindex=-1：roving tabindex 容器（焦点实际在按钮上，方向键事件冒泡至此） -->
<div
  class="ak-segmented"
  data-ak-scroll={scroll || undefined}
  role="radiogroup"
  aria-label={label || undefined}
  tabindex={-1}
  onkeydown={onKeydown}
>
  {#each items as item (item.id)}
    <button
      type="button"
      role="radio"
      class="ak-segmented__item"
      aria-checked={value === item.id}
      tabindex={value === item.id ? 0 : -1}
      onclick={() => onselect(item.id)}
    >
      {item.label}
    </button>
  {/each}
</div>
