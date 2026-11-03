// Pure UI logic for plain-chat "bound-workspace sessions" (shared by ChatView
// and the tests, following the extraction pattern of
// features/codex/code-permission-state.js): sessions/drafts share the code
// mode's safety posture — a one-shot confirmation gate before switching to
// YOLO and a bound-directory indicator next to the composer. The gate
// decision itself reuses the codex side's needsYoloConfirmation (same backend
// source of truth, get_code_permission_prefs). Round-7 review P2: the gate is
// entry-scoped — every Yolo entry consults the prefs, bound or not (the
// codex lane already did); the binding-scoped helper was removed and the
// adjudication lives inline in ChatView's chatYoloGateAllows.

/// "Unknown" sentinel for gate-scoped binding resolution: when the binding
/// query fails transiently (not an old backend missing the command), ChatView
/// returns this non-empty value so the gate fails closed with one extra
/// confirmation instead of silently skipping a bound session. Participates in
/// truthiness checks only; never displayed or cached.
export const CHAT_YOLO_GATE_UNKNOWN_BINDING = '(binding-query-failed)';

/// Display condition for the bound-directory chip next to the composer: an
/// active session with a queried binding path only (query failure, no binding,
/// or the web stub's null never shows the chip).
export function shouldShowWorkspaceBindingChip({ activeSessionId, sessionBinding }) {
  return !!activeSessionId && !!sessionBinding;
}
