import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

// 仅扫描本 PR 清理过的文件（工具商店 + 会话引用控件）中「引号内含 CJK 的活跃字面量」。
// 注意：这是针对性回归守卫，不是全仓 i18n 完整审计——它只看引号包裹的串，
// 不覆盖 JSX 文本节点（<div>中文</div>）、正则字面量（/中文/）或多行模板串。
// 整行注释（以 // 或 * 开头，去前导空白后）直接跳过。
const files = [
  'src/features/tools/ToolStoreView.jsx',
  // Session mention UI (PR #586): all copy comes from uiSessionMention in
  // shared/i18n (zh/en/ja); the injection contract in session-mention.js is
  // deliberately English model-context protocol, not UI copy.
  'src/features/chat/SessionMentionControls.jsx',
  // ChatView is NOT listed (round-18 m10, attempted and reverted): the
  // guard's scanner reports 12 pre-existing CJK literals in it — data-key
  // comparisons against backend status labels ('已忽略'/'不再提示'/… at the
  // review-status map, the '悟' transfer marker), not rendered UI copy, and
  // all predate this PR. Extending the guard needs those keyed maps
  // refactored to symbolic labels first; disclosed rather than smuggled
  // past an allowlist.
  'src/features/chat/session-mention.js',
  // Computer-use consent surfaces (virtual-display wave): every string comes
  // from uiComputerUse in shared/i18n (zh/en/ja), including the virtual
  // banner note (bannerNoteVirtual) and the target-aware dialog copy.
  // Scope of the pin below: this scanner enforces the CJK-IDEOGRAPH half of
  // that claim (quoted [\u4e00-\u9fff] literals); a hardcoded English or
  // kana-only literal here would pass it — full parity is the i18n key
  // tests' job.
  'src/features/computer-use/ComputerUseConsent.jsx',
  // Pure view-function logic for the same feature: carries no UI copy.
  'src/features/computer-use/computer-use-logic.js',
  // SettingsView is NOT listed: the scanner reports 2 pre-existing CJK
  // literals in it — the language picker's endonym labels ('中文', '日本語' on
  // the SSegmented options line), which are DELIBERATE: a language selector
  // names every language in its own script, so these are not single-language
  // UI copy and localizing them would be wrong. Disclosed per the ChatView
  // precedent rather than weakening the scanner with an endonym exemption.
  // All other SettingsView copy goes through shared/i18n.
];
// 用 fileURLToPath 而非 new URL(...).pathname：后者在 Windows 上会得到 '/D:/...',
// 再经 path.resolve 会拼成 'D:\D:\...' 双盘符路径导致 ENOENT。
const root = fileURLToPath(new URL('../', import.meta.url));

// 去掉行内尾随 `// ...` 注释，但保留引号字符串内的 `//`（例如 URL 'https://...'）。
// 实现：逐字符扫描，跟踪当前是否身处引号（' " `）内；仅在「不在引号内」时遇到
// 连续两个 `/` 才视为注释起点并截断；转义字符（如 \'）被跳过以免误判引号闭合。
function stripTrailingLineComment(line) {
  let quote = null;
  for (let i = 0; i < line.length; i++) {
    const ch = line[i];
    if (quote) {
      if (ch === '\\') { i += 1; continue; }
      if (ch === quote) quote = null;
    } else if (ch === "'" || ch === '"' || ch === '`') {
      quote = ch;
    } else if (ch === '/' && line[i + 1] === '/') {
      return line.slice(0, i);
    }
  }
  return line;
}

function countActiveZhLiterals(content) {
  const lines = content.split('\n');
  let count = 0;
  // 匹配引号内含 CJK 的串（简化的 CJK 范围 \u4e00-\u9fff）。
  const re = /['"`][^'"`\n]*[\u4e00-\u9fff][^'"`\n]*['"`]/g;
  for (const line of lines) {
    const trimmed = line.trimStart();
    if (trimmed.startsWith('//') || trimmed.startsWith('*')) continue; // 整行注释
    const code = stripTrailingLineComment(line);
    // 豁免：console.warn/error/log/info/debug 的字符串实参属于「开发者诊断」，
    // 不属于 AGENTS.md §4 约束的「界面文案」，故这些调用位置之后的 CJK 串不计入。
    const consoleIdx = code.search(/console\.(warn|error|log|info|debug)\s*\(/);
    re.lastIndex = 0;
    let m;
    while ((m = re.exec(code)) !== null) {
      if (consoleIdx !== -1 && m.index >= consoleIdx) continue; // console.* 诊断实参
      count += 1;
    }
  }
  return count;
}

let total = 0;
for (const rel of files) {
  const full = path.join(root, rel);
  const content = fs.readFileSync(full, 'utf8');
  const n = countActiveZhLiterals(content);
  total += n;
  if (n > 0) console.error(`${rel}: ${n} active CJK literal(s) remaining`);
}
assert.equal(total, 0, `Expected 0 active CJK string literals in the checked feature views, found ${total}`);
console.log('OK: no active CJK string literals in checked feature views');
