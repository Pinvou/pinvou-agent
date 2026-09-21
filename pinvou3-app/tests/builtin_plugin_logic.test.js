#!/usr/bin/env node
// builtin-plugin-logic 纯函数直测（《内置工具集长期契约》§3.1/§3.2 共享判定）。
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

// ── isBuiltinPlugin：严格 === true，缺省/假值按普通插件放行 ──
assert.strictEqual(isBuiltinPlugin({ builtin: true }), true);
assert.strictEqual(isBuiltinPlugin({ builtin: false }), false);
assert.strictEqual(isBuiltinPlugin({}), false, 'builtin 字段缺省（旧后端）应按普通插件放行');
assert.strictEqual(isBuiltinPlugin(null), false);
assert.strictEqual(isBuiltinPlugin(), false, '无入参按普通插件放行');
assert.strictEqual(isBuiltinPlugin({ builtin: 1 }), false, '真值非 true 不算内置（契约字段为 boolean）');

// ── builtinToolShortName：剥 mcp_<pluginId>_ 前缀，不匹配原样兜底 ──
assert.strictEqual(builtinToolShortName('mcp_session-reader_read_session', 'session-reader'), 'read_session');
assert.strictEqual(builtinToolShortName('mcp_session-reader_list_sessions', 'session-reader'), 'list_sessions');
assert.strictEqual(builtinToolShortName('mcp_weather_get_weather', 'session-reader'), 'mcp_weather_get_weather', '前缀不匹配原样返回');
assert.strictEqual(builtinToolShortName('read_session', 'session-reader'), 'read_session', '无前缀原样返回');
assert.strictEqual(builtinToolShortName('mcp_session-reader_read_session', ''), 'mcp_session-reader_read_session', '无 pluginId 不剥前缀');
assert.strictEqual(builtinToolShortName('', 'session-reader'), '');

console.log('builtin_plugin_logic: ok');
