// Fresh test keys and public fixture only; never use as shipping trust.
import fs from 'node:fs';
import path from 'node:path';
import { createRequire } from 'node:module';
import { createFixtureSet } from '../specs/upgrade/v1/reference/tests/fixtures.mjs';

const require = createRequire(import.meta.url);
const { canonicalBytes, sha256, TARGET, LAYOUT } = require('../pinvou3-app/scripts/tauri/windows-host-profile.js');
const destination = process.argv[2];
if (!destination || !path.isAbsolute(destination) || process.argv.length !== 3) throw new Error('absolute fixture directory required');
const root = Buffer.from(createFixtureSet().bytes.root);
const seed = {
  hostSeedVersion: 1, profileKind: 'lab', profileId: 'metadata-component-test',
  product: 'pinvou', component: 'app', targetKey: TARGET,
  buildId: 'b'.repeat(64), appVersion: '1.0.0', layoutId: LAYOUT,
  hostIpcVersion: 1, hostStateVersion: 1,
  originalRoot: { size: root.length, sha256: sha256(root) },
  approvedInitialPublisherPolicy: { policyId: 'isolated-test-only', signerCertificateSha256: 'c'.repeat(64) },
};
fs.writeFileSync(path.join(destination, 'seed.json'), await canonicalBytes(seed), { flag: 'wx' });
fs.writeFileSync(path.join(destination, 'root.json'), root, { flag: 'wx' });
