// src/utils/mock.ts
const mockRules = {
  yumi_scheduler: true,
  dynamic_enabled: true,
  global_mode: "powersave",
  // ==== FAS 暂禁用：app_modes 不再映射 "fas" ====
  app_modes: {},
  // app_modes: {
  //   'com.miHoYo.GenshinImpact': 'fas',
  //   'com.tencent.tmgp.sgame': 'fas',
  //   'com.tencent.tmgp.speedmobile': 'fas'
  // },
  ignored_apps: ['com.android.systemui'],
  // ==== FAS 暂禁用：fas_rules 配置模板整体注释 ====
  // fas_rules: {
  //   fps_gears: [30.0, 60.0, 90.0, 120.0, 144.0],
  //   fps_margin: 3.0,
  //   per_app_profiles: {
  //     "com.miHoYo.GenshinImpact": { target_fps: [30, 60], fps_margin: 4.0 },
  //     "com.tencent.tmgp.sgame": { target_fps: [60, 90, 120], fps_margin: 3.0 }
  //   },
  //   pid: { kp: 0.035, ki: 0.015, kd: 0.005 },
  //   auto_capacity_weight: true,
  //   cluster_profiles: [ { capacity_weight: 1.0 }, { capacity_weight: 1.5 }, { capacity_weight: 2.5 }, { capacity_weight: 3.5 } ],
  //   perf_floor: 0.22,
  //   perf_ceil: 1.0
  // }
};

// 生效配置文件抬头信息（meta 段）mock：配置文件信息页展示用
const mockMeta: Record<string, any> = {
  name: "config_8550",
  author: "ChiRi",
  language: "zh",
  loglevel: "INFO"
};

const mockApps = ['com.android.chrome', 'com.tencent.mm', 'com.miHoYo.GenshinImpact', 'com.hypergryph.arknights'];

// 内部特调白名单 mock：开发模式下预览“特调”标签与专属选项用
const mockSpecialTuned: Record<string, { modes: string[]; fallback: string }> = {
  'com.hypergryph.arknights': { modes: ['akmode'], fallback: 'akmode' }
};
const delay = (ms: number) => new Promise(resolve => setTimeout(resolve, ms));
let simulatedModeTxt = "balance";

export const MockBridge = {
  async isDaemonRunning(): Promise<boolean> { await delay(100); return true; },
  async getCurrentMode(): Promise<string> { await delay(200); return simulatedModeTxt; },
  async isChiri(): Promise<boolean> { await delay(100); return true; }, // dev 演示特调能力
  async setMode(mode: string): Promise<void> { await delay(200); mockRules.global_mode = mode; setTimeout(() => { simulatedModeTxt = mode; }, 800); },
  async getInstalledApps(): Promise<string[]> { await delay(500); return mockApps; },
  async getAppRules(): Promise<Record<string, string>> { await delay(300); return mockRules.app_modes; },
  async getSpecialTuned(): Promise<Record<string, { modes: string[]; fallback: string }>> { await delay(200); return { ...mockSpecialTuned }; },
  async saveAppRule(pkg: string, mode: string): Promise<void> { 
    await delay(200); 
    
    // 更新或删除应用模式
    if (mode === '') {
      delete (mockRules.app_modes as any)[pkg];
    } else {
      (mockRules.app_modes as any)[pkg] = mode; 
    }
  },
  async getRulesConfig(): Promise<any> { await delay(300); return JSON.parse(JSON.stringify(mockRules)); },
  async saveRulesConfig(config: any): Promise<void> { await delay(400); Object.assign(mockRules, config); },
  async getActiveConfigName(): Promise<string> { await delay(100); return '8550/config.yaml'; },
  async getConfigMeta(): Promise<Record<string, any>> { await delay(200); return { ...mockMeta }; },
  async setLogLevel(level: string): Promise<void> { await delay(200); mockMeta.loglevel = level; },
  async restartDaemon(): Promise<void> { await delay(300); },
  async getDaemonLog(): Promise<string> {
    await delay(300);
    return `[2026-02-23 02:31:07] [INFO] [yumi] daemon is running smoothly.\n[2026-02-23 02:48:18] [INFO] [Scheduler] Active mode: ${simulatedModeTxt}`;
  }
};
