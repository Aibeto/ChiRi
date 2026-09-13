<script lang="ts">
  // ToggleField.svelte: [switch]
  // 复用 ak-ui 的开关原语（.ak-choice--switch + 原生 checkbox[role=switch]），
  // 仅把硬编码的浅色值映射回语义 token 以适配深色主题。
  let {
    label,
    hint = '',
    checked = false,
    disabled = false,
    pending = false,
    onchange
  } = $props<{
    label: string
    hint?: string
    checked?: boolean
    disabled?: boolean
    /** 有未提交的改动（界面标记，不影响表单语义） */
    pending?: boolean
    onchange: (next: boolean) => void
  }>()
</script>

<label class="ak-choice ak-choice--switch toggle" class:toggle--disabled={disabled}>
  <input
    class="ak-choice__input"
    type="checkbox"
    role="switch"
    {checked}
    {disabled}
    onchange={event => onchange((event.currentTarget as HTMLInputElement).checked)}
  />
  <span class="ak-choice__label toggle__text">
    <span class="toggle__name">
      {label}
      {#if pending}
        <span class="toggle__pending" aria-hidden="true"></span>
      {/if}
    </span>
    {#if hint}
      <span class="toggle__hint">{hint}</span>
    {/if}
  </span>
</label>

<style>
  .toggle {
    --ak-choice-fill: var(--ak-signal-info);
    --ak-choice-signal: var(--ak-signal-action);
    grid-template-columns: 2.8rem minmax(0, 1fr);
    gap: var(--ak-space-3);
    color: var(--ak-text-primary);
    cursor: pointer;
  }

  .toggle--disabled {
    cursor: not-allowed;
    opacity: 0.55;
  }

  .toggle :global(.ak-choice__input) {
    width: 2.8rem;
    height: 1.6rem;
    border-color: color-mix(in srgb, var(--ak-text-secondary) 55%, transparent);
    background: var(--ak-surface-raised);
  }

  .toggle :global(.ak-choice__input::after) {
    top: 0.19rem;
    left: 0.22rem;
    width: 0.9rem;
    height: 0.9rem;
    border: 0;
    background: var(--ak-text-secondary);
  }

  .toggle :global(.ak-choice__input:checked) {
    border-color: var(--ak-signal-info);
    background: color-mix(in srgb, var(--ak-signal-info) 82%, #000);
  }

  .toggle :global(.ak-choice__input:checked::after) {
    left: 1.42rem;
    background: #fff;
    opacity: 1;
    transform: none;
  }

  .toggle :global(.ak-choice__input:focus-visible) {
    box-shadow: 0 0 0 3px color-mix(in srgb, var(--ak-signal-info) 30%, transparent);
  }

  .toggle__text {
    display: grid;
    gap: 0.2rem;
  }

  .toggle__name {
    display: inline-flex;
    align-items: center;
    gap: 0.4rem;
    color: var(--ak-text-primary);
    font-size: 0.875rem;
    font-weight: 600;
    line-height: 1.4;
  }

  .toggle__pending {
    width: 0.4rem;
    height: 0.4rem;
    background: var(--ak-signal-action);
  }

  .toggle__hint {
    color: var(--ak-text-secondary);
    font-size: 0.72rem;
    line-height: 1.5;
  }
</style>
