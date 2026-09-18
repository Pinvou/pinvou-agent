// Pure UI logic for plain-chat "bound-workspace sessions" (shared by ChatView
// and the tests, following the extraction pattern of
// features/codex/code-permission-state.js): bound sessions/drafts share the
// code mode's safety posture — a one-shot confirmation gate before switching
// to YOLO and a bound-directory indicator next to the composer. The gate
// decision itself reuses the codex side's needsYoloConfirmation (same backend
// source of truth, get_code_permission_prefs).

/// "Unknown" sentinel for gate-scoped binding resolution: when the binding
/// query fails transiently (not an old backend missing the command), ChatView
/// returns this non-empty value so the gate fails closed with one extra
/// confirmation instead of silently skipping a bound session. Participates in
/// truthiness checks only; never displayed or cached.
export const CHAT_YOLO_GATE_UNKNOWN_BINDING = '(binding-query-failed)';

/// Whether the confirmation-gate check is needed before switching to YOLO:
/// materialized sessions look at their directory binding, drafts at
/// draftWorkspacePath. Returning true only means "prefs must be consulted";
/// whether the card actually shows is decided by needsYoloConfirmation(prefs)
/// (not shown when yolo_confirmed=true was already recorded).
export function chatYoloGateApplies({ activeSessionId, sessionBinding, draftWorkspacePath }) {
  return activeSessionId ? !!sessionBinding : !!draftWorkspacePath;
}

/// Display condition for the bound-directory chip next to the composer: an
/// active session with a queried binding path only (query failure, no binding,
/// or the web stub's null never shows the chip).
export function shouldShowWorkspaceBindingChip({ activeSessionId, sessionBinding }) {
  return !!activeSessionId && !!sessionBinding;
}
