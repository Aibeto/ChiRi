<script lang="ts">
  // App.svelte: [shell] [bar] [views] [nav]
  // 顶层骨架：顶栏（模块名/版本 + 守护进程状态 + 界面语言）、路由视图区、底部导航。
  // 整站唯一的外层作用域标记 data-ak-ui="system"（ak-ui 风格强度的挂载点）。
  import { onMount } from 'svelte'
  import LiveStatus from '@/components/LiveStatus.svelte'
  import AppsView from '@/views/AppsView.svelte'
  import ConfigView from '@/views/ConfigView.svelte'
  import LabView from '@/views/LabView.svelte'
  import LogsView from '@/views/LogsView.svelte'
  import OverviewView from '@/views/OverviewView.svelte'
  import { i18n, t, toggleLocale } from '@/i18n/index.svelte'
  import { go, initRouter, navOwner, router, VIEW_IDS } from '@/router.svelte'
  import { app } from '@/state.svelte'

  const navItems = $derived(VIEW_IDS.map(id => ({ id, label: t(`nav.${id}`) })))
  // 二级视图（实验室）挂在配置页下，导航高亮跟着父视图走
  const navActive = $derived(navOwner(router.view))
  const daemonLabel = $derived(t(`daemon.${app.daemonState}`))

  onMount(() => {
    initRouter()
    if (!app.ready) void app.loadOverview()
  })

  /** 退出 WebUI：WebView 里 window.close() 通常无效，返回历史作为兜底 */
  function exitWebui(): void {
    window.close()
    setTimeout(() => window.history.back(), 120)
  }
</script>

<div class="shell" data-ak-ui="system">
  <header class="shell__bar u-between">
    <div class="shell__left u-row-tight">
      <button
        type="button"
        class="ak-button btn shell__exit"
        aria-label={t('app.exit')}
        onclick={exitWebui}
      >
        ✕
      </button>
      <div class="shell__brand">
        <p class="shell__title">{app.moduleProp.name || t('app.title')}</p>
        <p class="shell__version u-mono">
          {app.moduleProp.version || t('app.subtitle')}
        </p>
      </div>
    </div>
    <div class="shell__side u-row-tight">
      <LiveStatus state={app.daemonState} label={daemonLabel} />
      <button
        type="button"
        class="ak-button btn lang"
        aria-label={t('action.toggleLocale')}
        onclick={toggleLocale}
      >
        {i18n.locale === 'zh' ? 'EN' : '中文'}
      </button>
    </div>
  </header>

  <main class="shell__main">
    {#if router.view === 'overview'}
      <OverviewView />
    {:else if router.view === 'config'}
      <ConfigView />
    {:else if router.view === 'apps'}
      <AppsView />
    {:else if router.view === 'logs'}
      <LogsView />
    {:else}
      <LabView />
    {/if}
  </main>

  <nav class="shell__nav" aria-label={t('nav.main')}>
    {#each navItems as item (item.id)}
      <button
        type="button"
        class="ak-button nav {navActive === item.id ? 'nav--active' : ''}"
        aria-current={navActive === item.id ? 'page' : undefined}
        onclick={() => go(item.id)}
      >
        {item.label}
      </button>
    {/each}
  </nav>
</div>

<style>
  .shell {
    display: flex;
    flex-direction: column;
    min-height: 100vh;
    min-height: 100dvh;
    background: var(--ak-surface-canvas);
  }

  /* 行列布局来自 .u-between，这里只管吸顶与分隔线 */
  .shell__bar {
    position: sticky;
    top: 0;
    z-index: 10;
    padding: calc(var(--ak-space-3) + env(safe-area-inset-top)) var(--ak-space-4)
      var(--ak-space-3);
    border-bottom: var(--ak-line-hairline) solid var(--ak-surface-raised);
    background: var(--ak-surface-panel);
  }

  .shell__left {
    min-width: 0;
  }

  /* 命中区高度即 .btn 的 --ak-density-control-height（2.75rem），无需重复声明 */
  .shell__exit {
    flex: 0 0 auto;
    padding: 0 var(--ak-space-3);
    font-size: 0.875rem;
    font-weight: 700;
  }

  .shell__brand {
    min-width: 0;
  }

  .shell__title {
    margin: 0;
    font-size: 1.0625rem;
    font-weight: 700;
    letter-spacing: 0.04em;
    line-height: 1.3;
  }

  .shell__version {
    margin: 0;
    color: var(--ak-text-secondary);
    font-size: 0.625rem;
    letter-spacing: 0.1em;
    line-height: 1.4;
  }

  .shell__side {
    flex: 0 0 auto;
  }

  .lang {
    padding: 0 var(--ak-space-3);
    font-size: 0.75rem;
  }

  .shell__main {
    flex: 1 1 auto;
    padding: var(--ak-space-4);
    padding-bottom: var(--ak-space-6);
  }

  .shell__nav {
    position: sticky;
    bottom: 0;
    z-index: 10;
    display: grid;
    grid-template-columns: repeat(4, 1fr);
    gap: 1px;
    padding: 1px;
    background: var(--ak-surface-raised);
  }

  .nav {
    /* ak-ui 的 .ak-button 默认是 150px 固定宽，栅格下必须显式收掉，否则窄屏横向溢出 */
    width: 100%;
    min-width: 0;
    min-height: 3rem;
    padding: 0 var(--ak-space-2);
    border: 0;
    clip-path: none;
    background: var(--ak-surface-panel);
    color: var(--ak-text-secondary);
    font-size: 0.8125rem;
  }

  .nav--active {
    background: var(--ak-surface-muted);
    color: var(--ak-text-primary);
    box-shadow: inset 0 2px 0 0 var(--ak-signal-info);
  }
</style>
