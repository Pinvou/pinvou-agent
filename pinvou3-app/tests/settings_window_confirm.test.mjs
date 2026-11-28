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
// The shell's Escape guard lives in its own hook (extracted from ModalShell in
// the 2026-11-20 round so it can be pinned behaviorally — see
// tests/use_dialog_escape_key.test.mjs); this file pins the guard expression
// and ModalShell's wiring to both hooks.
const DIALOG_ESCAPE_KEY = fs.readFileSync(
  path.join(appRoot, 'src', 'hooks', 'useDialogEscapeKey.js'),
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
    /\{\.\.\.\(role === 'dialog' \|\| role === 'alertdialog'[\s\S]{0,240}'aria-modal': 'true', 'aria-labelledby': labelledBy, 'aria-describedby': describedBy[\s\S]{0,160}:\s*\{\}\)\}/,
    'ModalShell must gate its aria attributes on the dialog/alertdialog roles via a conditional spread',
  );
  // No static aria-* on the panel — and the gate must hold in every form the
  // lint cannot catch: a static string literal OR a brace expression
  // (aria-modal={true}) would both trip useAriaPropsSupportedByRole.
  assert.doesNotMatch(
    MODAL_SHELL,
    /aria-modal="true"|aria-modal=\{|aria-labelledby=\{|aria-describedby=\{/,
    'the shell panel must carry no static aria-* attributes (they would fail useAriaPropsSupportedByRole against the implicit generic role)',
  );
  // A caller passing neither title nor description would otherwise render an
  // unnamed alertdialog — the exact gap the audit flagged. No live site hits
  // this (every site passes title or description), but the component must not
  // offer the footgun to future callers: the confirm label is the last-resort
  // accessible name (aria-labelledby still wins whenever it is present).
  assert.match(
    CONFIRM_DIALOG,
    /ariaLabel=\{title \|\| description \? undefined : confirmLabel\}/,
    'ConfirmDialog must fall back to the confirm label when title and description are both absent',
  );
  assert.match(
    MODAL_SHELL,
    /\.\.\.\(ariaLabel \? \{ 'aria-label': ariaLabel \} : \{\}\)/,
    'ModalShell must forward the last-resort aria-label through the gated spread',
  );
  // The cancel testid must exist in BOTH layouts — a stacked-only gap would
  // silently break smoke drivers that target the cancel button of a stacked confirm.
  assert.strictEqual(
    (CONFIRM_DIALOG.match(/data-testid=\{cancelTestId\}/g) || []).length,
    2,
    'cancelTestId must be rendered in both the stacked and grid layouts',
  );
  assert.strictEqual(
    (CONFIRM_DIALOG.match(/data-testid=\{confirmTestId\}/g) || []).length,
    2,
    'confirmTestId must be rendered in both the stacked and grid layouts (a layout-only loss would silently break that layout\'s smoke driver)',
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
  // The shell hardening is load-bearing for every absorbed confirm: Escape is
  // IME-guarded (candidate-cancelling Escape must not close anything) and
  // busy-guarded (an in-flight confirmation cannot be dismissed), and the
  // dialog takes focus on open and restores it on close. The guard behavior is
  // pinned behaviorally in tests/use_dialog_escape_key.test.mjs and
  // tests/use_dialog_focus_restore.test.mjs; these pins keep ModalShell wired
  // to both hooks (dropping a call would silently unguard all 12 confirms —
  // the r3-review mutations that survived).
  assert.match(
    DIALOG_ESCAPE_KEY,
    /event\.key === 'Escape' && !busy && !isImeComposing\(event\)/,
    'the shared dialog Escape must stay busy-guarded and IME-guarded (behavioral pins: use_dialog_escape_key.test.mjs)',
  );
  assert.match(
    MODAL_SHELL,
    /useDialogEscapeKey\(busy, onCancel\)/,
    'ModalShell must keep its window-level Escape wired to useDialogEscapeKey',
  );
  assert.match(
    MODAL_SHELL,
    /useDialogFocusRestore\(dialogRef, initialFocusRef\)/,
    'ModalShell must keep focus capture/restore wired to useDialogFocusRestore',
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

test('the settings picker escape layer stays IME-guarded (r3 review)', () => {
  // With the add-all confirm stacked over the endpoint picker, the picker's
  // own window listener is what closes the confirm (the stack-aware
  // convention). An unguarded listener would let a candidate-cancelling
  // composition-Escape slam the confirm shut behind its own IME guard —
  // the r3-review minor.
  assert.match(
    SETTINGS_VIEW,
    /const onPickerKeyDown = e => \{\s*if \(e\.key !== 'Escape' \|\| isImeComposing\(e\)\) return;/,
    'the settings picker Escape must decline IME composition (shared/ime-guard.mjs)',
  );
});

test('the settings picker stack closes confirm-first (r4 review)', () => {
  // The settings twin of the ACP layered handler: with the add-all confirm
  // stacked over the endpoint picker, one Escape press must close exactly the
  // visual top (the confirm) — the r4 review found this branch order pinned
  // on the ACP side (source pin + ⑥.18e–g) but not here, so an order flip
  // (picker closing first) survived every suite. The r5 wave added the
  // write-in-flight guards to the picker branch plus an explanatory comment
  // between the branches; the pin tolerates them (bounded gap) but a flip —
  // picker branch ahead of the confirm branch — still cannot match.
  assert.match(
    SETTINGS_VIEW,
    /if \(addAllModelsConfirm && !addingAllModels\) setAddAllModelsConfirm\(null\);[\s\S]{0,400}else if \(endpointPickerOpen && !addingAllModels/,
    'the stacked confirm branch must stay ahead of the picker branch (confirm-first)',
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
