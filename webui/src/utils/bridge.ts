// bridge.ts: [paths] [helpers] [real-bridge] [status-fs] [rules-ro] [config] [scheduler] [apps] [log] [bridge-export]
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

// [paths] 
const MODULE_BASE_PATH = "/data/adb/modules/chiri";
const PATHS = {
  MODULE: MODULE_BASE_PATH,
  RULES_YAML: `${MODULE_BASE_PATH}/rules.yaml`,
  CONFIG_META: `${MODULE_BASE_PATH}/config/meta.yaml`,
  ACTIVE_CONFIG: `${MODULE_BASE_PATH}/active_config.chr`,
  SPECIAL_TUNED: `${MODULE_BASE_PATH}/special_tuned.yaml`,
  FAS_WHITELIST: `${MODULE_BASE_PATH}/fas_whitelist.yaml`,
  CURRENT_MODE: `${MODULE_BASE_PATH}/current_mode.chr`,
  DAEMON_LOG: `${MODULE_BASE_PATH}/logs/daemon.log`,
  WATCHDOG_PID: `${MODULE_BASE_PATH}/logs/watchdog.pid`
};

// [helpers] 
// 解析守护进程当前实际加载的配置文件（meta.yaml，可修改抬头）：
// Chiri 目标 SoC（如 8550）使用处理器子目录 config/8550/meta.yaml，守护进程启动时把
// 相对 config 目录的路径（如 "8550/meta.yaml"）写入 active_config.chr，这里读取它以保证
// WebUI 与守护进程读写同一份文件。读取失败/为空时回退到默认 config/meta.yaml。
async function resolveConfigPath(): Promise<string> {
  try {
    const { errno, stdout } = await exec(`cat "${PATHS.ACTIVE_CONFIG}"`);
    const name = stdout.trim();
    // 只接受相对 config 目录的合法路径（可含一层处理器子目录），禁止上级穿越（..）防路径注入
    if (errno === 0 && name && !name.includes('..')) {
      return `${MODULE_BASE_PATH}/config/${name}`;
    }
  } catch (e) { /* 回退默认 */ }
  return PATHS.CONFIG_META;
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

// 模式名 → 本地化文案已随 WebUI 模式切换入口移除（rules.yaml 只读，无写操作需 toast 文案）。

// meta.yaml 顶层字段行替换：精确键名（大小写不敏感）、仅匹配顶层缩进为 0 的行，
// 保留字段名原大小写/引号风格与注释；匹配不到返回 null，由调用方兜底
// （daemon sync_meta_snapshot 会在热重载前校验，非法字段回退内嵌默认）。
function replaceYamlFieldLine(content: string, field: string, value: string): string | null {
  const lines = content.split('\n');
  const want = field.toLowerCase();
  for (let i = 0; i < lines.length; i++) {
    const trimmed = lines[i].trimStart();
    if (!trimmed || trimmed.startsWith('#')) continue;
    if (lines[i].length !== trimmed.length) continue; // 仅顶层
    const m = trimmed.match(/^([^:\s]+)(\s*:\s*)(.*)$/);
    if (!m || m[1].toLowerCase() !== want) continue;
    // 原值带引号（"INFO" / 'INFO'）时保持引号风格，否则写裸值
    const val = /^["']/.test(m[3].trim()) ? `"${value}"` : value;
    lines[i] = `${m[1]}${m[2]}${val}`;
    return lines.join('\n');
  }
  return null;
}

// [real-bridge] 
const RealBridge = {
  // [status-fs] 
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

  // [rules-ro] 
  // rules.yaml 为只读：全局模式 / 应用性能模式 / ignored_apps 等均由模块维护，
  // WebUI 不提供任何写入路径（rules.yaml 已不可被用户修改），仅读取用于展示。
  async getRulesConfig(): Promise<any> { try { return yaml.load(await this.readFile(PATHS.RULES_YAML)) || {}; } catch (e) { return {}; } },

  // [config] 
  // 生效 meta.yaml 相对 config 目录的路径（如 "8550/meta.yaml"，非处理器时为 "meta.yaml"），
  // 由守护进程启动时写入 active_config.chr；WebUI 只读展示，不改动文件。
  async getActiveConfigName(): Promise<string> {
    try {
      const { errno, stdout } = await exec(`cat "${PATHS.ACTIVE_CONFIG}"`);
      const name = stdout.trim();
      if (errno === 0 && name && !name.includes('..')) return name;
    } catch (e) { /* 回退默认 */ }
    return 'meta.yaml';
  },

  // 配置文件抬头信息：直接读生效 meta.yaml（文件本身就是 meta 段），仅供查看
  async getConfigMeta(): Promise<Record<string, any>> {
    try {
      return yaml.load(await this.readFile(await resolveConfigPath())) || {};
    } catch (e) {
      return {};
    }
  },

  // meta.yaml 单字段写入：顶层行替换保留注释，字段缺失时兜底整文件重写。
  // daemon sync_meta_snapshot 会在热重载前严格校验，非法字段自动回退内嵌默认。
  async updateMetaField(field: string, value: string | boolean): Promise<void> {
    const path = await resolveConfigPath();
    const content = await this.readFile(path);
    const updated = replaceYamlFieldLine(content, field, String(value)) ?? (() => {
      const cfg = yaml.load(content) || {};
      (cfg as any)[field] = value;
      return yaml.dump(cfg);
    })();
    await this.writeFile(path, updated);
  },

  // 切换日志等级：写入 meta.yaml 的 loglevel，热重载即时生效
  async setLogLevel(level: string): Promise<void> {
    await this.updateMetaField('loglevel', level);
    toast(i18n.global.t('loglevel_updated') as string);
  },

  // 开发记录开关：开启后守护进程向 devimp/ 目录写按核调度诊断日志
  async setDevRecord(on: boolean): Promise<void> {
    await this.updateMetaField('dev_record', on);
    toast(i18n.global.t('dev_record_updated') as string);
  },

  // FAS 总闸开关：关闭后 determine_mode 不再产生 fas，运行中 FAS 实例立即注销
  async setFasEnabled(on: boolean): Promise<void> {
    await this.updateMetaField('fas_enabled', on);
    toast(i18n.global.t('fas_enabled_updated') as string);
  },

  // 息屏场景模式总闸：关闭后息屏不再使用scenemode，运行中实例立即退出
  async setScenemodeEnabled(on: boolean): Promise<void> {
    await this.updateMetaField('scenemode_enabled', on);
    toast(i18n.global.t('scenemode_enabled_updated') as string);
  },

  // [scheduler] 
  // 关闭调度：先终止看门狗（防止其崩溃自愈把主进程再拉起），再强杀主进程 yumi。
  // 看门狗 PID 在 service.sh/action.sh 启动时写入 logs/watchdog.pid。
  // 关闭后需点击模块 Action（action.sh 手动启动）或重启设备才恢复调度。
  async stopScheduler(): Promise<void> {
    const { errno } = await exec(
      `[ -f "${PATHS.WATCHDOG_PID}" ] && kill "$(cat "${PATHS.WATCHDOG_PID}" 2>/dev/null)" 2>/dev/null; ` +
      `killall -9 yumi 2>/dev/null; rm -f "${PATHS.WATCHDOG_PID}"`
    );
    if (errno !== 0) throw new Error(i18n.global.t('stop_failed') as string);
  },

  async getCurrentMode(): Promise<string> {
    try {
      // 文件被意外清空时回退默认档位；守护进程已常态每 5 秒重写该文件自愈
      return (await this.readFile(PATHS.CURRENT_MODE)).trim() || 'balance';
    } catch (e) { return 'balance'; }
  },

  // 判定是否处于 Chiri 专属调度（特定处理器）：active_config 若为"处理器子目录/config.yaml"
  // （含 '/'，如 "8550/config.yaml"）则为 Chiri，否则为默认 Yumi。
  // 特调 UI 仅在 Chiri 下激活，Yumi 设备看不到特调标签/选项。
  async isChiri(): Promise<boolean> {
    try {
      const { errno, stdout } = await exec(`cat "${PATHS.ACTIVE_CONFIG}"`);
      const name = stdout.trim();
      return errno === 0 && !!name && name.includes('/');
    } catch (e) {
      return false;
    }
  },

  // [apps] 
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

  // 读取守护进程启动时导出的内部特调白名单（每行 `包名:特调模式列表(逗号分隔):优先回退模式`）。
  // 该文件由守护进程维护，WebUI 只读用于展示“特调”标签，不提供修改入口；
  // 文件缺失（守护进程未启动等）时返回空表，标签静默降级。
  async getSpecialTuned(): Promise<Record<string, { modes: string[]; fallback: string }>> {
    try {
      const raw = await this.readFile(PATHS.SPECIAL_TUNED);
      const map: Record<string, { modes: string[]; fallback: string }> = {};
      raw.split('\n').forEach(line => {
        const [pkg, modesPart, fallback] = line.split(':');
        const modes = (modesPart || '').split(',').map(s => s.trim()).filter(Boolean);
        const pkgName = (pkg || '').trim();
        if (pkgName && modes.length) {
          map[pkgName] = { modes, fallback: (fallback || '').trim() || modes[0] };
        }
      });
      return map;
    } catch (e) {
      return {};
    }
  },

  // 读取 FAS 白名单（每行 `包名:配置名`）。该文件由守护进程维护，WebUI 只读用于展示
  // "FAS" 标签并禁用该应用的模式切换，不提供修改入口；
  // 文件缺失（守护进程未启动等）时返回空表，标签静默降级。
  async getFasWhitelist(): Promise<Record<string, string>> {
    try {
      const raw = await this.readFile(PATHS.FAS_WHITELIST);
      const map: Record<string, string> = {};
      raw.split('\n').forEach(line => {
        const [pkg, config] = line.split(':');
        const pkgName = (pkg || '').trim();
        const cfg = (config || '').trim();
        if (pkgName && cfg) {
          map[pkgName] = cfg;
        }
      });
      return map;
    } catch (e) {
      return {};
    }
  },

  // [log] 
  async getDaemonLog(): Promise<string> {
    try {
      const raw = await this.readFile(PATHS.DAEMON_LOG);
      return raw || '';
    } catch (e) {
      return '';
    }
  }
};

// [bridge-export] 
export const Bridge = isDev ? MockBridge : RealBridge;
