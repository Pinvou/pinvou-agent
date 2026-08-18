#!/usr/bin/env node
const assert = require('assert');
const fs = require('fs');
const path = require('path');
const vm = require('vm');

const logicPath = path.join(__dirname, '..', 'src', 'features', 'artifacts', 'design-runtime.js');
const code = fs.readFileSync(logicPath, 'utf8')
  .replace(/\bexport\s+\{[^}]+\};?/g, '')
  .replace(/\bexport\s+/g, '');

const ctx = {};
vm.createContext(ctx);
vm.runInContext(`${code}
this.DESIGN_MESSAGE_TYPES = DESIGN_MESSAGE_TYPES;
this.buildDesignRuntimeScript = buildDesignRuntimeScript;`, ctx, {
  filename: logicPath,
});

const {
  DESIGN_MESSAGE_TYPES,
  buildDesignRuntimeScript,
} = ctx;

assert.strictEqual(DESIGN_MESSAGE_TYPES.READY, 'pinvou:design-runtime-ready');
assert.strictEqual(DESIGN_MESSAGE_TYPES.ELEMENT_SELECTED, 'pinvou:design-element-selected');
assert.strictEqual(DESIGN_MESSAGE_TYPES.APPLY_CHANGE, 'pinvou:design-apply-change');
assert.strictEqual(DESIGN_MESSAGE_TYPES.CHANGE_APPLIED, 'pinvou:design-change-applied');
assert.strictEqual(DESIGN_MESSAGE_TYPES.CLEAR_CHANGES, 'pinvou:design-clear-changes');
assert.strictEqual(DESIGN_MESSAGE_TYPES.DESTROY, 'pinvou:design-runtime-destroy');

const script = buildDesignRuntimeScript();
assert.ok(script.includes('pinvou:design-runtime-ready'));
assert.ok(script.includes('pinvou:design-element-selected'));
assert.ok(script.includes('pinvou:design-apply-change'));
assert.ok(script.includes('pinvou:design-change-applied'));
assert.ok(script.includes('pinvou:design-clear-changes'));
assert.ok(script.includes('pinvou:design-runtime-destroy'));
assert.ok(script.includes('data-pinvou-design-hover'));
assert.ok(script.includes("Object.prototype.hasOwnProperty.call(payload, 'oldValue')"));
assert.ok(script.includes('hasOriginalValue ? String(originalValue'));

console.log('design_runtime_logic: ok');
