#!/usr/bin/env node
const assert = require('assert');
const fs = require('fs');
const path = require('path');
const vm = require('vm');

const logicPath = path.join(__dirname, '..', 'src', 'features', 'chat', 'personal-workbench-scene.js');
const code = fs.readFileSync(logicPath, 'utf8')
  .replace(/\bexport\s+\{[^}]+\};?/g, '')
  .replace(/\bexport\s+/g, '');

const ctx = {};
vm.createContext(ctx);
vm.runInContext(`${code}
this.PERSONAL_WORKBENCH_SCENE_ID = PERSONAL_WORKBENCH_SCENE_ID;
this.PERSONAL_WORKBENCH_SCENE_KEY = PERSONAL_WORKBENCH_SCENE_KEY;
this.PERSONAL_WORKBENCH_SCENE_NAME = PERSONAL_WORKBENCH_SCENE_NAME;
this.PERSONAL_WORKBENCH_TEMPLATES = PERSONAL_WORKBENCH_TEMPLATES;
this.DEFAULT_PERSONAL_WORKBENCH_PROMPT = DEFAULT_PERSONAL_WORKBENCH_PROMPT;
this.buildDefaultPersonalWorkbenchPayloadText = buildDefaultPersonalWorkbenchPayloadText;
this.createPersonalWorkbenchMessageMeta = createPersonalWorkbenchMessageMeta;
this.findPersonalWorkbenchTemplateDraft = findPersonalWorkbenchTemplateDraft;
this.getPersonalWorkbenchTemplate = getPersonalWorkbenchTemplate;
this.getPersonalWorkbenchTemplateById = getPersonalWorkbenchTemplateById;
this.isPersonalWorkbenchTemplateDraftForTemplate = isPersonalWorkbenchTemplateDraftForTemplate;
this.shouldUsePersonalWorkbenchScene = shouldUsePersonalWorkbenchScene;`, ctx, {
  filename: logicPath,
});

const {
  PERSONAL_WORKBENCH_SCENE_ID,
  PERSONAL_WORKBENCH_SCENE_KEY,
  PERSONAL_WORKBENCH_SCENE_NAME,
  PERSONAL_WORKBENCH_TEMPLATES,
  DEFAULT_PERSONAL_WORKBENCH_PROMPT,
  buildDefaultPersonalWorkbenchPayloadText,
  createPersonalWorkbenchMessageMeta,
  findPersonalWorkbenchTemplateDraft,
  getPersonalWorkbenchTemplate,
  getPersonalWorkbenchTemplateById,
  isPersonalWorkbenchTemplateDraftForTemplate,
  shouldUsePersonalWorkbenchScene,
} = ctx;

assert.strictEqual(PERSONAL_WORKBENCH_SCENE_ID, 39);
assert.strictEqual(PERSONAL_WORKBENCH_SCENE_KEY, 'personal-workbench');
assert.strictEqual(PERSONAL_WORKBENCH_SCENE_NAME, '个人工作台');
assert.deepStrictEqual(Array.from(PERSONAL_WORKBENCH_TEMPLATES, item => item.title), [
  '生活记录',
  '个人账本',
  '学习计划',
  '任务看板',
  '求职管理',
  '旅行计划',
  '运动打卡',
]);
assert.strictEqual(PERSONAL_WORKBENCH_TEMPLATES.length, 7);

assert.strictEqual(shouldUsePersonalWorkbenchScene('work', 'personal-workbench'), true);
assert.strictEqual(shouldUsePersonalWorkbenchScene('work', 'document-writing'), false);
assert.strictEqual(shouldUsePersonalWorkbenchScene('design', 'personal-workbench'), false);
assert.strictEqual(getPersonalWorkbenchTemplate(1).title, '个人账本');
assert.strictEqual(getPersonalWorkbenchTemplate(99), null);
assert.strictEqual(getPersonalWorkbenchTemplateById('personal-ledger').title, '个人账本');
assert.strictEqual(getPersonalWorkbenchTemplateById('missing-template'), null);
assert.strictEqual(findPersonalWorkbenchTemplateDraft('夜跑'), null);
assert.strictEqual(findPersonalWorkbenchTemplateDraft(getPersonalWorkbenchTemplate(1).prompt).index, 1);
assert.strictEqual(
  findPersonalWorkbenchTemplateDraft(`${getPersonalWorkbenchTemplate(6).prompt}\n\n用户补充需求：夜跑`).template.title,
  '运动打卡'
);
assert.strictEqual(
  isPersonalWorkbenchTemplateDraftForTemplate(`${getPersonalWorkbenchTemplate(1).prompt}\n\n用户补充需求：暗色模式`, getPersonalWorkbenchTemplateById('personal-ledger')),
  true
);
assert.strictEqual(
  isPersonalWorkbenchTemplateDraftForTemplate('我要做一个自由职业者客户管理工作台', getPersonalWorkbenchTemplateById('personal-ledger')),
  false
);

