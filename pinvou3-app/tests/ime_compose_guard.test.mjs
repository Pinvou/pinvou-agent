#!/usr/bin/env node
// 回归测试:中文输入法敲回车"确认候选词"时,不应触发业务动作(发送/提交/搜索/重命名等)。
//
// 背景:macOS 上用 CJK 输入法输入拉丁字符(如 test)后按 Enter 上屏,浏览器会派发
// key === 'Enter' 的 keydown。正确做法是把合成中的 Enter 视为"仅 IME 提交",不触发
// 业务动作。
//
// 仅检查 isComposing 在 macOS WKWebView 上不可靠:WebKit 历史缺陷(bug 165004,
// 直到 2026-04 才进入主线修复,无对应发布版本号)会把确认 IME 候选词的 Enter
// keydown 延迟到 compositionend 之后,使 isComposing 此时已为 false;但该 keydown
// 的 keyCode 仍为 229(W3C 规定的 IME 处理中按键标记)。因此守卫必须同时覆盖
// isComposing 与 keyCode === 229,二者集中在 src/shared/ime-guard.mjs 的
// isImeComposing 里,供全 app 所有"文本框 Enter → 业务动作"路径复用;唯一例外是
// design-runtime.js(脚本注入隔离 iframe,无法 ESM import),该处内联等价判断。
//
// 本测试分两层:
//  1) 源码契约(与 pet_reply_contract.test.mjs 同风格):守住每条 Enter 路径都带
//     IME 守卫——React 文本校验调用 isImeComposing,design-runtime 校验内联等价判断
//     (二者都必须覆盖 keyCode === 229,否则会漏掉 WebKit bug 165004 兜底)。
//  2) helper 行为单测:覆盖 isComposing / keyCode === 229 / 普通 Enter / 原生事件
//     四类场景,验证 isImeComposing 的判定正确。新增同类入口时,请同步在契约层补一条断言。
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { isImeComposing } from '../src/shared/ime-guard.mjs';

const here = path.dirname(fileURLToPath(import.meta.url));
const src = (...p) => readFileSync(path.join(here, '..', 'src', ...p), 'utf8');
const assertGuard = (label, code, re) =>
  assert.match(code, re, `${label}必须经 isImeComposing 守卫(勿直接读 isComposing)`);

const chatView = src('features', 'chat', 'ChatView.jsx');
const codexView = src('features', 'codex', 'CodexAcpView.jsx');
const knowledgeView = src('features', 'knowledge', 'KnowledgeView.jsx');
const navigation = src('components', 'layout', 'NavigationComponents.jsx');
const markdownPreview = src('features', 'artifacts', 'EditableMarkdownPreview.jsx');
const designInspector = src('features', 'artifacts', 'DesignInspectorPanel.jsx');
const designRuntime = src('features', 'artifacts', 'design-runtime.js');
const petWindow = src('features', 'pet', 'PetWindow.jsx');

