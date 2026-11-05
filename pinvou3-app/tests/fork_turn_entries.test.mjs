import assert from 'node:assert/strict';
import { forkEntriesByTurnId } from '../src/features/chat/forkTurnEntries.js';
import { turnHasUserText } from '../src/features/conversation/deepseek-conversation.js';

// 「从此轮分叉」入口表的序号口径（与 rewindEntriesByTurnId 同基准的纯逻辑
// 测试；Rust fork_session 用底座 user_turn_prompt_indices 的 Editable 分类
// 切消息，本表必须与之一一对应）。

const textTurn = id => ({ id, userItem: { type: 'user', text: `第${id}轮正文` } });
const imageOnlyTurn = id => ({ id, userItem: { type: 'user', text: '' } });

// 1. 带 userItem 的 turn 按 1-based 顺序计号，keepTurns = 序号（含当前轮）。
{
  const entries = forkEntriesByTurnId([textTurn('t1'), textTurn('t2'), textTurn('t3')]);
  assert.equal(entries.size, 3);
  assert.equal(entries.get('t1'), 1);
  assert.equal(entries.get('t2'), 2);
  assert.equal(entries.get('t3'), 3);
}

// 2. 非 userItem turn（preamble/系统项）与 null-id turn 不占序号。
{
  const turns = [
    { id: 'pre', items: [] },
    textTurn('t1'),
    { id: null, userItem: { type: 'user', text: '无 id' } },
    textTurn('t2'),
  ];
  const entries = forkEntriesByTurnId(turns);
  assert.equal(entries.size, 2);
  assert.equal(entries.get('t1'), 1);
  assert.equal(entries.get('t2'), 2, '序号不因 preamble/null-id 漂移');
}

// 3. 纯图片 turn（userItem 无文本）不占序号：Rust 的 Editable 分类不计这类
// 消息，前端计入会让「从此轮分叉」错位——轻则多报一轮的拒绝错误，重则把
// 分叉点静默漂移成全量分支。
{
  const turns = [textTurn('t1'), imageOnlyTurn('img'), textTurn('t3')];
  const entries = forkEntriesByTurnId(turns);
  assert.equal(entries.has('img'), false, 'image-only turn must not render a fork entry');
  assert.equal(entries.get('t3'), 2, 't3 is ordinal 2 (image-only turn skipped)');
}

// 4. 空白文本与缺 text 字段同纯图片处理（投影对 userItem 恒有 text 字段，
// 缺字段仅在测试桩中出现，按同一口径跳过）。
assert.equal(turnHasUserText({ id: 'x', userItem: { type: 'user', text: '  ' } }), false);
assert.equal(turnHasUserText({ id: 'x', userItem: { type: 'user' } }), false);
assert.equal(turnHasUserText({ id: 'x' }), false);
assert.equal(turnHasUserText(null), false);
assert.equal(turnHasUserText(textTurn('x')), true);

// 5. 蒸汽注入（steered）消息是普通 user turn：带文本即计序号（与 Rust 落盘
// 的 user prompt 一致）。
{
  const turns = [textTurn('t1'), { id: 's1', userItem: { type: 'user', text: '补充指令' } }, textTurn('t2')];
  const entries = forkEntriesByTurnId(turns);
  assert.equal(entries.get('s1'), 2);
  assert.equal(entries.get('t2'), 3);
}

// 6. 非数组输入安全返回空表。
assert.equal(forkEntriesByTurnId(null).size, 0);
assert.equal(forkEntriesByTurnId(undefined).size, 0);
