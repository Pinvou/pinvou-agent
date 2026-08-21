/**
 * 上传包展示名/说明编辑（edit_display 动作）的前端接线契约：
 * 后端 actions.rs 对 source=Upload 的已装包下发 edit_display；前端 specs 映射、
 * 编辑对话框与 update_bundle_display_meta 调用必须同宗。仿
 * tool_store_skill_drop_contract.test.mjs 的正则契约风格（不启动浏览器）。
 */
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';

const toolCommonSource = await readFile(
  new URL('../src/features/tools/tool-common.jsx', import.meta.url),
  'utf8',
);
const toolStoreSource = await readFile(
  new URL('../src/features/tools/ToolStoreView.jsx', import.meta.url),
  'utf8',
);
const i18nSource = await readFile(
  new URL('../src/shared/i18n.js', import.meta.url),
  'utf8',
);

// 1. TsActionBtn 的 specs 映射认识 edit_display 动作，点击路由到 onEditDisplay。
assert.match(
  toolCommonSource,
  /edit_display:\s*\{\s*label:\s*T\.editDisplay/,
  'specs must map the backend edit_display action to the T.editDisplay label',
);
assert.match(
  toolCommonSource,
  /onEditDisplay && onEditDisplay\(tool\.backendId\)/,
  'edit_display click must route to onEditDisplay(tool.backendId)',
);

// 2. ToolStoreView 把 handleEditDisplay 传给两处 PlatformToolAction（列表卡 + 详情卡）。
const propPasses = toolStoreSource.match(/onEditDisplay=\{handleEditDisplay\}/g) || [];
assert.equal(
  propPasses.length,
  2,
  'both PlatformToolAction sites must receive onEditDisplay',
);

// 2a. 上传 MCP/组合包必须进 readiness 批量取数：edit_display 动作与展示名覆盖都
// 来自 bundle_readiness，这类 id 不在 tsToolsData/skillList 里，漏并入则编辑按钮
// 永不渲染、卡面也不消费覆盖值（后端 bundle_readiness 已下发，前端断路）。
assert.match(
  toolStoreSource,
  /customToolIds/,
  'uploaded MCP/combo ids must join the readiness batch (customToolIds)',
);
assert.match(
  toolStoreSource,
  /\.\.\.tsToolsData\.map\(x => x\.backendId\)\.filter\(Boolean\),\s*\.\.\.customToolIds,/,
  'readiness batch ids must include customToolIds',
);

// 2b. 自定义 MCP 卡面标题/说明优先消费 readiness bundle 生效值（后端已应用 extra
// 覆盖），否则编辑保存成功后卡面不变。
assert.match(
  toolStoreSource,
  /title: \(bf && bf\.name\) \|\| x\.name \|\| x\.id/,
  'custom MCP card title must prefer the readiness bundle name (override applied)',
);
assert.match(
  toolStoreSource,
  /desc: \(bf && bf\.description\) \|\| x\.description \|\| ''/,
  'custom MCP card desc must prefer the readiness bundle description (override applied)',
);

// 3. 保存调 update_bundle_display_meta（camelCase 参数映射），成功后刷新列表。
assert.match(
  toolStoreSource,
  /invokeTauri\('update_bundle_display_meta',\s*\{[^}]*displayName:[^}]*displayDescription:[^}]*\}\)/s,
  'save must invoke update_bundle_display_meta with camelCase display meta args',
);
assert.match(
  toolStoreSource,
  /TsEditDisplayDialog/,
  'the edit dialog must be rendered',
);

// 4. 预填当前覆盖值：读 bundle 事实的 display_name / display_description 原值。
assert.match(
  toolStoreSource,
  /bf\.display_name/,
  'dialog prefill must read the raw display_name override from bundle facts',
);
assert.match(
  toolStoreSource,
  /bf\.display_description/,
  'dialog prefill must read the raw display_description override from bundle facts',
);

// 5. 三语词条：uiToolCommon.editDisplay 与 uiToolStore 的对话框 key 三语齐全
// （结构性奇偶由 ui_language_coverage.test.mjs 兜底，这里钉关键 key 存在）。
for (const lang of ['zh', 'en', 'ja']) {
  assert.match(
    i18nSource,
    new RegExp(`dict\\.${lang}\\.uiToolCommon = \\{[\\s\\S]*?editDisplay:`),
    `uiToolCommon.editDisplay missing for ${lang}`,
  );
}
for (const key of [
  'editDisplayTitle',
  'displayNameLabel',
  'displayDescriptionLabel',
  'editDisplayHint',
  'editDisplaySave',
  'editDisplaySaved',
]) {
  const occurrences = i18nSource.match(new RegExp(`${key}:`, 'g')) || [];
  assert.equal(occurrences.length, 3, `${key} must exist in all three uiToolStore dicts`);
}

console.log('tool store edit-display contract tests passed');
