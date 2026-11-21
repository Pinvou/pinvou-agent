// Shown after a persona card is saved: iOS-style confirm offering to view the
// user's own cards or dismiss. Lazy-loaded because it only appears after a save.
// #817 B1: the hand-rolled frosted card moved onto components/ConfirmDialog
// (portal, role=alertdialog, IME-guarded Escape); "Later" is the cancel row,
// "View" the neutral (non-destructive) confirm.
import { ConfirmDialog } from '../../components/ConfirmDialog.jsx';

export function SavedPersonaConfirmDialog({ onClose, onView, savedConfirm, t }) {
  return (
    <ConfirmDialog
      zIndexClass="z-[80]"
      title={t.cpSavedTitle}
      description={t.cpSavedDesc(savedConfirm.name || '')}
      confirmLabel={t.cpSavedView}
      cancelLabel={t.cpSavedLater}
      onConfirm={onView}
      onCancel={onClose}
    />
  );
}
