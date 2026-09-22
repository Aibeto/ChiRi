# 字体

本目录已内置字体，均为 **SIL OFL 1.1**，许可文件随字体同目录：

| 文件 | 来源 | 内容 | 大小 |
| --- | --- | --- | --- |
| `sans-regular.woff` | Noto Sans SC（Google） | 本页实际用字子集，441 字 | 65 KB |
| `sans-bold.woff` | Noto Sans SC Bold | 同上 | 66 KB |
| `mono-regular.woff2` | JetBrains Mono | 拉丁/数字/标点 | 21 KB |
| `mono-bold.woff2` | JetBrains Mono Bold | 同上 | 22 KB |
| `LICENSE-OFL.txt` | — | Noto Sans SC 的 OFL 1.1 全文 | — |
| `LICENSE-OFL-jetbrains-mono.txt` | — | JetBrains Mono 的 OFL 1.1 全文 | — |

字体经 `analyze.css` 的 `@font-face` 注册为 `Analyze Sans` / `Analyze Mono`，并前置到
`--ak-font-sans` / `--ak-font-mono`。文件缺失时自动回退系统字体，不会报错。

## 为什么中文要做子集

整份 Noto Sans SC 简中子集**单字重就有 1.14MB**，两个字重 2.3MB。按本页实际用字
（从 `index.html` 里抽出的全部字符 + 可打印 ASCII）裁到 441 字后，每个字重只剩约 65KB。
这与 `webui/src/assets/fonts/` 的先例一致（那里也按仓库用字裁中文子集，约 145KB/字重）。

## 重新生成子集

```bash
# 取源字体（fontsource 的 woff 版，无需 brotli 即可解包）
curl -o noto-400.woff https://cdn.jsdelivr.net/npm/@fontsource/noto-sans-sc@5.3.0/files/noto-sans-sc-chinese-simplified-400-normal.woff
curl -o noto-700.woff https://cdn.jsdelivr.net/npm/@fontsource/noto-sans-sc@5.3.0/files/noto-sans-sc-chinese-simplified-700-normal.woff

# 字表：本页出现的全部字符 + ASCII
python - <<'PY'
import string
chars = set(open('../index.html', encoding='utf-8').read()) | set(string.printable) | set('—·×→')
open('charset.txt', 'w', encoding='utf-8').write(''.join(sorted(chars)))
print(len(chars))
PY

# 子集化（--flavor=woff 走 zlib，不依赖 brotli）
pyftsubset noto-400.woff --text-file=charset.txt --flavor=woff \
  --layout-features= --no-hinting --desubroutinize --notdef-outline \
  --output-file=sans-regular.woff
pyftsubset noto-700.woff --text-file=charset.txt --flavor=woff \
  --layout-features= --no-hinting --desubroutinize --notdef-outline \
  --output-file=sans-bold.woff
```

`pip install fonttools brotli` 若因环境损坏失败，可直接下载
`fonttools-*-py3-none-any.whl` 解压后把路径加进 `PYTHONPATH` 再跑
`python -m fontTools.subset`。

## 在线加载（备选）

离线优先，但允许在线加载时可以把本地 `url()` 换成 CDN（jsdelivr 同样分发 OFL 包）：

```css
@font-face {
  font-family: 'Analyze Sans';
  src: url('https://cdn.jsdelivr.net/npm/@fontsource/noto-sans-sc@5.3.0/files/noto-sans-sc-chinese-simplified-400-normal.woff2')
    format('woff2');
  font-weight: 400;
  font-display: swap;
}
```

代价是首屏依赖网络、整份子集 1.14MB；本地子集只有 65KB，所以默认仍用本地文件。

## 合规边界

SIL OFL 1.1 的关键约束：

- 可以**使用、修改、捆绑、随软件再分发**
- **不能单独出售**字体本身
- 衍生字体**必须继续用 OFL**，且不得使用保留字体名（如 "Noto"）
- 许可文件必须随字体分发

**不能放**：Windows 的微软雅黑（Microsoft YaHei）、SimSun、SimHei、DengXian，
macOS 的 PingFang / SF —— 都是专有许可，从系统字体目录复制一份再分发也不行。
