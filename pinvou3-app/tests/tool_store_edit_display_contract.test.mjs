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
// 批次来源必须是本次刚拉回的 list（stale-closure 陷阱：闭包里的 toolBackend
// state 是渲染快照，首挂载/刚上传后为空 → 上传 MCP 卡漏批直到下次无关刷新）。
assert.match(
  toolStoreSource,
  /customToolIds/,
  'uploaded MCP/combo ids must join the readiness batch (customToolIds)',
);
assert.match(
  toolStoreSource,
  /const customToolIds = \(Array\.isArray\(list\) \? list : \[\]\)/,
  'customToolIds must derive from the fresh list fetch, not the stale toolBackend state',
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
// 3-. 弹窗按 backendId 强制重挂载：弹窗开着时拖放导入会原位替换 editDisplay，
// 无 key 则输入框保留旧包的 useState 初值、保存却写进新包 id（串台）。
assert.match(
  toolStoreSource,
  /<TsEditDisplayDialog\s+key=\{editDisplay\.backendId\}/,
  'the dialog must remount per target (key={editDisplay.backendId}) or inputs leak across packages',
);
// 3a. 保存失败保留弹窗与输入：doEditDisplaySave 返回错误文案（而非先卸载弹窗），
// 对话框内联展示；成功路径通知 composer 菜单刷新（覆盖名进了它的数据源）。
assert.match(
  toolStoreSource,
  /const err = await onConfirm\(\{ name, description: desc \}\);/,
  'dialog confirm must surface the save error inline (keep dialog open, inputs retained)',
);
// notify 必须钉在保存流程内（全文件共有 18 处 notifyComposerToolsChanged，
// 泛匹配不构成「保存后通知」的契约）：从 update 调用起向后取一个窗口断言。
{
  const idx = toolStoreSource.indexOf("invokeTauri('update_bundle_display_meta'");
  assert.ok(idx >= 0, 'update_bundle_display_meta invoke must exist');
  const saveFlowWindow = toolStoreSource.slice(idx, idx + 600);
  assert.match(
    saveFlowWindow,
    /notifyComposerToolsChanged\(\);/,
    'successful save must notify the composer tool menu (name override feeds its data source)',
  );
}
// 3-. 保存入口须拒绝在拖放导入进行中触发：busyId 槽位全局唯一，保存若覆盖
// '__upload__'，其 finally 会提前放开拖放闸，成功后的 setEditDisplay(null)
// 会误关导入刚自动弹出的预填对话框。
assert.match(
  toolStoreSource,
  /if \(busyRef\.current\) return storeCopy\.importingSkill;/,
  'display save must refuse while a drop-import is in flight (single busyId slot)',
);
// 3b. 输入框长度上限与后端校验一致（64/240 字符）：前端常量逐字对齐 Rust 侧
// MAX_DISPLAY_NAME_CHARS / MAX_DISPLAY_DESCRIPTION_CHARS（跨端单点真源防漂移）。
{
  const storeRs = await readFile(
    new URL('../src-tauri/src/features/marketplace/store.rs', import.meta.url),
    'utf8',
  );
  const rustMax = (name) => {
    const m = storeRs.match(new RegExp(`pub const ${name}: usize = (\\d+);`));
    assert.ok(m, `${name} must exist in store.rs`);
    return Number(m[1]);
  };
  assert.match(
    toolStoreSource,
    new RegExp(`const MAX_DISPLAY_NAME_CHARS = ${rustMax('MAX_DISPLAY_NAME_CHARS')};`),
    'frontend name cap must equal the Rust MAX_DISPLAY_NAME_CHARS',
  );
  assert.match(
    toolStoreSource,
    new RegExp(
      `const MAX_DISPLAY_DESCRIPTION_CHARS = ${rustMax('MAX_DISPLAY_DESCRIPTION_CHARS')};`,
    ),
    'frontend description cap must equal the Rust MAX_DISPLAY_DESCRIPTION_CHARS',
  );
  assert.match(
    toolStoreSource,
    /maxLength=\{MAX_DISPLAY_NAME_CHARS\}/,
    'name input must cap at MAX_DISPLAY_NAME_CHARS',
  );
  assert.match(
    toolStoreSource,
    /maxLength=\{MAX_DISPLAY_DESCRIPTION_CHARS\}/,
    'description input must cap at MAX_DISPLAY_DESCRIPTION_CHARS',
  );
}

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
// （词典已按语言拆分为 src/shared/i18n/{zh,en,ja}.js 惰性 chunk；结构性奇偶
// 由 ui_language_coverage.test.mjs 兜底，这里钉关键 key 存在）。
const dictNames = { zh: 'dictZh', en: 'dictEn', ja: 'dictJa' };
const chunkSources = {};
for (const lang of ['zh', 'en', 'ja']) {
  chunkSources[lang] = await readFile(
    new URL(`../src/shared/i18n/${lang}.js`, import.meta.url),
    'utf8',
  );
}
for (const lang of ['zh', 'en', 'ja']) {
  assert.match(
    chunkSources[lang],
    new RegExp(`${dictNames[lang]}\\.uiToolCommon = \\{[\\s\\S]*?editDisplay:`),
    `uiToolCommon.editDisplay missing for ${lang}`,
  );
}
for (const key of [
  'editDisplayTitle',
  'displayNameLabel',
  'displayNamePlaceholder',
  'displayDescriptionLabel',
  'displayDescriptionPlaceholder',
  'editDisplayHint',
  'editDisplaySave',
  'editDisplaySaved',
]) {
  const occurrences = ['zh', 'en', 'ja']
    .flatMap((lang) => chunkSources[lang].match(new RegExp(`${key}:`, 'g')) || []);
  assert.equal(occurrences.length, 3, `${key} must exist in all three uiToolStore dicts`);
}

// 6. 导入成功后即打开展示信息编辑弹窗：导入命令返回新包 id（None/null=用户取消），
// 前端据 id 拉 bundle_readiness 取后端生效默认名预填（extra 覆盖 > 上传文件名/
// manifest 回退），用户可直接保存或改名。
assert.match(
  toolStoreSource,
  /const newId = await invokeFn\(\);/,
  'import flow must read the new package id returned by the import command',
);
assert.match(
  toolStoreSource,
  /invokeTauri\('bundle_readiness', \{ bundleId: newId \}\)/,
  'import success must fetch bundle facts for the new package to prefill the dialog',
);
assert.match(
  toolStoreSource,
  /name: \(bf && \(bf\.display_name \|\| bf\.name\)\) \|\| newId/,
  'import dialog prefill must default to the effective display name (override > fallback)',
);

// 7. 输入清洗与后端 is_display_unsafe_char 同集：Cc 控制字符（含 Tab/换行/
// DEL/C1）+ 软连字符 + 零宽 + 行段/段落分隔符 + bidi + BOM。只剥 \r\n 会让
// TSV 粘贴（含 Tab）等仍前端放行、后端必败。名称与说明输入、粘贴 clamp 三处
// 都必须走同一清洗器。
assert.match(
  toolStoreSource,
  /const DISPLAY_UNSAFE_CHARS = \/\[\\p\{Cc\}\\u00AD\\u200B-\\u200D\\u2028-\\u2029\\u202A-\\u202E\\u2066-\\u2069\\uFEFF\]\/gu/,
  'sanitizer regex must mirror the backend is_display_unsafe_char rejection set (\\p{Cc} == char::is_control)',
);
const stripUses = toolStoreSource.match(/stripDisplayUnsafe\(/g) || [];
assert.ok(
  stripUses.length >= 3, // name onChange + desc onChange + paste clamp
  `stripDisplayUnsafe must be applied to name/desc onChange and paste clamp (found ${stripUses.length} call sites)`,
);
assert.match(
  toolStoreSource,
  /onChange=\{e => setName\(stripDisplayUnsafe\(e\.target\.value\)\)\}/,
  'name input must strip backend-rejected characters',
);
assert.match(
  toolStoreSource,
  /onChange=\{e => setDesc\(stripDisplayUnsafe\(e\.target\.value\)\)\}/,
  'description textarea must strip backend-rejected characters',
);
assert.match(
  toolStoreSource,
  /const clamp = s => \[\.\.\.stripDisplayUnsafe\(s\)\]\.slice\(0, MAX_DISPLAY_DESCRIPTION_CHARS\)\.join\(''\)/,
  'paste clamp must strip backend-rejected characters (not just \\r\\n)',
);

// 8. 编辑弹窗开着时拒绝拖放导入：导入成功会按新包自动预填并整体替换
// editDisplay（key 重挂载），A 包未保存输入被静默丢弃——模态期间拖放不可达。
assert.match(
  toolStoreSource,
  /const editDisplayRef = useRef\(null\)/,
  'dialog-open state must be exposed to the drop controller via a ref',
);
assert.match(
  toolStoreSource,
  /editDisplayRef\.current = editDisplay;/,
  'editDisplayRef must track the editDisplay state on render',
);
assert.match(
  toolStoreSource,
  /canAccept: \(\) => canMutateToolStore && !busyRef\.current && !editDisplayRef\.current/,
  'drop imports must be refused while the edit-display dialog is open',
);

console.log('tool store edit-display contract tests passed');
