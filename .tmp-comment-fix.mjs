// 一次性修复脚本 v2：以 git HEAD blob 原始字节为基准（保留其真实行尾风格），
// 仅插入注释行后写回（行尾与该文件 blob 保持一致）。
import { execSync } from 'node:child_process';
import fs from 'node:fs';

const files = {
  'webui/src/kernelsu/index.js': [
    {
      anchor: 'let callbackCounter = 0;', pos: 'before', lines: [
        '// index.js: [exec] [process-emitter] [spawn] [apis]',
        '// KernelSU WebView JS 桥：封装 ksu.* 原生注入 API',
        '',
        '// [exec] ',
      ]
    },
    {
      anchor: 'function Stdio() {', pos: 'before', lines: [
        '// [process-emitter] ',
        '// 事件发射器：Stdio 数据流与 ChildProcess 退出/错误事件',
      ]
    },
    {
      anchor: 'export function spawn(command, args, options) {', pos: 'before', lines: [
        '// [spawn] ',
      ]
    },
    {
      anchor: 'export function fullScreen(isFullScreen) {', pos: 'before', lines: [
        '// [apis] ',
        '// UI 与模块信息等直通 API',
      ]
    },
  ],
  'webui/src/views/AppRulesView.vue': [
    {
      anchor: '<script setup lang="ts">', pos: 'after', lines: [
        '// AppRulesView.vue: [state] [modes] [scan] [filter] [rule-actions]',
      ]
    },
    { anchor: '// pkg → appLabel 映射', pos: 'before', lines: ['// [state] '] },
    { anchor: '// 动作单：标准四档 + 删除规则。', pos: 'before', lines: ['// [modes] '] },
    { anchor: 'const refreshAppList = async () => {', pos: 'before', lines: ['// [scan] '] },
    { anchor: '// 用应用名或包名都能搜到', pos: 'before', lines: ['// [filter] '] },
    { anchor: 'const openMenu = (pkg: string) => {', pos: 'before', lines: ['// [rule-actions] '] },
  ],
  'webui/src/views/LogViewerView.vue': [
    {
      anchor: '<script setup lang="ts">', pos: 'after', lines: [
        '// LogViewerView.vue: [state] [fetch] [highlight]',
      ]
    },
    { anchor: 'const { t } = useI18n();', pos: 'before', lines: ['// [state] '] },
    { anchor: 'const fetchLog = async () => {', pos: 'before', lines: ['// [fetch] '] },
    { anchor: '// 使用正则对日志内容进行高亮解析', pos: 'before', lines: ['// [highlight] '] },
  ],
};

for (const [file, ops] of Object.entries(files)) {
  fs.copyFileSync(file, file + '.commentbak'); // 备份当前版本

  // 取 HEAD blob 原始内容（不做任何行尾转换）
  const head = execSync(`git show HEAD:${file.replaceAll('\\', '/')}`, { maxBuffer: 10 * 1024 * 1024 }).toString('utf8');
  const eol = head.includes('\r') ? '\r\n' : '\n'; // 跟随该文件 blob 的真实行尾风格
  const body = eol === '\r\n' ? head : head.replaceAll('\r\n', '\n');
  const lines = body.split(/(?<=\n)/);

  const inserts = [];
  for (const op of ops) {
    const idx = lines.findIndex(l => l.trim().startsWith(op.anchor));
    if (idx === -1) throw new Error(`anchor not found in ${file}: ${op.anchor}`);
    const at = op.pos === 'before' ? idx : idx + 1;
    inserts.push({ at, text: op.lines.map(s => s + eol).join('') });
  }
  inserts.sort((a, b) => b.at - a.at);
  for (const ins of inserts) lines.splice(ins.at, 0, ins.text);
  fs.writeFileSync(file, lines.join(''), 'utf8');
  console.log('OK', file, `eol=${eol === '\r\n' ? 'CRLF' : 'LF'}`);
}
