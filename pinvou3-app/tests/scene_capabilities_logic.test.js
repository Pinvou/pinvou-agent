#!/usr/bin/env node
const assert = require('assert');
const fs = require('fs');
const path = require('path');
const vm = require('vm');

const logicPath = path.join(__dirname, '..', 'src', 'features', 'chat', 'scene-capabilities.js');
const code = fs.readFileSync(logicPath, 'utf8')
  .replace(/\bexport\s+\{[^}]+\};?/g, '')
  .replace(/\bexport\s+/g, '');

const ctx = {};
vm.createContext(ctx);
vm.runInContext(`${code}
this.canPrepareSceneCapabilities = canPrepareSceneCapabilities;
this.requiredCapabilitiesForMeta = requiredCapabilitiesForMeta;`, ctx, { filename: logicPath });

const { canPrepareSceneCapabilities, requiredCapabilitiesForMeta } = ctx;

assert.strictEqual(
  canPrepareSceneCapabilities({ isWebHost: true, dependencyInstallAvailable: true }),
  false,
  'Web host must not call desktop marketplace install APIs',
);
assert.strictEqual(
  canPrepareSceneCapabilities({ isWebHost: false, dependencyInstallAvailable: false }),
  false,
  'Desktop host without dependency install capability must not call install APIs',
);
assert.strictEqual(
  canPrepareSceneCapabilities({ isWebHost: false, dependencyInstallAvailable: true }),
  true,
  'Desktop host with dependency install capability may prepare scene capabilities',
);

const dataVisualizationRequirements = requiredCapabilitiesForMeta({ pinvouScene: 'design:data-visualization' });
assert.strictEqual(dataVisualizationRequirements.key, 'data-visualization');
assert.strictEqual(dataVisualizationRequirements.label, '数据可视化');
assert.strictEqual(dataVisualizationRequirements.preparingText, '正在准备数据可视化能力...');
assert.strictEqual(dataVisualizationRequirements.readyText, '已启用数据可视化，开始生成');
assert.strictEqual(dataVisualizationRequirements.failureText, '数据可视化能力准备失败，请稍后重试。');
assert.deepStrictEqual([...dataVisualizationRequirements.tools], []);
assert.deepStrictEqual([...dataVisualizationRequirements.skills], ['visualizer']);

console.log('scene_capabilities_logic: ok');
