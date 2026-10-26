import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { conformance } from '../conformance.mjs';

test('model report binds actual source bytes and never certifies a platform', async () => {
  const report = await conformance('model');
  assert.equal(report.qualification, false); assert.equal(report.lab, null);
  assert.match(report.source.sha256, /^[a-f0-9]{64}$/);
  assert.ok(report.cases.length > 0 && report.cases.every(item => item.provenance === 'contract-model'));
  await assert.rejects(conformance('model', 'unexpected'));
  await assert.rejects(conformance('lab-primitives'));
});
test('isolated actual lab primitives and explicit production refusals have separate evidence classes', async () => {
  const parent = await mkdtemp(path.join(tmpdir(), 'pinvou-platform-conformance-'));
  const report = await conformance('lab-primitives', parent);
  assert.equal(report.qualification, false); assert.equal(report.lab.productionBackupFormat, false);
  assert.equal(report.lab.operations.length, report.cases.filter(item => item.provenance === 'contract-model').length);
  assert.ok(report.lab.operations.every(item => item.status === 'unsupported'));
  for (const id of ['lab.unsavable-writer-refuses-before-backup', 'lab.actual-writer-close-before-report',
    'lab.synthetic-freeze-cannot-prove-production-admission', 'lab.experimental-AEAD-complete-manifest-readback'])
    assert.ok(report.cases.some(item => item.id === id && item.provenance === 'experimental-primitive' && item.status === 'passed'));
  assert.equal(report.lab.runIds.length, 2);
});
