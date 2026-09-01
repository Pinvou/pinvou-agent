import assert from 'node:assert/strict';
import {
  checkpointMapByTurn,
  checkpointRefreshKey,
  reloadSessionAfterRewind,
  rewindEntriesByTurnId,
  rewindNoticeText,
  rewindUndoAvailable,
  summarizeCheckpointChanges,
} from '../src/features/codex/checkpoints.js';

// ── checkpointMapByTurn：turn 序号对齐 ──────────────────────────────
{
  const map = checkpointMapByTurn([
    { id: 'c1', turn: 1 },
    { id: 'c2', turn: 2 },
    { id: 'c3', turn: 3 },
  ]);
  assert.equal(map.get(1).id, 'c1');
  assert.equal(map.get(2).id, 'c2');
  assert.equal(map.get(3).id, 'c3');
  assert.equal(map.get(4), undefined);
}

// 回滚点（turn=None 的 preRestore）不占位，不与 turn 快照抢序号。
{
  const map = checkpointMapByTurn([
    { id: 'c1', turn: 1 },
    { id: 'undo', turn: null, kind: 'preRestore' },
    { id: 'c2', turn: 2 },
  ]);
  assert.equal(map.size, 2);
  assert.equal(map.get(2).id, 'c2');
}

// 同一 turn 多条快照取先创建者（turn 快照），后续不覆盖。
{
  const map = checkpointMapByTurn([
    { id: 'first', turn: 2 },
    { id: 'second', turn: 2 },
  ]);
  assert.equal(map.get(2).id, 'first');
}

// 快照创建失败的 turn 没有条目：后续 turn 的序号不漂移（靠 turn 字段而非位置）。
{
  const map = checkpointMapByTurn([
    { id: 'c1', turn: 1 },
    { id: 'c3', turn: 3 },
  ]);
  assert.equal(map.get(2), undefined);
  assert.equal(map.get(3).id, 'c3');
}

// 全部缺序号（计数失败兜底）时按创建顺序对齐。
{
  const map = checkpointMapByTurn([{ id: 'a' }, { id: 'b' }]);
  assert.equal(map.get(1).id, 'a');
  assert.equal(map.get(2).id, 'b');
}

// 空/非法输入。
assert.equal(checkpointMapByTurn([]).size, 0);
assert.equal(checkpointMapByTurn(null).size, 0);
assert.equal(checkpointMapByTurn(undefined).size, 0);

// ── summarizeCheckpointChanges：diff 摘要计数 ───────────────────────
{
  const summary = summarizeCheckpointChanges([
    { path: 'a.rs', status: 'added' },
    { path: 'b.rs', status: 'added' },
    { path: 'c.rs', status: 'modified' },
    { path: 'd.rs', status: 'deleted' },
    { path: 'e.rs', status: 'renamed' },
    { path: 'f.rs', status: 'copied' },
    { path: 'g.rs', status: 'weird' },
    { path: 'h.rs' },
    null,
  ]);
  assert.deepEqual(summary, {
    added: 2, modified: 1, deleted: 1, renamed: 1, copied: 1, other: 2, total: 8,
  });
}
assert.deepEqual(summarizeCheckpointChanges([]), {
  added: 0, modified: 0, deleted: 0, renamed: 0, copied: 0, other: 0, total: 0,
});
assert.deepEqual(summarizeCheckpointChanges(null).total, 0);

// ── rewindEntriesByTurnId：入口变体判定 ─────────────────────────────
const userTurn = id => ({ id, userItem: { type: 'user' } });

// turn N+1 有快照 → 该边界为「回退到第 N 轮」（代码+对话）；keepTurns = 序号-1。
{
  const turns = [userTurn('t1'), userTurn('t2'), userTurn('t3')];
  const entries = rewindEntriesByTurnId(turns, [
    { id: 'c1', turn: 1, kind: 'turn' },
    { id: 'c2', turn: 2, kind: 'turn' },
    { id: 'c3', turn: 3, kind: 'turn' },
  ]);
  assert.equal(entries.size, 3);
  assert.deepEqual(entries.get('t1'), { keepTurns: 0, checkpoint: { id: 'c1', turn: 1, kind: 'turn' }, conversationOnly: false });
  assert.equal(entries.get('t2').keepTurns, 1);
  assert.equal(entries.get('t2').checkpoint.id, 'c2');
  assert.equal(entries.get('t3').keepTurns, 2);
}

