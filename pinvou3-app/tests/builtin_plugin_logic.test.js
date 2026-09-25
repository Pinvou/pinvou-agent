#!/usr/bin/env node
// Direct unit tests for the builtin-plugin-logic pure functions
// (docs/builtin-toolset-contract.md §3.1/§3.2 shared judgement).
const assert = require('assert');
const fs = require('fs');
const path = require('path');
const vm = require('vm');

const logicPath = path.join(__dirname, '..', 'src', 'features', 'tools', 'builtin-plugin-logic.js');
const code = fs.readFileSync(logicPath, 'utf8')
  .replace(/\bexport\s+\{[^}]+\};?/g, '')
  .replace(/\bexport\s+/g, '');
const ctx = {};
vm.createContext(ctx);
vm.runInContext(`${code}\nthis.isBuiltinPlugin = isBuiltinPlugin; this.builtinToolShortName = builtinToolShortName;`, ctx, {
  filename: logicPath,
});

const { isBuiltinPlugin, builtinToolShortName } = ctx;

// ── isBuiltinPlugin: strict === true or visibility "system"; missing/falsy
// fields pass through as regular plugins ──
assert.strictEqual(isBuiltinPlugin({ builtin: true }), true);
assert.strictEqual(isBuiltinPlugin({ builtin: false }), false);
assert.strictEqual(isBuiltinPlugin({}), false, 'missing builtin field (old backend) passes as a regular plugin');
assert.strictEqual(isBuiltinPlugin(null), false);
assert.strictEqual(isBuiltinPlugin(), false, 'no argument passes as a regular plugin');
assert.strictEqual(isBuiltinPlugin({ builtin: 1 }), false, 'truthy non-true is not builtin (the contract field is boolean)');
assert.strictEqual(isBuiltinPlugin({ visibility: 'system' }), true, 'visibility: "system" (manifest-declared) counts as builtin');
assert.strictEqual(isBuiltinPlugin({ visibility: 'public' }), false, 'other visibility values pass as regular plugins');
assert.strictEqual(isBuiltinPlugin({ builtin: false, visibility: 'system' }), true, 'visibility: "system" wins over builtin: false');
assert.strictEqual(isBuiltinPlugin({ visibility: 'System' }), false, 'visibility matching is case-sensitive ("system" only)');

// ── builtinToolShortName: strips the mcp_<pluginId>_ prefix, falls back to the
// original name when the prefix does not match ──
assert.strictEqual(builtinToolShortName('mcp_session-reader_read_session', 'session-reader'), 'read_session');
assert.strictEqual(builtinToolShortName('mcp_session-reader_list_sessions', 'session-reader'), 'list_sessions');
assert.strictEqual(builtinToolShortName('mcp_weather_get_weather', 'session-reader'), 'mcp_weather_get_weather', 'non-matching prefix returns the name as-is');
assert.strictEqual(builtinToolShortName('read_session', 'session-reader'), 'read_session', 'no prefix returns the name as-is');
assert.strictEqual(builtinToolShortName('mcp_session-reader_read_session', ''), 'mcp_session-reader_read_session', 'no pluginId means no stripping');
assert.strictEqual(builtinToolShortName('', 'session-reader'), '');

console.log('builtin_plugin_logic: ok');
