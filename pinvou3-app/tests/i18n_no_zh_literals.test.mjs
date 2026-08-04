import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';

// 单引号或双引号或反引号包裹、含 CJK、且不在注释行内的字符串字面量。
// 注释行以 // 或 * 开头（去前导空白后）。
const files = [
  'src/features/tools/ToolStoreView.jsx',
  'src/features/workflow/WorkflowView.jsx',
];
const root = path.resolve(new URL('../', import.meta.url).pathname);

function countActiveZhLiterals(content) {
  const lines = content.split('\n');
  let count = 0;
  for (const line of lines) {
    const trimmed = line.trimStart();
    if (trimmed.startsWith('//') || trimmed.startsWith('*')) continue; // 注释
    // 匹配引号内含 CJK 的串（简化的 CJK 范围）
    const re = /['"`][^'"`\n]*[\u4e00-\u9fff][^'"`\n]*['"`]/g;
    const matches = line.match(re);
    if (matches) count += matches.length;
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
