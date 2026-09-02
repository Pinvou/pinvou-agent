/**
 * 技能包导入纯逻辑:pickSkillDrop 挑 zip/md、fileToBase64 读字节转 base64、大小软限。
 */
'use strict';
const { test } = require('node:test');
const assert = require('node:assert/strict');

const { MAX_SKILL_ZIP_BYTES, pickSkillDrop, fileToBase64 } = require('../src/features/tools/skill-import-logic.js');

test('pickSkillDrop: 大小写不敏感挑第一个 .zip', () => {
  const picked = pickSkillDrop([{ name: 'a.txt' }, { name: 'my-skill.ZIP' }]);
  assert.equal(picked.file.name, 'my-skill.ZIP');
  assert.equal(picked.kind, 'zip');
  assert.equal(pickSkillDrop([{ name: 'SKILL.zip' }]).file.name, 'SKILL.zip');
});

test('pickSkillDrop: 单个 .md/.markdown 技能文件', () => {
  const picked = pickSkillDrop([{ name: 'notes.md' }]);
  assert.equal(picked.file.name, 'notes.md');
  assert.equal(picked.kind, 'md');
  assert.equal(pickSkillDrop([{ name: 'guide.MARKDOWN' }]).kind, 'md');
});

test('pickSkillDrop: 无可导入文件 / 空数组 / null → null', () => {
  assert.equal(pickSkillDrop([{ name: 'a.txt' }]), null);
  assert.equal(pickSkillDrop([]), null);
  assert.equal(pickSkillDrop(null), null);
  // null 等非法元素跳过,继续找下一个可导入文件
  assert.equal(pickSkillDrop([null, { name: 'x.zip' }]).file.name, 'x.zip');
});

test('pickSkillDrop: 缺 name 的文件跳过', () => {
  assert.equal(pickSkillDrop([{}]), null);
  assert.equal(pickSkillDrop([{ name: undefined }]), null);
});

test('MAX_SKILL_ZIP_BYTES 拖放通道前端软限 50MiB', () => {
  assert.equal(MAX_SKILL_ZIP_BYTES, 50 * 1024 * 1024);
});

test('fileToBase64: 小文件与 Buffer 基准一致', async () => {
  const bytes = Buffer.from('PK\x03\x04 hello zip content');
  const file = { arrayBuffer: () => Promise.resolve(bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength)) };
  assert.equal(await fileToBase64(file), bytes.toString('base64'));
});

test('fileToBase64: 跨 0x8000 分块边界一致', async () => {
  // 32768 = 0x8000,刻意取边界两侧长度,验证分块拼接无丢字节
  for (const len of [0x8000 - 1, 0x8000, 0x8000 + 1, 3 * 0x8000 + 123]) {
    const bytes = Buffer.alloc(len);
    for (let i = 0; i < len; i++) bytes[i] = (i * 31 + 7) & 0xff; // 非平凡字节序列
    const file = { arrayBuffer: () => Promise.resolve(bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength)) };
    assert.equal(await fileToBase64(file), bytes.toString('base64'), `len=${len}`);
  }
});
