#!/usr/bin/env node
// Integrity contract for the vendored browser assets in src/vendor/:
// every distributed file must be registered in README.md with its exact
// SHA-256, and the working-tree bytes must match the registered value.
// This is what makes the registry an anti-drift guarantee instead of a
// documentation nicety — a refreshed or edited asset without a matching
// registry update fails `npm test`.
const assert = require('assert');
const crypto = require('crypto');
const fs = require('fs');
const path = require('path');

const vendorDir = path.join(__dirname, '..', 'src', 'vendor');
const readme = fs.readFileSync(path.join(vendorDir, 'README.md'), 'utf8');

const registered = [];
for (const line of readme.split('\n')) {
  const match = line.match(/^\|\s*`([^`]+)`\s*\|\s*([^|]+?)\s*\|\s*`([0-9a-f]{64})`\s*\|/);
  if (match) {
    registered.push({ file: match[1], version: match[2], sha256: match[3] });
  }
}

// The vendor set is intentionally a single file: only the Tailwind runtime
// still ships as a classic script loaded before `tauri-bridge.js`. marked and
// DOMPurify moved to npm dependencies bundled by Vite (Safari 14 baseline),
// so this contract pins the one remaining asset explicitly instead of
// assuming a minimum registry size.
assert.ok(
  registered.some((entry) => entry.file === 'tailwind.js'),
  'expected the README registry to list the Tailwind runtime (tailwind.js)'
);

for (const entry of registered) {
  assert.ok(entry.version.trim().length > 0, `registry row for ${entry.file} is missing a version`);
  const assetPath = path.join(vendorDir, entry.file);
  assert.ok(fs.existsSync(assetPath), `registered asset ${entry.file} does not exist in src/vendor/`);
  const actual = crypto.createHash('sha256').update(fs.readFileSync(assetPath)).digest('hex');
  assert.strictEqual(
    actual,
    entry.sha256,
    `SHA-256 mismatch for ${entry.file}: README registers ${entry.sha256}, working tree has ${actual}. ` +
      'If the asset was refreshed intentionally, update the README registry (and THIRD_PARTY_NOTICES.md ' +
      'when the version changes). If not, the file drifted from what was reviewed.'
  );
}

const registeredFiles = registered.map((entry) => entry.file).sort(); // eslint-disable-line unicorn/require-array-sort-compare -- lexicographic string order is the assertion's expectation
const onDiskFiles = fs
  .readdirSync(vendorDir)
  .filter((name) => name.endsWith('.js'))
  .sort(); // eslint-disable-line unicorn/require-array-sort-compare -- lexicographic string order is the assertion's expectation
assert.deepStrictEqual(
  onDiskFiles,
  registeredFiles,
  'every .js file in src/vendor/ must be registered in README.md with a SHA-256 so nothing ships untracked'
);

console.log(`vendor asset integrity: ${registered.length} registered files verified`);
