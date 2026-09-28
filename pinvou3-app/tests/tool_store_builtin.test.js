#!/usr/bin/env node
/**
 * Source guard for the builtin plugins section
 * (docs/builtin-toolset-contract.md §3.1/§3.2):
 * - ToolStoreView: entries matching the shared isBuiltinPlugin judgement are
 *   excluded from the regular card flow; a dedicated "Builtin Plugins" toolbar
 *   button opens a read-only subpage (mirroring the recycle-bin subpage) with a
 *   back button;
 * - tool-common: BuiltinPluginCard renders no action buttons; the TsActionBtn
 *   builtin branch comes before the uninstall fallback branch;
 * - composer-tool-menu-logic: the isBuiltinPlugin filter is present (§3.2
 *   configuration visibility).
 * Trilingual copy existence on the render layer is pinned by
 * ui_language_coverage.test.mjs.
 */
const assert = require('assert');
const fs = require('fs');
const path = require('path');

const read = (rel) => fs.readFileSync(path.join(__dirname, '..', 'src', rel), 'utf8');

const storeView = read('features/tools/ToolStoreView.jsx');
const toolCommon = read('features/tools/tool-common.jsx');
const composerLogic = read('features/settings/composer-tool-menu-logic.js');
const builtinLogic = read('features/tools/builtin-plugin-logic.js');

// Shared judgement: strict builtin === true or manifest visibility "system"
// (missing fields / truthy non-true values pass through as regular plugins)
assert.match(builtinLogic, /tool\.builtin === true/, 'isBuiltinPlugin must keep the strict === true check');
assert.match(builtinLogic, /tool\.visibility === 'system'/, 'isBuiltinPlugin must accept visibility: "system"');

// ToolStoreView: builtin plugins are excluded from the regular store card flow
// (customMcpTools filter, including search results)
assert.match(storeView, /\.filter\(x => !isBuiltinPlugin\(x\) && tsToolsData\.every/, 'the regular card flow must exclude builtin plugins');

// Dedicated entry + subpage: button opens the subpage, the subpage has a back
// button, and the whole page renders the read-only builtin card list
assert.match(storeView, /toolBackend\r?\n\s*\.filter\(isBuiltinPlugin\)/, 'builtin plugin data must come from toolBackend filtered by isBuiltinPlugin');
assert.match(storeView, /data-testid="tool-store-builtin-plugins" onClick=\{\(\) => setShowBuiltinPlugins\(true\)\}/, 'the toolbar must have a builtin plugins entry button');
assert.match(storeView, /\{showBuiltinPlugins && \(/, 'the builtin plugins subpage must render on showBuiltinPlugins');
assert.match(storeView, /data-testid="builtin-plugins-back" onClick=\{\(\) => setShowBuiltinPlugins\(false\)\}/, 'the builtin plugins subpage must have a back button');
assert.match(storeView, /data-testid="builtin-plugin-list"/, 'the builtin plugins subpage must render the read-only list');
assert.match(storeView, /<BuiltinPluginCard tool=\{tool\} copy=\{builtinCopy\} \/>/, 'the builtin plugins subpage renders read-only cards');
assert.match(storeView, /\{!showRecycleBin && !showBuiltinPlugins && \(/, 'the main list must yield while either subpage is open');
assert.doesNotMatch(storeView, /id: 'builtin-plugins'/, 'builtin plugins must not reappear as a main-list section');

// Builtin skills (visual design) join the same subpage: builtin === true
// entries of tsSkillsData merge into builtinPluginCards (the store keeps the
// feature card; the subpage is the transparency window). The card carries
// kind and version rows, and its copy goes through the same overlay as the
// store skill cards (localizeSkill → uiToolStore.storeData.skills) so en/ja
// render localized text.
assert.match(storeView, /tsSkillsData\r?\n\s*\.filter\(x => x\.builtin === true\)/, 'builtin skills must merge into the builtin plugins subpage');
assert.match(storeView, /kindLabel: \(storeCopy\.typeGroups \|\| \{\}\)\[/, 'builtin skill cards must carry a localized kind row');
assert.match(storeView, /localizeSkill\(x\)/, 'builtin skill cards must use the storeData.skills overlay');

// BuiltinPluginCard body: no TsActionBtn/PlatformToolAction/uninstall/onAction
// (docs/builtin-toolset-contract.md §3.1: no uninstall, no toggle); renders the
// read-only badge; no hardcoded Chinese (copy comes from uiBuiltinPlugins)
const cardStart = toolCommon.indexOf('const BuiltinPluginCard');
assert.ok(cardStart > 0, 'the BuiltinPluginCard component must exist');
const cardBody = toolCommon.slice(cardStart, toolCommon.indexOf('export {', cardStart));
assert.doesNotMatch(cardBody, /TsActionBtn|PlatformToolAction|uninstall|onAction|handleAction/, 'the builtin card must not render any action button');
assert.match(cardBody, /\{C\.readonlyBadge\}/, 'the builtin card must render the read-only badge');
assert.doesNotMatch(cardBody, /[一-鿿]/, 'the builtin card body must not contain hardcoded Chinese');

// TsActionBtn: the builtin read-only badge branch must come before the
// no-actions uninstall fallback branch (even if a builtin card ever reaches
// TsActionBtn, uninstall stays unreachable)
const builtinBranch = toolCommon.indexOf('if (tool.builtin)');
const uninstallFallback = toolCommon.indexOf('无 actions 时的旧分支');
assert.ok(builtinBranch > 0, 'TsActionBtn must have a builtin read-only branch');
assert.ok(uninstallFallback > builtinBranch, 'the builtin branch must come before the uninstall fallback branch');

// Composer input menu: builtin plugin filtering (§3.2 configuration
// visibility) via the shared judgement imported from builtin-plugin-logic.js
assert.match(composerLogic, /import \{ isBuiltinPlugin \} from '\.\.\/tools\/builtin-plugin-logic\.js'/, 'composer logic must import the shared isBuiltinPlugin judgement');
assert.match(composerLogic, /!isBuiltinPlugin\(tool\)/, 'the composer menu must filter builtin tools');

// Timeline ToolCard execution visibility is unaffected: tool-renderers.jsx
// must not introduce builtin filtering
const toolRenderers = read('features/tools/tool-renderers.jsx');
assert.doesNotMatch(toolRenderers, /isBuiltinPlugin|\.builtin\b/, 'timeline ToolCards must not filter by builtin (execution visibility)');

console.log('tool_store_builtin: ok');
