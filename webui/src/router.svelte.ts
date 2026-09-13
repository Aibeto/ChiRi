// router.svelte.ts: [routes] [nav]
// 四屏的 hash 路由：不用第三方 router，够小且便于在 WebView 中直接定位。
export type ViewId = 'overview' | 'config' | 'apps' | 'logs'

export const VIEW_IDS: readonly ViewId[] = ['overview', 'config', 'apps', 'logs']

// [routes]
function parse(hash: string): ViewId {
  const id = hash.replace(/^#\/?/, '').split('?')[0]
  return (VIEW_IDS as readonly string[]).includes(id) ? (id as ViewId) : 'overview'
}

export const router = $state<{ view: ViewId }>({ view: parse(globalThis.location?.hash ?? '') })

// [nav]
export function go(view: ViewId): void {
  router.view = view
  try {
    if (globalThis.location) globalThis.location.hash = `#/${view}`
  } catch {
    /* WebView 限制下退化为纯内存路由 */
  }
}

export function initRouter(): void {
  try {
    globalThis.addEventListener?.('hashchange', () => {
      router.view = parse(globalThis.location?.hash ?? '')
    })
  } catch {
    /* 无 window 环境（单测）忽略 */
  }
}
