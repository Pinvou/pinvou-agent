/**
 * Regression contract for native confirm in the settings page (the native
 * window.confirm does not render in Tauri WebView2): both SettingsView flows,
 * memory delete and feedback close, must route through in-app confirm dialogs
 * (same recipe as ProviderFormModal / ModelDeleteDialog / SearchDeleteDialog)
 * and must not rely on the native confirm. The memory-delete entry point is the
 * delete button on each row of the live "memory" section list
 * (MemorySettingsCard was removed as dead code; the live section carries the
 * delete capability).
 * Static source-reading assertions + zh/en/ja dictionary parity, following the
 * window.confirm assertion pattern of acp_providers_contract.test.js.
 */
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

import { dict } from './helpers/i18n-all.js'; // full three-language assertions: the browser entry lazy-loads via i18n.js, tests use the aggregate shim

const here = path.dirname(fileURLToPath(import.meta.url));
const appRoot = path.resolve(here, '..');
const SETTINGS_VIEW = fs.readFileSync(
  path.join(appRoot, 'src', 'features', 'settings', 'SettingsView.jsx'),
  'utf8',
);
// #817 B1: the stacked confirm skeleton moved out of SettingsView into the
// shared components/ConfirmDialog.jsx (SheetConfirmDialog was absorbed); the
// prop pass-through and stacking invariants live there now.
const CONFIRM_DIALOG = fs.readFileSync(
  path.join(appRoot, 'src', 'components', 'ConfirmDialog.jsx'),
  'utf8',
);
// ModalShell is the shell ConfirmDialog renders on; its aria wiring is part of
// the confirm contract (the alertdialog semantics came from the absorbed shell).
const MODAL_SHELL = fs.readFileSync(
  path.join(appRoot, 'src', 'components', 'ModalShell.jsx'),
  'utf8',
);
// Absorbed confirms that must keep their pre-absorption stacking tier (they
// used to ride the z-[200] ConfirmDialogShell / ModalBackdrop band).
const ARCHIVE_CONFIRM = fs.readFileSync(
  path.join(appRoot, 'src', 'components', 'ArchiveConfirmDialog.jsx'),
  'utf8',
);
const TOOL_STORE_VIEW = fs.readFileSync(
  path.join(appRoot, 'src', 'features', 'tools', 'ToolStoreView.jsx'),
  'utf8',
);
const SMOKE = fs.readFileSync(
  path.join(appRoot, 'tests', 'settings_ui_smoke.js'),
  'utf8',
);

/** Slice the source between startMarker and the first endMarker after it. */
function sliceSource(source, startMarker, endMarker) {
  const start = source.indexOf(startMarker);
  assert.notStrictEqual(start, -1, `source marker missing: ${startMarker}`);
  const end = source.indexOf(endMarker, start);
  assert.notStrictEqual(end, -1, `end marker missing: ${endMarker}`);
  return source.slice(start, end);
}

