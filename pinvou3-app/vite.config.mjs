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
// Exported for scripts/audit-compat.mjs: the verbatim-copied runtime scripts
// keep one shared list so the compatibility audit always matches the copy set.
export const staticRuntimeScripts = new Set([
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
  'shared/chunked-file-upload.js',
  'shared/format-utils.js',
  'shared/legacy-polyfills.js',
  'shared/markdown-bridge-fallback.js',
  'vendor/tailwind.js',
]);
export const staticRuntimeScriptPrefixes = ['platform/tauri/bridge/', 'platform/web/bridge/'];

// Verbatim-copied static assets referenced by string paths instead of ESM
// imports (JSX `src="..."` literals, `resolveAppAssetUrl('...')`, CSS url(), or
// dynamic prefixes like `file-icons/theme/${iconFile}` and
// `'avatars/avatar-' + n + '.svg'`). These must exist under their source
// relative paths in dist. Images imported through ESM are emitted hashed by
// Vite and must NOT be listed here — duplicating them verbatim doubles dist
// size. When new code starts referencing an asset by string path, register it
// in one of the two lists below; tests/runtime_asset_allowlist.test.mjs binds
// resolveAppAssetUrl('...') literals to these lists and rejects stale entries.
export const staticRuntimeAssetPaths = new Set([
  'assets/brand/brand-blue.png',
  'assets/megacube-icon.png',
]);
export const staticRuntimeAssetPrefixes = [
  'assets/tool-icons/',
  'avatars/',
  'brand-icons/',
  'file-icons/',
];

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
  } catch { /* not an http URL; keep raw */ }
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
          const isRuntimeScript = staticRuntimeScripts.has(relative)
            || staticRuntimeScriptPrefixes.some(prefix => relative.startsWith(prefix));
          const isStringPathAsset = staticExtensions.has(extname(entry.name).toLowerCase())
            && (staticRuntimeAssetPaths.has(relative)
              || staticRuntimeAssetPrefixes.some(prefix => relative.startsWith(prefix)));
          if (!isRuntimeScript && !isStringPathAsset) continue;
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

function enforceAcpLazyChunk() {
  return {
    name: 'pinvou-enforce-acp-lazy-chunk',
    apply: 'build',
    generateBundle(_options, bundle) {
      const acpChunks = Object.values(bundle).filter(output => output.type === 'chunk'
        && Object.keys(output.modules).some(moduleId => moduleId.replaceAll('\\', '/')
          .endsWith('/features/codex/CodexAcpView.jsx')));
      if (acpChunks.length !== 1 || acpChunks[0].isEntry || acpChunks[0].name === 'main') {
        throw new Error('CodexAcpView must remain in one non-entry lazy chunk');
      }
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
  plugins: [react(), copyRuntimeAssets(), enforceAcpLazyChunk()],
  build: {
    outDir: webBuild ? '../../remote-control-relay/web/dist' : '../dist',
    emptyOutDir: true,
    // Minimum supported WebViews: macOS 11 WKWebView is Safari 14.0 — the
    // default "baseline-widely-available" target emits syntax it cannot parse
    // and older macOS builds render a blank window. Keep in sync with
    // .browserslistrc; scripts/audit-compat.mjs verifies the output.
    target: 'safari14',
    cssTarget: 'safari14',
    rolldownOptions: {
      input: webBuild
        ? { main: resolve(sourceRoot, 'index.html') }
        : {
            main: resolve(sourceRoot, 'index.html'),
            pet: resolve(sourceRoot, 'pet.html'),
            reader: resolve(sourceRoot, 'reader.html'),
          },
      output: webBuild
        ? {
            // Single-entry web build has no multi-entry sharing, so react-dom/
            // react/scheduler would stay inlined in main and trip the 500 kB
            // chunk warning. Extract them into a dedicated vendor chunk; the
            // multi-entry UI build already gets an equivalent separate shared
            // chunk from rolldown's automatic splitting, so this only applies
            // to the web mode.
            codeSplitting: {
              groups: [{
                name: 'vendor',
                test: /node_modules[\\/](react|react-dom|scheduler)[\\/]/,
              }],
            },
          }
        : undefined,
    },
  },
  };
});
