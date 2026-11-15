import assert from 'node:assert/strict';
import test from 'node:test';

import { errorCode, localizedErrorMessage } from '../src/shared/user-facing-error.js';

test('errorCode parses snake_case codes from objects and message prefixes', () => {
  assert.equal(errorCode({ code: 'rebind_to_root' }), 'rebind_to_root');
  assert.equal(errorCode({ code: ' rebind_to_root ' }), 'rebind_to_root', 'object codes are trimmed');
  assert.equal(errorCode({ message: 'conn: timeout' }), 'conn');
  assert.equal(errorCode('create_session: invalid workspace_path'), 'create_session');
  assert.equal(errorCode({ code: 'OpFailed' }), '', 'uppercase is rejected');
  assert.equal(errorCode({ code: 123 }), '', 'non-string codes are rejected');
  assert.equal(errorCode('no separator here'), '');
  assert.equal(errorCode(':leading-separator'), '', 'an empty prefix is not a code');
  assert.equal(errorCode(null), '');
  assert.equal(errorCode({ other: 'x' }), '');
});

test('localizedErrorMessage maps a known code and falls back otherwise', () => {
  const messages = { rebind_to_root: 'localized copy', empty_code: '' };
  assert.equal(localizedErrorMessage({ code: 'rebind_to_root' }, messages, 'FALLBACK'), 'localized copy');
  assert.equal(localizedErrorMessage({ code: 'empty_code' }, messages, 'FALLBACK'), 'FALLBACK', 'an empty copy falls back');
  assert.equal(localizedErrorMessage({ code: 'unknown_code' }, messages, 'FALLBACK'), 'FALLBACK');
  assert.equal(localizedErrorMessage(null, messages, 'FALLBACK'), 'FALLBACK');
  assert.equal(localizedErrorMessage({ code: 'rebind_to_root' }, null, 'FALLBACK'), 'FALLBACK');
});

test('localizedErrorMessage never resolves inherited Object.prototype keys', () => {
  // 'constructor' passes the snake_case pattern; the lookup must return the
  // fallback string, not the inherited Object constructor function.
  const resolved = localizedErrorMessage('constructor: evil', {}, 'FALLBACK');
  assert.equal(resolved, 'FALLBACK');
  assert.equal(typeof resolved, 'string');
});
