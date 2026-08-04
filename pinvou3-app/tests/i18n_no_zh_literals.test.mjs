import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';

// 单引号 / 双引号 / 反引号包裹、含 CJK、且属于「界面文案」的活跃字符串字面量。
// 整行注释（以 // 或 * 开头，去前导空白后）直接跳过。
const files = [
  'src/features/tools/ToolStoreView.jsx',
  'src/features/workflow/WorkflowView.jsx',
];
const root = path.resolve(new URL('../', import.meta.url).pathname);

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
assert.equal(total, 0, `Expected 0 active CJK string literals in ToolStoreView/WorkflowView, found ${total}`);
console.log('OK: no active CJK string literals in ToolStoreView/WorkflowView');
