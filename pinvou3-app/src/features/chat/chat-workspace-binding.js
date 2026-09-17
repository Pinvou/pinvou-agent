// Pure UI logic for normal chat's "working-directory-bound session"
// (shared by ChatView and tests, mirroring the extraction pattern of
// features/codex/code-permission-state.js):
// bound sessions/drafts align their security posture with code mode — a
// one-time confirm gate before switching to YOLO and a bound-directory
// indicator beside the composer. The gate's own decision reuses the codex
// side's needsYoloConfirmation (same backend source of truth,
// get_code_permission_prefs).

/// "Unknown" sentinel for the gate's binding resolution: when the binding
/// query fails transiently (not an old backend missing the command), ChatView
/// returns this non-empty value so the confirm gate fails closed and
/// over-confirms once — better than silently skipping an actually bound
/// session (review #445 R3). Only used for truthiness; never displayed or
/// cached.
export const CHAT_YOLO_GATE_UNKNOWN_BINDING = '(binding-query-failed)';

/// Whether switching to YOLO needs the confirm-gate check: materialized
/// sessions look at their directory binding, drafts at draftWorkspacePath.
/// Returning true only means "prefs must be consulted"; whether the card
/// actually shows is decided by needsYoloConfirmation(prefs) (a previously
/// confirmed yolo_confirmed=true does not show it).
export function chatYoloGateApplies({ activeSessionId, sessionBinding, draftWorkspacePath }) {
  return activeSessionId ? !!sessionBinding : !!draftWorkspacePath;
}

/// Display condition for the bound-directory chip beside the composer: only
/// for the active session with a resolved binding path (query failure / no
/// binding / Web stub returning null all hide it).
export function shouldShowWorkspaceBindingChip({ activeSessionId, sessionBinding }) {
  return !!activeSessionId && !!sessionBinding;
}
