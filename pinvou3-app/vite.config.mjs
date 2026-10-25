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
// Exported for scripts/audit-compat.mjs: this is the Safari-14 *audit* set —
// the compatibility audit always audits these sources regardless of how the
// build ships them (verbatim copy or merged into a dist/startup bundle), so
// the minified-bundle layer never re-audits what the source layer already
// pinned. The set is keyed by script, but also carries one runtime-fetched
// non-script: platform/web/access-policy.json rides here so copyRuntimeAssets
// ships it (see runtimeFetchAssets below).
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
  'shared/theme-boot.js',
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

// Soft cap per generated startup bundle: a single minified source larger than
// the cap (e.g. platform/web/bridge.js) becomes its own oversize bundle
// instead of being split — a classic script cannot be divided across load
// boundaries. Exported with the grouping so the contract test pins the same
// constant the build packs with.
export const MAX_CLASSIC_BUNDLE_BYTES = 80_000;

// Order-preserving greedy packing: sources accumulate into the current bundle
// (joined with ';\n' so each original file starts on a fresh line) until the
// next source would push the bundle over the cap.
export function groupMinifiedSources(minifiedSources, maxBundleBytes = MAX_CLASSIC_BUNDLE_BYTES) {
  const grouped = [];
  let current = '';
  for (const code of minifiedSources) {
    const next = current ? `${current};\n${code}` : code;
    if (current && Buffer.byteLength(next) > maxBundleBytes) {
      grouped.push(current);
      current = code;
    } else {
      current = next;
    }
  }
  if (current) grouped.push(current);
  return grouped;
}

