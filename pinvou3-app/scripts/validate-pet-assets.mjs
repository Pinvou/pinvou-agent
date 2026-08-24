#!/usr/bin/env node
import { createHash } from 'node:crypto';
import { readFile, readdir } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const appRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const manifestPath = path.join(appRoot, 'src', 'features', 'pet', 'pet-manifest.json');
const registryPath = path.join(appRoot, 'src', 'features', 'pet', 'pet-registry.js');
const assetRoot = path.join(appRoot, 'src', 'assets', 'pet');
const expectedIds = ['lingling', 'langlang', 'ace-taffy'];
const atlasRowsByVersion = Object.freeze({ 1: 9, 2: 11 });
const releaseMode = process.argv.slice(2).includes('--release');
const errors = [];

function check(condition, message) {
  if (!condition) errors.push(message);
}

function sorted(values) {
  return [...values].sort(); // eslint-disable-line unicorn/require-array-sort-compare -- 字符串数组字典序即断言预期
}

function sameValues(actual, expected) {
  return JSON.stringify(sorted(actual)) === JSON.stringify(sorted(expected));
}

function validateManifest(manifest) {
  check(Array.isArray(manifest), 'manifest must be an array');
  if (!Array.isArray(manifest)) return [];

  check(manifest.length === 3, 'manifest must contain exactly three pets');
  const ids = [];
  const requiredKeys = [
    'description',
    'id',
    'name',
    'placeholder',
    'spriteVersionNumber',
    'themeColor',
  ];

  manifest.forEach((pet, index) => {
    const label = `manifest[${index}]`;
    check(pet !== null && typeof pet === 'object' && !Array.isArray(pet), `${label} must be an object`);
    if (pet === null || typeof pet !== 'object' || Array.isArray(pet)) return;

    check(sameValues(Object.keys(pet), requiredKeys), `${label} must have exactly ${requiredKeys.join(', ')}`);
    check(typeof pet.id === 'string' && pet.id.length > 0, `${label}.id must be a non-empty string`);
    check(typeof pet.name === 'string' && pet.name.length > 0, `${label}.name must be a non-empty string`);
    check(
      typeof pet.description === 'string' && pet.description.length > 0,
      `${label}.description must be a non-empty string`,
    );
    check(
      typeof pet.themeColor === 'string' && /^#[0-9A-Fa-f]{6}$/.test(pet.themeColor),
      `${label}.themeColor must be a six-digit hex color`,
    );
    check(typeof pet.placeholder === 'boolean', `${label}.placeholder must be an explicit boolean`);
    check(
      Number.isInteger(pet.spriteVersionNumber)
        && Object.hasOwn(atlasRowsByVersion, pet.spriteVersionNumber),
      `${label}.spriteVersionNumber must be 1 or 2`,
    );
    ids.push(pet.id);
  });

  check(new Set(ids).size === ids.length, 'manifest pet IDs must be unique');
  check(sameValues(ids, expectedIds), `manifest IDs must be exactly ${expectedIds.join(', ')}`);
  check(
    JSON.stringify(ids) === JSON.stringify(expectedIds),
    `manifest pet order must be ${expectedIds.join(' -> ')}`,
  );
  return ids;
}

async function loadRegistry(registrySource, manifest) {
  const importLine = "import manifest from './pet-manifest.json';";
  check(registrySource.includes(importLine), 'registry must import the manifest SSOT');
  if (!registrySource.includes(importLine)) return null;

  const executableSource = registrySource.replace(
    importLine,
    // eslint-disable-next-line unicorn/no-unsafe-string-replacement -- 替换值为受控字面量
    `const manifest = ${JSON.stringify(manifest)};`,
  );
  const dataUrl = `data:text/javascript;base64,${Buffer.from(executableSource).toString('base64')}`;
  return import(dataUrl);
}

async function listWebpFiles(directory) {
  const entries = await readdir(directory, { withFileTypes: true });
  const files = [];
  for (const entry of entries) {
    const entryPath = path.join(directory, entry.name);
    if (entry.isDirectory()) {
      files.push(...await listWebpFiles(entryPath));
    } else if (entry.isFile() && entry.name.endsWith('.webp')) {
      files.push(entryPath);
    }
  }
  return files;
}

function webpDimensions(buffer) {
  if (buffer.length < 16 || buffer.toString('ascii', 0, 4) !== 'RIFF'
    || buffer.toString('ascii', 8, 12) !== 'WEBP') {
    throw new Error('invalid WebP RIFF header');
  }

  let offset = 12;
  while (offset + 8 <= buffer.length) {
    const type = buffer.toString('ascii', offset, offset + 4);
    const size = buffer.readUInt32LE(offset + 4);
    const data = offset + 8;

    if (type === 'VP8X' && size >= 10) {
      return {
        width: 1 + buffer.readUIntLE(data + 4, 3),
        height: 1 + buffer.readUIntLE(data + 7, 3),
      };
    }
    if (type === 'VP8L' && size >= 5 && buffer[data] === 0x2f) {
      const bits = buffer.readUInt32LE(data + 1);
      return {
        width: 1 + (bits & 0x3fff),
        height: 1 + ((bits >>> 14) & 0x3fff),
      };
    }
    if (type === 'VP8 ' && size >= 10
      && buffer[data + 3] === 0x9d && buffer[data + 4] === 0x01 && buffer[data + 5] === 0x2a) {
      return {
        width: buffer.readUInt16LE(data + 6) & 0x3fff,
        height: buffer.readUInt16LE(data + 8) & 0x3fff,
      };
    }

    offset = data + size + (size % 2);
  }
  throw new Error('WebP dimensions were not found');
}

