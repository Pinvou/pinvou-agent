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
const codex = fs.readFileSync(path.join(root, 'src/features/codex/CodexAcpView.jsx'), 'utf8');
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

// 4. cardBtnCls 必须使用 P3 新签名（单参数 variant），不得残留旧签名
//    cardBtnCls(isDark, variant) —— 旧调用会把布尔值当 variant，静默丢失 primary 样式
//    （评审 #168 曾指出 WorkflowView 16 处旧签名调用）。
assert.doesNotMatch(
  wf,
  /cardBtnCls\(\s*isDark/,
  "cardBtnCls 不得再用旧签名 cardBtnCls(isDark, ...)；应改为 cardBtnCls() / cardBtnCls('primary')",
);
assert.match(
  wf,
  /cardBtnCls\('primary'\)/,
  "WorkflowView 主操作（提交/审批/启动/重新开始等）必须用 cardBtnCls('primary') 保持 primary 样式",
);
assert.match(
  wf,
  /cardBtnCls\(\)/,
  "WorkflowView 次要操作必须用 cardBtnCls() 保持默认样式",
);

// 5. cardBtnCls/cardBoxCls 的 P3 单参签名必须覆盖全部消费 View。
//    CodexAcpView 的 NativePlanCard/NativeYoloConfirmCard 曾漏改（旧签名把布尔当
//    variant/accent，主按钮静默退化为默认样式、卡片边框类丢失——评审 #168 指出）。
assert.doesNotMatch(
  codex,
  /cardBtnCls\(\s*isDark/,
  "CodexAcpView 不得再用旧签名 cardBtnCls(isDark, ...)；应改为 cardBtnCls() / cardBtnCls('primary')",
);
assert.doesNotMatch(
  codex,
  /cardBoxCls\(\s*isDark/,
  "CodexAcpView 不得再用旧签名 cardBoxCls(isDark, ...)；accent 应预拼 dark: 串",
);
assert.match(
  codex,
  /cardBtnCls\('primary'\)/,
  "CodexAcpView 主操作（接受方案/确认 yolo）必须用 cardBtnCls('primary') 保持 primary 样式",
);
assert.match(
  codex,
  /cardBtnCls\(\)/,
  "CodexAcpView 次要操作必须用 cardBtnCls() 保持默认样式",
);

console.log('OK: Workflow 「其他」答案契约 + cardBtnCls 签名契约 (kind:other 高亮 + i18n label 三语 + P3 签名全覆盖)');