function bundleClassicStartup(webBuild) {
  const bundlePaths = classicStartupBundlePaths(webBuild);
  const bundlePrefix = `startup/pinvou-${webBuild ? 'web' : 'desktop'}-classic`;
  let bundles;
  return {
    name: 'pinvou-bundle-classic-startup',
    apply: 'build',
    async buildStart() {
      const minifiedSources = [];
      for (const relative of bundlePaths) {
        const source = readFileSync(resolveContainedRuntimePath(sourceRoot, relative), 'utf8');
        // Minify options stay empty on purpose: these are classic scripts, so
        // rolldown's toplevel mangle defaults to off for non-modules and the
        // cross-script globals the tags exchange (window.PinvouPlatform,
        // __PINVOU_STARTUP__, ...) survive. Passing `module: true` here would
        // silently mangle that contract away.
        const result = await minify(relative, source, {});
        if (result.errors.length > 0 || typeof result.code !== 'string') {
          throw new Error(`Could not minify ${relative}: ${result.errors.map(error => error.message).join('; ') || 'minifier returned no code'}`);
        }
        minifiedSources.push(result.code);
      }
      const groupedSources = groupMinifiedSources(minifiedSources);
      // Bundles intentionally ship without sourcemaps: startup failures
      // attribute through the stage-level __PINVOU_STARTUP__ marks and the
      // app:tauri_bridge_* marks on the last bundle, and sources are joined
      // with ';\n' so each original file starts on a fresh line for manual
      // inspection of the minified output.
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

function assertClassicRuntimeScriptsCopied(outputRoot, webBuild) {
  for (const relative of requiredVerbatimRuntimeScripts(webBuild)) {
    const source = resolveContainedRuntimePath(sourceRoot, relative);
    const target = resolveContainedRuntimePath(outputRoot, relative);
    if (!existsSync(source)) {
      throw new Error(`Vite build references a missing local classic runtime script: ${relative}`);
    }
    if (!existsSync(target)) {
      throw new Error(`Vite build is missing local classic runtime script: ${relative}`);
    }
  }
  for (const relative of verbatimDroppedRuntimeScripts(webBuild)) {
    if (existsSync(join(outputRoot, relative))) {
      throw new Error(`Vite build shipped a dead verbatim copy of ${relative}: its code is served from dist/startup or was stripped for this platform`);
    }
  }
}

// Any script listed here keeps its verbatim copy in both builds' dist even
// though index.html merges it into a startup bundle: pet.html still loads
// model-service-errors.js as its own standalone tag, so dropping the copy
// would break the pet entry.
const bundledButStandaloneElsewhere = new Set([
  'shared/model-service-errors.js',
]);

// Files that are not <script> tags at all but runtime-fetched assets: the
// desktop bridge fetches platform/web/access-policy.json relative to
// document.baseURI when WebAccess boots (platform/tauri/bridge/
// remote-control.js loadAccessPolicy), so its copy must exist on BOTH
// builds regardless of the platform stripping. bootstrap.js resolves it the
// same way on web.
const runtimeFetchAssets = new Set([
  'platform/web/access-policy.json',
]);

// Auxiliary HTML entries load some classic scripts as their own standalone
// tags (pet.html: legacy-polyfills + model-service-errors; reader.html:
// legacy-polyfills). Their needs derive from these entries directly — never
// from index.html tag survival — so removing a script's index.html tag cannot
// silently 404 an auxiliary entry, and a new auxiliary-entry tag is picked up
// without touching this file.
const auxiliaryEntryHtmls = ['pet.html', 'reader.html'];

// Scripts/files whose verbatim copy dist actually needs in this build:
//   - standalone tags the classic-bundle layer intentionally left unbundled
//     (fail-closed even if a tag disappears from index.html);
//   - scripts another entry (pet.html / reader.html) references directly;
//   - runtime-fetched assets (both builds).
export function requiredVerbatimRuntimeScripts(webBuild, indexHtml = readFileSync(join(sourceRoot, 'index.html'), 'utf8')) {
  const platformRetained = localClassicScriptPaths(indexHtml).filter((relative) =>
    webBuild ? !relative.startsWith('platform/tauri/') : !relative.startsWith('platform/web/'));
  const required = new Set(platformRetained.filter((relative) =>
    startupBundleExcludedScripts.has(relative) || bundledButStandaloneElsewhere.has(relative)));
  for (const entry of auxiliaryEntryHtmls) {
    const entryPath = join(sourceRoot, entry);
    if (!existsSync(entryPath)) continue;
    for (const relative of localClassicScriptPaths(readFileSync(entryPath, 'utf8'))) {
      if (webBuild ? relative.startsWith('platform/tauri/') : relative.startsWith('platform/web/')) continue;
      required.add(relative);
    }
  }
  for (const asset of runtimeFetchAssets) required.add(asset);
  return required;
}

function listSourceFilesUnder(prefix) {
  const dir = join(sourceRoot, prefix);
  const files = [];
  const visit = (entryDir) => {
    for (const entry of readdirSync(entryDir, { withFileTypes: true })) {
      if (entry.isDirectory()) visit(join(entryDir, entry.name));
      else files.push(prefix + entry.name);
    }
  };
  visit(dir);
  return files;
}

// Concrete source files that must NOT keep a verbatim copy in this build's
// dist. Anything index.html references through a classic startup tag has its
// code inside dist/startup, so a verbatim copy would ship the same source
// twice (~350 kB of dead bytes per build); the other platform's scripts are
// stripped from this build's index.html entirely, so their copies are equally
// dead. The required set always wins: a script another entry (or a runtime
// fetch) needs keeps its copy even when a startup bundle also serves its
// code. audit-compat keeps auditing all of these at the source layer, so
// dropping the copies does not weaken the Safari 14 audit.
export function verbatimDroppedRuntimeScripts(webBuild, indexHtml = readFileSync(join(sourceRoot, 'index.html'), 'utf8')) {
  const dropped = new Set(classicStartupBundlePaths(webBuild, indexHtml));
  for (const relative of requiredVerbatimRuntimeScripts(webBuild, indexHtml)) {
    dropped.delete(relative);
  }
  const otherPlatformPrefix = webBuild ? 'platform/tauri/' : 'platform/web/';
  for (const relative of staticRuntimeScripts) {
    if (relative.startsWith(otherPlatformPrefix)) dropped.add(relative);
  }
  for (const prefix of staticRuntimeScriptPrefixes) {
    if (!prefix.startsWith(otherPlatformPrefix)) continue;
    for (const file of listSourceFilesUnder(prefix)) dropped.add(file);
  }
  // Runtime-fetched assets are exempt from the platform strip: the desktop
  // bridge fetches access-policy.json at WebAccess boot regardless of whose
  // platform directory the file lives in.
  for (const asset of runtimeFetchAssets) dropped.delete(asset);
  return dropped;
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
  let webBuild;
  return {
    name: 'pinvou-copy-runtime-assets',
    apply: 'build',
    configResolved(config) {
      outputRoot = resolve(config.root, config.build.outDir);
      webBuild = config.mode === 'web';
    },
    closeBundle() {
      const dropped = verbatimDroppedRuntimeScripts(webBuild);
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
          // Dropped scripts would ship the same source twice: their index.html
          // tag resolves to a dist/startup bundle (or the other platform's
          // build, stripped here), so the verbatim copy is dead bytes.
          if (dropped.has(relative)) continue;
          const containedSource = resolveContainedRuntimePath(sourceRoot, relative);
          const target = resolveContainedRuntimePath(outputRoot, relative);
          mkdirSync(resolve(target, '..'), { recursive: true });
          cpSync(containedSource, target);
        }
      };
      if (existsSync(sourceRoot)) visit(sourceRoot);
      assertClassicRuntimeScriptsCopied(outputRoot, webBuild);
    },
  };
}

export const lazyChunkContracts = {
  settings: ['features/settings/SettingsView.jsx', 'SettingsView'],
  codex: ['features/codex/CodexAcpView.jsx', 'CodexAcpView'],
  cardpool: ['features/personas/Personas.jsx', 'Personas'],
  toolStore: ['features/tools/ToolStoreView.jsx', 'ToolStoreView'],
  scheduled: ['features/scheduled/ScheduledTasksView.jsx', 'ScheduledTasksView'],
  browser: ['features/browser/BrowserView.jsx', 'BrowserView'],
  knowledge: ['features/knowledge/KnowledgeView.jsx', 'KnowledgeView'],
  monitor: ['features/monitor/MonitorView.jsx', 'MonitorView'],
  search: ['features/search/SearchView.jsx', 'SearchView'],
  searchOverlay: ['features/search/SearchOverlay.jsx', 'SearchOverlay'],
  moveToProjectDialog: ['features/projects/MoveToProjectDialog.jsx', 'MoveToProjectDialog'],
  rebindFolderDialog: ['features/projects/RebindFolderDialog.jsx', 'RebindFolderDialog'],
  pinvouSummon: ['features/tools/PinvouSummonCard.jsx', 'PinvouSummonModal'],
  updateNotice: ['features/updater/UpdateNoticeButton.jsx', 'UpdateNoticeButton'],
  savedPersonaConfirmDialog: ['features/personas/SavedPersonaConfirmDialog.jsx', 'SavedPersonaConfirmDialog'],
  apiKeyGateDialog: ['features/settings/ApiKeyGateDialog.jsx', 'ApiKeyGateDialog'],
  archiveConfirmDialog: ['components/ArchiveConfirmDialog.jsx', 'ArchiveConfirmDialog'],
};

export const lazyToolChunkContracts = {
  scheduledTaskCard: ['features/tools/scheduled-task-card.jsx', 'ScheduledTaskToolCard'],
};

export function assertLazyChunks(bundle, contracts = { ...lazyChunkContracts, ...lazyToolChunkContracts }) {
  for (const [modulePath, label] of Object.values(contracts)) {
    const chunks = Object.values(bundle).filter(output => output.type === 'chunk'
      && Object.keys(output.modules).some(moduleId => moduleId.replaceAll('\\', '/')
        .endsWith(`/${modulePath}`)));
    if (chunks.length === 0) {
      throw new Error(`${label} lazy module was not emitted; its contract may be stale or the module was tree-shaken`);
    }
    if (chunks.length > 1) {
      throw new Error(`${label} must be emitted in exactly one chunk; found ${chunks.length}`);
    }
    if (chunks[0].isEntry || chunks[0].name === 'main') {
      throw new Error(`${label} must remain in one non-entry lazy chunk`);
    }
  }
}

function enforceLazyChunks() {
  return {
    name: 'pinvou-enforce-lazy-chunks',
    apply: 'build',
    generateBundle(_options, bundle) {
      assertLazyChunks(bundle);
    },
  };
}

// The enforced gate is assertMainEntryBudget below; it sits deliberately
// ABOVE Vite's informational 500 kB chunk warning for the web build (the
// warning currently fires on every web build and is expected — the gate,
// not the warning, is the budget). New entry work should still be split or
// lazy-loaded. Raising this limit requires measured desktop + web
// before/after sizes and an explicit review decision.
//
// 500_000 → 502_000 (round-21 B-1; decided in PR #627 and disclosed after
// the round-22 review found the raise missing from that PR's description):
// the session-messaging receive path adds ≈ +2 kB of web entry (same-base:
// main@007f5ea4 alone = 498.54 kB web; merged head = 500.69 kB web /
// 498.34 kB desktop on the current toolchain). The deferrable UI (sender
// card) and the send-side builder were already split out (round-18/20);
// the remainder is the synchronous receive-path parser, which cannot defer
// without breaking render semantics. An in-PR trim (builder split into the
// lazy graph) was measured net +122 B and rejected.
//
// Round-43 (this PR): the same ceiling also absorbs the node-version
// minification delta between local (v22) and CI (v24) toolchains — the
// identical tree measured 499.7 kB locally and 500.6 kB on CI; only the
// minifier output differs.
//
// 502_000 → 504_000 (this PR's rebase onto the #627+#714 main, measured per
// policy): main's own web entry is now 500.69 kB on the local v22 toolchain
// (1.3 kB under its own ceiling); this PR's feature net adds +2.10 kB
// (500.69 → 502.79 local; CI projects ≈503.7 at the stable +0.9 kB toolchain
// delta). The picker/grouping helpers are render-path synchronous and cannot
// defer; the ceiling is the measured projection, not padded.
//
// 504_000 → 505_000 (this PR's re-cut onto the #733 main, measured per
// policy): #733's Tachyon app-side wiring rides the shared entry; the
// joint main entry measures 504,082 B locally and identically on CI —
// 82 B over the 504_000 ceiling this PR last raised. The picker/grouping
// helpers are render-path synchronous and cannot defer; the ceiling is
// the measured projection, not padded.
//
// 505_000 → 506_000 (PR #628 follow-ups, measured per policy):
// The author explicitly approved 506_000 on 2026-10-21 after reviewing
// both targets. On this branch, npm ci with pinned Vite 8.3.4 / Node 22.23.1
// produces 505,454 B web and 502,135 B desktop. Review 5596933329 measured
// its combined tree with main@0baf7fa6c at 505,848 / 502,533 B; that is
// 152 B web headroom, not a reproduced toolchain-tolerance allowance.
// The card/parser remain lazy. Future combined trees must pass this gate;
// this cap does not promise headroom for unrelated main changes.
//
// 506_000 → 507_000 (PR #773, measured per policy): the swarm overlay's
// structured worker-status merge rides the startup graph; same-base build
// against main@f1ae1279a measures 505,844 → 506,175 B web (CI node 24:
// 506,192) and 502,529 → 502,862 B desktop, i.e. +331 B of synchronous
// event-path merge rules (token precedence, identity guard, ending-token
// set). The merge runs on the always-mounted overlay and cannot defer
// without breaking render semantics; the minified delta is all live logic
// (verified by chunk diff — no comments or dead code ride along). The next
// thousand step leaves 808 B web headroom at the CI measurement; this cap
// does not promise headroom for unrelated main changes.
//
// 2026-10-25 (lazy browser route + web vendor groups, measured): the browser
// view moved into the lazy route table, so the startup graph no longer
// carries the browser pane graph. The web build additionally groups ChatView
// and the marked/dompurify stack out of the entry file; both chunks still
// load at startup (static dependency), so the groups are entry-file layout
// and cache stability, not a startup reduction. highlight.js stays ungrouped
// because its 48 low-frequency fence languages are dynamic imports
// (syntax-highlighter.js) — grouping the package hoisted ~176 kB of them
// into the web startup graph, and the desktop chat-view group this was first
// tried with absorbed the pet/reader windows' shared modules (+0.5 MB per
// aux window); both shapes were measured and removed. Measured entry:
// desktop 484,660 B and web 127,605 B (previously 502,952 / 506,357 B).
// Measured startup closures: desktop main window 1,096,545 B and web
// 1,093,018 B (previously 1,114,837 / 1,114,426 B); pet/reader windows are
// byte-identical to base. The 507_000 ceiling stays as the desktop
// projection (22 kB headroom); any future raise or re-cut follows the
// measured-projection policy above.
export const MAIN_ENTRY_BUDGET_BYTES = 507_000;

export function assertMainEntryBudget(bundle, budgetBytes = MAIN_ENTRY_BUDGET_BYTES) {
  const mainEntry = Object.values(bundle).find(output => (
    output.type === 'chunk' && output.isEntry && output.name === 'main'
  ));
  if (!mainEntry) throw new Error('Vite build did not emit the main entry chunk');
  const bytes = Buffer.byteLength(mainEntry.code);
  if (bytes > budgetBytes) {
    throw new Error(`Main entry chunk ${bytes} B exceeds the ${budgetBytes} B startup budget`);
  }
  return bytes;
}

function enforceMainEntryBudget() {
  return {
    name: 'pinvou-enforce-main-entry-budget',
    apply: 'build',
    // Vite's build-import-analysis mutates chunks after user generateBundle
    // hooks. writeBundle sees the final code that is written to disk, so the
    // reported byte count matches the built asset exactly.
    writeBundle(_options, bundle) {
      assertMainEntryBudget(bundle);
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
    enforceLazyChunks(),
    enforceMainEntryBudget(),
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
            // multi-entry UI build gets an equivalent separate shared chunk
            // from rolldown's automatic splitting, so the vendor group only
            // applies to the web mode.
            codeSplitting: {
              groups: [
                {
                  name: 'vendor',
                  test: /node_modules[\\/](react|react-dom|scheduler)[\\/]/,
                },
                {
                  // marked and dompurify stay statically imported by the entry,
                  // so this group only moves them out of the entry file for
                  // cache stability, not out of the startup graph. highlight.js
                  // must stay ungrouped: its 48 low-frequency fence languages
                  // are dynamic imports (syntax-highlighter.js), and grouping
                  // the package merges them into this statically-preloaded
                  // chunk, hoisting ~176 kB into the web startup graph.
                  name: 'markdown-vendor',
                  test: /node_modules[\\/](marked|dompurify)[\\/]/,
                },
                {
                  // Keep ChatView statically imported so the desktop/web shell
                  // and source-contract tests retain the same startup semantics,
                  // while preventing the large component module from inflating
                  // the entry chunk beyond its per-chunk budget.
                  name: 'chat-view',
                  test: /[\\/]features[\\/]chat[\\/]ChatView\.jsx$/,
                },
              ],
            },
          }
        : undefined,
    },
  },
  };
});
