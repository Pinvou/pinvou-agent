import assert from 'node:assert/strict';
import './helpers/i18n-all.js';
import { recoveryNotice, upsertRecoveryNotice, settleRecoveryNotices } from '../src/features/conversation/model-recovery.js';
import { conversationItemsForMode } from '../src/features/conversation/deepseek-conversation.js';

const base = { state: 'waiting', attempt: 2, max_attempts: 20, next_retry_at_ms: 1900000000000, operation_id: 'operation-1' };
for (const [language, word] of [['zh-Hans', '限流'], ['en', 'rate limited'], ['ja', 'レート制限']]) {
  const notice = recoveryNotice(base, language);
  assert.ok(notice.includes(word), notice);
  assert.ok(notice.includes('2/20'), notice);
  assert.ok(recoveryNotice({ ...base, state: 'interrupted' }, language));
  assert.equal(recoveryNotice({ ...base, state: 'recovered' }, language), '');
  assert.ok(recoveryNotice({ ...base, agent_id: 'detached-child' }, language).includes(word));
  assert.ok(!/Use Stop|可点击停止|「停止」/.test(recoveryNotice({ ...base, agent_id: 'detached-child' }, language)));
}
const items = [];
const add = (text, extra) => items.push({ text, ...extra });
upsertRecoveryNotice(base, items, add, 'en');
upsertRecoveryNotice({ ...base, attempt: 3 }, items, add, 'en');
assert.equal(items.length, 1);
assert.ok(items[0].text.includes('3/20'));
upsertRecoveryNotice({ ...base, agent_id: 'child-1' }, items, add, 'en');
assert.equal(items.length, 2);
upsertRecoveryNotice({ ...base, state: 'recovered' }, items, add, 'en');
assert.equal(items[0].legacyConversationOnly, true);
assert.notEqual(items[1].legacyConversationOnly, true);
assert.equal(upsertRecoveryNotice({ ...base, state: 'unexpected' }, items, add, 'en'), false);
console.log('model recovery UI: ok');

// Supported Safari 14 has no Array.findLast; recovery must still render.
const legacy = [];
Object.defineProperty(legacy, 'findLast', { value: undefined });
upsertRecoveryNotice(base, legacy, (text, extra) => { legacy.push({ text, ...extra }); }, 'en');
settleRecoveryNotices(legacy, 'Interrupted', null, 'en');
assert.equal(legacy[0].modelRecovery.state, 'cancelled');
assert.ok(!legacy[0].text.includes('Next attempt'));
assert.ok(recoveryNotice({ ...base, agent_id: '$&' }, 'en').includes('Subtask $&:'));

const children = [];
const addChild = (text, extra) => children.push({ text, ...extra });
upsertRecoveryNotice({ ...base, agent_id: 'child-a' }, children, addChild, 'en');
upsertRecoveryNotice({ ...base, agent_id: 'child-b' }, children, addChild, 'en');
upsertRecoveryNotice({ ...base, agent_id: 'child-a', state: 'cancelled', next_retry_at_ms: null }, children, addChild, 'en');
assert.equal(children[0].modelRecovery.state, 'cancelled');
assert.equal(children[1].modelRecovery.state, 'waiting');
assert.ok(!children[0].text.includes('Next attempt'));

// A detached child may have no new event throughout a long Retry-After.
// Finishing the root must retain that child's last truthful live observation.
settleRecoveryNotices(children, 'Completed', null, 'en');
assert.equal(children[1].modelRecovery.state, 'waiting');
assert.ok(conversationItemsForMode(children).includes(children[1]));
upsertRecoveryNotice({ ...base, agent_id: 'child-b', state: 'recovered' }, children, addChild, 'en');
assert.ok(!conversationItemsForMode(children).includes(children[1]));
