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
      <span class="ak-field__hint">{hint}</span>
    {/if}
  </span>
</label>

<style>
  /* 开关原语（轨道/旋钮/选中色/焦点环）来自官方 .ak-choice--switch，深色适配在
     app.css [ak-adapt]；这里只保留文本结构与「待提交」标记 */
  .toggle--disabled {
    cursor: not-allowed;
    opacity: 0.55;
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
</style>
