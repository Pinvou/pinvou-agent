#!/usr/bin/env node
const assert = require('assert');
const fs = require('fs');
const path = require('path');
const vm = require('vm');

const logicPath = path.join(__dirname, '..', 'src', 'features', 'chat', 'chat-input-limit.js');
const code = fs.readFileSync(logicPath, 'utf8')
  .replace(/\bexport\s+\{[^}]+\};?/g, '')
  .replace(/\bexport\s+/g, '');
const context = {};
vm.createContext(context);
vm.runInContext(
  `${code}\nthis.CHAT_INPUT_MAX_LENGTH = CHAT_INPUT_MAX_LENGTH; this.constrainChatInput = constrainChatInput;`,
  context,
  { filename: logicPath },
);

const { CHAT_INPUT_MAX_LENGTH, constrainChatInput } = context;

assert.strictEqual(CHAT_INPUT_MAX_LENGTH, 100000);
assert.deepStrictEqual(
  JSON.parse(JSON.stringify(constrainChatInput('hello'))),
  { text: 'hello', limitReached: false, truncated: false },
);

const exact = constrainChatInput('a'.repeat(CHAT_INPUT_MAX_LENGTH));
assert.strictEqual(exact.text.length, CHAT_INPUT_MAX_LENGTH);
assert.strictEqual(exact.limitReached, true);
assert.strictEqual(exact.truncated, false);

const oversized = constrainChatInput('文'.repeat(CHAT_INPUT_MAX_LENGTH + 500));
assert.strictEqual(oversized.text.length, CHAT_INPUT_MAX_LENGTH);
assert.strictEqual(oversized.limitReached, true);
assert.strictEqual(oversized.truncated, true);

const customLimit = constrainChatInput('abcdef', 4);
assert.strictEqual(customLimit.text, 'abcd');
assert.strictEqual(customLimit.truncated, true);

const chatPath = path.join(__dirname, '..', 'src', 'features', 'chat', 'ChatView.jsx');
const chatSource = fs.readFileSync(chatPath, 'utf8');
assert.match(chatSource, /maxLength=\{CHAT_INPUT_MAX_LENGTH\}/);
assert.match(chatSource, /data-testid="chat-input-limit-notice"/);
assert.match(chatSource, /<TextareaContextMenu[^>]+setValue=\{setInputText\}/);
assert.match(
  chatSource,
  /function handleSend\(\)[\s\S]*?const constrained = constrainChatInput\(inputText\);[\s\S]*?if \(constrained\.truncated\)/,
);

console.log('chat_input_limit_logic: ok');
