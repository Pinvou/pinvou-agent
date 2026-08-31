#!/usr/bin/env node
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

import { isValidVersion, updateCargoLockPackageVersion } from '../sync-version.mjs';

const TEST_FILE = fileURLToPath(import.meta.url);
const SYNC_SCRIPT = join(dirname(TEST_FILE), '..', 'sync-version.mjs');

function writeFixtureFile(root, relativePath, content) {
  const path = join(root, relativePath);
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, content);
}

function createFixtureRepo() {
  const root = mkdtempSync(join(tmpdir(), 'pinvou-sync-version-'));
  writeFixtureFile(root, 'VERSION', '0.8.9\n');
  writeFixtureFile(root, 'pinvou3-app/src-tauri/tauri.conf.json', '{"version":"0.8.9"}\n');
  writeFixtureFile(root, 'pinvou3-app/src-tauri/Cargo.toml', '[package]\nname = "pinvou3-tauri"\nversion = "0.8.9"\n');
  writeFixtureFile(root, 'pinvou-knowledge/Cargo.toml', '[package]\nname = "pinvou-knowledge"\nversion = "0.8.9"\n');
  writeFixtureFile(root, 'pinvou-knowledge/Cargo.lock', '[[package]]\nname = "pinvou-knowledge"\nversion = "0.8.9"\n');
  writeFixtureFile(root, 'pinvou3-app/src-tauri/Cargo.lock', `[[package]]
name = "dependency"
version = "1.0.0"

[[package]]
name = "pinvou-knowledge"
version = "0.8.8"

[[package]]
name = "pinvou3-tauri"
version = "0.8.8"
`);
  writeFixtureFile(root, 'pinvou3-app/package.json', '{"version":"0.8.9"}\n');
  writeFixtureFile(root, 'scripts/.gitkeep', '');
  copyFileSync(SYNC_SCRIPT, join(root, 'scripts/sync-version.mjs'));
  return root;
}

function runFixtureScript(root, args = []) {
  return spawnSync(process.execPath, [join(root, 'scripts/sync-version.mjs'), ...args], {
    cwd: root,
    encoding: 'utf8',
  });
}

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

test('写入模式同时同步应用 Cargo.lock 中的两个工作区包', (t) => {
  const root = createFixtureRepo();
  t.after(() => rmSync(root, { recursive: true, force: true }));

  const result = runFixtureScript(root);
  assert.equal(result.status, 0, result.stderr);

  const lock = readFileSync(join(root, 'pinvou3-app/src-tauri/Cargo.lock'), 'utf8');
  assert.match(lock, /name = "pinvou-knowledge"\nversion = "0\.8\.9"/u);
  assert.match(lock, /name = "pinvou3-tauri"\nversion = "0\.8\.9"/u);
  assert.match(lock, /name = "dependency"\nversion = "1\.0\.0"/u);
});

test('--check 拒绝应用 Cargo.lock 中任一工作区包的版本漂移', (t) => {
  for (const driftingPackageName of ['pinvou-knowledge', 'pinvou3-tauri']) {
    const root = createFixtureRepo();
    t.after(() => rmSync(root, { recursive: true, force: true }));
    const syncResult = runFixtureScript(root);
    assert.equal(syncResult.status, 0, syncResult.stderr);

    const lockPath = join(root, 'pinvou3-app/src-tauri/Cargo.lock');
    const lock = readFileSync(lockPath, 'utf8').replace(
      new RegExp(`(name = "${driftingPackageName}"\\nversion = ")0\\.8\\.9"`, 'u'),
      (_match, prefix) => `${prefix}0.8.8"`,
    );
    writeFileSync(lockPath, lock);

    const result = runFixtureScript(root, ['--check']);
    assert.equal(result.status, 1, `${driftingPackageName}: ${result.stdout}\n${result.stderr}`);
    assert.match(result.stderr, /pinvou3-app\/src-tauri\/Cargo\.lock/u);
  }
});
