/**
 * 技能包导入纯逻辑:pickSkillZip 挑 zip、fileToBase64 读字节转 base64、大小软限。
 */
'use strict';
const { test } = require('node:test');
const assert = require('node:assert/strict');

const { MAX_SKILL_ZIP_BYTES, pickSkillZip, fileToBase64 } = require('../src/features/tools/skill-import-logic.js');

test('pickSkillZip: 大小写不敏感挑第一个 .zip', () => {
  assert.equal(pickSkillZip([{ name: 'a.txt' }, { name: 'my-skill.ZIP' }]).name, 'my-skill.ZIP');
  assert.equal(pickSkillZip([{ name: 'SKILL.zip' }]).name, 'SKILL.zip');
});

test('pickSkillZip: 无 zip / 空数组 / null → null', () => {
  assert.equal(pickSkillZip([{ name: 'a.txt' }]), null);
  assert.equal(pickSkillZip([]), null);
  assert.equal(pickSkillZip(null), null);
  // null 等非法元素跳过,继续找下一个 zip
  assert.equal(pickSkillZip([null, { name: 'x.zip' }]).name, 'x.zip');
});

test('pickSkillZip: 缺 name 的文件跳过', () => {
  assert.equal(pickSkillZip([{}]), null);
  assert.equal(pickSkillZip([{ name: undefined }]), null);
});

test('MAX_SKILL_ZIP_BYTES 对齐后端 5MiB 软限', () => {
  assert.equal(MAX_SKILL_ZIP_BYTES, 5 * 1024 * 1024);
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
