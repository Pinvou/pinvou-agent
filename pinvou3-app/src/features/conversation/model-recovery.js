import { dict, languageFromLocaleTags } from '../../shared/i18n.js';

export function isActiveChildRecoveryNotice(item) {
  return Boolean(!item?.legacyConversationOnly && item?.modelRecovery?.agent_id && item.modelRecovery.state === 'waiting');
}

export function restoreChildRecoveryNotices(previous, current) {
  const key = item => JSON.stringify([item.modelRecovery.agent_id, item.modelRecovery.operation_id]);
  const latest = new Map(current.filter(item => item?.modelRecovery?.agent_id).map(item => [key(item), item]));
  const restored = previous.map(item => {
    if (!item?.modelRecovery?.agent_id) return item;
    const identity = key(item);
    const notice = latest.get(identity) || item;
    latest.delete(identity);
    return notice;
  });
  return [...restored, ...latest.values()];
}

export function recoveryNotice(receipt, language) {
  const lang = languageFromLocaleTags(language, 'en');
  const copy = dict[lang]?.uiChat;
  if (!copy || !receipt || !['waiting', 'interrupted', 'recovered', 'cancelled', 'stopped'].includes(receipt.state)) return null;
  const next = Number(receipt.next_retry_at_ms);
  const time = Number.isFinite(next) && next > 0
    ? new Date(next).toLocaleString(lang === 'zh' ? 'zh-CN' : lang === 'ja' ? 'ja-JP' : 'en-US') : '';
  const key = receipt.agent_id && receipt.state === 'waiting' ? 'modelRecoveryChildWaiting'
    : { waiting: 'modelRecoveryWaiting', interrupted: 'modelRecoveryInterrupted', cancelled: 'modelRecoveryCancelled', stopped: 'modelRecoveryStopped' }[receipt.state];
  if (receipt.state === 'recovered') return '';
  const prefix = receipt.agent_id ? copy.modelRecoveryAgent.replace('{agent}', () => receipt.agent_id) : '';
  return prefix + copy[key].replace('{attempt}', () => String(receipt.attempt))
    .replace('{max}', () => String(receipt.max_attempts))
    .replace('{time}', () => time || copy.modelRecoveryUnknownTime);
}

export function upsertRecoveryNotice(receipt, items, addItem, language) {
  const text = recoveryNotice(receipt, language);
  if (text === null) return false;
  let existing = null;
  for (let index = items.length - 1; index >= 0; index -= 1) {
    const item = items[index];
    if (item?.modelRecovery && item.modelRecovery.agent_id === receipt.agent_id && item.modelRecovery.operation_id === receipt.operation_id) {
      existing = item;
      break;
    }
  }
  if (!text) {
    if (existing) existing.legacyConversationOnly = true;
  } else if (existing) {
    existing.text = text;
    existing.modelRecovery = receipt;
    existing.legacyConversationOnly = false;
  } else {
    addItem(text, { modelRecovery: receipt, turnErrorNotice: true });
  }
  return true;
}

export function settleRecoveryNotices(items, status, error, language) {
  const outcome = String(status || '').toLowerCase();
  for (const item of items) {
    if (item?.modelRecovery?.state !== 'waiting') continue;
    // Detached children outlive a foreground turn. Only their own recovery or
    // terminal receipt can settle the notice, including its visibility.
    if (item.modelRecovery.agent_id) continue;
    const state = !error && outcome === 'completed' ? 'recovered'
      : !error && ['interrupted', 'cancelled', 'canceled'].includes(outcome) ? 'cancelled' : 'stopped';
    const receipt = { ...item.modelRecovery, state };
    item.modelRecovery = receipt;
    if (state === 'recovered') item.legacyConversationOnly = true;
    else item.text = recoveryNotice(receipt, language);
  }
}

export function settleRecoveryEvent(payload = {}, state = {}) {
  settleRecoveryNotices(state.chatItems || [], payload.status, payload.error, state.settings?.language);
}

// Classic desktop/web bridges share this feature with the React native lane.
globalThis.PinvouModelRecovery = { recoveryNotice, upsertRecoveryNotice, settleRecoveryNotices, settleRecoveryEvent, isActiveChildRecoveryNotice, restoreChildRecoveryNotices };
