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

// ── 治理写代数门：乱序完成下的回滚判权（out-of-order completion 回归）────
// 用真实 deferred promise 按乱序驱动：快速连点时较早的写可能较晚才失败，
// 组件 catch 只允许「仍是该控件最新一次写」的完成回滚/提示（评审 r2 阻塞项）。
async function toggleWriteGateTests() {
  const defer = () => {
    let resolve, reject;
    const promise = new Promise((res, rej) => { resolve = res; reject = rej; });
    return { promise, resolve, reject };
  };
  // 模拟组件协议：begin 发号 → 写完成 → catch 仅在 isCurrent 时回滚/提示。
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

  // 三次交替点击（评审场景）：写 A 被扣住后失败，写 B、C 相继成功且先完成。
  const gate = createToggleWriteGate();
  const writeA = defer();
  const clickA = simulateToggle(gate, 'pkg-a', () => writeA.promise);
  const clickB = simulateToggle(gate, 'pkg-a', () => Promise.resolve());
  const clickC = simulateToggle(gate, 'pkg-a', () => Promise.resolve());
  writeA.reject(new Error('late failure'));
  const resultA = await clickA;
  await Promise.all([clickB, clickC]);
  assert.strictEqual(resultA.stale, true, '迟到的旧失败对不上最新代数，不得回滚/提示');

  // 最新一次写失败仍须回滚/提示（旧写成功不豁免新失败）。
  const gate2 = createToggleWriteGate();
  const okWrite = defer();
  const staleClick = simulateToggle(gate2, 'pkg-a', () => okWrite.promise);
  const failClick = simulateToggle(gate2, 'pkg-a', () => Promise.reject(new Error('latest fails')));
  okWrite.resolve();
  const [staleResult, failResult] = await Promise.all([staleClick, failClick]);
  assert.strictEqual(staleResult.stale, false, '旧写成功仍是当前代数（成功路径无副作用）');
  assert.strictEqual(failResult.rolledBack, true, '最新写失败必须回滚/提示');

  // 控件间互不干扰：其他控件发新写不注销本控件在途写的代数。
  const gate3 = createToggleWriteGate();
  const pkgWrite = defer();
  const pkgClick = simulateToggle(gate3, 'pkg-a', () => pkgWrite.promise);
  const skillsClick = simulateToggle(gate3, TOGGLE_WRITE_KEY_PROJECT_SKILLS, () => Promise.resolve());
  pkgWrite.resolve();
  const [pkgResult, skillsResult] = await Promise.all([pkgClick, skillsClick]);
  assert.strictEqual(pkgResult.stale, false, '项目技能发新写不影响包控件的代数');
  assert.strictEqual(skillsResult.stale, false);
  // 失败的包写在其后完成仍被对号（未过期）。
  const pkgFail = simulateToggle(gate3, 'pkg-a', () => Promise.reject(new Error('pkg fails')));
  const pkgFailResult = await pkgFail;
  assert.strictEqual(pkgFailResult.rolledBack, true);
}

// eslint-disable-next-line unicorn/prefer-top-level-await -- logic test keeps its sync sections above and runs the async gate section from main()
toggleWriteGateTests().then(() => {
  console.log('composer_tool_menu_logic: ok');
}, (e) => {
  console.error(e);
  process.exit(1);
});
