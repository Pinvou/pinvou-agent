import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

// 守卫 PR #151 引入的行为变更：工作流「其他」自由文本答案不再依赖中文字符串做高亮判断。
// 旧逻辑：高亮比较 `label === '其他'`，且答案对象无稳定字段。
// 新逻辑：答案对象带稳定字段 `kind: 'other'`，高亮比较 `?.kind === 'other'`。
// 该字段是纯前端选择态；后端 UserInputAnswer 无 deny_unknown_fields，多余字段被静默丢弃。
//
// 这是一个源码契约守卫（无需 DOM/bundler）：仅断言关键代码形态成立。
const root = fileURLToPath(new URL('../', import.meta.url));
const wf = fs.readFileSync(path.join(root, 'src/features/workflow/WorkflowView.jsx'), 'utf8');
const i18n = fs.readFileSync(path.join(root, 'src/shared/i18n.js'), 'utf8');

// 1. 「其他」答案对象必须带稳定字段 kind:'other'，label 走 i18n。
assert.match(
  wf,
  /kind:\s*'other',\s*label:\s*t\.uiToolRender\.other/,
  "「其他」答案必须构建 { kind: 'other', label: t.uiToolRender.other, ... }",
);

// 2. 高亮判断必须基于 kind，不得回退到中文字面量比较。
assert.match(
  wf,
  /answers\[qi\]\?\.kind\s*===\s*'other'/,
  "「其他」高亮必须用 answers[qi]?.kind === 'other'，不得依赖中文 label",
);
assert.doesNotMatch(
  wf,
  /===\s*'其他'/,
  "不得保留 `=== '其他'` 形式的中文字面量比较",
);

// 3. dict 三语必须提供 uiToolRender.other（zh/en/ja），否则 label 会渲染 undefined。
for (const [lang, expected] of [['zh', '其他'], ['en', 'Other'], ['ja', 'その他']]) {
  const block = i18n.match(new RegExp(`dict\\.${lang}\\.uiToolRender\\s*=\\s*\\{[\\s\\S]*?\\}`));
  assert.ok(block, `dict.${lang}.uiToolRender must exist`);
  assert.match(block[0], new RegExp(`other:\\s*'${expected}'`), `dict.${lang}.uiToolRender.other must be '${expected}'`);
}

console.log('OK: Workflow 「其他」答案契约 (kind:other 高亮 + i18n label 三语)');
