<script lang="ts">
  // Segmented.svelte: [switch] [keyboard]
  // 单选切换：用 radiogroup/radio 语义（互斥选择，而非 tab 页），
  // 补方向键导航以满足键盘可达性。
  type SegmentedItem = { id: string; label: string }

  let {
    items,
    value,
    label = '',
    onselect
  } = $props<{
    items: SegmentedItem[]
    value: string
    label?: string
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

<!-- tabindex=-1：roving tabindex 容器（焦点实际在按钮上，方向键事件冒泡至此） -->
<div
  class="segmented"
  role="radiogroup"
  aria-label={label || undefined}
  tabindex={-1}
  onkeydown={onKeydown}
>
  {#each items as item (item.id)}
    <button
      type="button"
      role="radio"
      class="segmented__item"
      aria-checked={value === item.id}
      tabindex={value === item.id ? 0 : -1}
      onclick={() => onselect(item.id)}
    >
      {item.label}
    </button>
  {/each}
</div>

<style>
  .segmented {
    display: grid;
    grid-auto-flow: column;
    grid-auto-columns: 1fr;
    gap: 1px;
    padding: 1px;
    background: var(--ak-surface-raised);
  }

  .segmented__item {
    min-height: 2.75rem;
    padding: 0 var(--ak-space-3);
    border: 0;
    background: var(--ak-surface-panel);
    color: var(--ak-text-secondary);
    font-size: 0.8125rem;
    font-weight: 600;
    letter-spacing: 0.02em;
    cursor: pointer;
    transition:
      background var(--ak-motion-fast) var(--ak-ease-standard),
      color var(--ak-motion-fast) var(--ak-ease-standard);
  }

  .segmented__item[aria-checked='true'] {
    background: var(--ak-surface-muted);
    color: var(--ak-text-primary);
    box-shadow: inset 0 -2px 0 0 var(--ak-signal-info);
  }

  .segmented__item:hover:not([aria-checked='true']) {
    color: var(--ak-text-primary);
  }

  .segmented__item:disabled {
    color: var(--ak-signal-disabled);
    cursor: not-allowed;
  }
</style>
