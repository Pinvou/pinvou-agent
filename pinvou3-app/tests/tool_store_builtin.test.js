#!/usr/bin/env node
/**
 * 内置插件板块源码守卫（《内置工具集长期契约》§3.1/§3.2）：
 * - ToolStoreView：builtin === true 独立成只读区、常规卡片流排除、只读分支不渲染动作列；
 * - tool-common：BuiltinPluginCard 无任何动作按钮；TsActionBtn builtin 分支先于 uninstall 回退分支；
 * - composer-tool-menu-logic：builtin !== true 过滤存在（§3.2 配置可见性）。
 * 渲染层三语文案存在性由 ui_language_coverage.test.mjs 锁定。
 */
const assert = require('assert');
const fs = require('fs');
const path = require('path');

const read = (rel) => fs.readFileSync(path.join(__dirname, '..', 'src', rel), 'utf8');

const storeView = read('features/tools/ToolStoreView.jsx');
const toolCommon = read('features/tools/tool-common.jsx');
const composerLogic = read('features/settings/composer-tool-menu-logic.js');
const builtinLogic = read('features/tools/builtin-plugin-logic.js');

// 共享判定：严格 builtin === true（缺省/真值非 true 按普通插件放行）
assert.match(builtinLogic, /tool\.builtin === true/, 'isBuiltinPlugin 须严格 === true');

// ToolStoreView：常规商店卡片流排除内置插件（customMcpTools 过滤，含搜索结果）
assert.match(storeView, /\.filter\(x => !isBuiltinPlugin\(x\) && tsToolsData\.every/, '常规卡片流必须排除内置插件');

// 内置板块：数据来自 toolBackend 的 isBuiltinPlugin 过滤，独立 section 带 builtin 标记
assert.match(storeView, /toolBackend\r?\n\s*\.filter\(isBuiltinPlugin\)/, '内置板块数据须来自 toolBackend 的 isBuiltinPlugin 过滤');
assert.match(storeView, /id: 'builtin-plugins'/, '内置板块须使用独立 section id');
assert.match(storeView, /items: builtinPluginCards, builtin: true/, '内置板块 section 须带 builtin 标记');

// 只读分支：section.builtin 走 BuiltinPluginCard，不渲染动作列、不进详情
assert.match(storeView, /section\.builtin \? \(/, '内置板块须走只读渲染分支');
assert.match(storeView, /<BuiltinPluginCard key=\{`list-\$\{tool\.id\}`\} tool=\{tool\} copy=\{builtinCopy\} \/>/, '内置板块只读卡渲染');

// BuiltinPluginCard 组件体：无 TsActionBtn/PlatformToolAction/uninstall/onAction
// （契约 §3.1：无卸载、无开关）；渲染只读徽章；无硬编码中文（走 uiBuiltinPlugins）
const cardStart = toolCommon.indexOf('const BuiltinPluginCard');
assert.ok(cardStart > 0, 'BuiltinPluginCard 组件须存在');
const cardBody = toolCommon.slice(cardStart, toolCommon.indexOf('export {', cardStart));
assert.doesNotMatch(cardBody, /TsActionBtn|PlatformToolAction|uninstall|onAction|handleAction/, '内置卡不得渲染任何动作按钮');
assert.match(cardBody, /\{C\.readonlyBadge\}/, '内置卡须渲染只读徽章');
assert.doesNotMatch(cardBody, /[一-鿿]/, '内置卡组件体不得出现硬编码中文');

// TsActionBtn：builtin 只读徽章分支必须先于无 actions 的 uninstall 回退分支
// （即便内置卡误入 TsActionBtn，uninstall 也不可达）
const builtinBranch = toolCommon.indexOf('if (tool.builtin)');
const uninstallFallback = toolCommon.indexOf('无 actions 时的旧分支');
assert.ok(builtinBranch > 0, 'TsActionBtn 须有 builtin 只读分支');
assert.ok(uninstallFallback > builtinBranch, 'builtin 分支必须先于 uninstall 回退分支');

// composer 输入框菜单：内置插件过滤（§3.2 配置可见性）
assert.match(composerLogic, /tool\.builtin !== true/, 'composer 菜单须过滤 builtin 工具');

// 时间线 ToolCard 执行可见性不受影响：tool-renderers.jsx 不得引入 builtin 过滤
const toolRenderers = read('features/tools/tool-renderers.jsx');
assert.doesNotMatch(toolRenderers, /isBuiltinPlugin|\.builtin\b/, '时间线 ToolCard 不得按 builtin 过滤（执行可见性）');

console.log('tool_store_builtin_smoke: ok');
