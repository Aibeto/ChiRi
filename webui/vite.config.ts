// vite.config.ts: [embedded-config] [plugins] [resolve-alias] [base]
import { fileURLToPath, URL } from 'node:url'
import { readdirSync, readFileSync } from 'node:fs'
import { join, relative, sep } from 'node:path'
import { defineConfig, type Plugin } from 'vite'
import { svelte } from '@sveltejs/vite-plugin-svelte'

// [embedded-config]
// 构建期把仓库内的配置 yaml 整体嵌入（虚拟模块 virtual:chiri-config）：
// 目录级收录，新增/删除 yaml 无需改代码；dev 与 build 都读仓库磁盘内容。
// 注意：嵌入值是**仓库默认值**而非设备权威值——meta.yaml 是用户可修改文件，
// 真实设备路径一律走 contract 层读盘（见 src/dev/mock-shell.ts 说明）。
const REPO_ROOT = fileURLToPath(new URL('..', import.meta.url))
const EMBED_DIRS = ['module/config', 'src/chiri']
const EMBED_FILES = ['module/rules.yaml', 'module/module.prop']

function collectYaml(absDir: string, out: Record<string, string>) {
  for (const entry of readdirSync(absDir, { withFileTypes: true })) {
    const full = join(absDir, entry.name)
    if (entry.isDirectory()) collectYaml(full, out)
    else if (/\.ya?ml$/.test(entry.name)) {
      const rel = relative(REPO_ROOT, full).split(sep).join('/')
      // feature.yaml 仅 daemon 二进制使用、*-example.yaml 纯文档：mock 都不读取，不进产物
      if (/(^|\/)feature\.yaml$/.test(rel) || /-example\.yaml$/.test(rel)) continue
      out[rel] = readFileSync(full, 'utf-8')
    }
  }
}

function embeddedConfigPlugin(): Plugin {
  const virtualId = 'virtual:chiri-config'
  const resolvedId = '\0' + virtualId
  return {
    name: 'chiri-embedded-config',
    resolveId: id => (id === virtualId ? resolvedId : null),
    load: id => {
      if (id !== resolvedId) return null
      const files: Record<string, string> = {}
      for (const dir of EMBED_DIRS) collectYaml(join(REPO_ROOT, dir), files)
      for (const rel of EMBED_FILES) files[rel] = readFileSync(join(REPO_ROOT, rel), 'utf-8')
      return `export default ${JSON.stringify({ files })};`
    },
    configureServer(server) {
      // yaml 变化（含新增/删除）时重建虚拟模块并整页刷新
      for (const dir of EMBED_DIRS) server.watcher.add(join(REPO_ROOT, dir))
      for (const rel of EMBED_FILES) server.watcher.add(join(REPO_ROOT, rel))
      const onYaml = (f: string) => {
        if (!/\.ya?ml$/.test(f)) return
        const mod = server.moduleGraph.getModuleById(resolvedId)
        if (mod) server.moduleGraph.invalidateModule(mod)
        server.ws.send({ type: 'full-reload' })
      }
      server.watcher.on('add', onYaml).on('change', onYaml).on('unlink', onYaml)
    },
  }
}

// [plugins] [resolve-alias] [base]
export default defineConfig({
  plugins: [embeddedConfigPlugin(), svelte()],
  resolve: {
    alias: {
      '@': fileURLToPath(new URL('./src', import.meta.url))
    }
  },
  // 强制打包为相对路径，确保在 WebView（file:// 或自定义 scheme）下资源可加载
  base: './'
})
