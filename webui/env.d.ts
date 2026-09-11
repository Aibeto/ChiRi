/// <reference types="vite/client" />

// 构建期嵌入的仓库配置 yaml（vite.config.ts 的 chiri-embedded-config 插件提供）：
// files 键为相对仓库根的路径（如 "module/config/8550/config.yaml"），值为仓库默认原文；
// 仅 dev mock 取默认值用，设备权威值（含用户改过的 meta 四字段）仍走 bridge 读设备文件。
declare module 'virtual:chiri-config' {
  const embedded: { files: Record<string, string> };
  export default embedded;
}

declare module '*.vue' {
  import type { DefineComponent } from 'vue';
  const component: DefineComponent<{}, {}, any>;
  export default component;
}
