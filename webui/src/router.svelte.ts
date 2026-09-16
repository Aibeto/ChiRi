// router.svelte.ts: [routes] [nav]
// hash 路由：不用第三方 router，够小且便于在 WebView 中直接定位。
// 两级视图：主视图进底部导航，二级视图（实验室）只能从所属主视图进入，
// 但同样有 hash，刷新/后退行为与主视图一致。
export type ViewId = 'overview' | 'config' | 'apps' | 'logs'
/** 二级视图：不占底部导航位，从主视图进入 */
export type SubViewId = 'lab'
export type RouteId = ViewId | SubViewId

export const VIEW_IDS: readonly ViewId[] = ['overview', 'config', 'apps', 'logs']

/** 可解析的全部路由（主视图 ∪ 二级视图） */
const ROUTE_IDS: readonly RouteId[] = [...VIEW_IDS, 'lab']

/** 二级视图归属的主视图：底部导航据此保持高亮 */
const SUB_OF: Record<SubViewId, ViewId> = { lab: 'config' }

export function isSubView(id: RouteId): boolean {
  return id === 'lab'
}

/** 当前路由对应的主导航项（二级视图回落到它的父视图） */
export function navOwner(id: RouteId): ViewId {
  return isSubView(id) ? SUB_OF[id as SubViewId] : (id as ViewId)
}

// [routes]
function parse(hash: string): RouteId {
  const id = hash.replace(/^#\/?/, '').split('?')[0]
  return (ROUTE_IDS as readonly string[]).includes(id) ? (id as RouteId) : 'overview'
}

export const router = $state<{ view: RouteId }>({ view: parse(globalThis.location?.hash ?? '') })

// [nav]
export function go(view: RouteId): void {
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
