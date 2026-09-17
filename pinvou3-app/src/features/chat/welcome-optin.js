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

export { consumeWelcomeOptIn, resolveSendCapabilityStatus };
