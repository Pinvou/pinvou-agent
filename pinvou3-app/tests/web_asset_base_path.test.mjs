import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const { resolveAppAssetUrl } = await import(
  pathToFileURL(path.join(root, 'src', 'shared', 'asset-url.mjs')).href
);

assert.equal(
  resolveAppAssetUrl('/assets/brand/brand-blue.png', '/pinvou3/remote/'),
  '/pinvou3/remote/assets/brand/brand-blue.png',
);
assert.equal(
  resolveAppAssetUrl('file-icons/pdf.svg', '/pinvou3/remote'),
  '/pinvou3/remote/file-icons/pdf.svg',
);
assert.equal(
  resolveAppAssetUrl('assets/brand/brand-blue.png', '/'),
  '/assets/brand/brand-blue.png',
);
assert.equal(
  resolveAppAssetUrl('data:image/png;base64,AA==', '/pinvou3/remote/'),
  'data:image/png;base64,AA==',
);
assert.equal(
  resolveAppAssetUrl('https://cdn.example.com/logo.png', '/pinvou3/remote/'),
  'https://cdn.example.com/logo.png',
);

const jsxSources = [];
const collectJsx = (dir) => {
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const target = path.join(dir, entry.name);
    if (entry.isDirectory()) collectJsx(target);
    else if (entry.name.endsWith('.jsx')) jsxSources.push(fs.readFileSync(target, 'utf8'));
  }
};
collectJsx(path.join(root, 'src'));

const jsx = jsxSources.join('\n');
assert.doesNotMatch(
  jsx,
  /\bsrc\s*=\s*["']\/(?:assets|file-icons|avatars)\//,
  'JSX image sources must not bypass the configured Vite base path',
);
assert.doesNotMatch(
  jsx,
  /\bsrc\s*=\s*["']brand-blue\.png["']/,
  'brand images must use the shared base-path resolver',
);

const pinvouLogo = fs.readFileSync(
  path.join(root, 'src', 'components', 'PinvouLogo.jsx'),
  'utf8',
);
const knowledgeView = fs.readFileSync(
  path.join(root, 'src', 'features', 'knowledge', 'KnowledgeView.jsx'),
  'utf8',
);
assert.match(pinvouLogo, /resolveAppAssetUrl\('assets\/brand\/brand-blue\.png'\)/);
assert.match(knowledgeView, /const fileIconSrc = \(ext, category\) => resolveAppAssetUrl\(/);

const viteConfig = fs.readFileSync(path.join(root, 'vite.config.js'), 'utf8');
assert.match(
  viteConfig,
  /'shared\/bridge-messages\.js'/,
  'classic shared bridge dependencies must be copied into built UI assets',
);
assert.match(
  viteConfig,
  /'shared\/chunked-file-upload\.js'/,
  'the shared chunk uploader must be copied into built UI assets',
);

console.log('web asset base-path tests passed');
