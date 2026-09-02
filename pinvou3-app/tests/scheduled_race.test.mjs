/**
 * 定时任务竞态回归测试（PR #260 审计补充）：
 * 删除任务后，删除前在途的整表 list 响应不得把已删任务复活回侧边栏
 * （M1：deleteScheduledTask 补 invalidateScheduledTaskReads）。
 */
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const bridgeDir = path.join(here, '..', 'src', 'platform', 'tauri', 'bridge');

function loadScheduledFeature() {
  const root = { localStorage: { getItem() { return null; }, setItem() {} } };
  const src = fs.readFileSync(path.join(bridgeDir, 'scheduled.js'), 'utf8');
  vm.runInNewContext(src, {
    window: root,
    globalThis: root,
    setTimeout,
    clearTimeout,
  });
  const factory = root.__PINVOU_TAURI_BRIDGE_FEATURES__.scheduled;
  const deferreds = {};
  const state = { scheduledTasks: [] };
  const api = factory({
    state,
    notify() {},
    bt(key) { return key; },
    addSystemItem() {},
    addChatItem() {},
    timeStr() { return ''; },
    rememberScheduledRunOwner() {},
    isScheduledRunTerminal(status) { return ['completed', 'failed'].includes(status); },
    purgeSessionBuffer() {},
    createNewSession() {},
    prefillComposer() {},
    sessionStates: {},
    runSyncOnSession() { return Promise.resolve(); },
    // eslint-disable-next-line no-unused-vars -- stub keeps the full call signature
    invoke(name, args) {
      if (deferreds[name] && deferreds[name].promise) return deferreds[name].promise;
      return Promise.resolve({});
    },
  });
  return {
    api,
    state,
    defer(name) {
      const d = {};
      d.promise = new Promise((resolve, reject) => { d.resolve = resolve; d.reject = reject; });
      deferreds[name] = d;
      return d;
    },
  };
}

test('deleteScheduledTask 作废删除前在途的整表 list 响应（已删任务不得复活）', async () => {
  const rt = loadScheduledFeature();
  // 预置任务列表，选中另一任务 B（删除未选中的 A，正是复活竞态的路径）。
  rt.state.scheduledTasks = [
    { id: 'task-a', name: 'A' },
    { id: 'task-b', name: 'B' },
  ];
  await rt.api.selectScheduledTask('task-b');

  // 3 秒轮询在删除前发出 list_scheduled_tasks（在途，读到删除前快照）。
  const dList = rt.defer('list_scheduled_tasks');
  const pLoad = rt.api.loadScheduledTasks();
  // 用户确认删除 A：delete 完成后本地同步修剪 + 作废在途读。
  const pDelete = rt.api.deleteScheduledTask('task-a');
  // delete invoke 排在 list 之后（deferred 按名占用）。
  const dDelete = rt.defer('delete_scheduled_task');
  dDelete.resolve({ deletedSessionIds: [] });
  await pDelete;
  assert.ok(
    !rt.state.scheduledTasks.some((task) => task.id === 'task-a'),
    '删除后本地列表立即移除任务',
  );

  // 删除前发出的 list 响应此刻才返回（含已删的 A）——不得整表写回。
  dList.resolve([
    { id: 'task-a', name: 'A' },
    { id: 'task-b', name: 'B' },
  ]);
  await pLoad;
  assert.ok(
    !rt.state.scheduledTasks.some((task) => task.id === 'task-a'),
    '陈旧整表响应不得复活已删任务',
  );
  assert.ok(
    rt.state.scheduledTasks.some((task) => task.id === 'task-b'),
    '未删任务保持存在',
  );
});
