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
vm.runInContext(
  `${code}\nthis.buildComposerToolMenuState = buildComposerToolMenuState;`
  + `\nthis.createToggleWriteGate = createToggleWriteGate;`
  + `\nthis.TOGGLE_WRITE_KEY_PROJECT_SKILLS = TOGGLE_WRITE_KEY_PROJECT_SKILLS;`,
  ctx,
  { filename: logicPath },
);

const { buildComposerToolMenuState, createToggleWriteGate, TOGGLE_WRITE_KEY_PROJECT_SKILLS } = ctx;

// ── 开关（disabled）与可见性（hidden）正交 ────────────────────────────
let state = buildComposerToolMenuState({
  marketplaceTools: [{ id: 'weather', name: '高德天气', installed: true }],
});
assert.strictEqual(state.toolRows.length, 1);
assert.strictEqual(state.toolRows[0].id, 'weather');
assert.strictEqual(state.toolRows[0].enabled, true);
assert.strictEqual(state.enabledCount, 2); // weather + builtin visual-design

// 开关关（disabled）：工具仍在列表，仅 enabled=false
state = buildComposerToolMenuState({
  marketplaceTools: [{ id: 'weather', name: '高德天气', installed: true }],
  disabledIds: ['weather'],
});
assert.strictEqual(state.toolRows.length, 1, '开关关的工具应仍在列表');
assert.strictEqual(state.toolRows[0].enabled, false, '开关关的工具应置灰');
assert.strictEqual(state.enabledCount, 1); // 仅 builtin visual-design

// 不可见（hidden）：工具从列表直接消失
state = buildComposerToolMenuState({
  marketplaceTools: [{ id: 'weather', name: '高德天气', installed: true }],
  hiddenIds: ['weather'],
});
assert.strictEqual(state.toolRows.length, 0, '不可见工具应从 composer 菜单过滤');
assert.strictEqual(state.enabledCount, 1);

// ── 技能：开关关 = 置灰；不可见 = 消失 ──────────────────────────────
state = buildComposerToolMenuState({
  marketplaceSkills: [{ id: 'visualizer', title: '数据分析可视化', installed: true }],
});
let visualizer = state.skillRows.find(row => row.id === 'visualizer');
assert.ok(visualizer);
assert.strictEqual(visualizer.switchable, true);
assert.strictEqual(visualizer.enabled, true);

state = buildComposerToolMenuState({
  marketplaceSkills: [{ id: 'visualizer', title: '数据分析可视化', installed: true }],
  disabledIds: ['visualizer'],
});
visualizer = state.skillRows.find(row => row.id === 'visualizer');
assert.ok(visualizer, '开关关的技能应仍在列表');
assert.strictEqual(visualizer.enabled, false, '开关关的技能应置灰');

state = buildComposerToolMenuState({
  marketplaceSkills: [{ id: 'visualizer', title: '数据分析可视化', installed: true }],
  hiddenIds: ['visualizer'],
});
visualizer = state.skillRows.find(row => row.id === 'visualizer');
assert.ok(!visualizer, '不可见技能应从 composer 菜单过滤');

// ── companion 技能跟随所属工具，不作为独立技能行 ───────────────────────
state = buildComposerToolMenuState({
  marketplaceTools: [{ id: 'gongwen', name: '公文写作', installed: true, companion_skills: ['government-writing'] }],
  marketplaceSkills: [{ id: 'government-writing', title: '党政机关公文写作', installed: true }],
});
assert.ok(state.toolRows.find(row => row.id === 'gongwen'));
assert.ok(!state.skillRows.find(row => row.skillId === 'government-writing'));

// ── 内置技能（视觉设计）：plain 只读开关，code 设计期隐藏 ──────────────
state = buildComposerToolMenuState({ activeSkill: 'visual-design' });
const builtin = state.skillRows.find(row => row.id === 'builtin-skill:visual-design');
assert.ok(builtin);
assert.strictEqual(builtin.switchable, false);
assert.strictEqual(builtin.readonly, true);
assert.strictEqual(builtin.enabled, true);
assert.strictEqual(builtin.active, true);

// ── CLI 服务：开关关 = 置灰；不可见 = 消失 ────────────────────────────
state = buildComposerToolMenuState({
  serviceStates: [
    { id: 'feishu', title: '飞书（Lark）', connected: true },
    { id: 'wecom', title: '企业微信', connected: false },
  ],
});
assert.strictEqual(state.connectedServices.length, 1);
assert.strictEqual(state.connectedServices[0].id, 'feishu');
assert.strictEqual(state.enabledCount, 2); // feishu + builtin visual-design

