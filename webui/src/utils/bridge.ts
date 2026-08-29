// src/utils/bridge.ts
import { exec, toast, listPackages } from '@/kernelsu'; 
import yaml from 'js-yaml';
import { MockBridge } from './mock';
import i18n from '@/i18n';

declare global {
  interface Window {
    ksu?: any;
  }
}

const MODULE_BASE_PATH = "/data/adb/modules/yumi"; 
const PATHS = {
  RULES_YAML: `${MODULE_BASE_PATH}/rules.yaml`,          
  CONFIG_YAML: `${MODULE_BASE_PATH}/config/config.yaml`, 
  ACTIVE_CONFIG: `${MODULE_BASE_PATH}/active_config.txt`,
  CURRENT_MODE: `${MODULE_BASE_PATH}/current_mode.txt`,
  DAEMON_LOG: `${MODULE_BASE_PATH}/logs/daemon.log`
};

// 解析守护进程当前实际加载的配置文件：
// 8550 等 Chiri 目标 SoC 会使用 config_8550.yaml，守护进程启动时把文件名写入
// active_config.txt，这里读取它以保证 WebUI 与守护进程读写同一份文件。
// 读取失败/为空时回退到默认 config.yaml。
async function resolveConfigPath(): Promise<string> {
  try {
    const { errno, stdout } = await exec(`cat "${PATHS.ACTIVE_CONFIG}"`);
    const name = stdout.trim();
    // 只接受合法文件名，防止路径注入
    if (errno === 0 && name && !name.includes('/') && !name.includes('..')) {
      return `${MODULE_BASE_PATH}/config/${name}`;
    }
  } catch (e) { /* 回退默认 */ }
  return PATHS.CONFIG_YAML;
}

const isDev = import.meta.env.DEV || typeof window.ksu === 'undefined';

// UTF-8 安全的字符串 → base64（TextEncoder 兼容中文/特殊字符，分块避免栈溢出）
function utf8ToBase64(str: string): string {
  const bytes = new TextEncoder().encode(str);
  let bin = '';
  const CHUNK = 0x8000;
  for (let i = 0; i < bytes.length; i += CHUNK) {
    bin += String.fromCharCode(...bytes.subarray(i, i + CHUNK));
  }
  return btoa(bin);
}

const RealBridge = {
  async isDaemonRunning(): Promise<boolean> {
    try {
      const { errno, stdout } = await exec(`pidof yumi`);
      return errno === 0 && stdout.trim().length > 0;
    } catch (e) {
      return false;
    }
  },

  async readFile(path: string): Promise<string> {
    const { errno, stdout } = await exec(`cat "${path}"`);
    if (errno !== 0) throw new Error(i18n.global.t('read_failed', { path }) as string);
    return stdout;
  },
  async writeFile(path: string, content: string): Promise<void> {
    // base64 传输规避 shell 对 $、`、!、引号等字符的解释与注入风险；
    // 先写同目录临时文件再原子 mv：避免直接 `>` 截断原文件时，被守护进程
    // config_watcher（inotify CLOSE_WRITE）读到半截/交错内容导致解析失败。
    const b64 = utf8ToBase64(content);
    const { errno } = await exec(`echo '${b64}' | base64 -d > "${path}.tmp" && mv -f "${path}.tmp" "${path}"`);
    if (errno !== 0) throw new Error(i18n.global.t('write_failed', { path }) as string);
  },

  async getRulesConfig(): Promise<any> { try { return yaml.load(await this.readFile(PATHS.RULES_YAML)) || {}; } catch (e) { return {}; } },
  async saveRulesConfig(config: any): Promise<void> { await this.writeFile(PATHS.RULES_YAML, yaml.dump(config)); },
  async getMainConfig(): Promise<any> { try { return yaml.load(await this.readFile(await resolveConfigPath())) || {}; } catch (e) { return {}; } },
  async saveMainConfig(config: any): Promise<void> { await this.writeFile(await resolveConfigPath(), yaml.dump(config)); toast(i18n.global.t('core_config_saved') as string); },

  async getCurrentMode(): Promise<string> { try { return (await this.readFile(PATHS.CURRENT_MODE)).trim(); } catch (e) { return 'balance'; } },
  async setMode(mode: string): Promise<void> {
    const rules = await this.getRulesConfig();
    rules.global_mode = mode;
    await this.saveRulesConfig(rules);
    toast(i18n.global.t('switch_success', { mode }) as string);
  },

  async getInstalledApps(): Promise<string[]> {
    // 主方法: KernelSU 原生 bridge
    try {
      const apps = await listPackages('user');
      if (apps.length > 0) return apps;
    } catch (_) { /* 尝试备用方法 */ }

    // 备用方法: pm shell 命令
    try {
      const { errno, stdout } = await exec('pm list packages -3');
      if (errno === 0 && stdout.trim()) {
        return stdout.trim().split('\n')
          .map(line => line.replace(/^package:/, '').trim())
          .filter(Boolean);
      }
    } catch (_) { /* 备用方法也失败 */ }

    return [];
  },
  async getAppRules(): Promise<Record<string, string>> { return (await this.getRulesConfig()).app_modes || {}; },
  
  // ================= 修改这里 =================
  async saveAppRule(packageName: string, mode: string): Promise<void> {
     const rules = await this.getRulesConfig();
     if (!rules.app_modes) rules.app_modes = {};
     
     if (mode === '') {
       delete rules.app_modes[packageName];
     } else {
       rules.app_modes[packageName] = mode;
     }

     // ==== FAS 暂禁用：设为 fas 时同步初始化 per_app_profiles 的逻辑注释 ====
     // if (mode === 'fas') {
     //   if (!rules.fas_rules) rules.fas_rules = {};
     //   if (!rules.fas_rules.per_app_profiles) rules.fas_rules.per_app_profiles = {};
     //   if (!rules.fas_rules.per_app_profiles[packageName]) {
     //     rules.fas_rules.per_app_profiles[packageName] = {
     //       target_fps: [30, 60, 90, 120], // 默认覆盖常用帧率
     //       fps_margin: 3.0
     //     };
     //   }
     // }
     
     await this.saveRulesConfig(rules);
     toast(i18n.global.t('app_rules_saved') as string);
  },
  // ============================================

  async getDaemonLog(): Promise<string> {
    try {
      const raw = await this.readFile(PATHS.DAEMON_LOG);
      return raw || '';
    } catch (e) {
      return '';
    }
  },

  async getCpuPolicies(): Promise<number[]> {
    try {
      const { errno, stdout } = await exec('ls /sys/devices/system/cpu/cpufreq/');
      if (errno !== 0) return [];
      return stdout.trim().split(/\s+/)
        .filter(s => /^policy\d+$/.test(s))
        .map(s => parseInt(s.replace('policy', ''), 10))
        .sort((a, b) => a - b);
    } catch (e) {
      return [];
    }
  },

  async getAvailableFreqs(policyNum: number): Promise<string[]> {
    try {
      const path = `/sys/devices/system/cpu/cpufreq/policy${policyNum}/scaling_available_frequencies`;
      const raw = await this.readFile(path);
      return raw.trim().split(/\s+/).filter(Boolean);
    } catch (e) {
      return [];
    }
  },

  async getAvailableGovernors(policyNum: number): Promise<string[]> {
    try {
      const path = `/sys/devices/system/cpu/cpufreq/policy${policyNum}/scaling_available_governors`;
      const raw = await this.readFile(path);
      return raw.trim().split(/\s+/).filter(Boolean);
    } catch (e) {
      return [];
    }
  }
};

export const Bridge = isDev ? MockBridge : RealBridge;
