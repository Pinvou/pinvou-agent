// Pre-send opt-in for the welcome card (review #455 R8-2 / R9 coverage note):
// the install path deliberately keeps the switch off (DenyAll convergence), so
// the first send while the welcome card is shown — sample-question click or
// free input — must first move the pack out of the plain disabled set before
// the model can receive the tool. Extracted as a pure module so it can be
// node-tested directly like scene-capabilities (the project has no React test
// infrastructure yet). Failure does not block the send (fail-visible: the
// caller shows a notice based on failed, and the tool's absence is visible in
// the reply); errors are never swallowed silently.

async function consumeWelcomeOptIn({ getToolId, consume, invoke }) {
  const toolId = getToolId && getToolId();
  if (!toolId) return { attempted: false };
  // One-shot consumption: regardless of the enable outcome, each welcome card
  // opts in only once (retry after failure is done explicitly by the user in
  // the tools list, not repeatedly re-attempted on the send path).
  if (consume) consume();
  try {
    // Explicit outcome shape (round-11 m11): blocked non-empty = the pack
    // sits in the user's explicit switch state and the backend enabled
    // nothing (round-10 Major 2): surface it so the caller can abort the send
    // with guidance instead of sending a degraded reply. An install-default
    // off lifts freely (round-11 B2) and returns enabled with empty blocked.
    const outcome = await invoke('enable_marketplace_packages', { packageIds: [toolId], scope: 'plain' });
    const blocked = Array.isArray(outcome && outcome.blocked) ? outcome.blocked : [];
    if (blocked.length) {
      return { attempted: true, blocked: [...blocked] };
    }
    // Round-13 m3: not_applied non-empty = the id matched nothing in the
    // DenyAll expansion (e.g. the install had not committed yet) — nothing
    // was enabled. Fail-visible like a rejected invoke instead of reporting
    // success for an opt-in that never happened.
    const notApplied = Array.isArray(outcome && outcome.not_applied) ? outcome.not_applied : [];
    if (notApplied.length) {
      return {
        attempted: true,
        failed: true,
        error: `not applied by backend: ${notApplied.join(', ')}`,
      };
    }
    return { attempted: true, failed: false };
  } catch (error) {
    return {
      attempted: true,
      failed: true,
      error: String((error && error.message) || error || ''),
    };
  }
}

// Final capability-status resolution for a send (round-10 Major 1): the
// scene block computes its status into a local; the welcome opt-in failure
// must not be clobbered by a later synchronous setSceneCapabilityStatus call
// (React batches them, only the last would render). Welcome failure wins over
// a ready/preparing scene status — fail-visible beats success copy; an
// aborted scene send surfaces its own error before this resolution runs.
function resolveSendCapabilityStatus({ welcomeFailed, welcomeText, sceneStatus }) {
  if (welcomeFailed) return { kind: 'error', text: welcomeText };
  return sceneStatus || null;
}

// Round-16 minor 13: a send arriving while the welcome opt-in's enable invoke
// is still in flight must await the SAME attempt, not start a second one (the
// tool id is already consumed, so a second attempt would no-op and the second
// send's own banner resolution would later clear the first send's failure
// notice). `attemptSlot` is the caller-owned ref-style holder ({ current } —
// a React useRef on the ChatView side); `run` must never reject
// (consumeWelcomeOptIn catches internally and returns a result object), so
// the raw promise is safe to store and share.
//
// Share condition: an in-flight attempt whose tool id matches, or whose id is
// null — a null ref while an attempt is in flight means that attempt consumed
// it (consume() clears the ref at attempt start), not that no card exists; a
// null ref with an empty slot never reaches this branch. A non-null id that
// differs from the in-flight one is a session switch: start a fresh attempt
// for the new pack (the backend's DISABLED_BUNDLES_FILE_LOCK serializes the
// two invokes).
async function runSharedWelcomeOptIn(attemptSlot, { toolId, run }) {
  const shared = attemptSlot.current;
  if (shared && (shared.toolId === toolId || toolId == null)) {
    return shared.promise;
  }
  const promise = Promise.resolve().then(run);
  attemptSlot.current = { toolId, promise };
  const clear = () => {
    if (attemptSlot.current && attemptSlot.current.promise === promise) {
      attemptSlot.current = null;
    }
  };
  promise.then(clear, clear);
  return promise;
}

export { consumeWelcomeOptIn, resolveSendCapabilityStatus, runSharedWelcomeOptIn };