assert.strictEqual(state.connectedServices[0].switchable, true);
assert.strictEqual(state.connectedServices[0].enabled, true);
state = buildComposerToolMenuState({
  serviceStates: [{ id: 'feishu', title: '飞书（Lark）', connected: true }],
  disabledIds: ['feishu'],
});
assert.strictEqual(state.connectedServices.length, 1, '开关关的 CLI 应仍在列表');
assert.strictEqual(state.connectedServices[0].enabled, false, '开关关的 CLI 应置灰');
assert.strictEqual(state.enabledCount, 1); // 仅 builtin visual-design

state = buildComposerToolMenuState({
  serviceStates: [{ id: 'feishu', title: '飞书（Lark）', connected: true }],
  hiddenIds: ['feishu'],
});
assert.strictEqual(state.connectedServices.length, 0, '不可见 CLI 应从 composer 菜单过滤');

// 旧「停用」marker 仍显示但置灰
state = buildComposerToolMenuState({
  serviceStates: [{ id: 'feishu', title: '飞书（Lark）', connected: true, enabled: false }],
});
assert.strictEqual(state.connectedServices[0].enabled, false);

// ── code scope：视觉设计设计期隐藏 ───────────────────────────────────
state = buildComposerToolMenuState({
  scope: 'code',
  marketplaceTools: [{ id: 'weather', name: '高德天气', installed: true }],
  marketplaceSkills: [{ id: 'visualizer', title: '数据分析可视化', installed: true }],
});
visualizer = state.skillRows.find(row => row.id === 'visualizer');
assert.ok(visualizer);
assert.strictEqual(visualizer.switchable, true);
assert.strictEqual(visualizer.unavailable, false);
assert.ok(!state.skillRows.find(row => row.id === 'builtin-skill:visual-design'), 'code 模式应隐藏视觉设计');
assert.strictEqual(state.enabledCount, 2); // weather + visualizer

// code scope 开关全关：技能置灰（非过滤）
state = buildComposerToolMenuState({
  scope: 'code',
  marketplaceSkills: [{ id: 'visualizer', title: '数据分析可视化', installed: true }],
  disabledIds: ['visualizer'],
});
visualizer = state.skillRows.find(row => row.id === 'visualizer');
assert.ok(visualizer, '开关关的技能应仍在列表');
assert.strictEqual(visualizer.enabled, false);
assert.strictEqual(state.enabledCount, 0); // visualizer 关 + 视觉设计 code 隐藏

// ── 未传 scope 行为与 plain 一致 ──────────────────────────────────────
state = buildComposerToolMenuState({
  marketplaceSkills: [{ id: 'visualizer', title: '数据分析可视化', installed: true }],
});
visualizer = state.skillRows.find(row => row.id === 'visualizer');
assert.strictEqual(visualizer.switchable, true);
assert.strictEqual(visualizer.unavailable, false);
assert.strictEqual(state.enabledCount, 2); // visualizer + builtin visual-design

// ── 「所有技能已关闭」提示 ────────────────────────────────────────────
state = buildComposerToolMenuState({
  marketplaceTools: [{ id: 'pptx', name: 'PPT 生成', installed: true, companion_skills: ['pptx'] }],
  marketplaceSkills: [{ id: 'visualizer', title: '数据分析可视化', installed: true }],
  disabledIds: ['pptx', 'visualizer'],
});
assert.strictEqual(state.allSkillsDisabled, true, '独立技能 + companion 工具全关应提示');

state = buildComposerToolMenuState({
  marketplaceTools: [{ id: 'pptx', name: 'PPT 生成', installed: true, companion_skills: ['pptx'] }],
  marketplaceSkills: [{ id: 'visualizer', title: '数据分析可视化', installed: true }],
  disabledIds: ['visualizer'],
});
assert.strictEqual(state.allSkillsDisabled, false, '开启 pptx(companion)后不应提示');

state = buildComposerToolMenuState({
  marketplaceSkills: [{ id: 'visualizer', title: '数据分析可视化', installed: true }],
  disabledIds: ['visualizer'],
  serviceStates: [{ id: 'feishu', title: '飞书（Lark）', connected: true }],
});
assert.strictEqual(state.allSkillsDisabled, false, '开启 feishu(CLI companion)后不应提示');

