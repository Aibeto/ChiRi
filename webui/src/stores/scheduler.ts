// src/stores/scheduler.ts
import { defineStore } from 'pinia';
import { Bridge } from '@/utils/bridge';

export const useSchedulerStore = defineStore('scheduler', {
  state: () => ({
    currentMode: 'balance',
    appRules: {} as Record<string, string>,
    // 是否处于 Chiri 专属调度（特定处理器）。特调 id 体系仅 Chiri 下激活，Yumi 设备恒为 false
    isChiri: false,
    // 内部特调白名单（守护进程导出，只读展示“特调”标签与专属模式选项）
    specialTuned: {} as Record<string, { modes: string[]; fallback: string }>,
    // FAS 白名单（守护进程导出，只读展示“FAS”标签并禁用该应用的模式切换）
    fasWhitelist: {} as Record<string, string>,
    isDaemonRunning: false, // 必须有这个初始状态
    loading: false
  }),
  actions: {
    async initData() {
      this.loading = true;
      try {
        // 必须在这里同时调用四个接口
        const [mode, rules, running, specialTuned, fasWhitelist, chiri] = await Promise.all([
          Bridge.getCurrentMode(),
          Bridge.getAppRules(),
          Bridge.isDaemonRunning(),
          Bridge.getSpecialTuned(),
          Bridge.getFasWhitelist(),
          Bridge.isChiri()
        ]);
        this.currentMode = mode;
        this.appRules = rules;
        this.isDaemonRunning = running; // 必须有这一行赋值
        this.specialTuned = specialTuned;
        this.fasWhitelist = fasWhitelist;
        this.isChiri = chiri;
      } finally {
        this.loading = false;
      }
    },
    async switchMode(mode: string) {
      await Bridge.setMode(mode);
      this.currentMode = mode;
    }
  }
});