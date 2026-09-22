// Unified "choose workspace" picker (design §2/§3/§9.3/§9.4): the project is
// the only organizing unit; physical folders are absorbed as "single-root
// projects"; browsing = one way to create a project; a temporary session is an
// explicit option. The hot view (recency sort + cold-project hiding) is
// computed by ./workspacePickerState.js; this component is pure display and
// hands every consequential action to the container (main.jsx) via props
// callbacks. A Web host (§9.8) renders only the "temporary session" option,
// passed in by the container via webOnly.
import { useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { AlertTriangle, ChevronDown, FolderOpen, Layers, Search, Sparkles, X } from '../../components/icons.jsx';
import { isImeComposing } from '../../shared/ime-guard.mjs';
import { useDialogFocusRestore } from '../../hooks/useDialogFocusRestore.js';
import { useDialogFocusTrap } from '../../hooks/useDialogFocusTrap.js';
import { formatSessionDate } from '../../shared/date-utils.js';
import { workspaceName } from '../../shared/workspace-recents.js';
import { pickerPrimaryRoot, pickerProjectRoots, workspaceNoticeTone } from './workspacePickerState.js';

const WorkspacePickerDialog = ({
  open,
  rows,
  mode,
  language,
  busy,
  webOnly,
  excludedFolder,
  // Codex lane + third-party ACP agent: the stage-gate (§6) delivers only the
  // primary root on the wire, so every grant notice says "recorded" instead
  // of promising access to the additional roots.
  deliveryLimited,
  t,
  onClose,
  onSelectProject,
  onTemporary,
  onBrowse,
  onBrowseExcluded,
  onDismissExcluded,
}) => {
  const [query, setQuery] = useState('');
  // Multi-root projects expand inline (re-pick the root + permission notice);
  // only one row is expanded at a time.
  const [expandedId, setExpandedId] = useState(null);
  const onCloseRef = useRef(onClose);
  const dialogRef = useRef(null);
  // Escape reads the latest excluded-panel state through refs so the key
  // listener stays subscribed once per open instead of per render.
  const excludedFolderRef = useRef(excludedFolder);
  const onDismissExcludedRef = useRef(onDismissExcluded);
  const backdropPressRef = useRef(false);
  // busyRef mirrors the ManageProjectFoldersDialog pattern: Escape/backdrop/X
  // read it inside listeners and handlers so a mid-operation close is
  // intercepted (the async ensure/materialize retry context survives).
  const busyRef = useRef(busy);
  useEffect(() => {
    onCloseRef.current = onClose;
    excludedFolderRef.current = excludedFolder || null;
    onDismissExcludedRef.current = onDismissExcluded;
    busyRef.current = busy;
  });

  // Shared modal recipe (review #484 M5): the Tab trap and the focus
  // capture/restore come from the shared hooks — the hand-rolled trap
  // dropped focus when every control was busy-disabled and never restored
  // focus on close. Only the Escape tiering stays in-component (child state
  // backs out before the dialog closes).
  useDialogFocusTrap(dialogRef);
  useDialogFocusRestore(dialogRef, null, null);

  useEffect(() => {
    if (!open) return () => {};
    const onKey = (e) => {
      if (e.key === 'Escape' && !isImeComposing(e)) {
        e.preventDefault();
        // The excluded-folder panel backs out to the list first; only the
        // list state closes the dialog (same precedent as the move picker's
        // add-folder panel and the manage-folders remove confirm).
        if (excludedFolderRef.current) {
          if (onDismissExcludedRef.current) onDismissExcludedRef.current();
          return;
        }
        // Busy gate parity with the backdrop path: closing mid-operation
        // would destroy the in-flight ensure/materialize retry context
        // (review #484 round-5 M3).
        if (busyRef.current) return;
        onCloseRef.current();
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [open]);

  if (!open || typeof document === 'undefined') return null;

  const copy = t.uiWorkspacePicker;
  // One notice shape for all three grant surfaces (single-root row,
  // expansion panel, browse row): mode picks grant vs visibility tone,
  // deliveryLimited picks access vs recorded-only wording (§6 stage-gate).
  const rowNotice = count => (workspaceNoticeTone(mode) === 'restricted'
    ? (deliveryLimited ? copy.noticeRestrictedRecorded(count) : copy.noticeRestricted(count))
    : (deliveryLimited ? copy.noticeVisibilityRecorded(count) : copy.noticeVisibility(count)));
  const rowCls = 'w-full px-3.5 py-2.5 flex items-center gap-2.5 text-left text-[14px] rounded-2xl transition-colors text-[#1F1F1F] hover:bg-[#F1F3F4] dark:text-[#E3E3E3] dark:hover:bg-[#303134]';

  const filtered = (() => {
    const q = query.trim().toLowerCase();
    const list = Array.isArray(rows) ? rows : [];
    if (!q) return list;
    return list.filter((row) => {
      const name = String(row.project.name || '').toLowerCase();
      const roots = pickerProjectRoots(row.project).join(' ').toLowerCase();
      return name.includes(q) || roots.includes(q);
    });
  })();

  const chooseProject = (project) => {
    if (busy) return;
    const roots = pickerProjectRoots(project);
    // Rootless (tag-only) projects never reach the picker: computePickerRows
    // drops them so counting and rendering stay consistent. Kept as a
    // defensive guard only.
    if (roots.length === 0) return;
    if (roots.length > 1) {
      // Multi-root: expand to re-pick the root + the mode-aware permission
      // notice (§9.4: the notice lands at the moment of selection).
      setExpandedId(prev => (prev === project.id ? null : project.id));
      return;
    }
    onSelectProject(project, roots[0]);
  };

  const projectRow = ({ project, lastActivity }) => {
    const roots = pickerProjectRoots(project);
    if (roots.length === 0) return null;
    const primary = pickerPrimaryRoot(project);
    const multi = roots.length > 1;
    const expanded = expandedId === project.id;
    return (
      <div key={project.id}>
        <button
          type="button"
          disabled={busy}
          onClick={() => chooseProject(project)}
          title={primary}
          className={`${rowCls} disabled:opacity-60`}
        >
          {multi
            ? <Layers size={15} className="shrink-0 text-[#5F6368] dark:text-[#9AA0A6]" />
            : <FolderOpen size={15} className="shrink-0 text-[#0B57D0] dark:text-[#A8C7FA]" />}
          <span className="min-w-0 flex-1">
            <span className="block truncate">{project.name}</span>
            <span className="block truncate text-[12px] text-[#8A8F94] dark:text-[#9AA0A6]">
              {multi
                ? copy.multiRootSummary(roots.length, primary)
                : primary}
            </span>
            {/* Grant notice parity (§9.4): single-root rows select directly
                without the expansion panel, so the mode-aware notice rides
                the row itself; multi-root rows show it in the panel. */}
            {!multi && (
              <span className="block truncate text-[11px] text-[#8A8F94] dark:text-[#9AA0A6]">
                {rowNotice(1)}
              </span>
            )}
          </span>
          <span className="shrink-0 text-[11px] text-[#8A8F94] dark:text-[#9AA0A6]">
            {formatSessionDate(lastActivity, language)}
          </span>
          {multi && (
            <ChevronDown
              size={13}
              className={`shrink-0 text-[#8A8F94] transition-transform ${expanded ? 'rotate-180' : ''}`}
            />
          )}
        </button>
        {multi && expanded && (
          <div className="mx-2 mb-1 rounded-2xl bg-[#EAECEF] dark:bg-[#303134] px-3.5 py-2.5">
            {/* Mode-aware permission notice (§9.4): restricted = grant semantics; YOLO = visibility semantics. */}
            <div className="mb-2 flex items-start gap-1.5 text-[12px] text-[#5F6368] dark:text-[#C4C7C5]">
              <AlertTriangle size={13} className="shrink-0 mt-0.5" />
              <span>
                {rowNotice(roots.length)}
              </span>
            </div>
            {roots.map(root => (
              <button
                key={root}
                type="button"
                disabled={busy}
                onClick={() => !busy && onSelectProject(project, root)}
                title={root}
                className="w-full rounded-lg px-2.5 py-1.5 flex items-center gap-2 text-left hover:bg-black/[0.04] dark:hover:bg-white/[0.06] disabled:opacity-60"
              >
                <FolderOpen size={13} className="shrink-0 text-gray-400" />
                <span className="min-w-0 flex-1 truncate text-[12px]">{root}</span>
                {root === primary && (
                  <span className="shrink-0 text-[11px] text-[#0B57D0] dark:text-[#A8C7FA]">
                    {copy.primaryRootBadge}
                  </span>
                )}
              </button>
            ))}
          </div>
        )}
      </div>
    );
  };

  return createPortal(
    // biome-ignore lint/a11y/noStaticElementInteractions: backdrop click-to-close; keyboard path is the Escape listener and the close button
    <div
      role="presentation"
      className="fixed inset-0 z-[200] flex items-center justify-center p-4"
      style={{ background: 'rgba(0,0,0,.34)', backdropFilter: 'blur(14px) saturate(140%)', WebkitBackdropFilter: 'blur(14px) saturate(140%)' }}
      onMouseDown={(e) => { backdropPressRef.current = e.target === e.currentTarget && !busyRef.current; }}
      onMouseUp={(e) => {
        // Two-phase close (MoveToProjectDialog idiom): a press that started
        // on the backdrop AND ended there closes — a drag-select that begins
        // inside the panel must not. Busy presses never arm the close.
        if (backdropPressRef.current && e.target === e.currentTarget) onCloseRef.current();
        backdropPressRef.current = false;
      }}
    >
      {/* biome-ignore lint/a11y/useKeyWithClickEvents: dialog body stops bubbling so backdrop close is not triggered accidentally; not interactive itself */}
      <div
        ref={dialogRef}
        role="dialog"
        aria-modal="true"
        aria-label={copy.title}
        onClick={e => e.stopPropagation()}
        className="w-[380px] max-w-[calc(100vw-48px)] overflow-hidden rounded-[16px] shadow-2xl bg-[rgba(250,250,250,.96)] dark:bg-[rgba(44,44,46,.96)] text-[#000] dark:text-[#F2F2F7]"
        style={{ fontFamily: '-apple-system, BlinkMacSystemFont, "SF Pro Text", "PingFang SC", "Microsoft YaHei", sans-serif' }}
      >
        <div className="px-4 pt-4 pb-2 flex items-center justify-between gap-2">
          <div className="text-[15px] font-semibold truncate">{copy.title}</div>
          <button
            type="button"
            title={t.cpCancel}
            disabled={busy}
            onClick={onClose}
            className="w-8 h-8 shrink-0 rounded-full flex items-center justify-center text-[#5F6368] hover:bg-[#D3D7DB] dark:text-[#C4C7C5] dark:hover:bg-[#444746] disabled:opacity-50"
          >
            <X size={16} />
          </button>
        </div>
        {/* Keyed on the UNFILTERED row count: with a >6-row list the search
            field must stay mounted while a query filters the body down —
            otherwise typing past the threshold unmounts the input with the
            query still set, leaving a filtered list with no way to clear it
            and dropping focus to body (review #484 M5). Hidden while the
            excluded-folder panel replaces the list: a query cannot act on it
            (review #484 m4). */}
        {!excludedFolder && !webOnly && (Array.isArray(rows) ? rows.length : 0) > 6 && (
          <div className="px-4 pb-2">
            <div className="flex h-9 items-center gap-2 rounded-full px-3 bg-[#EAECEF] dark:bg-[#303134]">
              <Search size={14} className="shrink-0 text-[#5F6368] dark:text-[#9AA0A6]" />
              {/* biome-ignore lint/a11y/noAutofocus: modal opens for a single purpose; focus belongs in the filter field immediately */}
              <input autoFocus
                value={query}
                onChange={e => setQuery(e.target.value)}
                placeholder={copy.searchPlaceholder}
                className="w-full bg-transparent border-0 outline-none text-[14px] placeholder:text-[#8A8F94] dark:placeholder:text-[#9AA0A6]"
              />
            </div>
          </div>
        )}
        {excludedFolder ? (
          /* The browse channel hit the exclusion list (§3): say so honestly + still allow starting a plain folder session. */
          <div className="px-4 pb-4 pt-1">
            <div className="rounded-2xl bg-[#EAECEF] dark:bg-[#303134] px-3.5 py-3">
              <div className="text-[13px] font-semibold mb-1">{copy.excludedTitle}</div>
              <div className="text-[12px] text-[#5F6368] dark:text-[#C4C7C5] mb-3 break-all">
                {copy.excludedBody(workspaceName(excludedFolder, copy.unknownDirectory))}
              </div>
              <div className="flex gap-2">
                <button
                  type="button"
                  disabled={busy}
                  onClick={() => !busy && onBrowseExcluded(excludedFolder)}
                  className="flex-1 h-9 rounded-full bg-[#0B57D0] text-white text-[13px] font-medium hover:bg-[#0A4CB8] disabled:opacity-50"
                >
                  {copy.excludedProceed}
                </button>
                <button
                  type="button"
                  disabled={busy}
                  onClick={onDismissExcluded}
                  className="flex-1 h-9 rounded-full bg-[#D3D7DB] dark:bg-[#444746] text-[#1F1F1F] dark:text-[#E3E3E3] text-[13px] font-medium hover:opacity-90 disabled:opacity-50"
                >
                  {t.cpCancel}
                </button>
              </div>
            </div>
          </div>
        ) : (
          <div className="px-2 pb-3 max-h-[340px] overflow-y-auto">
            {!webOnly && filtered.length === 0 && (
              <div className="px-3.5 py-4 text-[13px] text-[#8A8F94] dark:text-[#9AA0A6]">
                {(Array.isArray(rows) ? rows : []).length > 0 ? copy.noMatch : copy.empty}
              </div>
            )}
            {!webOnly && filtered.map(projectRow)}
            <button
              type="button"
              disabled={busy}
              onClick={() => !busy && onTemporary()}
              className={`${rowCls} disabled:opacity-60`}
            >
              <Sparkles size={15} className="shrink-0 text-emerald-500" />
              <span className="min-w-0 flex-1">
                <span className="block truncate">{copy.temporary}</span>
                <span className="block truncate text-[12px] text-[#8A8F94] dark:text-[#9AA0A6]">{copy.temporaryDesc}</span>
              </span>
            </button>
            {!webOnly && (
              <button
                type="button"
                disabled={busy}
                onClick={() => !busy && onBrowse()}
                className={`${rowCls} disabled:opacity-60`}
              >
                <FolderOpen size={15} className="shrink-0 text-[#5F6368] dark:text-[#9AA0A6]" />
                <span className="min-w-0 flex-1">
                  <span className="block truncate">{copy.browse}</span>
                  <span className="block truncate text-[12px] text-[#8A8F94] dark:text-[#9AA0A6]">{copy.browseDesc}</span>
                  {/* Grant notice parity (§9.4): the browse channel grants the
                      picked folder (exactly one root), same notice weight as
                      the project rows. */}
                  <span className="block truncate text-[11px] text-[#8A8F94] dark:text-[#9AA0A6]">
                    {rowNotice(1)}
                  </span>
                </span>
              </button>
            )}
          </div>
        )}
      </div>
    </div>,
    document.body,
  );
};

export { WorkspacePickerDialog };
