// Codex-lane picker request lifecycle (review #484 M1, completed round-32
// MAJOR 1). The picker result is a host-held request object ({ epoch, path,
// projectId, roots }) delivered to CodexAcpView as a prop. Three clears
// complete the lifecycle: (1) the view consumes a request once per mount
// (epoch ref) and acknowledges, so the host clears the object — otherwise a
// remount (the epoch ref resets to 0) replays the stale request and hijacks
// the view back into the old draft; (2) picker dismissal or a chat-lane
// stage invalidates a lingering unconsumed request; (3) the host clears the
// request whenever the view leaves codex, covering the mount-never-happened
// window (navigate away during the lazy chunk load: no ack ever fires, and
// without this clear the NEXT codex entry replays the stale pick into a
// fresh draft). No UI dependencies; node-side unit testable.

// View side: returns the request when it is new for this mount (not yet
// consumed here), null otherwise. Callers record `request.epoch` as the
// last-consumed epoch and then notify the host.
export function consumePickerRequest(request, lastConsumedEpoch) {
  if (!request || request.epoch === lastConsumedEpoch) return null;
  return request;
}