// ── Governance write generation gate: rollback authority under out-of-order
// completion (regression) ──────────────────────────────────────────────
// Driven out of order with real deferred promises: with rapid toggles an
// earlier write can fail later, and the component catch may roll back/report
// only when it is still the control's latest write (review round-2 blocker).
async function toggleWriteGateTests() {
  const defer = () => {
    let resolve, reject;
    const promise = new Promise((res, rej) => { resolve = res; reject = rej; });
    return { promise, resolve, reject };
  };
  // Simulates the component protocol: begin issues a generation → the write
  // completes → the catch rolls back/reports only when isCurrent.
  const simulateToggle = async (gate, key, outcome) => {
    const generation = gate.begin(key);
    try {
      await outcome();
      return { generation, stale: false, rolledBack: false };
    } catch (error) {
      if (!gate.isCurrent(key, generation)) return { generation, stale: true };
      return { generation, stale: false, rolledBack: true, error };
    }
  };

  // Three alternating clicks (review scenario): write A is held and then fails,
  // writes B and C succeed and complete first.
  const gate = createToggleWriteGate();
  const writeA = defer();
  const clickA = simulateToggle(gate, 'pkg-a', () => writeA.promise);
  const clickB = simulateToggle(gate, 'pkg-a', () => Promise.resolve());
  const clickC = simulateToggle(gate, 'pkg-a', () => Promise.resolve());
  writeA.reject(new Error('late failure'));
  const resultA = await clickA;
  await Promise.all([clickB, clickC]);
  assert.strictEqual(resultA.stale, true, 'a late stale failure does not match the latest generation and must not roll back or report');

  // The latest write failing must still roll back/report (an older success does
  // not exempt a newer failure).
  const gate2 = createToggleWriteGate();
  const okWrite = defer();
  const staleClick = simulateToggle(gate2, 'pkg-a', () => okWrite.promise);
  const failClick = simulateToggle(gate2, 'pkg-a', () => Promise.reject(new Error('latest fails')));
  okWrite.resolve();
  const [staleResult, failResult] = await Promise.all([staleClick, failClick]);
  assert.strictEqual(staleResult.stale, false, 'the older write completing successfully is still current generation (success path has no side effects)');
  assert.strictEqual(failResult.rolledBack, true, 'the latest write failing must roll back and report');

  // Controls are independent: another control issuing a newer write does not
  // invalidate this control's in-flight write generation.
  const gate3 = createToggleWriteGate();
  const pkgWrite = defer();
  const pkgClick = simulateToggle(gate3, 'pkg-a', () => pkgWrite.promise);
  const skillsClick = simulateToggle(gate3, TOGGLE_WRITE_KEY_PROJECT_SKILLS, () => Promise.resolve());
  pkgWrite.resolve();
  const [pkgResult, skillsResult] = await Promise.all([pkgClick, skillsClick]);
  assert.strictEqual(pkgResult.stale, false, 'a project-skills write does not invalidate the package control generation');
  assert.strictEqual(skillsResult.stale, false);
  // A failed package write completing afterwards is still matched (not stale).
  const pkgFail = simulateToggle(gate3, 'pkg-a', () => Promise.reject(new Error('pkg fails')));
  const pkgFailResult = await pkgFail;
  assert.strictEqual(pkgFailResult.rolledBack, true);
}

// ── 内置插件（契约 §3.2 配置可见性）：builtin === true 的工具不进菜单 ──
state = buildComposerToolMenuState({
  marketplaceTools: [
    { id: 'session-reader', name: '会话读取', installed: true, builtin: true },
    { id: 'weather', name: '高德天气', installed: true },
  ],
});
assert.ok(!state.toolRows.find(row => row.id === 'session-reader'), '内置插件应从 composer 菜单过滤');
assert.ok(state.toolRows.find(row => row.id === 'weather'), '普通工具不受影响');
assert.strictEqual(state.enabledCount, 2); // weather + builtin visual-design

// builtin 字段缺省（旧后端）按普通工具放行
state = buildComposerToolMenuState({
  marketplaceTools: [{ id: 'weather', name: '高德天气', installed: true }],
});
assert.strictEqual(state.toolRows.length, 1, '无 builtin 字段的普通工具应放行');

// builtin: false 显式普通插件同样放行
state = buildComposerToolMenuState({
  marketplaceTools: [{ id: 'weather', name: '高德天气', installed: true, builtin: false }],
});
assert.strictEqual(state.toolRows.length, 1, 'builtin:false 的普通工具应放行');

// eslint-disable-next-line unicorn/prefer-top-level-await -- logic test keeps its sync sections above and runs the async gate section from main()
toggleWriteGateTests().then(() => {
  console.log('composer_tool_menu_logic: ok');
}, (e) => {
  console.error(e);
  process.exit(1);
});

