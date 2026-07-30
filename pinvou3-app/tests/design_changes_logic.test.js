#!/usr/bin/env node
const assert = require('assert');
const fs = require('fs');
const path = require('path');
const vm = require('vm');

const logicPath = path.join(__dirname, '..', 'src', 'features', 'chat', 'design-changes.js');
const code = fs.readFileSync(logicPath, 'utf8')
  .replace(/\bexport\s+\{[^}]+\};?/g, '')
  .replace(/\bexport\s+/g, '');

const ctx = {};
vm.createContext(ctx);
vm.runInContext(`${code}
this.createDesignChange = createDesignChange;
this.createDesignChangeScopeKey = createDesignChangeScopeKey;
this.reduceDesignChanges = reduceDesignChanges;
this.reduceScopedDesignChanges = reduceScopedDesignChanges;
this.uniqueDesignChanges = uniqueDesignChanges;`, ctx, { filename: logicPath });

const {
  createDesignChange,
  createDesignChangeScopeKey,
  reduceDesignChanges,
  reduceScopedDesignChanges,
  uniqueDesignChanges,
} = ctx;

const change = createDesignChange({
  element: { id: 'el-1', selector: 'main#app > h1.title' },
  type: 'style',
  property: 'fontSize',
  oldValue: '32px',
  newValue: '40px',
});

assert.ok(change.id.startsWith('design-change-'));
assert.strictEqual(change.elementId, 'el-1');
assert.strictEqual(change.selector, 'main#app > h1.title');
assert.strictEqual(change.type, 'style');
assert.strictEqual(change.property, 'fontSize');
assert.strictEqual(change.oldValue, '32px');
assert.strictEqual(change.newValue, '40px');
assert.strictEqual(change.status, 'todo');

let state = reduceDesignChanges([], { type: 'add', change });
assert.strictEqual(state.length, 1);
assert.strictEqual(state[0].status, 'todo');
state = reduceDesignChanges(state, { type: 'add', change: { ...change, id: 'duplicate-id' } });
assert.strictEqual(state.length, 1);
assert.strictEqual(uniqueDesignChanges([change, { ...change, id: 'duplicate-id' }]).length, 1);

state = reduceDesignChanges(state, { type: 'mark-applied', changeId: change.id, ok: true });
assert.strictEqual(state[0].status, 'applied');

state = reduceDesignChanges(state, { type: 'mark-applied', changeId: change.id, ok: false, error: 'not found' });
assert.strictEqual(state[0].status, 'failed');
assert.strictEqual(state[0].error, 'not found');

state = reduceDesignChanges(state, { type: 'clear' });
assert.strictEqual(state.length, 0);

const sessionAArtifactA = createDesignChangeScopeKey('session-a', '/tmp/a.html');
const sessionAArtifactB = createDesignChangeScopeKey('session-a', '/tmp/b.html');
const sessionBArtifactA = createDesignChangeScopeKey('session-b', '/tmp/a.html');
assert.notStrictEqual(sessionAArtifactA, sessionAArtifactB);
assert.notStrictEqual(sessionAArtifactA, sessionBArtifactA);

let scoped = reduceScopedDesignChanges({}, sessionAArtifactA, { type: 'add', change });
scoped = reduceScopedDesignChanges(scoped, sessionAArtifactB, {
  type: 'add',
  change: { ...change, id: 'artifact-b-change', selector: 'main#app > p.copy' },
});
scoped = reduceScopedDesignChanges(scoped, sessionBArtifactA, {
  type: 'add',
  change: { ...change, id: 'session-b-change', selector: 'main#app > button.cta' },
});
assert.strictEqual(scoped[sessionAArtifactA].length, 1);
assert.strictEqual(scoped[sessionAArtifactB].length, 1);
assert.strictEqual(scoped[sessionBArtifactA].length, 1);
assert.strictEqual(scoped[sessionAArtifactA][0].selector, 'main#app > h1.title');
assert.strictEqual(scoped[sessionAArtifactB][0].selector, 'main#app > p.copy');
assert.strictEqual(scoped[sessionBArtifactA][0].selector, 'main#app > button.cta');

scoped = reduceScopedDesignChanges(scoped, sessionAArtifactA, { type: 'clear' });
assert.strictEqual(scoped[sessionAArtifactA], undefined);
assert.strictEqual(scoped[sessionAArtifactB].length, 1);
assert.strictEqual(scoped[sessionBArtifactA].length, 1);

console.log('design_changes_logic: ok');
