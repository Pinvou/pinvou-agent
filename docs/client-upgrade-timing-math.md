# Client upgrade timing arithmetic

T26 batch 1 provides a private, unpublished, dependency-free Rust crate at
`pinvou3-app/src-tauri/src/features/updater/timing/math`. The updater facade links
the same crate into the actual App graph. This batch implements deterministic
deadline and original-budget arithmetic; it supplies no clock source, protected
storage, cleanup policy or execution authority.

## Inputs and results

Milliseconds are whole, finite values in the frozen protocol's safe integer range
`0..=9007199254740991`. Negative zero is zero. Budget durations must be positive.
Addition rejects overflow instead of truncating or saturating. A deadline is
immutable and an assessment is either a positive remaining duration, reached, or
unknown with a distinct unproven-clock, confirmed-component-fault or backwards-
observation reason. Unknown has no estimated remaining duration.

`ClockProjection::Known` is an owner-supplied claim: arithmetic does not establish
its provenance. Stateless deadline comparison requires externally proven source
continuity. Budget records additionally refuse observations earlier than their
last known value. Unknown and fault observations preserve the original origin,
deadline and known watermark. Cloning preserves fields in memory; it proves no
durable persistence or recovery across restart or power loss.

## Separate original windows

- Coordination/freeze and backup preparations have separate origins and qualified
  positive durations. Each captures the original active session expiry. Its
  effective deadline is the earlier of the original budget and session cutoff.
  Later restrictions can only tighten that cutoff; a refreshed session cannot
  extend or revive this record. Authentication and session/scope binding remain
  the protected owner's responsibility.
- Full-artifact validation accepts an actual qualified duration up to 30 minutes.
  Preinstall, install and activate each accept a separate actual qualified
  duration up to two hours. These limits do not certify a target's budget and
  provide no default two-hour execution window.
- Consume recovery has a fixed five-minute window from the first consume
  transmission. ACK loss, status queries and retries cannot change this object's
  origin. Authorization expiry is a separate constraint owned by the transaction.

There is no download duration type or total/cumulative download deadline. Health
attempt and total-window semantics are a subsequent batch; generic `Reached`
does not classify a health result as failure.

## Verification and remaining integration

Eight focused test groups cover protocol numbers, overflow, equality boundaries,
independent preparation origins, session tightening, stage caps, backwards and
unknown observations, and the fixed original consume window. Two App controls
check linkage and preservation of the preparation/session boundary. CI runs the
standalone locked tests and Clippy even when heavy App checks are skipped; the
routing regression covers source, tests, manifest and lockfile changes.

Standalone fmt, Clippy with warnings denied and all eight groups passed. The
45 CI policy tests, complete locked App/CLI metadata, App formatting, architecture,
version consistency, workflow YAML and fast fork guard passed. Actual Windows App
integration passed all seven updater tests, including the two timing controls
(0.00s after a 31m59s build with the existing compiler stack wrapper). Each App/CLI
lock adds only the local dependency-free package and its App edge. No App-wide
test pass is claimed; the earlier unrelated Windows limitations remain documented
in [the byte codec evidence](client-upgrade-byte-codec.md).
Both independent roles gave final B1 scoped PASS after reviewing the frozen source,
staged scope and actual App evidence. No mandatory arithmetic finding remains.

Protected clock collection and continuity/epoch evidence, durable ledger ownership,
actual platform/off-power qualification, retention cleanup and gate transitions
remain unfinished. No OS target or trusted-time provider is certified by these
arithmetic tests. Deployment remains deferred.
