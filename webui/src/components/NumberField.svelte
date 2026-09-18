<script lang="ts">
  // NumberField.svelte: [state] [input]
  // 数值输入：与 ToggleField 同构（label + hint + 待提交标记）。提交时机是失焦/回车，
  // 不是每敲一个字符——否则会按字符写一次 meta.yaml（每次写入都触发全量热重载）。
  // 非法值不提交、也不回滚：保留原文让用户继续改，下方给红字。
  import { t } from '@/i18n/index.svelte'

  let {
    label,
    hint = '',
    value,
    unit = '',
    min = 0,
    max = 1e9,
    disabled = false,
    pending = false,
    onchange
  } = $props<{
    label: string
    hint?: string
    value: number
    /** 值之后显示的单位后缀（空 = 不显示） */
    unit?: string
    min?: number
    max?: number
    disabled?: boolean
    /** 有未提交的改动（界面标记，不影响表单语义） */
    pending?: boolean
    onchange: (next: number) => void
  }>()

  // 初值也由 effect 写入（在组件初始化里直接读 prop 会触发 svelte 的
  // state_referenced_locally 告警）；外部值变化（写后回读、热重载）同样走它
  let text = $state('')
  let focused = $state(false)
  let invalid = $state(false)

  $effect(() => {
    if (!focused) {
      text = String(value)
      invalid = false
    }
  })

  /** 输入了但还没提交（失焦/回车才提交）：界面上要看得见，别让改动静默丢掉 */
  const dirty = $derived(text !== String(value))

  function commit(): void {
    const raw = text.trim()
    const n = Number(raw)
    if (raw === '' || !Number.isFinite(n) || n < min || n > max) {
      invalid = true
      return
    }
    invalid = false
    if (n !== value) {
      onchange(n)
    } else {
      // 值没变（例如把 1000 打成 1000.0）：把文本归一回去，别让「待提交」标记常亮
      text = String(value)
    }
  }
</script>

<label class="ak-field num" class:num--disabled={disabled}>
  <span class="num__text">
    <span class="num__name">
      {label}
      {#if pending || dirty}
        <span class="num__pending" aria-hidden="true"></span>
      {/if}
    </span>
    {#if hint}
      <span class="ak-field__hint">{hint}</span>
    {/if}
  </span>
  <span class="num__row">
    <input
      class="ak-input num__input"
      type="number"
      inputmode="decimal"
      step="any"
      {disabled}
      min={String(min)}
      max={String(max)}
      value={text}
      oninput={event => (text = (event.currentTarget as HTMLInputElement).value)}
      onfocus={() => (focused = true)}
      onblur={() => {
        focused = false
        commit()
      }}
      onkeydown={event => {
        if (event.key === 'Enter') (event.currentTarget as HTMLInputElement).blur()
      }}
    />
    {#if unit}
      <span class="num__unit u-note">{unit}</span>
    {/if}
  </span>
  {#if invalid}
    <span class="u-note u-danger">{t('field.number.invalid')}</span>
  {/if}
</label>

<style>
  .num {
    display: grid;
    gap: 0.5rem;
  }

  .num--disabled {
    cursor: not-allowed;
    opacity: 0.55;
  }

  .num__text {
    display: grid;
    gap: 0.2rem;
  }

  .num__name {
    display: inline-flex;
    align-items: center;
    gap: 0.4rem;
    color: var(--ak-text-primary);
    font-size: 0.875rem;
    font-weight: 600;
    line-height: 1.4;
  }

  .num__pending {
    width: 0.4rem;
    height: 0.4rem;
    background: var(--ak-signal-action);
  }

  .num__row {
    display: inline-flex;
    align-items: center;
    gap: 0.4rem;
  }

  .num__input {
    width: 8rem;
  }
</style>
