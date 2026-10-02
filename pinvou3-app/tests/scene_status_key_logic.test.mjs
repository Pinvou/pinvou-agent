// Round-32 minor 10 (review #455): the full scene-status key (round-31 m5)
// extracted pure — banner guard, ready-toast guard and welcome-card reset all
// consume this builder, so its null/epoch contract is pinned here directly.
import assert from 'node:assert/strict';
import { sceneStatusKey } from '../src/features/chat/scene_status_key.js';

// Drafts are null session ids; the epoch discriminates them.
assert.strictEqual(sceneStatusKey(null, 0), 'draft:0');
assert.strictEqual(sceneStatusKey(null, 1), 'draft:1');
assert.notStrictEqual(
  sceneStatusKey(null, 0),
  sceneStatusKey(null, 1),
  'a new-chat click must invalidate the previous draft key (null→null)',
);

// Distinct sessions never share a key, epochs only matter within a session id.
assert.strictEqual(sceneStatusKey('s1', 3), 's1:3');
assert.notStrictEqual(sceneStatusKey('s1', 3), sceneStatusKey('s2', 3));
assert.strictEqual(sceneStatusKey('s1', 3), sceneStatusKey('s1', 3));

console.log('scene_status_key_logic: ok');
