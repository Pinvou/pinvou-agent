#!/usr/bin/env node
const assert = require('assert');
const fs = require('fs');
const path = require('path');
const vm = require('vm');

const logicPath = path.join(__dirname, '..', 'src', 'features', 'settings', 'composer-tool-menu-logic.js');
const code = fs.readFileSync(logicPath, 'utf8')
  .replace(/\bexport\s+\{[^}]+\};?/g, '')
  .replace(/\bexport\s+/g, '');
const ctx = {};
vm.createContext(ctx);
vm.runInContext(`${code}\nthis.buildComposerToolMenuState = buildComposerToolMenuState;`, ctx, {
  filename: logicPath,
});

const { buildComposerToolMenuState } = ctx;

let state = buildComposerToolMenuState({
  marketplaceTools: [{ id: 'weather', name: '高德天气', installed: true }],
});
assert.strictEqual(state.toolRows.length, 1);
assert.strictEqual(state.toolRows[0].id, 'weather');
assert.strictEqual(state.toolRows[0].enabled, true);
assert.strictEqual(state.enabledCount, 2); // weather + builtin visual-design

state = buildComposerToolMenuState({
  marketplaceTools: [{ id: 'weather', name: '高德天气', installed: true }],
  disabledIds: ['weather'],
});
assert.strictEqual(state.toolRows[0].enabled, false);
assert.strictEqual(state.enabledCount, 1); // builtin visual-design

state = buildComposerToolMenuState({
  marketplaceSkills: [{ id: 'visualizer', title: '数据分析可视化', installed: true }],
});
let visualizer = state.skillRows.find(row => row.id === 'skill:visualizer');
assert.ok(visualizer);
assert.strictEqual(visualizer.switchable, true);
assert.strictEqual(visualizer.enabled, true);

state = buildComposerToolMenuState({
  marketplaceSkills: [{ id: 'visualizer', title: '数据分析可视化', installed: true }],
  disabledIds: ['skill:visualizer'],
});
visualizer = state.skillRows.find(row => row.id === 'skill:visualizer');
assert.strictEqual(visualizer.enabled, false);

state = buildComposerToolMenuState({
  marketplaceTools: [{ id: 'gongwen', name: '公文写作', installed: true, companion_skills: ['government-writing'] }],
  marketplaceSkills: [{ id: 'government-writing', title: '党政机关公文写作', installed: true }],
});
assert.ok(state.toolRows.find(row => row.id === 'gongwen'));
assert.ok(!state.skillRows.find(row => row.skillId === 'government-writing'));

state = buildComposerToolMenuState({ activeSkill: 'visual-design' });
const builtin = state.skillRows.find(row => row.id === 'builtin-skill:visual-design');
assert.ok(builtin);
assert.strictEqual(builtin.switchable, false);
assert.strictEqual(builtin.active, true);

state = buildComposerToolMenuState({
  serviceStates: [
    { id: 'feishu', title: '飞书（Lark）', connected: true },
    { id: 'wecom', title: '企业微信', connected: false },
  ],
});
assert.strictEqual(state.connectedServices.length, 1);
assert.strictEqual(state.connectedServices[0].id, 'feishu');
assert.strictEqual(state.enabledCount, 2); // feishu + builtin visual-design

// code scope: 技能行只读不可用且不计入启用数,工具行不受影响
state = buildComposerToolMenuState({
  scope: 'code',
  marketplaceTools: [{ id: 'weather', name: '高德天气', installed: true }],
  marketplaceSkills: [{ id: 'visualizer', title: '数据分析可视化', installed: true }],
});
visualizer = state.skillRows.find(row => row.id === 'skill:visualizer');
assert.ok(visualizer);
assert.strictEqual(visualizer.switchable, false);
assert.strictEqual(visualizer.unavailable, true);
const builtinInCode = state.skillRows.find(row => row.id === 'builtin-skill:visual-design');
assert.ok(builtinInCode);
assert.strictEqual(builtinInCode.unavailable, true);
assert.strictEqual(state.enabledCount, 1); // 仅 weather,技能行不计入

// 未传 scope 时行为与 plain 一致(回归保护)
state = buildComposerToolMenuState({
  marketplaceSkills: [{ id: 'visualizer', title: '数据分析可视化', installed: true }],
});
visualizer = state.skillRows.find(row => row.id === 'skill:visualizer');
assert.strictEqual(visualizer.switchable, true);
assert.strictEqual(visualizer.unavailable, false);
assert.strictEqual(state.enabledCount, 2); // visualizer + builtin visual-design

console.log('composer_tool_menu_logic: ok');
