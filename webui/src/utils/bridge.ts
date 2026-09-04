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

const MODULE_BASE_PATH = "/data/adb/modules/chiri"; 
const PATHS = {
  MODULE: MODULE_BASE_PATH,
  RULES_YAML: `${MODULE_BASE_PATH}/rules.yaml`,          
  CONFIG_YAML: `${MODULE_BASE_PATH}/config/config.yaml`, 
  ACTIVE_CONFIG: `${MODULE_BASE_PATH}/active_config.txt`,
  SPECIAL_TUNED: `${MODULE_BASE_PATH}/special_tuned.txt`,
  CURRENT_MODE: `${MODULE_BASE_PATH}/current_mode.txt`,
  DAEMON_LOG: `${MODULE_BASE_PATH}/logs/daemon.log`,
  WATCHDOG_PID: `${MODULE_BASE_PATH}/logs/watchdog.pid`
};

// 解析守护进程当前实际加载的配置文件：
// Chiri 目标 SoC（如 8550）使用处理器子目录 config/8550/config.yaml，守护进程启动时把
// 相对 config 目录的路径（如 "8550/config.yaml"）写入 active_config.txt，这里读取它以保证
// WebUI 与守护进程读写同一份文件。读取失败/为空时回退到默认 config/config.yaml。
async function resolveConfigPath(): Promise<string> {
  try {
    const { errno, stdout } = await exec(`cat "${PATHS.ACTIVE_CONFIG}"`);
    const name = stdout.trim();
    // 只接受相对 config 目录的合法路径（可含一层处理器子目录），禁止上级穿越（..）防路径注入
    if (errno === 0 && name && !name.includes('..')) {
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

// 模式名 → 本地化文案：标准四档用 i18n 标签，特调（如 akmode）/未知模式回退原键名，
// 避免 toast 直接展示裸 key。
function modeLabel(modeKey: string): string {
  const key = `mode_${modeKey}`;
  const localized = i18n.global.t(key);
  return typeof localized === 'string' && localized !== key ? localized : modeKey;
}

// 单字段 YAML 行内替换：只改首个匹配行的值，保留缩进/字段名大小写/引号风格及其余内容与注释。
// 用于日志等级等单字段写入，避免整文件重写丢失用户手写注释。匹配不到返回 null，由调用方兜底。
function replaceYamlFieldLine(content: string, field: string, value: string): string | null {
  const re = new RegExp(`^(\\s*)(${field})(\\s*:\\s*)(.*)$`, 'im');
  const m = content.match(re);
  if (!m || m.index === undefined) return null;
  const [, indent, name, sep, raw] = m;
  // 原值带引号（"INFO" / 'INFO'）时保持引号风格，否则写裸值
  const quoted = /^["']/.test(raw.trim());
  const val = quoted ? `"${value}"` : value;
  return content.slice(0, m.index) + `${indent}${name}${sep}${val}` + content.slice(m.index + m[0].length);
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

  // 内部读取/写入 rules.yaml：仅服务于模式切换与应用性能模式，不提供整文件编辑
  async getRulesConfig(): Promise<any> { try { return yaml.load(await this.readFile(PATHS.RULES_YAML)) || {}; } catch (e) { return {}; } },
  async saveRulesConfig(config: any): Promise<void> {
    // 空/缺失的 app_modes 不要落盘为 null：js-yaml 会把值为 null 的字段序列化成
    // "app_modes: null"，而 serde_yaml 无法把 null 反序列化为 HashMap（#[serde(default)]
    // 只对缺失字段生效），导致守护进程每次加载 rules.yaml 都告警。直接移除该键即可。
    if (config.app_modes === null || config.app_modes === undefined) {
      delete config.app_modes;
    }
    await this.writeFile(PATHS.RULES_YAML, yaml.dump(config));
  },

  // 生效配置文件相对 config 目录的路径（如 "8550/config.yaml"，非处理器时为 "config.yaml"），
  // 由守护进程启动时写入 active_config.txt；WebUI 只读展示，不改动文件。
  async getActiveConfigName(): Promise<string> {
    try {
      const { errno, stdout } = await exec(`cat "${PATHS.ACTIVE_CONFIG}"`);
      const name = stdout.trim();
      if (errno === 0 && name && !name.includes('..')) return name;
    } catch (e) { /* 回退默认 */ }
    return 'config.yaml';
  },

  // 配置文件抬头信息（meta 段：配置名/作者/语言/日志等级），仅供查看
  async getConfigMeta(): Promise<Record<string, any>> {
    try {
      const cfg = yaml.load(await this.readFile(await resolveConfigPath())) || {};
      return (cfg as any).meta || {};
    } catch (e) {
      return {};
    }
  },

  // 切换日志等级：只替换生效 config.yaml 中 meta.loglevel 行的值（保留注释与其余内容），
  // 守护进程 config_watcher 检测到变更后热重载即时生效。
  async setLogLevel(level: string): Promise<void> {
    const path = await resolveConfigPath();
    const content = await this.readFile(path);
    // 字段缺失（异常精简的配置文件）时兜底整文件重写
    const updated = replaceYamlFieldLine(content, 'loglevel', level) ?? (() => {
      const cfg = yaml.load(content) || {};
      if (!(cfg as any).meta) (cfg as any).meta = {};
      (cfg as any).meta.loglevel = level;
      return yaml.dump(cfg);
    })();
    await this.writeFile(path, updated);
    toast(i18n.global.t('loglevel_updated') as string);
  },

  // 开发记录开关：只替换 meta.dev_record 行（布尔裸值，保留注释与其余内容），
  // 热重载即时生效；开启后守护进程向 devimp/ 目录写按核调度诊断日志。
  async setDevRecord(on: boolean): Promise<void> {
    const path = await resolveConfigPath();
    const content = await this.readFile(path);
    const value = on ? 'true' : 'false';
    // 字段缺失时兜底整文件重写（replaceYamlFieldLine 保留原裸值风格，不加引号）
    const updated = replaceYamlFieldLine(content, 'dev_record', value) ?? (() => {
      const cfg = yaml.load(content) || {};
      if (!(cfg as any).meta) (cfg as any).meta = {};
      (cfg as any).meta.dev_record = on;
      return yaml.dump(cfg);
    })();
    await this.writeFile(path, updated);
    toast(i18n.global.t('dev_record_updated') as string);
  },

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
  async setMode(mode: string): Promise<void> {
    const rules = await this.getRulesConfig();
    rules.global_mode = mode;
    await this.saveRulesConfig(rules);
    toast(i18n.global.t('switch_success', { mode: modeLabel(mode) }) as string);
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

  // 设定/清除单个应用的性能模式（写 rules.yaml 的 app_modes，mode 为空串表示清除）
  async saveAppRule(packageName: string, mode: string): Promise<void> {
    const rules = await this.getRulesConfig();
    if (!rules.app_modes) rules.app_modes = {};

    if (mode === '') {
      delete rules.app_modes[packageName];
    } else {
      rules.app_modes[packageName] = mode;
    }

    await this.saveRulesConfig(rules);
    toast(i18n.global.t('app_rules_saved') as string);
  },

  async getDaemonLog(): Promise<string> {
    try {
      const raw = await this.readFile(PATHS.DAEMON_LOG);
      return raw || '';
    } catch (e) {
      return '';
    }
  }
};

export const Bridge = isDev ? MockBridge : RealBridge;
