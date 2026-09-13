// module-info.ts: [types] [parse]
// module.prop 是 Magisk/KernelSU 标准的 KEY=VALUE 清单（module/module.prop）：
// id / name / version / versionCode / author / description / updateJson。
// WebUI 只用它做标题展示，缺失时回退到 KernelSU moduleInfo()。

// [types]
export interface ModuleProp {
  id: string
  name: string
  version: string
  versionCode: string
  author: string
  description: string
}

export const EMPTY_MODULE_PROP: ModuleProp = {
  id: '',
  name: '',
  version: '',
  versionCode: '',
  author: '',
  description: ''
}

// [parse]
export function parseModuleProp(text: string): ModuleProp {
  const map = new Map<string, string>()
  for (const line of text.split('\n')) {
    const raw = line.trim()
    if (!raw || raw.startsWith('#')) continue
    const idx = raw.indexOf('=')
    if (idx <= 0) continue
    map.set(raw.slice(0, idx).trim(), raw.slice(idx + 1).trim())
  }
  const get = (k: string) => map.get(k) ?? ''
  return {
    id: get('id'),
    name: get('name'),
    version: get('version'),
    versionCode: get('versionCode'),
    author: get('author'),
    description: get('description')
  }
}
