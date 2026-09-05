/** 蜂群模式 spawn 聚合（spawn-aggregation.mjs）：连续 spawn → 单条计数行。 */
import assert from 'node:assert/strict';
import test from 'node:test';
import {
  annotateAgentSpawnGroups,
  annotateTurnSpawnGroups,
  isAgentSpawnChatItem,
  isAgentSpawnProjectedItem,
  spawnGroupAgentIds,
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

test('统一时间线车道：投影条目按 turn 内相邻 spawn 聚合并透传', () => {
  const turn = {
    id: 't1',
    status: 'running',
    items: [
      { type: 'tool', id: 'i1', legacyItem: spawnItem('aaaa0001') },
      { type: 'text', id: 'i2' },
      { type: 'tool', id: 'i3', tool: { name: 'agent', rawInput: { action: 'start', prompt: 'x' } } },
    ],
  };
  const [annotated] = annotateTurnSpawnGroups([turn]);
  assert.equal(annotated.items[0].spawnGroup.count, 1);
  assert.equal(annotated.items[2].spawnGroup.count, 1, '文本块断组');

  const running = {
    id: 't2',
    status: 'running',
    items: [
      { type: 'tool', id: 'j1', legacyItem: spawnItem('aaaa0001') },
      { type: 'tool', id: 'j2', legacyItem: spawnItem('aaaa0002') },
    ],
  };
  const [, annotatedRunning] = annotateTurnSpawnGroups([turn, running]);
  assert.equal(annotatedRunning.items[0].spawnGroup.count, 2);
  assert.equal(annotatedRunning.items[1].spawnGroupHidden, true);
  assert.equal(isAgentSpawnProjectedItem({ type: 'tool', tool: { name: 'agent', rawInput: { action: 'wait' } } }), false);
});

test('无 spawn 的 turns 原样返回（引用相等，不触发重渲染）', () => {
  const turns = [{ id: 't1', items: [{ type: 'text', id: 'i1' }] }];
  assert.equal(annotateTurnSpawnGroups(turns), turns);
});

test('spawnGroupAgentIds 只收集成功返回的正式实例 id', () => {
  const ids = spawnGroupAgentIds([
    spawnItem('aaaa0001'),
    spawnItem('aaaa0002', { state: 'running', output: null }),
    spawnItem('aaaa0003', { success: false, output: 'Error: contention with agent_bbbb0004' }),
  ]);
  assert.deepEqual(ids, ['agent_aaaa0001']);
});
