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
  DAEMON_LOG: `${MODULE_BASE_PATH}/logs/daemon.log`
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
  async saveRulesConfig(config: any): Promise<void> { await this.writeFile(PATHS.RULES_YAML, yaml.dump(config)); },

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

  // 手动热重启守护进程：先杀掉旧进程，再重新执行 module 的 service.sh 拉起守护进程。
  // 关键点：ksu.exec 返回时会清理执行 shell 所在的进程组，若像之前那样直接
  // `nohup ... &` 后台拉起，新守护进程会作为该 shell 的子进程被一并杀掉（表现为
  // “只杀死了、没启动起来”）。这里用 `setsid` 把 service.sh 开成一个全新会话，
  // 并重定向全部标准流，使其脱离 exec 的进程组存活，等效于手动执行 service.sh 重启。
  async restartDaemon(): Promise<void> {
    const { errno } = await exec(
      `killall -9 yumi 2>/dev/null; sleep 1; ` +
      `setsid "${PATHS.MODULE}/service.sh" </dev/null >/dev/null 2>&1 &`
    );
    if (errno !== 0) throw new Error(i18n.global.t('restart_failed') as string);
  },

  async getCurrentMode(): Promise<string> { try { return (await this.readFile(PATHS.CURRENT_MODE)).trim(); } catch (e) { return 'balance'; } },

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

  // 清理 rules.yaml 中非法特调映射（应用列表扫描完成后调用，与后端门控一致）：
  // 特调模式只允许白名单内且在该包名 modes 列表中的条目；
  // 特调模式集合 = 所有白名单条目 modes 的并集。返回删除条数。
  async pruneSpecialTunedRules(specialTuned: Record<string, { modes: string[]; fallback: string }>): Promise<number> {
    const rules = await this.getRulesConfig();
    if (!rules.app_modes) return 0;
    const specialModes = new Set<string>();
    Object.values(specialTuned).forEach(e => e.modes.forEach(m => specialModes.add(m)));
    let removed = 0;
    Object.keys(rules.app_modes).forEach(pkg => {
      const mode = rules.app_modes[pkg];
      if (specialModes.has(mode)) {
        const entry = specialTuned[pkg];
        if (!entry || !entry.modes.includes(mode)) {
          delete rules.app_modes[pkg];
          removed++;
        }
      }
    });
    if (removed > 0) await this.saveRulesConfig(rules);
    return removed;
  },

  // 读取守护进程启动时导出的内部特调白名单（每行 `包名:特调模式列表(逗号分隔):优先回退模式`）。
  // 该文件由守护进程维护，WebUI 只读用于展示“特调”标签与专属模式选项，不提供修改入口；
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
