// Codex-lane picker request lifecycle (review #484 M1). The picker result is
// a host-held request object ({ epoch, path, projectId, roots }) delivered to
// CodexAcpView as a prop. The view consumes a request only once per mount
// (epoch ref) and must then acknowledge consumption so the host clears the
// object — otherwise a remount (the epoch ref resets to 0) replays the stale
// request and hijacks the view back into the old draft. No UI dependencies;
// node-side unit testable.

// View side: returns the request when it is new for this mount (not yet
// consumed here), null otherwise. Callers record `request.epoch` as the
// last-consumed epoch and then notify the host.
export function consumePickerRequest(request, lastConsumedEpoch) {
  if (!request || request.epoch === lastConsumedEpoch) return null;
  return request;
}
