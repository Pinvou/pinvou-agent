import assert from 'node:assert/strict';
import test from 'node:test';
import { uninstallErrorCopy } from '../src/features/tools/uninstall-logic.js';
import { dict } from './helpers/i18n-all.js';

for (const language of ['zh', 'en', 'ja']) {
  test(`uninstall refusals explain recovery in ${language}`, () => {
    const copy = dict[language].uiToolStore;
    assert.ok(copy.uninstallBusy?.length);
    assert.ok(copy.uninstallReleaseFailed?.length);
    assert.equal(uninstallErrorCopy('marketplace_uninstall_busy', copy), copy.uninstallBusy);
    assert.equal(uninstallErrorCopy(new Error('marketplace_uninstall_release_failed'), copy), copy.uninstallReleaseFailed);
    assert.equal(uninstallErrorCopy('rename failed: os error 5', copy), 'rename failed: os error 5');
    assert.equal(uninstallErrorCopy('x'.repeat(300), copy).length, 240);
  });
}
