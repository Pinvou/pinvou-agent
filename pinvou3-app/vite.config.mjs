import { createHash } from 'node:crypto';
import { cpSync, existsSync, mkdirSync, readFileSync, readdirSync } from 'node:fs';
import { extname, join, resolve } from 'node:path';
import { defineConfig, minify } from 'vite';
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
  'shared/bridge-shared-helpers.js',
  'shared/chunked-file-upload.js',
  'shared/format-utils.js',
  'shared/legacy-polyfills.js',
  'shared/markdown-bridge-fallback.js',
  'shared/model-service-errors.js',
]);
export const staticRuntimeScriptPrefixes = ['platform/tauri/bridge/', 'platform/web/bridge/'];

// Desktop marker injected in place of platform/web/bootstrap.js. It replicates
// bootstrap.js's only desktop side effect (the `window.__TAURI__` branch at the
// top of that file) verbatim: kind/isWeb without a capabilities map, so
// shared/platform.js keeps merging in DEFAULT_DESKTOP_CAPABILITIES exactly as
// before. Kept ES2021-clean and attribute-free so audit-compat and CSP stay
// unaffected.
export const desktopPlatformMarkerScript = '<script>'
  + 'if (window.__TAURI__) { window.PinvouPlatform = Object.freeze({ kind: "desktop", isWeb: false }); }'
  + '</script>';

const scriptSrcLinePattern = /<script\b[^>]*?\ssrc=["']([^"']*)["'][^>]*>/u;