async function main() {
  let manifest;
  try {
    manifest = JSON.parse(await readFile(manifestPath, 'utf8'));
  } catch (error) {
    errors.push(`cannot read manifest: ${error.message}`);
    manifest = [];
  }
  const manifestIds = validateManifest(manifest);

  let registry = null;
  try {
    const registrySource = await readFile(registryPath, 'utf8');
    registry = await loadRegistry(registrySource, manifest);
  } catch (error) {
    errors.push(`cannot load registry: ${error.message}`);
  }

  if (registry) {
    const loaderIds = Object.keys(registry.PET_LOADERS);
    check(sameValues(loaderIds, manifestIds), 'manifest IDs and registry loader keys must match exactly');
    for (const id of loaderIds) {
      check(typeof registry.PET_LOADERS[id]?.cover === 'function', `${id} must have a cover loader`);
      check(typeof registry.PET_LOADERS[id]?.atlas === 'function', `${id} must have an atlas loader`);
    }
  }

  const atlasBuffers = [];
  for (const id of manifestIds) {
    const pet = manifest.find((entry) => entry?.id === id);
    const coverPath = path.join(assetRoot, id, 'cover.webp');
    const atlasPath = path.join(assetRoot, id, 'spritesheet.webp');
    const packagePath = path.join(assetRoot, id, 'pet.json');

    try {
      const packageManifest = JSON.parse(await readFile(packagePath, 'utf8'));
      check(packageManifest.id === id, `${id} pet.json id must match the manifest`);
      check(packageManifest.displayName === pet?.name, `${id} pet.json displayName must match the manifest`);
      check(packageManifest.description === pet?.description, `${id} pet.json description must match the manifest`);
      check(
        packageManifest.spriteVersionNumber === pet?.spriteVersionNumber,
        `${id} pet.json spriteVersionNumber must match the manifest`,
      );
      check(
        packageManifest.spritesheetPath === 'spritesheet.webp',
        `${id} pet.json spritesheetPath must be spritesheet.webp`,
      );
    } catch (error) {
      errors.push(`${id} pet.json is invalid or missing: ${error.message}`);
    }

    for (const [kind, filePath] of [['cover', coverPath], ['atlas', atlasPath]]) {
      try {
        const buffer = await readFile(filePath);
        if (kind === 'atlas') {
          atlasBuffers.push([id, buffer]);
          const dimensions = webpDimensions(buffer);
          const rows = atlasRowsByVersion[pet?.spriteVersionNumber];
          if (rows) {
            const expectedHeight = 208 * rows;
            check(
              dimensions.width === 1536 && dimensions.height === expectedHeight,
              `${id} v${pet.spriteVersionNumber} atlas must be 1536x${expectedHeight}, got ${dimensions.width}x${dimensions.height}`,
            );
          }
        } else {
          // 封面同样必须是合法 WebP 且为单帧规格——否则改名充数的坏文件
          // 能通过发布校验,直到设置页才暴露"封面加载失败"。
          const dimensions = webpDimensions(buffer);
          check(
            dimensions.width === 192 && dimensions.height === 208,
            `${id} cover must be 192x208, got ${dimensions.width}x${dimensions.height}`,
          );
        }
      } catch (error) {
        errors.push(`${id} ${kind} is invalid or missing: ${error.message}`);
      }
    }
  }

  try {
    const allWebpFiles = await listWebpFiles(assetRoot);
    const actualAtlases = allWebpFiles
      .filter((filePath) => path.basename(filePath) !== 'cover.webp')
      .map((filePath) => path.relative(assetRoot, filePath).replaceAll('\\', '/'));
    const expectedAtlases = manifestIds.map((id) => `${id}/spritesheet.webp`);
    check(
      sameValues(actualAtlases, expectedAtlases),
      `unregistered atlas files found or registered atlases missing: ${actualAtlases.join(', ')}`,
    );
  } catch (error) {
    errors.push(`cannot scan pet assets: ${error.message}`);
  }

  if (releaseMode) {
    for (const pet of manifest) {
      check(pet.placeholder === false, `${pet.id || 'unknown'} is still marked as a placeholder`);
    }
    const hashes = atlasBuffers.map(([id, buffer]) => [
      id,
      createHash('sha256').update(buffer).digest('hex'),
    ]);
    check(
      new Set(hashes.map(([, hash]) => hash)).size === manifestIds.length,
      'release atlases must have pairwise-distinct SHA-256 hashes',
    );
  }

  if (errors.length > 0) {
    console.error(`pet asset validation failed (${releaseMode ? 'release' : 'normal'} mode):`);
    for (const error of errors) console.error(`- ${error}`);
    process.exitCode = 1;
    return;
  }

  console.log(`pet asset validation passed (${releaseMode ? 'release' : 'normal'} mode)`);
}

await main();
