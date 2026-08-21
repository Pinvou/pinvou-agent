import { cpSync, existsSync, mkdirSync, readFileSync, readdirSync } from 'node:fs';
import { extname, join, resolve } from 'node:path';
import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

import {
  localClassicScriptPaths,
  resolveContainedRuntimePath,
} from './scripts/vite-runtime-assets.mjs';

const sourceRoot = resolve(import.meta.dirname, 'src');
const staticExtensions = new Set([
  '.avif', '.gif', '.ico', '.jpeg', '.jpg', '.png', '.svg', '.webp',
]);
const staticScripts = new Set([
  'features/attachments/attachment-drop-controller.js',
  'features/personas/personas-i18n.js',
  'features/updater/update-notice-logic.js',
  'platform/tauri/bridge.js',
  'platform/web/bootstrap.js',
  'platform/web/bridge.js',
  'platform/web/host-file-picker.js',
  'platform/web/access-policy.json',
  'shared/authority-sync-diagnostics.js',
  'shared/bridge-messages.js',
  'vendor/marked.min.js',
  'vendor/purify.min.js',
  'vendor/tailwind.js',
]);
const staticScriptPrefixes = ['platform/tauri/bridge/', 'platform/web/bridge/'];

function assertClassicRuntimeScriptsCopied(outputRoot) {
  const indexHtml = readFileSync(join(sourceRoot, 'index.html'), 'utf8');
  for (const relative of localClassicScriptPaths(indexHtml)) {
    const source = resolveContainedRuntimePath(sourceRoot, relative);
    const target = resolveContainedRuntimePath(outputRoot, relative);
    if (!existsSync(source)) {
      throw new Error(`Vite build references a missing local classic runtime script: ${relative}`);
    }
    if (!existsSync(target)) {
      throw new Error(`Vite build is missing local classic runtime script: ${relative}`);
    }
  }
}

function normalizeWebBasePath(value) {
  let raw = String(value || '/pinvou3/remote').trim();
  try {
    if (/^https?:\/\//i.test(raw)) raw = new URL(raw).pathname;
  } catch {}
  const trimmed = raw.replace(/^\/+|\/+$/g, '');
  return trimmed ? `/${trimmed}/` : '/';
}

function copyRuntimeAssets() {
  let outputRoot;
  return {
    name: 'pinvou-copy-runtime-assets',
    apply: 'build',
    configResolved(config) {
      outputRoot = resolve(config.root, config.build.outDir);
    },
    closeBundle() {
      const visit = (dir) => {
        for (const entry of readdirSync(dir, { withFileTypes: true })) {
          const source = join(dir, entry.name);
          if (entry.isDirectory()) {
            visit(source);
            continue;
          }
          const relative = source.slice(sourceRoot.length + 1).replaceAll('\\', '/');
          const isRuntimeScript = staticScripts.has(relative) || staticScriptPrefixes.some(prefix => relative.startsWith(prefix));
          if (!staticExtensions.has(extname(entry.name).toLowerCase()) && !isRuntimeScript) continue;
          const containedSource = resolveContainedRuntimePath(sourceRoot, relative);
          const target = resolveContainedRuntimePath(outputRoot, relative);
          mkdirSync(resolve(target, '..'), { recursive: true });
          cpSync(containedSource, target);
        }
      };
      if (existsSync(sourceRoot)) visit(sourceRoot);
      assertClassicRuntimeScriptsCopied(outputRoot);
    },
  };
}

export default defineConfig(({ mode }) => {
  const webBuild = mode === 'web';
  return {
  root: 'src',
  // The Relay and Vite build intentionally share one deployment variable;
  // each side only normalizes the trailing slash for its own router contract.
  base: webBuild ? normalizeWebBasePath(process.env.PINVOU_REMOTE_PUBLIC_BASE_PATH) : '/',
  publicDir: false,
  server: {
    host: process.env.PINVOU3_UI_DEV_HOST || '127.0.0.1',
    port: Number(process.env.PINVOU3_UI_DEV_PORT || 1420),
    strictPort: true,
  },
  plugins: [react(), copyRuntimeAssets()],
  build: {
    outDir: webBuild ? '../../remote-control-relay/web/dist' : '../dist',
    emptyOutDir: true,
    rolldownOptions: {
      input: webBuild
        ? { main: resolve(sourceRoot, 'index.html') }
        : {
            main: resolve(sourceRoot, 'index.html'),
            pet: resolve(sourceRoot, 'pet.html'),
            reader: resolve(sourceRoot, 'reader.html'),
          },
    },
  },
  };
});
