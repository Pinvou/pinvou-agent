import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';

const root = path.resolve(import.meta.dirname, '..');
const read = file => fs.readFileSync(path.join(root, file), 'utf8');
const count = (source, pattern) => (source.match(pattern) || []).length;

// Round-26 M7 discipline (same stripComments as scheduled_task_tool_card and
// ui_language_coverage): positive shape pins must run against comment-stripped
// text, or comment-masking a live line satisfies the pin — commenting out a
// guard kept this test green before the strip was applied. Every pin below is
// POSITIVE, so over-stripping can only fail loudly, never satisfy one.
const stripComments = text => text
  .replace(/\/\*[\s\S]*?\*\//g, comment => comment.replace(/[^\n]/g, ''))
  .split('\n')
  .map(line => line.replace(/\/\/.*$/, ''))
  .join('\n');
const readStripped = file => stripComments(read(file));

// Negative lock: role="button" rows host real nested <button>s (shell-task
// cancel in tool cards, artifact locate, browser-tab close). If the row-level
// onKeyDown calls preventDefault on Enter/Space unconditionally, keys bubbled
// from a nested button get their default activation cancelled and instead
// trigger the row's own action — the nested button becomes unusable from the
// keyboard (any regression turns this test red). Every such handler must run
// nothing before the target !== currentTarget guard, then handle the row's own
// Enter/Space.
test('role=button rows with nested buttons guard Enter/Space against bubbled key events', () => {
  const toolRenderers = readStripped('src/features/tools/tool-renderers.jsx');
  // Guard-first shape: the target check must be the handler's first statement,
  // so no side effect (not even preventDefault) can leak onto bubbled keys.
  const guardedHandler = /onKeyDown=\{\(event\) => \{\s+if \(event\.target !== event\.currentTarget\) return;\s+if \(event\.key === 'Enter' \|\| event\.key === ' '\) \{\s+event\.preventDefault\(\);/g;
  assert.equal(
    count(toolRenderers, guardedHandler),
    3,
    'all three tool-card collapse headers (timeline/quiet/output) must ignore bubbled keys first, then handle their own Enter/Space',
  );
  assert.equal(
    count(toolRenderers, /if \(event\.target !== event\.currentTarget\) return;/g),
    3,
    'exactly three guarded handlers exist; bump this count together with the shape pin when adding a guarded header',
  );
  // displayExpanded (not the raw `expanded` state) is what gates the rendered
  // detail — live shell output forces it open — so the aria state, the chevron
  // and the quiet wrapper border must all follow it or they under-report while
  // content is visible.
  assert.equal(
    count(toolRenderers, /aria-expanded=\{displayExpanded\}/g),
    3,
    'every collapse header must expose the visible expanded state to assistive tech',
  );
  assert.equal(
    count(toolRenderers, /\$\{displayExpanded \? 'rotate-180' : ''\}/g),
    3,
    'every collapse header chevron must reflect the visible expanded state',
  );
  assert.equal(
    count(toolRenderers, /displayExpanded \? `rounded-\[12px\] overflow-hidden border/g),
    1,
    'the quiet card wrapper must show its border whenever the detail is visibly open',
  );

  const artifactsPanel = readStripped('src/features/artifacts/ArtifactsPanel.jsx');
  assert.match(
    artifactsPanel,
    /onKeyDown=\{\(e\) => \{\s+if \(e\.target !== e\.currentTarget\) return;\s+if \(e\.key === 'Enter' \|\| e\.key === ' '\) \{\s+e\.preventDefault\(\);\s+preview\(a\);/,
    "the artifacts row hosts the locate <button>: ignore bubbled keys first and keep the row's own Enter/Space activation",
  );
  assert.equal(
    count(artifactsPanel, /group-hover:opacity-100 group-focus-within:opacity-100/g),
    2,
    'both hover-only locate buttons (list row and switcher menu) must also reveal on keyboard focus',
  );

  const browserView = readStripped('src/features/browser/BrowserView.jsx');
  assert.match(
    browserView,
    /onKeyDown=\{\(event\) => \{\s+if \(event\.target !== event\.currentTarget\) return;\s+if \(event\.key !== 'Enter' && event\.key !== ' '\) return;\s+event\.preventDefault\(\);/,
    "the browser tab row hosts the close <button>: the guard must precede everything, and the tab's own Enter/Space activation must survive",
  );
  assert.equal(
    count(browserView, /group-hover:opacity-100 group-focus-within:opacity-100/g),
    1,
    'the hover-only tab close button must also reveal on keyboard focus',
  );
});
