// main.ts: [bootstrap] [ksu-chrome]
import { mount } from 'svelte'
import '@yunyoujun/ak-ui/style.css'
import './app.css'
import App from './App.svelte'
import { enableEdgeToEdge, fullScreen, hasKsu } from '@/kernel/ksu'
import { installMockShell } from '@/dev/mock-shell'

// 无 KernelSU 注入（浏览器 dev / 自动化走查）时用设备替身，保证四屏可真实走查
if (!hasKsu()) {
  installMockShell()
}

// [ksu-chrome]
// WebView 外层镶边：KernelSU 提供 enableEdgeToEdge / fullScreen（老版本可能缺失，
// wrapper 内部已做能力探测，调用失败不影响渲染）。
enableEdgeToEdge(true)
fullScreen(false)

// [bootstrap]
const target = document.getElementById('app')
if (target) {
  mount(App, { target })
}