// --- 源码契约:每条"Enter → 业务动作"路径都调用 isImeComposing ----------------
// --- features/chat:发送 / 提交 -------------------------------------------------
// 主输入框 handleKeyDown:Enter 发送需带守卫。
assertGuard(
  '主输入框 handleKeyDown',
  chatView,
  /function handleKeyDown\([^)]*\)\s*\{[\s\S]*e\.key === ['"]Enter['"][\s\S]*!isImeComposing\(e\)[\s\S]*handleSend\(\)/,
);

// 消息编辑框内联 onKeyDown:Enter 提交重发也需带守卫。
assertGuard(
  '消息编辑框 Enter 提交',
  chatView,
  /onKeyDown=\{e => \{ if \(e\.key === ['"]Enter['"] && !e\.shiftKey && !isImeComposing\(e\)\) \{ e\.preventDefault\(\); commit\(\); \}/,
);

// --- features/codex:发送 ------------------------------------------------------
assertGuard(
  '代码会话输入框 Enter 发送',
  codexView,
  /event\.key === ['"]Enter['"] && !event\.shiftKey && !isImeComposing\(event\)/,
);

// --- features/knowledge:搜索 / 新建集合 --------------------------------------
assertGuard(
  '知识库搜索框 Enter',
  knowledgeView,
  /e\.key === ['"]Enter['"] && !isImeComposing\(e\)\) runSearch\(/,
);

assertGuard(
  '新建知识集合名称 Enter',
  knowledgeView,
  /e\.key === ['"]Enter['"] && !isImeComposing\(e\)\) createColl\(\)/,
);

// --- components/layout:会话重命名 --------------------------------------------
assertGuard(
  '会话重命名 Enter',
  navigation,
  /e\.key === ['"]Enter['"] && !isImeComposing\(e\)\) \{ e\.preventDefault\(\); save\(\); \}/,
);

// --- features/artifacts:Markdown AI 编辑 / 设计检查器 ------------------------
assertGuard(
  'Markdown AI 编辑指令 Enter',
  markdownPreview,
  /e\.key === ['"]Enter['"] && !isImeComposing\(e\)\) submitAiEdit\(\)/,
);

assertGuard(
  '设计检查器颜色 hex Enter',
  designInspector,
  /e\.key === ['"]Enter['"] && !isImeComposing\(e\)\) \{[\s\S]*submitColorDraft\(\)/,
);

assertGuard(
  '设计检查器文本元素 Enter',
  designInspector,
  /e\.key === ['"]Enter['"] && !isImeComposing\(e\)\) \{ e\.preventDefault\(\); e\.currentTarget\.blur\(\); \}/,
);

// --- features/artifacts/design-runtime:隔离 iframe 的 contentEditable 文本编辑
// 注意:此处脚本由 buildDesignRuntimeScript 生成并注入隔离 iframe(测试以 vm.runInContext
// 模拟),无法 ESM import,故内联 keyCode === 229 兜底而非调用 isImeComposing。
assertGuard(
  '设计画布文本编辑 Enter',
  designRuntime,
  /event\.key === ['"]Enter['"] && !event\.shiftKey[\s\S]*!\(event\.isComposing \|\| event\.keyCode === 229\)/,
);

// --- features/pet:宠物窗口回复 ------------------------------------------------
assertGuard(
  '宠物窗口回复 Enter',
  petWindow,
  /event\.key === ['"]Enter['"] && !event\.shiftKey\s*&&\s*!isImeComposing\(event\)[\s\S]*submitPetReply/,
);

// --- features/browser:地址栏导航 ----------------------------------------------
const browserView = src('features', 'browser', 'BrowserView.jsx');
assertGuard(
  '浏览器地址栏 Enter 导航',
  browserView,
  /e\.key === ['"]Enter['"] && isImeComposing\(e\)\) e\.preventDefault\(\)/,
);

// --- helper 行为单测:isImeComposing 四类场景 ----------------------------------
// 构造轻量事件对象模拟 KeyboardEvent:React 合成事件带 nativeEvent,原生事件无。
const synth = (isComposing, keyCode) => ({ nativeEvent: { isComposing, keyCode } });
const native = (isComposing, keyCode) => ({ isComposing, keyCode });

// 1) IME 合成中(isComposing === true):标准 W3C 行为,回车仅上屏候选词 → 阻止业务动作。
assert.equal(isImeComposing(synth(true, 13)), true, '合成事件 isComposing=true 应判定为合成中');
assert.equal(isImeComposing(native(true, 13)), true, '原生事件 isComposing=true 应判定为合成中');

// 2) WebKit bug 165004 场景:isComposing 已为 false,但 keyCode === 229 仍保留 → 必须阻止。
//    这是仅靠 isComposing 会漏判的核心场景。
assert.equal(isImeComposing(synth(false, 229)), true, '合成事件 keyCode=229(isComposing=false)应判定为合成中');
assert.equal(isImeComposing(native(false, 229)), true, '原生事件 keyCode=229(isComposing=false)应判定为合成中');

// 3) 普通 Enter:非合成,keyCode=13 → 允许业务动作。
assert.equal(isImeComposing(synth(false, 13)), false, '普通 Enter 应判定为非合成');
assert.equal(isImeComposing(native(false, 13)), false, '原生普通 Enter 应判定为非合成');

// 4) 安全性:缺失字段(理论不应发生)不抛错,回退到非合成以保留旧行为。
assert.equal(isImeComposing(undefined), false, 'undefined 事件应安全返回 false');
assert.equal(isImeComposing({}), false, '空事件应安全返回 false');
assert.equal(isImeComposing({ nativeEvent: null }), false, 'nativeEvent 为 null 应安全返回 false');

console.log('IME compose guard tests passed (12 guarded Enter paths + helper behavior)');
