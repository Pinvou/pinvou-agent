// Regression pins for the 2026-10 full-app visual e2e sweep fixes.
//
// These run Chrome-free under `node --test` (every CI leg, every platform), so
// they must not depend on puppeteer or a browser binary. Where the repo already
// exposes a real artefact (the three-language dictionary, the icon bytes, the
// theme storage key constant) the pin checks that artefact; the remaining pins
// are source anchors over comment-stripped text, because the behaviours they
// protect (paint-containment clipping, cascade order, first-paint colour) are
// only observable in a real browser and are exercised by the sweep harness.
import assert from 'node:assert/strict';
import { readFileSync, readdirSync, existsSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';
import { dict } from './helpers/i18n-all.js';
import { COLOR_SCHEME_STORAGE_KEY } from '../src/shared/color-scheme.js';

const srcRoot = fileURLToPath(new URL('../src/', import.meta.url));
const sourceUrl = relative => new URL(`../src/${relative}`, import.meta.url);
const source = relative => readFileSync(sourceUrl(relative), 'utf8');
// Positive anchors run against comment-stripped text so a guard that was
// commented out cannot keep its pin green; negative anchors run against the
// RAW text so a mention inside a comment or string is also caught (same split
// as ui_language_coverage.test.mjs).
const stripComments = text => text
  .replace(/\/\*[\s\S]*?\*\//g, comment => comment.replace(/[^\n]/g, ''))
  .split('\n')
  // A `//` inside a string literal would be stripped too; keep `//` out of
  // any line an anchor below reads (none of the current anchor lines have one).
  .map(line => line.replace(/\/\/.*$/, ''))
  .join('\n');
const count = (text, re) => (text.match(re) || []).length;

test('chat markdown: long tokens wrap with a Safari-14 fallback and tables keep their scroll box', () => {
  const baseCss = stripComments(source('styles/base.css'));
  // break-word must precede anywhere: `anywhere` is Safari 15.4+, and alone it
  // shrinks table-cell min-content until auto-layout tables stop overflowing
  // into the block-table scroll box.
  assert.match(
    baseCss,
    /\.msg-md,\.persona-body,\.codex-markdown \{ overflow-wrap:break-word; overflow-wrap:anywhere; \}/,
    'markdown bodies must wrap long tokens with break-word declared before anywhere',
  );
  assert.match(
    baseCss,
    /\.msg-md th,\.msg-md td,\.codex-markdown th,\.codex-markdown td,\.persona-body th,\.persona-body td \{ overflow-wrap:normal; \}/,
    'table cells must opt back out of anywhere so wide tables scroll instead of collapsing',
  );
  // The persona modal body renders LLM markdown too: it shares the msg-md
  // block-table recipe through one selector list instead of a byte-identical
  // copy, and the codex timeline table carries width:max-content like the
  // other two so the recipes stay uniform.
  assert.match(
    baseCss,
    /\.msg-md table,\.persona-body table \{[^}]*display:block; width:max-content; max-width:100%; overflow-x:auto; \}/,
    'persona-body tables must keep the shared block-table scroll box',
  );
  assert.doesNotMatch(baseCss, /(?<!,)\.persona-body table \{/,
    'the persona table recipe must stay merged into the shared selector list');
  assert.match(baseCss, /\.codex-markdown table \{[^}]*width:max-content;/,
    'the codex timeline table must carry the same width:max-content recipe');
  // The pet window renders marked markdown under overflow-wrap:anywhere with
  // no cell opt-out, so a wide table would squeeze per-character there.
  const petCss = source('features/pet/pet.css');
  assert.match(petCss, /\.pet-activity-body table \{[^}]*width:\s*max-content;\s*max-width:\s*100%;\s*overflow-x:\s*auto;/,
    'pet tables must scroll in their own block-table box');
  assert.match(petCss, /\.pet-activity-body th,\s*\.pet-activity-body td \{\s*overflow-wrap:\s*normal;/,
    'pet table cells must opt out of the inherited overflow-wrap:anywhere');
});

test('knowledge dialogs fit narrow panes', () => {
  const knowledgeView = stripComments(source('features/knowledge/KnowledgeView.jsx'));
  assert.match(knowledgeView, /bg-black\/40 p-4" onClick=\{onClose\}/,
    'KbDialog overlay must pad its contents away from the pane edge');
  assert.match(knowledgeView, /max-w-full rounded-2xl p-6 bg-white dark:bg-\[#1E1F20\]/,
    'KbDialog card must never exceed its overlay width');
});

test('personas banner avatars hide below xl through utilities, not a cascade hack', () => {
  const personas = source('features/personas/Personas.jsx');
  assert.match(stripComments(personas), /hidden xl:flex items-center justify-evenly/,
    'avatar row must be hidden below xl (1280px) with responsive utilities');
  assert.doesNotMatch(personas, /bnr-avatars/, 'the dead bnr-avatars hook must stay removed');
  assert.doesNotMatch(source('styles/base.css'), /bnr-avatars/,
    'base.css must not reintroduce a media-query hide rule that loses to the flex utility');
});

test('reader window paints its own dark canvas before first paint and keeps it on mount', () => {
  const readerHtml = source('reader.html');
  const tag = readerHtml.match(/<script src="\/shared\/theme-boot\.js" data-dark-bg="(#[0-9A-Fa-f]{6})"><\/script>/);
  assert.ok(tag, 'reader.html must load the shared theme-boot script with a data-dark-bg colour');
  const darkBg = tag[1];
  const readerApp = stripComments(source('features/reader/ReaderApp.jsx'));
  assert.match(readerApp, new RegExp(`bg-white dark:bg-\\[${darkBg}\\]`),
    'the pre-paint canvas colour must equal the reader root dark background (no shade pop at mount)');
  // Every React root that reconciles the pre-paint class must seed from the
  // stored scheme (shared readStoredColorScheme); the detached shell loads
  // index.html, so its root strips the class exactly like the reader's did.
  for (const seed of ['app/main.jsx', 'features/reader/ReaderApp.jsx', 'app/DetachedShell.jsx']) {
    assert.match(stripComments(source(seed)), /useState\(readStoredColorScheme\)/,
      `${seed} must seed its colour scheme from the stored value or it strips the pre-paint class on mount`);
  }
  const colorSchemeModule = stripComments(source('shared/color-scheme.js'));
  assert.match(colorSchemeModule, /normalizeColorScheme\(window\.localStorage\.getItem\(COLOR_SCHEME_STORAGE_KEY\)\)/,
    'readStoredColorScheme must apply the same normalize rule the pre-paint scripts implement');

  const themeBoot = stripComments(source('shared/theme-boot.js'));
  assert.match(themeBoot, new RegExp(`getItem\\('${COLOR_SCHEME_STORAGE_KEY.replaceAll('.', '\\.')}'\\)`),
    'theme-boot must read the same storage key as color-scheme.js');
  assert.match(themeBoot, /dataset\.darkBg/, 'theme-boot must take the canvas colour from its script tag');
  assert.match(themeBoot, /classList\.add\('dark'\)/, 'theme-boot must set the dark class pre-paint');
  // index.html keeps an inline copy on the startup critical path; it must stay
  // on the same key, the same stored-value gate, and the same OS fallback, or
  // the two boots drift apart (a loosened gate boots a corrupted store as
  // light; an inverted media query boots an OS-dark machine light).
  const indexBoot = stripComments(source('index.html'));
  assert.match(indexBoot, new RegExp(`getItem\\('${COLOR_SCHEME_STORAGE_KEY.replaceAll('.', '\\.')}'\\)`),
    'index.html inline boot must read the same storage key color-scheme.js exports');
  assert.match(indexBoot, /stored === 'light' \|\| stored === 'dark'/,
    'index.html must keep the strict stored-value gate');
  assert.match(indexBoot, /prefers-color-scheme: dark/,
    'index.html must fall back to the OS dark preference');
  assert.match(indexBoot, /html\.dark \{ background: #131314; \}/,
    'index.html canvas must equal the theme-boot.js default dark background');
  // The recipe is more than key + colour: the inline boot must still apply
  // the class, or a dark cold boot paints a white first frame again.
  assert.match(indexBoot, /if \(dark\) document\.documentElement\.classList\.add\('dark'\)/,
    'index.html inline boot must still apply the dark class pre-paint');
  assert.match(themeBoot, /\|\| '#131314'/, 'theme-boot default must stay equal to the index.html dark canvas');
  assert.match(themeBoot, /stored === 'light' \|\| stored === 'dark'/,
    'theme-boot must keep the same strict stored-value gate as the inline boot');
  assert.match(themeBoot, /prefers-color-scheme: dark/,
    'theme-boot must fall back to the OS dark preference like the inline boot');
});

test('web-access primaries and the refresh confirm carry the shared light + dark pairing', () => {
  const settingsView = stripComments(source('features/settings/SettingsView.jsx'));
  assert.match(settingsView, /bg-\[#0B57D0\] text-white hover:bg-\[#0A4BB8\] disabled:opacity-60 dark:bg-\[#A8C7FA\] dark:text-\[#062E6F\] dark:hover:bg-\[#C2DBFF\]/,
    'refresh confirm button must carry light + dark primary styling');
  assert.ok(count(settingsView, /bg-\[#0B57D0\] text-white hover:bg-\[#0A4BB8\] disabled:opacity-50 dark:bg-\[#A8C7FA\] dark:text-\[#062E6F\] dark:hover:bg-\[#C2DBFF\]/g) >= 2,
    'the allow-workspace and enable primaries in the same panel must carry the dark pairing too');
});

test('dark-capable hint text and toggle tracks have dark variants', () => {
  const remoteKnowledge = stripComments(source('features/remote-knowledge/RemoteKnowledgeView.jsx'));
  assert.match(remoteKnowledge, /bg-\[#E5E5EA\] dark:bg-\[#39393D\] transition-colors/,
    'remote-KB toggle track must follow the shared Toggle light/dark track colours');
  assert.match(remoteKnowledge, /rounded-xl bg-\[#F7F9FC\] px-3\.5 py-4 text-\[12px\] dark:bg-white\/\[0\.04\]/,
    'the empty-state chip must get the same dark tint as its sibling chips');
  // The two hero calls-to-action share one const carrying the canonical
  // card-buttons hover pair (#0A4BB8 / #C2DBFF) instead of two pasted strings.
  assert.match(remoteKnowledge, /const heroPrimary = '[^']*hover:bg-\[#0A4BB8\][^']*dark:hover:bg-\[#C2DBFF\]'/,
    'the hero primary const must carry the canonical light + dark hover pair');
  assert.ok(count(remoteKnowledge, /className=\{heroPrimary\}/g) >= 2,
    'both hero calls-to-action must use the shared heroPrimary const');
  assert.equal(count(source('features/remote-knowledge/RemoteKnowledgeView.jsx'), /(?<![-\w])hover:bg-\[#0A4BB8\](?![^"'`]*dark:hover:bg-\[#C2DBFF\])/g), 0,
    'every canonical light primary hover must carry its dark half in the same string');
  // The file pairs every error/success text with its dark half (#ff8a80 /
  // #7DD3A8); a bare light-only instance re-opens the unreadable-in-dark
  // class. The lookahead stays inside the className string (no quote may be
  // crossed) and tolerates dark:bg tokens sitting between the pair. The
  // negatives run on RAW source: a legal string containing /* makes the
  // comment stripper swallow whole regions, and a regression inside it must
  // not go unpinned (same policy as ui_language_coverage round-26 M9).
  const remoteKnowledgeRaw = source('features/remote-knowledge/RemoteKnowledgeView.jsx');
  assert.equal(count(remoteKnowledgeRaw, /text-\[#d63a3a\](?![^"'`]*dark:text-\[#ff8a80\])/g), 0,
    'every error-red text in remote knowledge must carry its dark pair');
  assert.equal(count(remoteKnowledgeRaw, /text-\[#b72f2f\](?![^"'`]*dark:text-\[#ff8a80\])/g), 0,
    'every deep error-red text in remote knowledge must carry its dark pair');
  assert.equal(count(remoteKnowledgeRaw, /text-\[#16894a\](?![^"'`]*dark:text-\[#7DD3A8\])/g), 0,
    'every success-green text in remote knowledge must carry its dark pair');
  // The light success green was deepened to #0F7A40 (the old #16894a sat at
  // 3.95–4.46:1 on every light surface); the pair pin above only checks the
  // dark half, so the deepening needs its own guard.
  assert.doesNotMatch(remoteKnowledgeRaw, /16894a/i, 'the light success green must stay deepened to #0F7A40');
  // Warning amber (#a76518) sat at 4.20:1 on its own #fff2dd chip; the file
  // now carries the deeper #96600F everywhere the pair appears.
  assert.doesNotMatch(remoteKnowledgeRaw, /a76518/i, 'the light warning amber must stay deepened to #96600F');
  assert.match(remoteKnowledge, /bg-\[#fff2dd\] text-\[#96600F\] dark:bg-\[#412d13\] dark:text-\[#eab66f\]/,
    'the not-ready status chip must use the deepened amber ink');
  // Every error text on a red tint (restored /[0.08] or the on-scale /10) must
  // use the deep red: #d63a3a composited over its own tint lands at 4.0–4.2:1.
  const TINTED_RED_WITHOUT_DEEP_INK = /bg-\[#d63a3a\]\/(?:\[0\.08\]|10)(?![^"'`]*text-\[#b72f2f\])/g;
  assert.equal(count(remoteKnowledgeRaw, TINTED_RED_WITHOUT_DEEP_INK), 0,
    'every red-tinted banner in remote knowledge must carry the deep-red ink');
  assert.equal(count(source('features/knowledge/KnowledgeView.jsx'), TINTED_RED_WITHOUT_DEEP_INK), 0,
    'every red-tinted banner in local knowledge must carry the deep-red ink');
  // Row-hover inks darken on the hovered row; each must carry the dark
  // restore of the SAME variant chain, or dark mode's hover state degrades
  // (a plain dark half loses the cascade to the variant rule).
  assert.equal(count(remoteKnowledgeRaw, /group-hover:text-\[#(?:b72f2f|d63a3a)\](?![^"'`]*dark:group-hover:text-\[#ff8a80\])/g), 0,
    'every group-hover red darkening must restore its dark group-hover ink');
  assert.equal(count(remoteKnowledgeRaw, /(?<![-\w])hover:text-\[#(?:b72f2f|d63a3a)\](?![^"'`]*dark:hover:text-\[#ff8a80\])/g), 0,
    'every hover red darkening must restore its dark hover ink');
  // The group-hover inks above only fire when the row itself is the `group`;
  // dropping that class silently kills every row-hover darkening.
  assert.match(remoteKnowledgeRaw, /data-testid="remote-document-row" className="group /,
    'the document row must stay the group its group-hover inks depend on');
  // The local knowledge view pairs its error reds with the same dark half as
  // the remote one; a bare light-only instance re-opens the class there too.
  assert.equal(count(source('features/knowledge/KnowledgeView.jsx'), /text-\[#d63a3a\](?![^"'`]*dark:text-\[#ff8a80\])/g), 0,
    'every error-red text in local knowledge must carry its dark pair');
  const acpNotices = stripComments(source('features/codex/AcpRuntimeNotices.jsx'));
  const acpNoticesRaw = source('features/codex/AcpRuntimeNotices.jsx');
  // Both pair directions are zero-tolerance: a gray-500 without a dark
  // gray-400 half anywhere later in its className string, and any gray-400
  // that is not the dark half of an exact pair (bare, reordered, or riding
  // its own variant chain like dark:hover:) — a count pin would let two
  // hints lose their half and stay green.
  assert.equal(count(acpNoticesRaw, /text-gray-500(?![^"'`]*dark:text-gray-400)/g), 0,
    'every gray-500 hint in the runtime notices must carry its dark gray-400 half');
  const withoutPairs = acpNoticesRaw.replaceAll('text-gray-500 dark:text-gray-400', '');
  assert.equal(count(withoutPairs, /text-gray-400/g), 0,
    'every gray-400 in the runtime notices must be the dark half of a gray-500 pair');
  assert.ok(count(acpNotices, /text-gray-500 dark:text-gray-400/g) >= 8,
    'runtime-notice hints must raise contrast in dark mode');
  // Action buttons and detail texts carry their AA-passing recipes: dark ink
  // on the amber fill (white was 2.15:1), red-600 under white (red-500 was
  // 3.76:1), and the file's red-600 + red-300 pair on error details. The
  // tinted error banner darkens its light leg to red-700: red-600 sat at
  // 4.35:1 on the restored red-500/[0.08] tint, below AA.
  assert.match(acpNotices, /bg-amber-500 text-amber-950 text-\[12px\]/,
    'the authorize button must use dark ink on the amber fill');
  // Zero-tolerance form of the pin above: a string literal elsewhere in the
  // file containing the pinned text can satisfy a positive, but any unpaired
  // amber fill fails this.
  assert.equal(count(acpNoticesRaw, /bg-amber-500(?!\/)(?![^"'`]*text-amber-950)/g), 0,
    'every solid amber-500 fill must pair the dark amber-950 ink (the /[0.0N] tints are backgrounds, not fills)');
  assert.match(acpNotices, /rounded-xl bg-red-600 px-3 py-1\.5/,
    'the account switch must use red-600 under white text');
  assert.match(acpNotices, /bg-red-500\/\[0\.08\] text-red-700 dark:text-red-300/,
    'the tinted error banner must darken its light leg to red-700');
  assert.equal(count(acpNoticesRaw, /bg-red-500\/\[0\.08\](?![^"'`]*text-red-700)/g), 0,
    'every red-500 tint must pair the red-700 light leg');
  assert.ok(count(acpNotices, /text-red-600 dark:text-red-300/g) >= 4,
    'error details must use the file red-600 + red-300 pair');
  assert.match(remoteKnowledge, /bg-\[#d63a3a\]\/\[0\.08\] px-3 py-2 text-\[12px\] text-\[#b72f2f\]/,
    'the tinted connection-error banner must use the file deep-red ink (d63a3a sat at 4.14:1 on its own tint)');
});

test('tool store: hint, dialogs, labels, badge, icon', () => {
  const raw = source('features/tools/ToolStoreView.jsx');
  const toolStore = stripComments(raw);
  assert.match(toolStore, /hidden xl:flex min-w-0 items-center gap-1\.5/,
    'drag hint must hide when the chip row cannot fit it');
  assert.match(toolStore, /bg-black\/40 p-4 backdrop-blur-sm/, 'ModalBackdrop must pad its cards away from the viewport edge');
  assert.match(toolStore, /z-\[210\] flex items-center justify-center bg-black\/40 p-4 backdrop-blur-sm/,
    'the TsAlert backdrop must pad its card away from the viewport edge too');
  assert.match(toolStore, /w-\[280px\] max-w-full rounded-\[20px\]/, 'the TsAlert card must carry the same max-width guard');
  assert.ok(count(toolStore, /w-\[300px\] max-w-full rounded-\[20px\]/g) >= 4,
    'the 300px dialog cards must carry max-w-full');
  // String-bound (not adjacency-bound): an unguarded card written
  // w-[300px] shadow-2xl rounded-[20px] must fail here too. Raw source, so
  // no comment mention can satisfy it either.
  assert.equal(count(raw, /w-\[300px\](?![^"'`]*max-w-full)/g), 0,
    'no 300px dialog card may lose its max-width guard');
  assert.ok(count(toolStore, /text-slate-500 dark:text-slate-400 font-semibold uppercase tracking-wider/g) >= 3,
    'detail labels must have a dark variant');
  // The sub-labels under those headers were light-only slate-400 (2.56:1 on
  // white); inside the detail header block slate-400 may only be a dark half.
  const detailHeader = raw.slice(raw.indexOf('storeCopy.firstUseOnlineInstall'), raw.indexOf('storeCopy.detailGlobalAccel'));
  assert.ok(detailHeader.length > 0, 'detail header block must be locatable');
  assert.equal(count(detailHeader, /(?<!dark:)text-slate-400/g), 0,
    'detail header labels and sub-labels must pair slate-500 with a dark slate-400 half');
  // Per-item badges read the singular typeLabels dictionary; typeGroups is the
  // plural chip/section-header dictionary (en "Skills" on a single card was a
  // copy regression).
  assert.match(toolStore, /\(storeCopy\.typeLabels \|\| \{\}\)\[item\.kind\]/, 'recycle badge must use the singular type labels');
  assert.equal(count(raw, /\(storeCopy\.typeGroups \|\| \{\}\)\.(?:bundle|skill|mcp)\b/g), 0,
    'card type badges must not read the plural group dictionary');
  assert.ok(count(toolStore, /\(storeCopy\.typeLabels \|\| \{\}\)\.(?:bundle|skill|mcp)\b/g) >= 4,
    'the uploaded-skill, companion and MCP card badges must read the singular type labels');
  assert.match(toolStore, /\(storeCopy\.catalogTypes \|\| \{\}\)\[tool\.type\] \|\| tool\.type/,
    'preset card badges must resolve through the localized catalogue types');
  assert.match(toolStore, /\(storeCopy\.catalogTypes \|\| \{\}\)\[selectedTool\.type\]/,
    'the detail header must resolve the catalogue type too');
  // Raw (not comment-stripped): a ternary leg like `: 'Skill'` must fail too,
  // not only a whole-property `type: 'Skill'` literal.
  assert.doesNotMatch(raw, /:\s*'Skill'/, 'card type labels must resolve through the localized dictionaries');
  assert.doesNotMatch(raw, /:\s*'MCP Server'/, 'the MCP card badge must resolve through the localized dictionary too');
  assert.match(toolStore, /wb-qq-mail\.jpg/, 'QQ-mail icon asset must be referenced by its real jpeg extension');

  const jpg = sourceUrl('assets/tool-icons/wb-qq-mail.jpg');
  assert.ok(existsSync(jpg), 'wb-qq-mail.jpg must exist');
  assert.ok(!existsSync(sourceUrl('assets/tool-icons/wb-qq-mail.png')), 'the mislabelled .png must stay removed');
  const magic = readFileSync(jpg).subarray(0, 3);
  assert.deepEqual([...magic], [0xff, 0xd8, 0xff], 'wb-qq-mail.jpg must really be JPEG data');
});

test('community group name and type badge labels exist in all three languages', () => {
  for (const language of ['zh', 'en', 'ja']) {
    assert.ok(typeof dict[language].communityGroupName === 'string' && dict[language].communityGroupName.trim(),
      `${language}.communityGroupName must exist`);
    const kinds = dict[language].uiToolStore.typeLabels;
    // recycle_bin.rs only ever records mcp / skill / bundle packages; the store
    // cards read the same three keys.
    for (const kind of ['bundle', 'mcp', 'skill']) {
      assert.ok(kinds && typeof kinds[kind] === 'string' && kinds[kind].trim(), `${language}.uiToolStore.typeLabels.${kind} must exist`);
    }
  }
  // dictJa spreads dictEn: a ja-missing key silently falls back to English.
  assert.notEqual(dict.ja.communityGroupName, dict.en.communityGroupName, 'ja community group name must not fall back to English');
  for (const kind of ['bundle', 'skill']) {
    assert.notEqual(dict.ja.uiToolStore.typeLabels[kind], dict.en.uiToolStore.typeLabels[kind],
      `ja typeLabels.${kind} must not fall back to English`);
  }
  // ja and zh have no singular/plural split, so their typeLabels must stay
  // value-identical to typeGroups — a divergent edit to one dictionary would
  // silently fork the badge copy.
  for (const kind of Object.keys(dict.ja.uiToolStore.typeLabels)) {
    assert.equal(dict.ja.uiToolStore.typeLabels[kind], dict.ja.uiToolStore.typeGroups[kind],
      `ja typeLabels.${kind} must stay in sync with typeGroups`);
    assert.equal(dict.zh.uiToolStore.typeLabels[kind], dict.zh.uiToolStore.typeGroups[kind],
      `zh typeLabels.${kind} must stay in sync with typeGroups`);
  }
  // The static preset cards render their type through catalogTypes; every
  // interface-type literal in the data file must be a key in all three
  // dictionaries. ('text' is a content-block discriminator, not an interface
  // type; the CLI + … connectors are localized per backend id through the
  // uiToolDetails overlay instead.)
  const catalogueLiterals = [...new Set([...source('features/tools/tool-common.jsx').matchAll(/type: '([^']+)'/g)].map(m => m[1]))]
    .filter(value => value !== 'text' && !value.startsWith('CLI +'));
  assert.ok(catalogueLiterals.length >= 7, 'preset catalogue type literals must be discoverable in tool-common.jsx');
  for (const language of ['zh', 'en', 'ja']) {
    const catalog = dict[language].uiToolStore.catalogTypes;
    for (const literal of catalogueLiterals) {
      assert.ok(catalog && typeof catalog[literal] === 'string' && catalog[literal].trim(),
        `${language}.uiToolStore.catalogTypes must carry the preset descriptor '${literal}'`);
    }
  }
  assert.equal(dict.en.uiToolStore.typeLabels.skill, 'Skill', 'en per-item badge labels must be singular');
  assert.equal(dict.en.uiToolStore.typeLabels.bundle, 'Plugin bundle', 'en per-item badge labels must be singular');

  const communityPanel = source('features/settings/CommunityPanel.jsx');
  assert.match(stripComments(communityPanel), /copy\.communityGroupName/, 'the community panel must render the localized group name');
  assert.doesNotMatch(communityPanel, /COMMUNITY_QQ_GROUP_NAME/, 'the raw Chinese constant must not come back');
  assert.doesNotMatch(source('features/settings/community-config.js'), /COMMUNITY_QQ_GROUP_NAME/, 'community-config must not export the name');
});

test('inks on the restored tints clear AA where the tint re-composited them below it', () => {
  // Restoring a dead tint puts text on a darker surface; these legs landed
  // below 4.5:1 on the real ancestor chain and were deepened.
  const artifacts = stripComments(source('features/artifacts/ArtifactsPanel.jsx'));
  assert.match(artifacts, /text-\[#636366\] \$\{active \? 'dark:text-\[#D1D1D6\]' : 'dark:text-\[#A1A1AA\]'\}/,
    'the active switcher row must lift its meta ink in dark (#A1A1AA was 4.16:1 on the restored row tint)');
  // The 11px menu text inks sat on the iOS secondary gray in light (2.83:1 on
  // the active row, 3.23:1 elsewhere); the deepened #636366 clears AA on every
  // light surface in the menu. The 10px ▼ glyph is a state indicator held to
  // the 3:1 non-text bar and keeps the lighter gray.
  const artifactsRaw = source('features/artifacts/ArtifactsPanel.jsx');
  assert.equal(count(artifactsRaw, /text-\[11px\][^"'`]*text-\[#8E8E93\]/g), 0,
    '11px menu text must not use the 3.2:1 iOS gray in light');
  assert.ok(count(artifacts, /text-\[#636366\](?=[^"'`]*dark:text-\[#A1A1AA\])/g) >= 2,
    'the menu header and footer must carry the deepened light gray');
  const scheduled = stripComments(source('features/scheduled/ScheduledTasksView.jsx'));
  assert.match(scheduled, /const mutedOnTrack = 'text-\[#3C3C43\]\/80 dark:text-\[#EBEBF5\]\/60';/,
    'inactive filter tabs need the 80% light ink on the restored track tint (60% was 3.1:1)');
  assert.match(scheduled, /\n\s*: mutedOnTrack\n/, 'the filter tabs must use the on-track ink');
  const timeline = stripComments(source('features/conversation/ConversationTimeline.jsx'));
  assert.match(timeline, /bg-blue-500\/\[0\.08\] text-\[10px\] text-blue-700 dark:text-blue-300/,
    'the resource chip must use blue-700 on its restored tint (blue-600 was 4.48:1)');
});

test('alpha modifiers stay on the opacity scale or use the bracket form', () => {
  // Tailwind's bare alpha scale covers multiples of five only: /8, /12, /88
  // … all compile to no CSS at all, so the tint silently vanishes in both
  // themes (the bracket form /[0.08] is the working spelling). The utility
  // prefix keeps the scan inside class tokens, so a URL like
  // img/v2/16/logo.png does not trip it. Scanned repo-wide: the sweep audited
  // four files and missed the same class in ten more.
  const BARE_ALPHA = /(?:bg|text|border|divide|ring|shadow|from|to|via|outline|decoration|accent|caret|fill|stroke|placeholder)-[^\s'"`]*\/(\d+)(?![\d.%])/g;
  const walk = (dir, out) => {
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
      const p = path.join(dir, entry.name);
      if (entry.isDirectory()) walk(p, out);
      else if (/\.(?:jsx?|mjs|html)$/.test(entry.name)) out.push(p);
    }
    return out;
  };
  const offScale = [];
  for (const file of walk(srcRoot, [])) {
    const src = readFileSync(file, 'utf8');
    for (const m of src.matchAll(BARE_ALPHA)) {
      if (Number(m[1]) % 5 !== 0) offScale.push(`${path.relative(srcRoot, file)}: ${m[0]}`);
    }
  }
  assert.deepEqual(offScale, [], 'no source file may carry alpha modifiers outside tailwind\'s opacity scale');
});