// turn 2 快照缺失（LRU 淘汰/当时失败）→ 该边界降级为「仅回退对话」变体，
// 其余边界不受影响、序号不漂移。
{
  const turns = [userTurn('t1'), userTurn('t2'), userTurn('t3')];
  const entries = rewindEntriesByTurnId(turns, [
    { id: 'c1', turn: 1, kind: 'turn' },
    { id: 'c3', turn: 3, kind: 'turn' },
  ]);
  assert.equal(entries.get('t1').conversationOnly, false);
  assert.deepEqual(entries.get('t2'), { keepTurns: 1, checkpoint: null, conversationOnly: true });
  assert.equal(entries.get('t3').conversationOnly, false);
  assert.equal(entries.get('t3').checkpoint.id, 'c3');
}

// preRestore 回滚点不参与对齐（turn=null），不会制造入口。
{
  const turns = [userTurn('t1'), userTurn('t2')];
  const entries = rewindEntriesByTurnId(turns, [
    { id: 'c1', turn: 1, kind: 'turn' },
    { id: 'undo', turn: null, kind: 'preRestore' },
  ]);
  assert.equal(entries.size, 2);
  assert.equal(entries.get('t2').conversationOnly, true);
}

// preamble/系统项投影 turn（无 userItem）不占用户 turn 序号、不渲染入口。
{
  const turns = [
    { id: 'pre', items: [] },
    userTurn('t1'),
    userTurn('t2'),
  ];
  const entries = rewindEntriesByTurnId(turns, [
    { id: 'c1', turn: 1, kind: 'turn' },
    { id: 'c2', turn: 2, kind: 'turn' },
  ]);
  assert.equal(entries.has('pre'), false);
  assert.equal(entries.get('t1').keepTurns, 0);
  assert.equal(entries.get('t2').keepTurns, 1);
}

// checkpoint 列表为空（系统无 git/快照全部失败）→ 整个会话无入口（设计 §5）。
{
  const turns = [userTurn('t1'), userTurn('t2')];
  assert.equal(rewindEntriesByTurnId(turns, []).size, 0);
  assert.equal(rewindEntriesByTurnId(turns, null).size, 0);
  assert.equal(rewindEntriesByTurnId(null, [{ id: 'c1', turn: 1 }]).size, 0);
}

// ── 联调 Bug A：列表竞态导致变体误判 ────────────────────────────────
// 复现：turn 3 进行中（或完成但未重拉）时列表停在 [c1,c2] → t3 边界误判为
// 「仅回退对话」；turn 完成（busy→idle）重拉拿到 [c1,c2,c3] 后收敛为全量回退。
{
  const turns = [userTurn('t1'), userTurn('t2'), userTurn('t3')];
  const stale = rewindEntriesByTurnId(turns, [
    { id: 'c1', turn: 1, kind: 'turn' },
    { id: 'c2', turn: 2, kind: 'turn' },
  ]);
  assert.equal(stale.get('t3').conversationOnly, true);
  const fresh = rewindEntriesByTurnId(turns, [
    { id: 'c1', turn: 1, kind: 'turn' },
    { id: 'c2', turn: 2, kind: 'turn' },
    { id: 'c3', turn: 3, kind: 'turn' },
  ]);
  assert.equal(fresh.get('t3').conversationOnly, false);
  assert.equal(fresh.get('t3').checkpoint.id, 'c3');
}

// checkpointRefreshKey：turn 数不变时 busy 边沿（turn 完成/失败）也必须改变键，
// 否则旧列表永不重拉（Bug A 的刷新缺口）；无任何变化时键保持稳定（不空转）。
{
  const idle2 = checkpointRefreshKey({ turnCount: 2, busy: false });
  const busy3 = checkpointRefreshKey({ turnCount: 3, busy: true });
  const idle3 = checkpointRefreshKey({ turnCount: 3, busy: false });
  assert.notEqual(busy3, idle3); // turn 完成边沿触发重拉
  assert.notEqual(idle2, busy3); // turn 数变化触发重拉
  assert.equal(idle3, checkpointRefreshKey({ turnCount: 3, busy: false }));
  assert.equal(checkpointRefreshKey({ turnCount: undefined, busy: false }), '0:idle');
}

// ── 联调 Bug B：回退成功后的重载编排 ────────────────────────────────
// reload 成功：先重载后 bumpTick，error 为空。
{
  const order = [];
  const { error } = await reloadSessionAfterRewind({
    reload: async () => { order.push('reload'); },
    bumpTick: () => { order.push('tick'); },
  });
  assert.equal(error, null);
  assert.deepEqual(order, ['reload', 'tick']);
}

// reload 失败：错误如实返回（留在弹窗上屏），且 bumpTick 仍必须执行——hydrate
// 可能已完成而重载尾部步骤失败/被守卫提前返回，lane 已是新内容时必须兜底重投影。
{
  let ticks = 0;
  const { error } = await reloadSessionAfterRewind({
    reload: async () => { throw new Error('load_session failed'); },
    bumpTick: () => { ticks += 1; },
  });
  assert.equal(error, 'load_session failed');
  assert.equal(ticks, 1);
}

