#!/usr/bin/env node
import assert from 'node:assert/strict';
import test from 'node:test';

import { isValidVersion } from '../sync-version.mjs';

test('接受合法 SemVer 版本号', () => {
  for (const version of [
    '0.0.0',
    '0.6.5',
    '1.2.3-rc.1',
    '1.2.3-alpha.beta',
    '1.2.3+build.01',
    '1.2.3-rc.1+build.7',
  ]) {
    assert.equal(isValidVersion(version), true, version);
  }
});

test('拒绝会导致 Cargo 或打包阶段失败的非法版本号', () => {
  for (const version of [
    '1.2',
    'v1.2.3',
    '01.2.3',
    '1.02.3',
    '1.2.03',
    '1.2.3foo',
    '1.2.3-',
    '1.2.3-01',
    '1.2.3..',
    '1.2.3+',
  ]) {
    assert.equal(isValidVersion(version), false, version);
  }
});
