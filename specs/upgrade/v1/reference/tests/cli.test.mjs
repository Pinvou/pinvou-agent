import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, realpath, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { basename, dirname, join } from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { createFixtureSet, NOW } from './fixtures.mjs';

const cli = fileURLToPath(new URL('../cli.mjs', import.meta.url));
// Bound a hung test process while allowing simultaneous contract-review jobs.
// This is a test harness budget, never a network or download deadline.
const run = (...args) => {
  const result = spawnSync(process.execPath, [cli, ...args], { encoding: 'utf8', timeout: 60_000 });
  assert.equal(result.error, undefined, `CLI test process failed: ${result.error?.code ?? 'unknown'}`);
  return result;
};

test('local CLI E2E verifies a real Ed25519 envelope and rejects tampered/sensitive inputs without echoing them', async () => {
  const temporaryRoot = await realpath(tmpdir());
  const directory = await mkdtemp(join(temporaryRoot, 'pinvou-t01-contract-'));
  try {
    const fixtures = createFixtureSet();
    const claims = fixtures.claims.decision;
    const paths = ['envelope.json', 'root.json', 'context.json'].map((name) => join(directory, name));
    await writeFile(paths[0], fixtures.sign(claims));
    await writeFile(paths[1], JSON.stringify(fixtures.metadata.root));
    await writeFile(paths[2], JSON.stringify({ now: NOW, expected: { role: claims.role,
      product: claims.product, component: claims.component, scope: claims.scope } }));
    const valid = run('verify', ...paths);
    assert.equal(valid.status, 0, valid.stderr);
    assert.equal(valid.stdout, 'VALID\n');
    await writeFile(paths[0], '{"private-user-input":"do-not-echo","private-user-input":2}');
    const invalid = run('verify', ...paths);
    assert.equal(invalid.status, 1);
    assert.equal(invalid.stdout, '');
    assert.equal(invalid.stderr, 'JSON_DUPLICATE_KEY\n');
    await writeFile(paths[0], Buffer.alloc(1024 * 1024 + 1, 32));
    const oversized = run('canonicalize', paths[0]);
    assert.equal(oversized.status, 1);
    assert.equal(oversized.stderr, 'JSON_INPUT_LIMIT\n');
  } finally {
    const cleanupTarget = await realpath(directory);
    assert.equal(dirname(cleanupTarget), temporaryRoot);
    assert.ok(basename(cleanupTarget).startsWith('pinvou-t01-contract-'));
    await rm(cleanupTarget, { recursive: true, force: true });
  }
});