// 非 Error 抛出（Tauri invoke 常 reject 字符串）按 String 归一化。
{
  let ticks = 0;
  const { error } = await reloadSessionAfterRewind({
    reload: async () => { throw '会话正在执行，请先停止当前任务再回退'; },
    bumpTick: () => { ticks += 1; },
  });
  assert.equal(error, '会话正在执行，请先停止当前任务再回退');
  assert.equal(ticks, 1);
}

// rewindNoticeText：restoredCheckpoint 非空 → 可反悔提示；degraded → 代码未回退；
// 兜底 → 仅对话。
{
  const copy = {
    rewindNoticeDegraded: 'degraded',
    rewindNoticeRestored: n => `restored:${n}`,
    rewindNoticeConversationOnly: 'conversationOnly',
    rewindNoticeCompaction: 'compaction-note',
  };
  assert.equal(rewindNoticeText(copy, { degraded: true, restoredCheckpoint: null }, 2), 'degraded');
  assert.equal(rewindNoticeText(copy, { degraded: false, restoredCheckpoint: { id: 'p1' } }, 2), 'restored:2');
  assert.equal(rewindNoticeText(copy, { degraded: false, restoredCheckpoint: null }, 0), 'conversationOnly');
  // hadCompaction：在基础提示后如实追加压缩摘要警示，不替换基础语义；
  // false/缺省时不追加。
  assert.equal(
    rewindNoticeText(copy, { degraded: false, restoredCheckpoint: { id: 'p1' }, hadCompaction: true }, 2),
    'restored:2 compaction-note',
  );
  assert.equal(
    rewindNoticeText(copy, { degraded: true, restoredCheckpoint: null, hadCompaction: true }, 1),
    'degraded compaction-note',
  );
  assert.equal(
    rewindNoticeText(copy, { degraded: false, restoredCheckpoint: null, hadCompaction: true }, 0),
    'conversationOnly compaction-note',
  );
  assert.equal(
    rewindNoticeText(copy, { degraded: false, restoredCheckpoint: { id: 'p1' }, hadCompaction: false }, 2),
    'restored:2',
  );
}

// ── rewindUndoAvailable：「撤销回退」入口可见性 ─────────────────────
// 后端 rewind_undo_state 非 null 即可反悔（sidecar 备份 + 未发新轮/尾部未编辑 +
// 绑定的回滚点仍在），前端只做形状校验。checkpointId 为 null 是合法的降级形态
// （仅对话回退的撤销只还原对话），不是残缺状态。
{
  assert.equal(rewindUndoAvailable(null), false);
  assert.equal(rewindUndoAvailable(undefined), false);
  assert.equal(rewindUndoAvailable({}), false);
  assert.equal(rewindUndoAvailable('checkpoint-x'), false);
  assert.equal(rewindUndoAvailable({ checkpointId: 'pre-1' }), false);
  // 防御：rewoundTurns 为 0 的状态不渲染（否则文案是「还原被截掉的 0 轮对话」）。
  assert.equal(rewindUndoAvailable({ checkpointId: 'pre-1', keptTurns: 2, rewoundTurns: 0 }), false);
  assert.equal(
    rewindUndoAvailable({ checkpointId: 'pre-1', keptTurns: 2, rewoundTurns: 1, rewoundAt: '2026-08-21T10:00:00Z' }),
    true,
  );
  // 降级（仅对话回退）的撤销：无回滚点，仅还原对话——入口照常渲染。
  assert.equal(
    rewindUndoAvailable({ checkpointId: null, keptTurns: 2, rewoundTurns: 1, rewoundAt: '2026-09-01T10:00:00Z' }),
    true,
  );
}

// ── Web 车道策略锚定：rewind 直接改写本地文件，保持桌面专属 ─────────
// 与 multiagent_plan_normalize.test.mjs 的「桌面专属」断言同款：防止未来误放行
// （放行需单独评估 relay 侧的本地文件写语义）。
{
  const { readFileSync } = await import('node:fs');
  const policy = JSON.parse(readFileSync(new URL('../src/platform/web/access-policy.json', import.meta.url), 'utf8'));
  for (const command of [
    'list_checkpoints',
    'checkpoint_diff',
    'restore_checkpoint',
    'rewind_to_turn',
    'rewind_undo_state',
    'undo_last_rewind',
  ]) {
    assert.equal(
      policy.allowed_commands.includes(command),
      false,
      `${command} 必须保持桌面专属（rewind 直接改写本地文件，web relay 放行需单独评估）`,
    );
  }
}

console.log('codex_checkpoints_logic: all assertions passed');