test('SettingsView no longer calls native window.confirm', () => {
  assert.doesNotMatch(
    SETTINGS_VIEW,
    /window\.confirm\s*\(/,
    'settings page must not call window.confirm (does not render in Tauri WebView2)',
  );
});

test('memory delete routes through the in-app confirm dialog', () => {
  // The in-app confirm dialog must exist, with a stable testid on the OK button.
  // MemoryDeleteDialog renders through the shared ConfirmDialog (stacked
  // layout), which turns the testid/confirmTestId props into data-testid
  // attributes.
  assert.match(SETTINGS_VIEW, /testid="memory-delete-confirm"/, 'memory delete confirm dialog must exist');
  assert.match(SETTINGS_VIEW, /confirmTestId="memory-delete-confirm-ok"/, 'memory delete confirm button must carry a testid');
  assert.match(CONFIRM_DIALOG, /testid=\{testid\}/, 'ConfirmDialog must pass the backdrop testid through to ModalShell');
  assert.match(CONFIRM_DIALOG, /data-testid=\{confirmTestId\}/, 'ConfirmDialog must render the confirm testid from its props');

  // The delete entry point must be a row-level action of the memory section (the row delete button routes into deleteItem)
  assert.match(
    SETTINGS_VIEW,
    /data-testid="memory-item-delete" onClick=\{\(\) => deleteItem\(item\)\}/,
    'memory list rows must provide a delete button routed into deleteItem',
  );

  // deleteItem only records the pending item; it must not delete directly
  const deleteItem = sliceSource(
    SETTINGS_VIEW,
    'const deleteItem = item => {',
    'const confirmDeleteItem',
  );
  assert.match(deleteItem, /setMemoryDeleteConfirm\(item\)/, 'deleteItem must record the pending item first');
  assert.doesNotMatch(deleteItem, /window\.confirm\s*\(/, 'deleteItem must not call the native confirm');
  assert.doesNotMatch(deleteItem, /deleteMemoryItem\(item\.kind/, 'deleteItem must not delete before confirmation');

  // The actual delete lives only in the confirmed path confirmDeleteItem (single call site)
  const confirmHandler = sliceSource(
    SETTINGS_VIEW,
    'const confirmDeleteItem = async item => {',
    'const editProfile',
  );
  assert.match(
    confirmHandler,
    /await bridge\.memory\.deleteMemoryItem\(item\.kind, item\.id\)/,
    'deleteMemoryItem must be called after confirmation',
  );
  assert.strictEqual(
    (SETTINGS_VIEW.match(/await bridge\.memory\.deleteMemoryItem\(/g) || []).length,
    1,
    'deleteMemoryItem may appear only once, in the confirmed path',
  );

  // The dialog OK button deletes first, then clears state (mirroring ModelDeleteDialog's order)
  const dialog = sliceSource(SETTINGS_VIEW, 'const MemoryDeleteDialog', 'const SettingsView = (');
  assert.match(
    dialog,
    /onConfirmDelete\(item\);\s*setMemoryDeleteConfirm\(null\);/,
    'the confirm button must delete before clearing state',
  );
  assert.doesNotMatch(dialog, /window\.confirm\s*\(/, 'the confirm dialog must not call the native confirm');
});

test('feedback close routes through the in-app confirm layer', () => {
  assert.match(SETTINGS_VIEW, /testid="feedback-close-confirm"/, 'feedback close confirm layer must exist');
  assert.match(SETTINGS_VIEW, /confirmTestId="feedback-close-confirm-ok"/, 'feedback close confirm button must carry a testid');

  // A first close with a dirty draft opens the in-app confirm layer instead of relying on the native confirm
  const closeFeedback = sliceSource(
    SETTINGS_VIEW,
    'const closeFeedback = () => {',
    'const pickFeedbackAttachments',
  );
  assert.match(closeFeedback, /!feedbackCloseConfirm/, 'closeFeedback must gate on the in-app confirm state');
  assert.match(closeFeedback, /setFeedbackCloseConfirm\(true\)/, 'a first dirty-draft close must open the in-app confirm layer');
  assert.doesNotMatch(closeFeedback, /window\.confirm\s*\(/, 'closeFeedback must not call the native confirm');

  // The confirm layer sits above the feedback panel (z-[100]) and only appears while the panel is open
  assert.match(
    SETTINGS_VIEW,
    /feedbackOpen && feedbackCloseConfirm && \(/,
    'the confirm layer must open and close together with the feedback panel',
  );
  assert.match(
    SETTINGS_VIEW,
    /<ConfirmDialog[\s\S]{0,120}?testid="feedback-close-confirm"/,
    'the confirm layer must be the shared ConfirmDialog',
  );
  // The stacking order is the real invariant: if the shared dialog ever drops below the
  // feedback panel (z-[100]) the confirm becomes unclickable and the panel locks. Pin it
  // on the shared component itself (#817 B1 folded SheetConfirmDialog into
  // ConfirmDialog) — asserting only the call site leaves the z-index claim
  // living in a message string.
  assert.match(
    CONFIRM_DIALOG,
    /zIndexClass = 'z-\[110\]'/,
    'the shared confirm dialog must stack above the feedback panel (z-[100])',
  );

  // The OK button must route back into closeFeedback to truly close
  assert.match(
    SETTINGS_VIEW,
    /onConfirm=\{\(\) => \{ setFeedbackCloseConfirm\(false\); closeFeedback\(\); \}\}/,
    'the confirm button must route back into closeFeedback to truly close',
  );
});

test('the settings smoke no longer stubs native confirm', () => {
  assert.doesNotMatch(
    SMOKE,
    /window\.confirm\s*=/,
    'settings smoke must not stub window.confirm again (a reintroduced native confirm must fail loudly)',
  );
});

test('the shared confirm keeps its alertdialog semantics on the shared shell', () => {
  // ConfirmDialog must render as an alertdialog (destructive-confirm semantics
  // per the absorbed reference shell); a dropped role would silently regress
  // every absorbed confirm to a generic dialog.
  assert.match(
    CONFIRM_DIALOG,
    /role="alertdialog"/,
    'ConfirmDialog must render role=alertdialog',
  );
  // ModalShell can only emit aria-* for roles that support them; `role` is a
  // prop, so the attributes must ride a conditional spread gated on the
  // dialog/alertdialog roles (static aria-modal/labelledby/describedby next to
  // role={role} fails Biome's useAriaPropsSupportedByRole and would red CI —
  // the 2026-11-19 round).
  assert.match(
    MODAL_SHELL,
    /\{\.\.\.\(role === 'dialog' \|\| role === 'alertdialog'[\s\S]{0,80}'aria-modal': 'true', 'aria-labelledby': labelledBy, 'aria-describedby': describedBy[\s\S]{0,20}:\s*\{\}\)\}/,
    'ModalShell must gate its aria attributes on the dialog/alertdialog roles via a conditional spread',
  );
  assert.doesNotMatch(
    MODAL_SHELL,
    /aria-modal="true"|aria-labelledby=\{|aria-describedby=\{/,
    'the shell panel must carry no static aria-* attributes (they would fail useAriaPropsSupportedByRole against the implicit generic role)',
  );
  // The cancel testid must exist in BOTH layouts — a stacked-only gap would
  // silently break smoke drivers that target the cancel button of a stacked confirm.
  assert.strictEqual(
    (CONFIRM_DIALOG.match(/data-testid=\{cancelTestId\}/g) || []).length,
    2,
    'cancelTestId must be rendered in both the stacked and grid layouts',
  );
  // The backdrop button is a pointer-only affordance: tabIndex={-1} keeps it
  // out of Tab order so keyboard users cannot land on a full-viewport invisible
  // "Cancel" or escape the modal through it (the 2026-11-20 round). Anchored to
  // the button's own attribute order — the panel div also carries tabIndex={-1}
  // and must not satisfy this pin.
  assert.match(
    MODAL_SHELL,
    /aria-label=\{backdropLabel\}[\s\S]{0,500}tabIndex=\{-1\}[\s\S]{0,60}disabled=\{busy\}[\s\S]{0,30}onClick=\{onCancel\}/,
    'the ModalShell backdrop button must stay out of Tab order (tabIndex={-1} between its label and handlers)',
  );
});

test('absorbed confirms keep their pre-absorption stacking tier', () => {
  // ArchiveConfirmDialog/ArchivedDeleteConfirmDialog used to ride the
  // z-[200] ConfirmDialogShell; dropping to ConfirmDialog's z-[110] default
  // would let blocking z-[200] overlays (e.g. WebConnectionStatus) cover them.
  assert.strictEqual(
    (ARCHIVE_CONFIRM.match(/zIndexClass="z-\[200\]"/g) || []).length,
    2,
    'both archive confirms must explicitly keep the z-[200] tier',
  );
  // The two SheetRowConfirm sites (skill update, recycle purge) rode the
  // z-[200] ModalBackdrop before absorption — same tier preservation.
  assert.strictEqual(
    (TOOL_STORE_VIEW.match(/zIndexClass="z-\[200\]"/g) || []).length,
    2,
    'both tool-store confirms must explicitly keep the z-[200] tier',
  );
});

test('feedback/memory confirm copy exists in zh/en/ja', () => {
  // Assert key existence only (matching the acp_providers_contract key-existence
  // pattern); freezing exact copy would red CI on normal wording tweaks.
  for (const language of ['zh', 'en', 'ja']) {
    const d = dict[language];
    assert.ok(d.feedbackCloseConfirm, `${language}.feedbackCloseConfirm must exist`);
    assert.ok(d.feedbackCloseAnyway, `${language}.feedbackCloseAnyway must exist (feedback close confirm button)`);
    assert.ok(d.cancel, `${language}.cancel must exist (feedback close cancel button)`);
    assert.ok(d.uiSettingsView, `${language}.uiSettingsView must exist`);
    assert.ok(d.uiSettingsView.memoryDeleteConfirm, `${language}.uiSettingsView.memoryDeleteConfirm must exist`);
    assert.ok(d.uiSettingsDetail, `${language}.uiSettingsDetail must exist`);
    assert.ok(d.uiSettingsDetail.delete, `${language}.uiSettingsDetail.delete must exist (memory delete confirm button)`);
    assert.ok(d.uiSettingsDetail.cancel, `${language}.uiSettingsDetail.cancel must exist (memory delete cancel button)`);
    assert.ok(d.uiSettingsDetail.memoryDeleteFailed, `${language}.uiSettingsDetail.memoryDeleteFailed must exist (memory delete failure banner)`);
  }
});
