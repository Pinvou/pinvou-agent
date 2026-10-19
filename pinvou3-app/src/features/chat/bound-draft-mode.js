// Grant-notice mode for the chat lane's workspace surfaces (review #484
// round-11 M6; deduplicated round-47 M5 — ChatView's boundDraftNoticeMode
// and main.jsx's chatLaneGrantMode were body-for-body twins): the notice
// must describe the posture the draft takes ONCE the pick binds it — the
// bridge resolves a bound draft to the code lane's default (fallback plan),
// matching the composer chip that flips to exactly that mode when the pick
// lands. The render-time snapshot still shows the unbound draft's mode.
// Older hosts without the query keep the snapshot read.
export function boundDraftNoticeMode(bridge, bs) {
  if (bridge.sessions && typeof bridge.sessions.boundDraftMode === 'function') {
    return bridge.sessions.boundDraftMode();
  }
  return (bs && bs.modeState && bs.modeState.mode) || null;
}
