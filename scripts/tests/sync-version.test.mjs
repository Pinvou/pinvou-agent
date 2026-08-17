#!/usr/bin/env node
import assert from 'node:assert/strict';
import test from 'node:test';

import { isValidVersion, updateCargoLockPackageVersion } from '../sync-version.mjs';

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

test('只同步独立 Cargo.lock 中 pinvou-knowledge 自身的版本', () => {
  const lock = `# generated
[[package]]
name = "dependency"
version = "0.8.1"

[[package]]
name = "pinvou-knowledge"
version = "0.8.1"
dependencies = ["dependency"]
`;

  const result = updateCargoLockPackageVersion(lock, 'pinvou-knowledge', '0.8.3');

  assert.equal(result.version, '0.8.1');
  assert.match(result.content, /name = "dependency"\nversion = "0\.8\.1"/u);
  assert.match(result.content, /name = "pinvou-knowledge"\nversion = "0\.8\.3"/u);
});

test('Cargo.lock 缺少或重复根包时拒绝静默通过', () => {
  assert.throws(
    () => updateCargoLockPackageVersion('[[package]]\nname = "other"\nversion = "1.0.0"\n', 'pinvou-knowledge'),
    /实际找到 0 个/u,
  );
  const duplicate = `${'[[package]]\nname = "pinvou-knowledge"\nversion = "0.8.1"\n\n'.repeat(2)}`;
  assert.throws(
    () => updateCargoLockPackageVersion(duplicate, 'pinvou-knowledge'),
    /实际找到 2 个/u,
  );
});
