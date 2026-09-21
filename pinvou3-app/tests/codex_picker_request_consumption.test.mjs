// 行为级测试(review #484 M1):codex 车道的 picker 请求消费闭环——视图消费后
// 回执宿主清空请求,重挂载(epoch ref 归零)不得重放旧请求劫持视图。
// 视图侧的 epoch 去重判定走纯函数 consumePickerRequest
// (src/features/codex/picker-request.js),宿主/视图生命周期在测试内按
// main.jsx ↔ CodexAcpView 的真实契约建模;接线形态用源码扫描钉住。
import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import { consumePickerRequest } from '../src/features/codex/picker-request.js';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const read = (...segments) => fs.readFileSync(path.join(root, ...segments), 'utf8');

// Host model: main.jsx holds pickerCodexRequest state; applyWorkspaceTarget
// (codex lane) writes a fresh-epoch request; the view's consumption ack
// clears it (onWorkspacePickerRequestConsumed → setPickerCodexRequest(null)).
function makeHost() {
  let epoch = 0;
  let request = null;
  return {
    applyWorkspaceTarget(payload) {
      epoch += 1;
      request = { epoch, ...payload };
    },
    onConsumed() { request = null; },
    prop() { return request; },
  };
}

// View model: CodexAcpView's mount — a fresh epoch ref per mount, the effect
// applies new requests via beginDraft and then acknowledges consumption.
function mountView(host) {
  let lastEpoch = 0; // useRef(0) — resets on every remount
  const drafts = [];
  return {
    drafts,
    effect() {
      const request = consumePickerRequest(host.prop(), lastEpoch);
      if (!request) return;
      lastEpoch = request.epoch;
      drafts.push({ path: request.path || null, projectId: request.projectId || null, roots: request.roots || [] });
      host.onConsumed();
    },
  };
}

test('picker request: mount consumes once, remount does not replay (M1)', () => {
  const host = makeHost();
  host.applyWorkspaceTarget({ path: '/work/x', projectId: 'p1', roots: ['/work/x'] });

  const mount1 = mountView(host);
  mount1.effect();
  assert.deepEqual(mount1.drafts, [{ path: '/work/x', projectId: 'p1', roots: ['/work/x'] }]);
  assert.equal(host.prop(), null, '消费回执后宿主必须清空请求');

  // Re-render of the same mount: no new request, no second beginDraft.
  mount1.effect();
  assert.equal(mount1.drafts.length, 1);

  // Remount (chat → code): the epoch ref resets to 0; the cleared request
  // must not replay the old draft.
  const mount2 = mountView(host);
  mount2.effect();
  assert.deepEqual(mount2.drafts, [], '重挂载不得重放陈旧请求');
});

test('picker request: a fresh request after remount still lands', () => {
  const host = makeHost();
  host.applyWorkspaceTarget({ path: '/work/x', projectId: null, roots: ['/work/x'] });
  const mount1 = mountView(host);
  mount1.effect();

  const mount2 = mountView(host);
  mount2.effect();
  assert.deepEqual(mount2.drafts, []);
  host.applyWorkspaceTarget({ path: '/work/y', projectId: 'p2', roots: ['/work/y'] });
  mount2.effect();
  assert.deepEqual(mount2.drafts, [{ path: '/work/y', projectId: 'p2', roots: ['/work/y'] }], '新请求按 epoch 正常落地');
});

test('consumePickerRequest: same epoch is not consumed twice, null never lands', () => {
  const request = { epoch: 7, path: '/a', projectId: null, roots: [] };
  assert.equal(consumePickerRequest(request, 0), request);
  assert.equal(consumePickerRequest(request, 7), null, '同 epoch 不重复消费');
  assert.equal(consumePickerRequest(null, 0), null);
  assert.equal(consumePickerRequest(undefined, 0), null);
});

test('picker request consumption wiring (M1, source scan)', () => {
  const main = read('src', 'app', 'main.jsx');
  const codexView = read('src', 'features', 'codex', 'CodexAcpView.jsx');

  // 宿主:消费回执清空 pickerCodexRequest。
  assert.match(main, /onWorkspacePickerRequestConsumed=\{\(\) => setPickerCodexRequest\(null\)\}/, '宿主接消费回执');
  // 视图:epoch 去重走共享纯函数,消费后必须回执宿主。
  assert.match(codexView, /consumePickerRequest\(workspacePickerRequest, pickerRequestEpochRef\.current\)/, '视图经纯函数判定');
  assert.match(codexView, /if \(onWorkspacePickerRequestConsumed\) onWorkspacePickerRequestConsumed\(\)/, '视图消费后回执');
});

test('draft project binding never outlives its draft (round-5 M4, source scan)', () => {
  const codexView = read('src', 'features', 'codex', 'CodexAcpView.jsx');

  // createSession 成功后按捕获值条件清除(await 期间重选的新值必须保留)——
  // 否则陈旧 binding 会随下一次临时创建下发,载荷谎报授权范围。
  assert.match(
    codexView,
    /setDraftProjectBinding\(current => \(\s*current === requestedProjectBinding \? null : current\s*\)\)/,
    'createSession 消费后清项目绑定',
  );
  // activeId→null 的草稿复位 effect:else 分支(非 beginDraft 保留路径)连
  // binding 一起清,不得只清 draftWorkspacePath。
  assert.match(
    codexView,
    /else \{[\s\S]{0,200}?setDraftWorkspacePath\(null\);[\s\S]{0,300}?setDraftProjectBinding\(null\);/,
    '草稿复位连项目绑定一起清',
  );
});
