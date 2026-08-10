#!/usr/bin/env node
import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';

const manifestUrl = new URL('../src/features/pet/pet-manifest.json', import.meta.url);
const registryUrl = new URL('../src/features/pet/pet-registry.js', import.meta.url);
const manifest = JSON.parse(readFileSync(manifestUrl, 'utf8'));
const registrySource = readFileSync(registryUrl, 'utf8');
const importLine = "import manifest from './pet-manifest.json';";

assert.equal(registrySource.includes(importLine), true, 'registry must import the manifest SSOT');
const executableSource = registrySource.replace(
  importLine,
  `const manifest = ${JSON.stringify(manifest)};`,
);
const dir = mkdtempSync(join(tmpdir(), 'pinvou3-pet-registry-'));
const modulePath = join(dir, 'pet-registry.mjs');
writeFileSync(modulePath, executableSource);

try {
  const {
    DEFAULT_PET_ID,
    PET_LOADERS,
    PET_REGISTRY,
    normalizePetId,
    resolvePet,
  } = await import(`${pathToFileURL(modulePath).href}?t=${Date.now()}`);

  assert.equal(DEFAULT_PET_ID, 'lingling');
  for (const id of ['lingling', 'langlang', 'ace-taffy', 'vivi']) {
    assert.equal(normalizePetId(id), id);
  }
  for (const invalidId of [undefined, null, '', 'unknown', 'pinwu-lingling', 42, false, {}]) {
    assert.equal(normalizePetId(invalidId), 'lingling');
  }

  assert.equal(resolvePet('langlang').id, 'langlang');
  assert.equal(resolvePet('langlang').name, '浪浪');
  assert.equal(resolvePet('ace-taffy').name, 'Ace Taffy');
  assert.deepEqual(
    Object.fromEntries(manifest.map((pet) => [pet.id, pet.spriteVersionNumber])),
    { lingling: 1, langlang: 2, 'ace-taffy': 1, vivi: 1 },
  );
  assert.equal(resolvePet('missing'), PET_REGISTRY.lingling);
  assert.equal(resolvePet(), PET_REGISTRY.lingling);

  const manifestIds = manifest.map((pet) => pet.id).sort();
  const loaderIds = Object.keys(PET_LOADERS).sort();
  assert.deepEqual(loaderIds, manifestIds);
  assert.deepEqual(Object.keys(PET_REGISTRY).sort(), manifestIds);
  assert.deepEqual(
    Object.keys(PET_REGISTRY),
    ['lingling', 'langlang', 'ace-taffy', 'vivi'],
    'registry iteration order drives the visible card order',
  );
  for (const id of loaderIds) {
    const expectedLoaderKeys = id === 'vivi'
      ? ['atlas', 'cover', 'dragAtlas', 'idleSpecial', 'walkAtlas']
      : ['atlas', 'cover'];
    assert.deepEqual(Object.keys(PET_LOADERS[id]).sort(), expectedLoaderKeys);
    assert.equal(typeof PET_LOADERS[id].cover, 'function');
    assert.equal(typeof PET_LOADERS[id].atlas, 'function');
  }
  assert.equal(typeof resolvePet('vivi').walkAtlas, 'function');
  assert.equal(typeof resolvePet('vivi').dragAtlas, 'function');
  assert.equal(typeof resolvePet('vivi').idleSpecial, 'function');

  const hasEagerWebpImport = registrySource
    .split('\n')
    .some((line) => line.trimStart().startsWith('import ') && line.includes('.webp'));
  assert.equal(hasEagerWebpImport, false, 'registry must not eagerly import WebP assets');

  console.log('pet registry logic tests passed');
} finally {
  rmSync(dir, { recursive: true, force: true });
}
