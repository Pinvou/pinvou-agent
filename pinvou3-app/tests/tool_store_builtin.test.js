#!/usr/bin/env node
/**
 * 内置插件板块源码守卫（《内置工具集长期契约》§3.1/§3.2）：
 * - ToolStoreView：builtin === true 从常规卡片流排除；工具栏独立「内置插件」按钮进入
 *   专属只读子页（仿回收站子页），子页整页只读、带返回；
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

// 独立入口 + 专属子页：按钮进子页、子页带返回、整页只读渲染内置卡列表
assert.match(storeView, /toolBackend\r?\n\s*\.filter\(isBuiltinPlugin\)/, '内置插件数据须来自 toolBackend 的 isBuiltinPlugin 过滤');
assert.match(storeView, /data-testid="tool-store-builtin-plugins" onClick=\{\(\) => setShowBuiltinPlugins\(true\)\}/, '工具栏须有内置插件入口按钮');
assert.match(storeView, /\{showBuiltinPlugins && \(/, '内置插件子页须按 showBuiltinPlugins 渲染');
assert.match(storeView, /data-testid="builtin-plugins-back" onClick=\{\(\) => setShowBuiltinPlugins\(false\)\}/, '内置插件子页须有返回按钮');
assert.match(storeView, /data-testid="builtin-plugin-list"/, '内置插件子页须渲染只读列表');
assert.match(storeView, /<BuiltinPluginCard tool=\{tool\} copy=\{builtinCopy\} \/>/, '内置插件子页只读卡渲染');
assert.match(storeView, /\{!showRecycleBin && !showBuiltinPlugins && \(/, '主列表须在任一子页打开时让位');
assert.doesNotMatch(storeView, /id: 'builtin-plugins'/, '内置插件不得再作为主列表 section 出现');

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
