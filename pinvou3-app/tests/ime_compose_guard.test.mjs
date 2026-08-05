#!/usr/bin/env node
// 回归测试:中文输入法敲回车"确认候选词"时,不应触发业务动作(发送/提交/搜索/重命名等)。
//
// 背景:macOS 上用 CJK 输入法输入拉丁字符(如 test)后按 Enter 上屏,浏览器会派发
// key === 'Enter' 且 nativeEvent.isComposing === true 的 keydown。正确做法是把合成
// 中的 Enter 视为"仅 IME 提交",不触发业务动作。早期版本多处文本框的 Enter 路径都
// 漏判了 isComposing,导致一次回车既上屏又触发动作(误发消息/误搜索/误重命名等)。
//
// 这里用源码字符串契约断言(与 pet_reply_contract.test.mjs 同风格)守住全 app 所有
// "文本框 Enter → 业务动作"路径都带 isComposing 守卫。React 合成事件查
// e.nativeEvent.isComposing;design 预览运行时里隔离 iframe 的原生 DOM 事件查
// event.isComposing。新增同类入口时,请同步在此补一条断言,避免回归。
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const src = (...p) => readFileSync(path.join(here, '..', 'src', ...p), 'utf8');
const assertGuard = (label, code, re) =>
  assert.match(code, re, `${label}必须带 !isComposing 守卫`);

const chatView = src('features', 'chat', 'ChatView.jsx');
const codexView = src('features', 'codex', 'CodexAcpView.jsx');
const knowledgeView = src('features', 'knowledge', 'KnowledgeView.jsx');
const navigation = src('components', 'layout', 'NavigationComponents.jsx');
const markdownPreview = src('features', 'artifacts', 'EditableMarkdownPreview.jsx');
const designInspector = src('features', 'artifacts', 'DesignInspectorPanel.jsx');
const designRuntime = src('features', 'artifacts', 'design-runtime.js');

// --- features/chat:发送 / 提交 -------------------------------------------------
// 主输入框 handleKeyDown:Enter 发送需带守卫。
assertGuard(
  '主输入框 handleKeyDown',
  chatView,
  /function handleKeyDown\([^)]*\)\s*\{[\s\S]*e\.key === ['"]Enter['"][\s\S]*!e\.nativeEvent\.isComposing[\s\S]*handleSend\(\)/,
);

// 消息编辑框内联 onKeyDown:Enter 提交重发也需带守卫。
assertGuard(
  '消息编辑框 Enter 提交',
  chatView,
  /onKeyDown=\{e => \{ if \(e\.key === ['"]Enter['"] && !e\.shiftKey && !e\.nativeEvent\.isComposing\) \{ e\.preventDefault\(\); commit\(\); \}/,
);

// --- features/codex:发送 ------------------------------------------------------
assertGuard(
  '代码会话输入框 Enter 发送',
  codexView,
  /event\.key === ['"]Enter['"] && !event\.shiftKey && !event\.nativeEvent\.isComposing/,
);

// --- features/knowledge:搜索 / 新建集合 --------------------------------------
assertGuard(
  '知识库搜索框 Enter',
  knowledgeView,
  /e\.key === ['"]Enter['"] && !e\.nativeEvent\.isComposing\) runSearch\(/,
);

assertGuard(
  '新建知识集合名称 Enter',
  knowledgeView,
  /e\.key === ['"]Enter['"] && !e\.nativeEvent\.isComposing\) createColl\(\)/,
);

// --- components/layout:会话重命名 --------------------------------------------
assertGuard(
  '会话重命名 Enter',
  navigation,
  /e\.key === ['"]Enter['"] && !e\.nativeEvent\.isComposing\) \{ e\.preventDefault\(\); save\(\); \}/,
);

// --- features/artifacts:Markdown AI 编辑 / 设计检查器 ------------------------
assertGuard(
  'Markdown AI 编辑指令 Enter',
  markdownPreview,
  /e\.key === ['"]Enter['"] && !e\.nativeEvent\.isComposing\) submitAiEdit\(\)/,
);

assertGuard(
  '设计检查器颜色 hex Enter',
  designInspector,
  /e\.key === ['"]Enter['"] && !e\.nativeEvent\.isComposing\) \{[\s\S]*submitColorDraft\(\)/,
);

assertGuard(
  '设计检查器文本元素 Enter',
  designInspector,
  /e\.key === ['"]Enter['"] && !e\.nativeEvent\.isComposing\) \{ e\.preventDefault\(\); e\.currentTarget\.blur\(\); \}/,
);

// --- features/artifacts/design-runtime:隔离 iframe 的 contentEditable 文本编辑
// 注意:此处是原生 DOM 事件(非 React),直接查 event.isComposing。
assertGuard(
  '设计画布文本编辑 Enter',
  designRuntime,
  /event\.key === ['"]Enter['"] && !event\.shiftKey && !event\.isComposing/,
);

console.log('IME compose guard tests passed (10 guarded Enter paths)');
