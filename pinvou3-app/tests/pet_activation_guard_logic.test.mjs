import assert from 'node:assert/strict';
import { createPetActivationGuard } from '../src/features/pet/activation-guard.js';

function fakeClick() {
  const calls = [];
  return {
    calls,
    preventDefault() { calls.push('preventDefault'); },
    stopPropagation() { calls.push('stopPropagation'); },
    stopImmediatePropagation() { calls.push('stopImmediatePropagation'); },
  };
}

let now = 1_000;
const guard = createPetActivationGuard({ now: () => now, durationMs: 220 });

const ordinaryFocusClick = fakeClick();
assert.equal(guard.handleClick(ordinaryFocusClick), false);
assert.deepEqual(ordinaryFocusClick.calls, [], 'ordinary focus must not swallow a click');

guard.arm();
const activationClick = fakeClick();
assert.equal(guard.handleClick(activationClick), true);
assert.deepEqual(activationClick.calls, [
  'preventDefault',
  'stopPropagation',
  'stopImmediatePropagation',
]);

const nextRealClick = fakeClick();
assert.equal(guard.handleClick(nextRealClick), false);
assert.deepEqual(nextRealClick.calls, [], 'only the activation click should be swallowed');

guard.arm();
now += 221;
const expiredClick = fakeClick();
assert.equal(guard.handleClick(expiredClick), false);
assert.deepEqual(expiredClick.calls, [], 'expired guards must not affect later clicks');

console.log('pet activation guard logic tests passed');
