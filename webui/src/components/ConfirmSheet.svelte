<script lang="ts">
  // ConfirmSheet.svelte: [dialog]
  // 破坏性操作确认：原生 <dialog> 提供焦点陷阱、Esc 关闭与无障碍语义，
  // 视觉用语义 token 绘制（切角面板 + 状态信号）。
  let {
    open = false,
    title,
    message = '',
    notes = [],
    confirmText,
    cancelText,
    danger = false,
    busy = false,
    onconfirm,
    ondismiss
  } = $props<{
    open: boolean
    title: string
    message?: string
    notes?: string[]
    confirmText: string
    cancelText: string
    danger?: boolean
    busy?: boolean
    onconfirm: () => void
    ondismiss: () => void
  }>()

  let dialogEl = $state<HTMLDialogElement | null>(null)

  $effect(() => {
    const node = dialogEl
    if (!node) return
    if (open && !node.open) node.showModal()
    else if (!open && node.open) node.close()
  })

  function onBackdrop(event: MouseEvent) {
    if (event.target === dialogEl && !busy) ondismiss()
  }
</script>

<dialog
  bind:this={dialogEl}
  class="sheet"
  data-danger={danger ? 'true' : undefined}
  onclick={onBackdrop}
  oncancel={event => {
    // busy 期间 Esc 不允许关闭：操作仍在进行，关掉会让界面与实际状态脱节
    if (busy) event.preventDefault()
  }}
  onclose={() => {
    if (open) ondismiss()
  }}
>
  <div class="sheet__panel">
    <p class="sheet__eyebrow u-mono">{danger ? 'DANGER' : 'CONFIRM'}</p>
    <h2 class="sheet__title">{title}</h2>
    {#if message}
      <p class="sheet__message">{message}</p>
    {/if}
    {#each notes as note}
      <p class="sheet__note">{note}</p>
    {/each}
    <div class="sheet__actions">
      <button type="button" class="ak-button btn btn--ghost" disabled={busy} onclick={ondismiss}>
        {cancelText}
      </button>
      <button
        type="button"
        class="ak-button btn {danger ? 'btn--danger' : 'btn--primary'}"
        disabled={busy}
        onclick={onconfirm}
      >
        {confirmText}
      </button>
    </div>
  </div>
</dialog>

<style>
  .sheet {
    max-width: min(30rem, calc(100vw - 2 * var(--ak-space-4)));
    padding: 0;
    border: 0;
    background: transparent;
    color: var(--ak-text-primary);
  }

  .sheet::backdrop {
    background: rgba(6, 8, 10, 0.72);
  }

  .sheet__panel {
    position: relative;
    display: grid;
    gap: var(--ak-space-3);
    padding: var(--ak-space-4);
    padding-left: calc(var(--ak-space-4) + 4px);
    border: var(--ak-line-hairline) solid var(--ak-surface-raised);
    background: var(--ak-surface-panel);
    clip-path: polygon(
      0 0,
      calc(100% - var(--ak-cut-md)) 0,
      100% var(--ak-cut-md),
      100% 100%,
      0 100%
    );
  }

  .sheet__panel::before {
    position: absolute;
    top: 0;
    bottom: 0;
    left: 0;
    width: 4px;
    content: '';
    background: var(--ak-signal-info);
  }

  .sheet[data-danger] .sheet__panel::before {
    background: var(--ak-signal-danger);
  }

  .sheet__eyebrow {
    margin: 0;
    color: var(--ak-text-secondary);
    font-size: 0.625rem;
    font-weight: 800;
    letter-spacing: 0.12em;
  }

  .sheet__title {
    margin: 0;
    font-size: 1.0625rem;
    font-weight: 700;
    line-height: 1.35;
  }

  .sheet__message,
  .sheet__note {
    margin: 0;
    font-size: 0.8125rem;
    line-height: 1.6;
  }

  .sheet__note {
    padding-left: var(--ak-space-3);
    border-left: 2px solid var(--ak-signal-action);
    color: var(--ak-text-secondary);
  }

  .sheet__actions {
    display: flex;
    justify-content: flex-end;
    gap: var(--ak-space-3);
    margin-top: var(--ak-space-2);
  }
</style>