for (const template of PERSONAL_WORKBENCH_TEMPLATES) {
  assert.ok(template.id, `${template.title} must have id`);
  assert.ok(template.prompt.length > 80, `${template.title} prompt must be substantive`);
  assert.match(template.prompt, /HTML/, `${template.title} prompt must mention HTML`);
  assert.match(template.prompt, /localStorage/, `${template.title} prompt must mention localStorage`);
  assert.match(template.prompt, /视觉要求/, `${template.title} prompt must include visual requirements`);
  assert.match(template.prompt, /iOS \/ macOS/, `${template.title} prompt must include iOS/macOS visual direction`);
}

assert.ok(!PERSONAL_WORKBENCH_TEMPLATES.some(item => item.title === '宝宝学习乐园'));
assert.ok(!PERSONAL_WORKBENCH_TEMPLATES.some(item => item.title === 'CyberFit 运动打卡'));
assert.match(getPersonalWorkbenchTemplate(1).prompt, /真实时薪计算器/);
assert.match(getPersonalWorkbenchTemplate(6).prompt, /不允许引用 CDN 或外部图表库/);
assert.doesNotMatch(getPersonalWorkbenchTemplate(6).prompt, /Chart\.js/);

assert.match(DEFAULT_PERSONAL_WORKBENCH_PROMPT, /个人数字工作台/);
assert.match(DEFAULT_PERSONAL_WORKBENCH_PROMPT, /单文件 HTML/);
assert.match(DEFAULT_PERSONAL_WORKBENCH_PROMPT, /localStorage/);
assert.match(DEFAULT_PERSONAL_WORKBENCH_PROMPT, /导出 JSON/);
assert.match(DEFAULT_PERSONAL_WORKBENCH_PROMPT, /内联 SVG/);
assert.match(DEFAULT_PERSONAL_WORKBENCH_PROMPT, /今天要处理/);

const defaultPayload = buildDefaultPersonalWorkbenchPayloadText('运动');
assert.match(defaultPayload, /^你是 PINVOU 的个人数字工作台搭建专家/);
assert.match(defaultPayload, /用户需求：\n运动$/);

const meta = createPersonalWorkbenchMessageMeta('生成一个任务看板', 3);
assert.strictEqual(meta.pinvouScene, 'work:personal-workbench');
assert.strictEqual(meta.pinvouTemplateId, 'task-board');
assert.strictEqual(meta.pinvouTemplateTitle, '任务看板');
assert.ok(!Object.prototype.hasOwnProperty.call(meta, 'pinvouPayloadText'));

const metaById = createPersonalWorkbenchMessageMeta('生成一个任务看板', 'task-board');
assert.strictEqual(metaById.pinvouScene, 'work:personal-workbench');
assert.strictEqual(metaById.pinvouTemplateId, 'task-board');
assert.strictEqual(metaById.pinvouTemplateTitle, '任务看板');
assert.ok(!Object.prototype.hasOwnProperty.call(metaById, 'pinvouPayloadText'));

const sceneOnlyMeta = createPersonalWorkbenchMessageMeta('运动');
assert.strictEqual(sceneOnlyMeta.pinvouScene, 'work:personal-workbench');
assert.strictEqual(sceneOnlyMeta.pinvouTemplateId, undefined);
assert.strictEqual(sceneOnlyMeta.pinvouTemplateTitle, undefined);
assert.match(sceneOnlyMeta.pinvouPayloadText, /个人数字工作台/);
assert.match(sceneOnlyMeta.pinvouPayloadText, /用户需求：\n运动$/);

const emptySceneOnlyMeta = createPersonalWorkbenchMessageMeta('');
assert.strictEqual(emptySceneOnlyMeta.pinvouScene, 'work:personal-workbench');
assert.ok(!Object.prototype.hasOwnProperty.call(emptySceneOnlyMeta, 'pinvouPayloadText'));

console.log('personal_workbench_scene_logic: ok');
