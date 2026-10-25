import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';

const root = path.resolve(import.meta.dirname, '..');
const read = file => fs.readFileSync(path.join(root, file), 'utf8');

// Negative lock: role="button" collapse headers host real nested <button>s
// (shell-task cancel, artifact locate). If the row-level onKeyDown calls
// preventDefault on Enter/Space unconditionally, keys bubbled from a nested
// button get their default activation cancelled and instead trigger the row's
// own action — the nested button becomes unusable from the keyboard (any
// regression turns this test red). Every such handler must first ignore
// events originating from nested interactive elements via
// target !== currentTarget, then handle the row's own Enter/Space.
test('role=button rows with nested buttons guard Enter/Space against bubbled key events', () => {
  const toolRenderers = read('src/features/tools/tool-renderers.jsx');
  const guardedInToolRenderers = toolRenderers.match(/if \(event\.target !== event\.currentTarget\) return;/g) || [];
  assert.ok(
    guardedInToolRenderers.length >= 3,
    'all three tool-card collapse headers (timeline/quiet/output) must ignore keys from nested buttons',
  );
  assert.equal(
    (toolRenderers.match(/aria-expanded=\{(?:displayExpanded|expanded)\}/g) || []).length,
    3,
    'every collapse header must expose its expanded state to assistive tech',
  );

  const artifactsPanel = read('src/features/artifacts/ArtifactsPanel.jsx');
  assert.match(
    artifactsPanel,
    /onKeyDown=\{\(e\) => \{\s+if \(e\.target !== e\.currentTarget\) return;/,
    'the artifacts row hosts the locate <button>; its key handler must ignore bubbled keys',
  );
  assert.match(
    artifactsPanel,
    /group-hover:opacity-100 group-focus-within:opacity-100/,
    'the hover-only locate button must also reveal on keyboard focus',
  );
});
