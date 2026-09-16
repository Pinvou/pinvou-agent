#!/usr/bin/env node
// consumeWelcomeOptIn 契约（评审 #455 R9 覆盖注记）：
// - 无欢迎卡 → 不尝试、不消费、不调用后端；
// - 有欢迎卡 → 单次消费 + 单次 enable_marketplace_packages（plain scope）；
// - enable 失败 → failed:true + error，且仍已消费（不重复打点）。
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const logicPath = path.join(__dirname, '..', 'src', 'features', 'chat', 'welcome-optin.js');
const code = fs.readFileSync(logicPath, 'utf8')
  .replace(/\bexport\s+\{[^}]+\};?/g, '');

const ctx = {};
vm.createContext(ctx);
vm.runInContext(`${code}\nthis.consumeWelcomeOptIn = consumeWelcomeOptIn;`, ctx, { filename: logicPath });
const { consumeWelcomeOptIn } = ctx;

// 1. 无欢迎卡：零动作。
{
  const calls = [];
  const result = await consumeWelcomeOptIn({
    getToolId: () => null,
    consume: () => calls.push('consume'),
    invoke: async (cmd) => { calls.push(cmd); },
  });
  assert.strictEqual(result.attempted, false);
  assert.strictEqual(result.failed, undefined);
  assert.deepStrictEqual(calls, []);
}

// 2. 正常路径：消费一次 + 单次批量 enable（plain）。
{
  const calls = [];
  const result = await consumeWelcomeOptIn({
    getToolId: () => 'gongwen',
    consume: () => calls.push('consume'),
    invoke: async (cmd, args) => {
      calls.push([cmd, args]);
      return null;
    },
  });
  assert.strictEqual(result.attempted, true);
  assert.strictEqual(result.failed, false);
  // vm 跨 realm 对象不做引用相等：按字段断言。
  assert.strictEqual(calls[0], 'consume');
  assert.strictEqual(calls[1][0], 'enable_marketplace_packages');
  assert.deepStrictEqual([...calls[1][1].packageIds], ['gongwen']);
  assert.strictEqual(calls[1][1].scope, 'plain');
  assert.strictEqual(calls.length, 2);
}

// 3. enable 失败：failed:true + error，仍已消费。
{
  const calls = [];
  const result = await consumeWelcomeOptIn({
    getToolId: () => 'pptx',
    consume: () => calls.push('consume'),
    invoke: async () => { throw new Error('backend locked'); },
  });
  assert.strictEqual(result.attempted, true);
  assert.strictEqual(result.failed, true);
  assert.match(result.error, /backend locked/);
  assert.deepStrictEqual(calls, ['consume']);
}

console.log('welcome_optin_logic: ok');
