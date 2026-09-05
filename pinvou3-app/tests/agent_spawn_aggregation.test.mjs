/** 蜂群模式 spawn 聚合（spawn-aggregation.mjs）：连续 spawn → 单条计数行。 */
import assert from 'node:assert/strict';
import test from 'node:test';
import {
  annotateAgentSpawnGroups,
  isAgentSpawnChatItem,
} from '../src/features/multiagent/spawn-aggregation.mjs';

const spawnItem = (id, extra = {}) => ({
  type: 'tool',
  id,
  name: 'agent',
  args: { action: 'start', prompt: `task ${id}` },
  state: 'done',
  success: true,
  output: JSON.stringify({ agent_id: `agent_${id}` }),
  ...extra,
});

test('spawn 判定：只有 start 动作且带任务正文的 agent 调用是 spawn', () => {
  assert.equal(isAgentSpawnChatItem(spawnItem('aaaa0001')), true);
  assert.equal(isAgentSpawnChatItem(spawnItem('aaaa0002', { args: { action: 'status', agent_id: 'agent_aaaa0001' } })), false);
  assert.equal(isAgentSpawnChatItem(spawnItem('aaaa0003', { args: { action: 'wait', agent_id: 'agent_aaaa0001' } })), false);
  assert.equal(isAgentSpawnChatItem(spawnItem('aaaa0004', { args: { action: 'cancel', agent_id: 'agent_aaaa0001' } })), false);
  assert.equal(isAgentSpawnChatItem({ type: 'tool', name: 'exec_shell', args: { command: 'ls' } }), false);
  assert.equal(isAgentSpawnChatItem(null), false);
});

test('连续 spawn 聚合为一条计数行：首条带 spawnGroup，其余隐藏', () => {
  const items = [
    spawnItem('aaaa0001'),
    spawnItem('aaaa0002'),
    spawnItem('aaaa0003'),
  ];
  const annotated = annotateAgentSpawnGroups(items);
  assert.equal(annotated.length, 3);
  assert.equal(annotated[0].spawnGroup.count, 3);
  assert.equal(annotated[0].spawnGroup.failed, 0);
  assert.equal(annotated[1].spawnGroupHidden, true);
  assert.equal(annotated[2].spawnGroupHidden, true);
  assert.ok(!annotated[0].spawnGroupHidden);
});

test('spawn 之间夹着其他内容块时断组开新行', () => {
  const items = [
    spawnItem('aaaa0001'),
    { type: 'assistant', text: '中间的话' },
    spawnItem('aaaa0002'),
  ];
  const annotated = annotateAgentSpawnGroups(items);
  assert.equal(annotated[0].spawnGroup.count, 1);
  assert.ok(!annotated[1].spawnGroupHidden && !annotated[1].spawnGroup, '非工具条目不受标注影响');
  assert.equal(annotated[2].spawnGroup.count, 1, '新序列重新从 1 计数');
});

test('status/wait/cancel 协调调用打断 spawn 序列且不计数', () => {
  const items = [
    spawnItem('aaaa0001'),
    spawnItem('aaaa0002', { args: { action: 'status', agent_id: 'agent_aaaa0001' } }),
    spawnItem('aaaa0003'),
  ];
  const annotated = annotateAgentSpawnGroups(items);
  assert.equal(annotated[0].spawnGroup.count, 1);
  assert.ok(!annotated[1].spawnGroupHidden, '协调行保持原样渲染');
  assert.equal(annotated[2].spawnGroup.count, 1, '协调调用后的 spawn 属于新序列');
});

test('失败 spawn 计入 failed，不影响计数行总数', () => {
  const items = [
    spawnItem('aaaa0001', { success: false, output: 'Error: spawn failed' }),
    spawnItem('aaaa0002'),
  ];
  const annotated = annotateAgentSpawnGroups(items);
  assert.equal(annotated[0].spawnGroup.count, 2);
  assert.equal(annotated[0].spawnGroup.failed, 1);
});

test('annotateAgentSpawnGroups 不改动未进组的条目引用', () => {
  const plain = { type: 'user', text: 'hi' };
  const annotated = annotateAgentSpawnGroups([plain]);
  assert.equal(annotated[0], plain);
});

test('无 spawn 的条目按引用原样返回（数组是新建的，条目不换引用）', () => {
  const plain = { type: 'user', text: 'hi' };
  const items = [plain];
  const annotated = annotateAgentSpawnGroups(items);
  assert.notEqual(annotated, items, '数组总是新建');
  assert.equal(annotated[0], plain, '条目保持原引用');
});
