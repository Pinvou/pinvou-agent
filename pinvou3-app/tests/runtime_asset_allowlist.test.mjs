#!/usr/bin/env node
// Contract test for the copyRuntimeAssets allowlist in vite.config.mjs:
// assets referenced by string path (not ESM import) are copied verbatim into
// dist, so a reference missing from the allowlist becomes a silent runtime
// 404 and a stale allowlist entry silently bloats dist. Bind both directions.
import assert from 'node:assert/strict';
import { existsSync, readdirSync, readFileSync } from 'node:fs';
import { dirname, extname, join, resolve } from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';

import { staticRuntimeAssetPaths, staticRuntimeAssetPrefixes } from '../vite.config.mjs';

const appRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const sourceRoot = join(appRoot, 'src');

// Mirror the static-extension set in vite.config.mjs: only these file kinds
// are verbatim-copied as string-path assets.
const STATIC_EXTENSIONS = new Set(['.avif', '.gif', '.ico', '.jpeg', '.jpg', '.png', '.svg', '.webp']);
const SOURCE_FILE_EXTENSIONS = new Set(['.js', '.jsx', '.mjs', '.ts', '.tsx']);

function* walkSourceFiles(dir) {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) {
      yield* walkSourceFiles(path);
    } else if (SOURCE_FILE_EXTENSIONS.has(extname(entry.name).toLowerCase())) {
      yield path;
    }
  }
}

function isAllowlisted(relative) {
  return staticRuntimeAssetPaths.has(relative)
    || staticRuntimeAssetPrefixes.some(prefix => relative.startsWith(prefix));
}

test('every resolveAppAssetUrl string literal with a static extension is allowlisted', () => {
  // Only single-argument string literals are statically checkable; dynamic
  // paths (template literals, prefixes) are covered by the prefix entries.
  const literalCall = /resolveAppAssetUrl\(\s*(['"])([^'"]+)\1/gu;
  const referenced = new Map();
  for (const file of walkSourceFiles(sourceRoot)) {
    const content = readFileSync(file, 'utf8');
    for (const match of content.matchAll(literalCall)) {
      const relative = match[2].replace(/^\/+/, '');
      if (!STATIC_EXTENSIONS.has(extname(relative).toLowerCase())) continue;
      referenced.set(relative, file);
    }
  }

  assert.ok(referenced.size > 0, 'expected at least one static resolveAppAssetUrl literal');
  for (const [relative, file] of referenced) {
    assert.ok(
      isAllowlisted(relative),
      `${relative} (referenced in ${file}) is not covered by staticRuntimeAssetPaths/Prefixes in vite.config.mjs and would 404 at runtime`,
    );
    assert.ok(
      existsSync(join(sourceRoot, relative)),
      `${relative} (referenced in ${file}) does not exist under src/`,
    );
  }
});

test('every allowlisted asset entry still exists under src/', () => {
  for (const relative of staticRuntimeAssetPaths) {
    assert.ok(
      existsSync(join(sourceRoot, relative)),
      `stale staticRuntimeAssetPaths entry: ${relative} no longer exists under src/`,
    );
  }
  for (const prefix of staticRuntimeAssetPrefixes) {
    assert.ok(
      existsSync(join(sourceRoot, prefix)),
      `stale staticRuntimeAssetPrefixes entry: ${prefix} no longer exists under src/`,
    );
  }
});
