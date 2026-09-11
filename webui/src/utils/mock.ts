// mock.ts: [embedded] [mock-data] [mock-bridge]
// src/utils/mock.ts
import yaml from 'js-yaml';
import embedded from 'virtual:chiri-config';

// [embedded] 
// 构建期嵌入的仓库配置 yaml（vite 插件 virtual:chiri-config 生成）：dev mock 不再手写
// 配置内容——改 yaml 重构建即生效，SoC/白名单随目录自动增删。
// 注意：这里取的是**仓库默认值**（模拟一台未改过设置的设备）；meta.yaml 是用户可修改
// 文件（daemon 严格校验），真实设备值由 bridge 从 active_config.chr 指向的文件读取，
// 不经此嵌入副本。
const FILES = embedded.files;
// 按相对仓库根的路径取嵌入文本（缺失返回空串）并解析为对象
const file = (rel: string): string => FILES[rel] ?? '';
const parseYaml = (rel: string): any => yaml.load(file(rel)) || {};
// 生效 meta.yaml：Chiri SoC 为 {soc}/meta.yaml，默认 Yumi 为 config/meta.yaml
const configRel = (soc: string): string => `module/config/${soc ? `${soc}/` : ''}meta.yaml`;

// [mock-soc]
// 可预览的配置场景从嵌入目录自动推导：存在 {soc}/meta.yaml 的处理器即 Chiri 专属
// 调度；'' 为默认 Yumi（无特调/FAS 白名单，isChiri=false）。console 执行
// __mockSoc('8998') / __mockSoc('') 切换后刷新页面即可预览不同设备形态。
const MOCK_SOCS: string[] = Object.keys(FILES)
  .map(rel => /^module\/config\/([^/]+)\/meta\.yaml$/.exec(rel)?.[1])
  .filter((soc): soc is string => Boolean(soc))
  .sort();
let mockSoc: string = MOCK_SOCS[0] ?? '';
const mockActiveConfigName = (): string => (mockSoc ? `${mockSoc}/meta.yaml` : 'meta.yaml');

// rules.yaml 只读：内容取自构建期嵌入的真实文件（WebUI 无写入入口）
const mockRules: any = parseYaml('module/rules.yaml');

// FAS 白名单 mock：取自嵌入的 normal/fas.yaml（开发模式预览“FAS”标签）
const mockFasWhitelist: Record<string, string> =
  parseYaml('module/config/normal/fas.yaml').fas?.apps || {};

// 内部特调白名单 mock：解析嵌入的 special_tuned.yaml（仅精确包名条目，与守护进程
// 导出规则一致——正则条目无法按包名精确查找）
const mockSpecialTuned: Record<string, { modes: string[]; fallback: string }> = {};
file('src/chiri/special_tuned.yaml').split('\n').forEach(line => {
  const t = line.trim();
  if (!t || t.startsWith('#')) return;
  // 正则条目（re: 前缀）无法按包名精确查找，与守护进程导出规则一致地跳过
  if (t.startsWith('re:')) return;
  const [pkg, modesPart, fallback] = t.split(':');
  const modes = (modesPart || '').split(',').map(s => s.trim()).filter(Boolean);
  const pkgName = (pkg || '').trim();
  if (pkgName && modes.length) {
    mockSpecialTuned[pkgName] = { modes, fallback: (fallback || '').trim() || modes[0] };
  }
});

// 已安装应用 mock：由嵌入的真实白名单/规则包名汇总（不手写应用列表）
const mockApps: string[] = Array.from(new Set([
  ...Object.keys(mockRules.app_modes || {}),
  ...Object.keys(mockSpecialTuned),
  ...Object.keys(mockFasWhitelist)
]));

// 允许用户修改的 meta 字段（与守护进程 read_external_meta 白名单一致）
const USER_META_KEYS = ['loglevel', 'dev_record', 'fas_enabled', 'scenemode_enabled'] as const;

// 生效配置文件抬头信息（meta 段）：meta.yaml 本身即抬头文件，直接取嵌入默认值。
// keep 传入上一份 meta 时保留用户已改动的上述四字段——不被新配置文件的默认值覆盖。
const seededMeta = (soc: string, keep?: Record<string, any>): Record<string, any> => {
  const next: Record<string, any> = { ...parseYaml(configRel(soc)) };
  if (keep) for (const k of USER_META_KEYS) if (k in keep) next[k] = keep[k];
  return next;
};
let mockMeta: Record<string, any> = seededMeta(mockSoc);

const delay = (ms: number) => new Promise(resolve => setTimeout(resolve, ms));
let simulatedModeTxt = "balance";

// [mock-bridge] 
export const MockBridge = {
  async isDaemonRunning(): Promise<boolean> { await delay(100); return true; },
  async getCurrentMode(): Promise<string> { await delay(200); return simulatedModeTxt; },
  async isChiri(): Promise<boolean> { await delay(100); return mockSoc !== ''; },
  async getInstalledApps(): Promise<string[]> { await delay(500); return mockApps; },
  async getAppRules(): Promise<Record<string, string>> { await delay(300); return mockRules.app_modes || {}; },
  async getSpecialTuned(): Promise<Record<string, { modes: string[]; fallback: string }>> { await delay(200); return { ...mockSpecialTuned }; },
  async getFasWhitelist(): Promise<Record<string, string>> { await delay(200); return { ...mockFasWhitelist }; },
  // rules.yaml 只读：WebUI 不再提供模式切换/应用规则写入，仅保留读取
  async getRulesConfig(): Promise<any> { await delay(300); return JSON.parse(JSON.stringify(mockRules)); },
  async getActiveConfigName(): Promise<string> { await delay(100); return mockActiveConfigName(); },
  // 抬头信息取自嵌入的真实配置（配置名等 meta 字段随模拟 SoC 变化）
  async getConfigMeta(): Promise<Record<string, any>> { await delay(200); return { ...mockMeta }; },
  async setLogLevel(level: string): Promise<void> { await delay(200); mockMeta.loglevel = level; },
  async setDevRecord(on: boolean): Promise<void> { await delay(200); mockMeta.dev_record = on; },
  async setFasEnabled(on: boolean): Promise<void> { await delay(200); mockMeta.fas_enabled = on; },
  async setScenemodeEnabled(on: boolean): Promise<void> { await delay(200); mockMeta.scenemode_enabled = on; },
  async stopScheduler(): Promise<void> { await delay(300); },
  async getDaemonLog(): Promise<string> {
    await delay(300);
    return `[2026-02-23 02:31:07] [INFO] [yumi] daemon is running smoothly.\n[2026-02-23 02:48:18] [INFO] [Scheduler] Active mode: ${simulatedModeTxt}`;
  }
};

// dev 调试入口：console 执行 __mockSoc('<soc>') 切换模拟场景（'' 为默认 Yumi），
// 可选项来自构建期嵌入的目录（MOCK_SOCS），切换后同步刷新 meta。
if (typeof window !== 'undefined') {
  (window as any).__mockSoc = (soc: string) => {
    if (soc === '' || MOCK_SOCS.includes(soc)) {
      mockSoc = soc;
      mockMeta = seededMeta(soc, mockMeta);
      console.info(`[mock] active config -> ${mockActiveConfigName()}`);
    } else {
      console.warn(`[mock] unknown soc: ${soc}, available: ${MOCK_SOCS.join(' / ')} or ''`);
    }
  };
}
