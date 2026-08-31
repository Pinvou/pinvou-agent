// local-engine-gate.mjs — 本地识图引擎发送门的纯判定函数。
//
// ChatView.jsx 持有 refs / invoke / 轮询等副作用；这里只沉淀可单测的决策
// （下载所有权认领/继承/归还、迟到回调接受、死主补停引擎），历史发送门
// 缺陷全部出在这些判定上。修改发送门行为时先改这里并补
// tests/local_engine_gate.test.mjs，再回 ChatView 接线。

/**
 * 在途下载所有权判定（installEngine/installModel 每次 invoke 前）。
 *
 * - claimed：无在途下载 → 本次 invoke 将发起新下载，乐观认领。登记必须
 *   早于 await——invoke 待决期间（下载进行中）用户点取消要能收掉它。
 * - inherited：在途下载由本组件先前（可能已被顶替的）流程发起（owned 仍
 *   为 true）→ 由当前流程继承，取消按钮必须仍能收掉它。否则顶替流入口
 *   把 owned 清零后谁都不认领，取消退化为 no-op，最高 GB 级的模型下载在
 *   用户明确取消后仍继续到底（顶替窗口回归点）。
 * - 外部入口（设置页等）发起的在途下载：既不认领也不继承，绝不代取消。
 *
 * @param {{inFlight: boolean, owned: boolean}} input
 * @returns {{claimed: boolean, inherited: boolean, owned: boolean}}
 */
export function resolveDownloadOwnership({ inFlight, owned }) {
  const inherited = inFlight && owned;
  const claimed = !inFlight;
  return { claimed, inherited, owned: claimed || inherited };
}

/**
 * invoke 返回后的所有权（抢先返回 false 与本方下载结束共用）：
 * 乐观认领的下载要么已完成要么被他人抢先，使命结束立即归还；继承的
 * 在途下载不属于本次 invoke，所有权原样保持（等待收敛或由取消收口）。
 *
 * @param {{claimed: boolean, owned: boolean}} ownership resolveDownloadOwnership 的结果
 * @returns {boolean} 下一次读写 ref 应持有的 owned 值
 */
export function ownershipAfterInstall({ claimed, owned }) {
  return claimed ? false : owned;
}

/**
 * 迟到回调（旧 install/start 流程的回调）是否仍可接受：挂起条目已换人
 * （新流程顶替）或已清空（取消/卸载）时一律丢弃，不得 resolve 新
 * promise，也不得关掉新流程的对话框。
 *
 * @param {{token: number}|null} pending 当前挂起的发送条目
 * @param {number} token 回调所属流程的 token
 */
export function acceptsLateCallback(pending, token) {
  return !!(pending && pending.token === token);
}

/**
 * startEngine 的 invoke 返回进死流程（token 已被取消/顶替/卸载作废）后
 * 是否补停引擎：仅本流程是发起方（ownedStart）且所有权已被取消路径清零
 * （startedRef === 0）时补停——覆盖「取消路径的 stop 先于在途 start 到达
 * 后端、后端只在临界区置 STOP_REQUESTED 后照常 spawn」的竞态；接管的新
 * 流程持有自己的 token（startedRef 非零），不得替它停。
 */
export function shouldStopEngineAfterLateInvoke({ ownedStart, alive, startedRef }) {
  return ownedStart && !alive && startedRef === 0;
}
