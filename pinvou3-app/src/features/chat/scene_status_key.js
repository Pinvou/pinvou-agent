// Round-31 m5 / round-32 minor 10 (review #455): the FULL scene-status
// identity — `session:draftEpoch`. Two drafts both carry a `null` session id,
// so the epoch is what keeps a send in flight from draft D1 from painting its
// banner or ready toast into fresh draft D2 (a new-chat click bumps the epoch
// while the session id stays null→null). Extracted pure for direct node
// testing; ChatView's banner guard, ready-toast guard, and welcome-card reset
// all build the same key through this helper so the three sites can never
// drift apart.
export const sceneStatusKey = (sessionId, draftEpoch) =>
  `${sessionId || 'draft'}:${draftEpoch}`;
