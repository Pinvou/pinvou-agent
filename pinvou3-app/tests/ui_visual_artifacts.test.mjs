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
import { readFileSync, existsSync } from 'node:fs';
import { test } from 'node:test';
import { dict } from './helpers/i18n-all.js';
import { COLOR_SCHEME_STORAGE_KEY } from '../src/shared/color-scheme.js';

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
  // The persona modal body renders LLM markdown too: without its own
  // block-table recipe its cells squeeze per-character instead of scrolling.
  assert.match(
    baseCss,
    /\.persona-body table \{[^}]*display:block; width:max-content; max-width:100%; overflow-x:auto; \}/,
    'persona-body tables must keep the block-table scroll box',
  );
});

test('reload hydration joins adjacent assistant text blocks with a blank line (both lanes)', () => {
  const shared = stripComments(source('shared/bridge-shared-helpers.js'));
  assert.match(shared, /textBuf = textBuf \? textBuf \+ "\\n\\n" \+ b\.text : b\.text/,
    'shared rerender must join adjacent text blocks with a blank line');
  const codeLane = stripComments(source('features/codex/code-native-lane.js'));
  assert.match(codeLane, /textBuf = textBuf \? `\$\{textBuf\}\\n\\n\$\{blockText\}` : blockText/,
    'code-native hydration must use the same blank-line join');
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
  // on the same key and canvas colour so the two boots never drift apart.
  assert.match(source('index.html'), new RegExp(COLOR_SCHEME_STORAGE_KEY.replaceAll('.', '\\.')));
  assert.match(stripComments(source('index.html')), /html\.dark \{ background: #131314; \}/,
    'index.html canvas must equal the theme-boot.js default dark background');
  // The recipe is more than key + colour: the inline boot must still apply
  // the class, or a dark cold boot paints a white first frame again.
  assert.match(stripComments(source('index.html')), /if \(dark\) document\.documentElement\.classList\.add\('dark'\)/,
    'index.html inline boot must still apply the dark class pre-paint');
  assert.match(themeBoot, /\|\| '#131314'/, 'theme-boot default must stay equal to the index.html dark canvas');
});

test('web-access primaries and the refresh confirm carry the shared light + dark pairing', () => {
  const settingsView = stripComments(source('features/settings/SettingsView.jsx'));
  assert.match(settingsView, /bg-\[#0B57D0\] text-white hover:bg-\[#0842A0\] disabled:opacity-60 dark:bg-\[#A8C7FA\] dark:text-\[#062E6F\] dark:hover:bg-\[#c2d8fb\]/,
    'refresh confirm button must carry light + dark primary styling');
  assert.ok(count(settingsView, /bg-\[#0B57D0\] text-white hover:bg-\[#0842A0\] disabled:opacity-50 dark:bg-\[#A8C7FA\] dark:text-\[#062E6F\] dark:hover:bg-\[#c2d8fb\]/g) >= 2,
    'the allow-workspace and enable primaries in the same panel must carry the dark pairing too');
});

test('dark-capable hint text and toggle tracks have dark variants', () => {
  const remoteKnowledge = stripComments(source('features/remote-knowledge/RemoteKnowledgeView.jsx'));
  assert.match(remoteKnowledge, /bg-\[#E5E5EA\] dark:bg-\[#39393D\] transition-colors/,
    'remote-KB toggle track must follow the shared Toggle light/dark track colours');
  assert.match(remoteKnowledge, /rounded-xl bg-\[#F7F9FC\] px-3\.5 py-4 text-\[12px\] dark:bg-white\/\[0\.04\]/,
    'the empty-state chip must get the same dark tint as its sibling chips');
  assert.ok(count(remoteKnowledge, /bg-\[#0B57D0\] px-5 text-\[14px\] font-bold text-white[^"']*dark:bg-\[#A8C7FA\]/g) >= 2,
    'the inline hero primaries must carry the shared light + dark pairing');
  // The file pairs every error/success text with its dark half (#ff8a80 /
  // #7DD3A8); a bare light-only instance re-opens the unreadable-in-dark
  // class. The lookahead stays inside the className string (no quote may be
  // crossed) but tolerates dark:bg tokens sitting between the pair.
  assert.equal(count(remoteKnowledge, /text-\[#d63a3a\](?![^"'`]*dark:text-\[#ff8a80\])/g), 0,
    'every error-red text in remote knowledge must carry its dark pair');
  assert.equal(count(remoteKnowledge, /text-\[#b72f2f\](?![^"'`]*dark:text-\[#ff8a80\])/g), 0,
    'every deep error-red text in remote knowledge must carry its dark pair');
  assert.equal(count(remoteKnowledge, /text-\[#16894a\](?![^"'`]*dark:text-\[#7DD3A8\])/g), 0,
    'every success-green text in remote knowledge must carry its dark pair');
  // The local knowledge view pairs its error reds with the same dark half as
  // the remote one; a bare light-only instance re-opens the class there too.
  const knowledgeView = stripComments(source('features/knowledge/KnowledgeView.jsx'));
  assert.equal(count(knowledgeView, /text-\[#d63a3a\](?![^"'`]*dark:text-\[#ff8a80\])/g), 0,
    'every error-red text in local knowledge must carry its dark pair');
  const acpNotices = stripComments(source('features/codex/AcpRuntimeNotices.jsx'));
  // Both pair directions are zero-tolerance: a gray-500 without a dark
  // gray-400 half anywhere later in its className string, and any gray-400
  // that is not the dark half of an exact pair (bare, reordered, or riding
  // its own variant chain like dark:hover:) — a count pin would let two
  // hints lose their half and stay green.
  assert.equal(count(acpNotices, /text-gray-500(?![^"'`]*dark:text-gray-400)/g), 0,
    'every gray-500 hint in the runtime notices must carry its dark gray-400 half');
  const withoutPairs = acpNotices.replaceAll('text-gray-500 dark:text-gray-400', '');
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
  assert.match(acpNotices, /rounded-xl bg-red-600 px-3 py-1\.5/,
    'the account switch must use red-600 under white text');
  assert.match(acpNotices, /bg-red-500\/\[0\.08\] text-red-700 dark:text-red-300/,
    'the tinted error banner must darken its light leg to red-700');
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
  // w-[300px] shadow-2xl rounded-[20px] must fail here too.
  assert.equal(count(toolStore, /w-\[300px\](?![^"'`]*max-w-full)/g), 0,
    'no 300px dialog card may lose its max-width guard');
  assert.ok(count(toolStore, /text-slate-500 dark:text-slate-400 font-semibold uppercase tracking-wider/g) >= 3,
    'detail labels must have a dark variant');
  assert.match(toolStore, /\(storeCopy\.recycleKinds \|\| \{\}\)\[item\.kind\]/, 'recycle badge must use the singular type labels');
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

test('community group name and recycle-bin badge labels exist in all three languages', () => {
  for (const language of ['zh', 'en', 'ja']) {
    assert.ok(typeof dict[language].communityGroupName === 'string' && dict[language].communityGroupName.trim(),
      `${language}.communityGroupName must exist`);
    const kinds = dict[language].uiToolStore.recycleKinds;
    // recycle_bin.rs only ever records mcp / skill / bundle packages.
    for (const kind of ['bundle', 'mcp', 'skill']) {
      assert.ok(kinds && typeof kinds[kind] === 'string' && kinds[kind].trim(), `${language}.uiToolStore.recycleKinds.${kind} must exist`);
    }
  }
  // dictJa spreads dictEn: a ja-missing key silently falls back to English.
  assert.notEqual(dict.ja.communityGroupName, dict.en.communityGroupName, 'ja community group name must not fall back to English');
  assert.equal(dict.en.uiToolStore.recycleKinds.skill, 'Skill', 'en recycle badge labels must be singular');

  const communityPanel = source('features/settings/CommunityPanel.jsx');
  assert.match(stripComments(communityPanel), /copy\.communityGroupName/, 'the community panel must render the localized group name');
  assert.doesNotMatch(communityPanel, /COMMUNITY_QQ_GROUP_NAME/, 'the raw Chinese constant must not come back');
  assert.doesNotMatch(source('features/settings/community-config.js'), /COMMUNITY_QQ_GROUP_NAME/, 'community-config must not export the name');
});

test('alpha modifiers stay on the opacity scale or use the bracket form', () => {
  // Tailwind's bare alpha scale covers multiples of 5 only: /8, /16, /18 and
  // /28 compile to no CSS at all, so the tint silently vanishes in both
  // themes (the bracket form /[0.08] is the working spelling).
  for (const file of [
    'features/remote-knowledge/RemoteKnowledgeView.jsx',
    'features/knowledge/KnowledgeView.jsx',
    'features/codex/AcpRuntimeNotices.jsx',
    'features/settings/SettingsView.jsx',
  ]) {
    const src = source(file);
    assert.equal(count(src, /[\]\d]\/(?:8|16|18|28)(?![\d.])/g), 0,
      `${file} must not carry alpha modifiers outside tailwind's opacity scale`);
  }
});
