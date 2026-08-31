// 本地识图引擎发送门纯判定函数的行为测试（node --test 直跑）。
// ChatView.jsx 只做接线；这些用例钉住历史缺陷对应的判定语义：
// 下载所有权的认领/继承/归还（顶替窗口）、迟到回调丢弃、死主补停引擎。
import test from "node:test";
import assert from "node:assert/strict";
import {
  acceptsLateCallback,
  ownershipAfterInstall,
  resolveDownloadOwnership,
  shouldStopEngineAfterLateInvoke,
} from "../src/features/chat/local-engine-gate.mjs";

test("resolveDownloadOwnership：无在途下载 → 本次 invoke 认领", () => {
  assert.deepEqual(resolveDownloadOwnership({ inFlight: false, owned: false }), {
    claimed: true,
    inherited: false,
    owned: true,
  });
});

test("resolveDownloadOwnership：在途下载属于外部入口（owned=false）→ 不认领不继承", () => {
  assert.deepEqual(resolveDownloadOwnership({ inFlight: true, owned: false }), {
    claimed: false,
    inherited: false,
    owned: false,
  });
});

test("resolveDownloadOwnership：在途下载由本组件先前流程发起（owned=true）→ 继承", () => {
  assert.deepEqual(resolveDownloadOwnership({ inFlight: true, owned: true }), {
    claimed: false,
    inherited: true,
    owned: true,
  });
});

test("ownershipAfterInstall：乐观认领的下载结束（完成或被抢先）→ 立即归还", () => {
  assert.strictEqual(
    ownershipAfterInstall({ claimed: true, inherited: false, owned: true }),
    false,
  );
});

test("ownershipAfterInstall：继承的在途下载不属于本次 invoke → 所有权保持", () => {
  assert.strictEqual(
    ownershipAfterInstall({ claimed: false, inherited: true, owned: true }),
    true,
  );
  assert.strictEqual(
    ownershipAfterInstall({ claimed: false, inherited: false, owned: false }),
    false,
  );
});

// 顶替窗口回归（ChatView installThenResolve 的完整判定时序）：
// 流程 A 认领下载后被流程 B 顶替，B 必须继承那次下载，取消才不是 no-op。
test("双流程顶替：B 继承 A 的在途下载，取消语义全程有效", () => {
  // 流程 A：无在途 → 认领 → invoke（下载在途，A 的 token 被顶替作废）。
  const a = resolveDownloadOwnership({ inFlight: false, owned: false });
  let owned = a.owned;
  assert.strictEqual(owned, true);
  // 流程 B 入口（不无条件清零）：读到在途 + owned → 继承。
  const b = resolveDownloadOwnership({ inFlight: true, owned });
  owned = b.owned;
  assert.strictEqual(b.inherited, true);
  assert.strictEqual(owned, true);
  // B 的 invoke 被桥幂等守卫跳过（返回 false）：继承所有权不得被归还。
  owned = ownershipAfterInstall(b);
  assert.strictEqual(owned, true, "顶替后被跳过的 invoke 仍须持有继承所有权");
  // A 的 invoke 先返回（自身认领）：A 归还自己那份，不影响 B 继承的语义
  // （此时 A 发起的下载已完成或仍在途，B 侧按最新 owned 继续判定）。
  const aAfter = ownershipAfterInstall(a);
  assert.strictEqual(aAfter, false);
  // 取消按钮读取的 owned 为 true → cancelDownload 真正生效。
  assert.strictEqual(owned, true);
});

// 外部入口（设置页）下载在途时，聊天发送门任何时序都不得接管其取消权。
test("外部入口在途下载：发送门认领→被抢先也不残留所有权", () => {
  const o = resolveDownloadOwnership({ inFlight: true, owned: false });
  assert.strictEqual(o.owned, false);
  assert.strictEqual(ownershipAfterInstall(o), false);
});

test("acceptsLateCallback：挂起条目缺失/换人时丢弃迟到回调", () => {
  assert.strictEqual(acceptsLateCallback(null, 1), false);
  assert.strictEqual(acceptsLateCallback({ token: 2 }, 1), false);
  assert.strictEqual(acceptsLateCallback({ token: 1 }, 1), true);
});

test("shouldStopEngineAfterLateInvoke：仅死主且所有权已被取消清零时补停", () => {
  // 活流程（token 仍有效）绝不自停。
  assert.strictEqual(
    shouldStopEngineAfterLateInvoke({ ownedStart: true, alive: true, startedRef: 1 }),
    false,
  );
  // 死流程但新流程已接管（持有自己的 token）→ 不得替它停。
  assert.strictEqual(
    shouldStopEngineAfterLateInvoke({ ownedStart: true, alive: false, startedRef: 2 }),
    false,
  );
  // 非发起方（用户在设置页手动启动）→ 不停。
  assert.strictEqual(
    shouldStopEngineAfterLateInvoke({ ownedStart: false, alive: false, startedRef: 0 }),
    false,
  );
  // 唯一命中：本流程发起 + token 已死 + 取消路径已清零所有权。
  assert.strictEqual(
    shouldStopEngineAfterLateInvoke({ ownedStart: true, alive: false, startedRef: 0 }),
    true,
  );
});