// Resolve the runtime-relative path of a single-line classic `<script src>`
// tag, or null for anything else (inline scripts, module entries, external
// URLs). Mirrors localClassicScriptPaths normalization so both layers agree.
function lineScriptRuntimePath(line) {
  const match = scriptSrcLinePattern.exec(line);
  if (!match) return null;
  const src = match[1].trim();
  if (/^(?:[a-z][a-z0-9+.-]*:|\/\/)/iu.test(src)) return null;
  const withoutBase = src.replace(/^%BASE_URL%/u, '').replace(/^\/+/u, '');
  const relative = withoutBase.split(/[?#]/u, 1)[0] || null;
  // Build/dev base rewriting may prefix the deployment base (the web relay
  // serves under e.g. /pinvou3/remote/), so the runtime-relative path starts
  // at the platform segment, not at the leading slash.
  const platformAt = relative.indexOf('/platform/');
  return platformAt >= 0 ? relative.slice(platformAt + 1) : relative;
}

// The desktop and web builds ship one shared index.html; every window of both
// products previously parsed BOTH platforms' bridge code (~0.5 MB per window
// on the losing side) and each bridge returned immediately through its
// platform guard. This pure transform keeps each build's own bridge tags in
// their original order and drops the other platform's:
//   - desktop (every mode except `web`, including the dev server): removes the
//     five `platform/web/` tags, replacing bootstrap.js in place with the
//     desktop marker above (its sole desktop side effect).
//   - web (`--mode web`): removes the `platform/tauri/` fragment tags and
//     platform/tauri/bridge.js. On web those scripts parse and no-op today:
//     the fragments only fill `window.__PINVOU_TAURI_BRIDGE_FEATURES__`, whose
//     sole reader (platform/tauri/bridge.js) returns before any side effect
//     when PinvouPlatform.kind === "web" (set earlier by the retained
//     platform/web/bootstrap.js).
// Build-time `%BASE_URL%`/base rewriting runs before this hook in both the dev
// and build HTML pipelines, and the path normalization above accepts both the
// raw placeholder and a rewritten base. Tag order of everything retained is
// untouched, so execution-order semantics are preserved on both sides.
export function transformIndexHtmlForPlatform(webBuild, html) {
  return html.split('\n').map((line) => {
    const relative = lineScriptRuntimePath(line);
    if (!relative) return line;
    if (webBuild) {
      return relative.startsWith('platform/tauri/') ? '' : line;
    }
    if (!relative.startsWith('platform/web/')) return line;
    if (relative === 'platform/web/bootstrap.js') return desktopPlatformMarkerScript;
    return '';
  }).join('\n');
}

function conditionalPlatformScripts(webBuild) {
  return {
    name: 'pinvou-conditional-platform-scripts',
    transformIndexHtml(html) {
      return transformIndexHtmlForPlatform(webBuild, html);
    },
  };
}

const startupBundleExcludedScripts = new Set([
  // legacy-polyfills.js must precede every bundled script on each entry, and
  // pet.html/reader.html also load it standalone.
  'shared/legacy-polyfills.js',
  // The Web bootstrap resolves its policy and WebSocket endpoints relative to
  // document.currentScript. It must retain its own URL instead of observing a
  // generated bundle URL.
  'platform/web/bootstrap.js',
  // personas-i18n.js has no static tag — it is injected at runtime by the
  // inline loader in index.html, which the manifest parser skips. Listed so
  // the never-bundle contract stays explicit if it ever gains a static tag.
  'features/personas/personas-i18n.js',
  // update-notice-logic.js carries its own onload/onerror lifecycle marks.
  'features/updater/update-notice-logic.js',
]);
// Static classic tags that intentionally stay unbundled: desktop loads the
// polyfill and update-notice-logic, web additionally keeps bootstrap.js. The
// retained-set contract test pins the exact list per platform.
const unbundledStartupScriptCount = webBuild => webBuild ? 3 : 2;

export function classicStartupBundlePaths(webBuild, indexHtml = readFileSync(join(sourceRoot, 'index.html'), 'utf8')) {
  return localClassicScriptPaths(indexHtml).filter((relative) => {
    if (startupBundleExcludedScripts.has(relative)) return false;
    if (webBuild) return !relative.startsWith('platform/tauri/');
    return !relative.startsWith('platform/web/');
  });
}

export function transformIndexHtmlForClassicBundle(webBuild, html, bundlePaths, bundleFiles) {
  const remaining = new Set(bundlePaths);
  let inserted = false;
  const transformed = html.split('\n').map((line) => {
    const match = scriptSrcLinePattern.exec(line);
    if (!match) return line;
    const source = match[1].split(/[?#]/u, 1)[0];
    const relative = bundlePaths.find(candidate => source === candidate
      || source === `%BASE_URL%${candidate}`
      || source.endsWith(`/${candidate}`));
    if (!relative) return line;
    remaining.delete(relative);
    if (inserted) return '';
    inserted = true;
    const prefix = source.slice(0, -relative.length);
    const indentation = line.match(/^\s*/u)[0];
    return bundleFiles.map((bundleFile, index) => {
      const lifecycle = !webBuild && index === bundleFiles.length - 1
        ? ' onload="__PINVOU_STARTUP__.mark(\'app:tauri_bridge_loaded\')" onerror="__PINVOU_STARTUP__.mark(\'app:tauri_bridge_error\')"'
        : '';
      return `${indentation}<script src="${prefix}${bundleFile}"${lifecycle}></script>`;
    }).join('\n');
  }).join('\n');
  if (!inserted || remaining.size > 0) {
    throw new Error(`Classic startup bundle paths missing from transformed index: ${[...remaining].join(', ')}`);
  }
  return transformed;
}

function bundleClassicStartup(webBuild) {
  const bundlePaths = classicStartupBundlePaths(webBuild);
  const bundlePrefix = `startup/pinvou-${webBuild ? 'web' : 'desktop'}-classic`;
  // Soft per-request cap: a single minified source larger than the cap (e.g.
  // platform/web/bridge.js) becomes its own oversize bundle instead of being
  // split — a classic script cannot be divided across load boundaries.
  const maxBundleBytes = 80_000;
  let bundles;
  return {
    name: 'pinvou-bundle-classic-startup',
    apply: 'build',
    async buildStart() {
      const minifiedSources = [];
      for (const relative of bundlePaths) {
        const source = readFileSync(resolveContainedRuntimePath(sourceRoot, relative), 'utf8');
        const result = await minify(relative, source, {});
        if (result.errors.length > 0 || typeof result.code !== 'string') {
          throw new Error(`Could not minify ${relative}: ${result.errors.map(error => error.message).join('; ') || 'minifier returned no code'}`);
        }
        minifiedSources.push(result.code);
      }
      const groupedSources = [];
      let current = '';
      for (const code of minifiedSources) {
        const next = current ? `${current};\n${code}` : code;
        if (current && Buffer.byteLength(next) > maxBundleBytes) {
          groupedSources.push(current);
          current = code;
        } else {
          current = next;
        }
      }
      if (current) groupedSources.push(current);
      bundles = groupedSources.map((source, index) => ({
        source,
        fileName: `${bundlePrefix}-${index + 1}-${createHash('sha256').update(source).digest('hex').slice(0, 8)}.js`,
      }));
      const startupScriptCount = bundles.length + unbundledStartupScriptCount(webBuild);
      if (startupScriptCount > 10) {
        throw new Error(`Classic startup script budget exceeded: ${startupScriptCount} > 10`);
      }
    },
    transformIndexHtml(html, context) {
      if (!context.path.endsWith('/index.html')) return html;
      const bundleFiles = bundles.map(bundle => bundle.fileName);
      return transformIndexHtmlForClassicBundle(webBuild, html, bundlePaths, bundleFiles);
    },
    generateBundle() {
      bundles.forEach(({ fileName, source }) => {
        this.emitFile({ type: 'asset', fileName, source });
      });
    },
  };
}

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
  plugins: [
    react(),
    copyRuntimeAssets(),
    enforceAcpLazyChunk(),
    conditionalPlatformScripts(webBuild),
    bundleClassicStartup(webBuild),
  ],
  build: {
    outDir: webBuild ? '../../remote-control-relay/web/dist' : '../dist',
    emptyOutDir: true,
    // Minimum supported WebViews: macOS 11 WKWebView is Safari 14.0 — the
    // default "baseline-widely-available" target emits syntax it cannot parse
    // and older macOS builds render a blank window. cssTarget matters beyond
    // minification: Tailwind emits the inset shorthand (Safari 14.1+) and
    // only target-aware lightningcss expands it to physical properties; the
    // dist CSS scan in scripts/audit-compat.mjs pins this. Keep in sync with
    // .browserslistrc; scripts/audit-compat.mjs audits the built JS chunks
    // and CSS assets.
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
