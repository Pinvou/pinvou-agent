#!/usr/bin/env node
import assert from 'node:assert/strict';
import { copyFileSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.join(here, '..');

// The paste-image pre-read cap (the r5 size pre-check) rejects oversized
// attachments BEFORE FileReader materializes the byte copy. Node has no
// FileReader, so this host-free test proves the reject-before-read order
// directly: an over-cap file must reject with the routing code without ever
// reaching the reader, and deleting the pre-check branch fails here (the
// read path hits the missing FileReader instead of the cap rejection).
const temp = mkdtempSync(path.join(tmpdir(), 'pinvou3-paste-precheck-'));
try {
  const moduleDir = path.join(temp, 'features', 'attachments');
  const sharedDir = path.join(temp, 'shared');
  mkdirSync(moduleDir, { recursive: true });
  mkdirSync(sharedDir, { recursive: true });
  writeFileSync(path.join(temp, 'package.json'), '{"type":"module"}\n');
  copyFileSync(path.join(root, 'src', 'features', 'attachments', 'paste-image.js'), path.join(moduleDir, 'paste-image.js'));
  copyFileSync(path.join(root, 'src', 'shared', 'chunked-file-upload.js'), path.join(sharedDir, 'chunked-file-upload.js'));

  const mod = await import(pathToFileURL(path.join(moduleDir, 'paste-image.js')).href);
  const cap = globalThis.PinvouChunkedFileUpload.MAX_FILE_BYTES;
  assert.ok(Number.isFinite(cap) && cap > 0, 'the shared attachment cap must be initialized');

  await assert.rejects(
    mod.readPasteImageAsBytes({ size: cap + 1, type: 'image/png' }),
    (error) => {
      assert.equal(error.code, 'device_upload_too_large', 'the rejection must carry the routing code');
      assert.match(error.message, /paste image exceeds the \d+ MB attachment cap/);
      return true;
    },
  );

  // Exactly at the cap is still allowed through to the reader lane — the
  // boundary is strictly greater-than. Node may or may not define a global
  // FileReader, so only assert that this failure is NOT the cap rejection.
  await assert.rejects(
    mod.readPasteImageAsBytes({ size: cap, type: 'image/png' }),
    (error) => error.code !== 'device_upload_too_large',
    'an at-cap file must reach the reader lane, not the cap rejection',
  );
} finally {
  rmSync(temp, { recursive: true, force: true });
}
