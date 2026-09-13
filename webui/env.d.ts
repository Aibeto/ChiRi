// env.d.ts: [refs] [virtual]
/// <reference types="svelte" />
/// <reference types="vite/client" />

// [virtual]
// 构建期嵌入的仓库配置（vite.config.ts embeddedConfigPlugin 生成）：
// 仅 dev mock 与文档参考使用，真实设备值一律走 contract 层读盘。
declare module 'virtual:chiri-config' {
  const embedded: { files: Record<string, string> }
  export default embedded
}
