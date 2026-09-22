// Pure classification of a `rebind_workspace_root` failure into the state the
// rebind dialog renders.
//
// The backend signals every user-reachable outcome with a stable ASCII marker
// prefix and never expects the frontend to match human copy (review #463
// finding 11 / Minor 7 / round-8 M4). This module is the single place that
// maps those prefixes, extracted so both halves of the contract —
// classification and the trilingual copy each marker resolves to — are
// unit-testable without mounting the app shell (review #463 round-8 minor 10);
// adding a backend marker without a mapping now fails that test.

const REBIND_OLD_ROOT_EXISTS = 'REBIND_OLD_ROOT_EXISTS';
const REBIND_IN_PROGRESS = 'REBIND_IN_PROGRESS';
const REBIND_SESSIONS_BUSY = 'REBIND_SESSIONS_BUSY';
const REBIND_TO_ROOT = 'REBIND_TO_ROOT';
const REBIND_TO_NESTED = 'REBIND_TO_NESTED';
const REBIND_TO_UNUSABLE = 'REBIND_TO_UNUSABLE';
const REBIND_ROOTS_CONFLICT = 'REBIND_ROOTS_CONFLICT';
// The root write failed for a non-conflict reason (disk full, permissions,
// newer on-disk schema). Distinct from the conflict marker so the copy does
// not send the user off to pick another destination, which cannot help
// (review #463 round-10 R2, restored by round-11 B2).
const REBIND_ROOTS_PERSIST = 'REBIND_ROOTS_PERSIST';
// An ACP runtime was starting up (its spawn holds the pool state lock), so the
// busy fence could not read whether the affected sessions are busy. Distinct
// from SESSIONS_BUSY, which carries the ids of sessions that ARE busy
// (review #463 round-10 T13, restored by round-11 B2).
const REBIND_RUNTIME_STARTING = 'REBIND_RUNTIME_STARTING';
// The legacy global table could not be synced, so the run aborted before any
// lane mutated (review #463 round-12 B1, made literally true by round-13 M1's
// upfront precheck): nothing was moved, and the copy must say so — "retry"
// is literally all the user needs to do.
const REBIND_LEGACY_TABLE_UNWRITABLE = 'REBIND_LEGACY_TABLE_UNWRITABLE';
// The legacy global table is on disk but this run's parse attempt failed
// (review #463 round-13 M2), so the run aborted before anything moved.
// Distinct from the writability marker: "make the folder writable" cannot
// fix a corrupt table — the user must repair or remove the file.
const REBIND_LEGACY_TABLE_CORRUPT = 'REBIND_LEGACY_TABLE_CORRUPT';

// Markers that resolve to a single trilingual `uiProjects` key. Busy and
// old-root-exists are handled separately below: the former carries a
// session-id list rendered verbatim as data, the latter escalates the dialog
// to its strong-confirmation state instead of showing an error.
const REBIND_MARKER_MESSAGE_KEYS = {
  [REBIND_IN_PROGRESS]: 'rebindInProgress',
  [REBIND_TO_ROOT]: 'rebindToRoot',
  [REBIND_TO_NESTED]: 'rebindToNested',
  [REBIND_TO_UNUSABLE]: 'rebindToUnusable',
  [REBIND_ROOTS_CONFLICT]: 'rebindRootsConflict',
  [REBIND_ROOTS_PERSIST]: 'rebindRootsPersist',
  [REBIND_RUNTIME_STARTING]: 'rebindRuntimeStarting',
  [REBIND_LEGACY_TABLE_UNWRITABLE]: 'rebindLegacyTableUnsynced',
  [REBIND_LEGACY_TABLE_CORRUPT]: 'rebindLegacyTableCorrupt',
};

// Suffix the backend appends to a roots-commit failure (review #463 round-14
// should-fix 1): the session lanes are already durable at `to` when the roots
// commit fails, but an Err carries no report, so the moved ids ride the error
// string — the dialog feeds them back as the post-busy carryover, and without
// them a refused eviction on the retry would be dropped and the dialog would
// close "up to date" with an old-cwd runtime still resident. The suffix is
// data, not copy: strip it from anything user-visible.
const REBOUND_IDS_SUFFIX = '\nrebound-session-ids:';

function splitReboundIdsSuffix(message) {
  const at = message.indexOf(REBOUND_IDS_SUFFIX);
  if (at < 0) return { message, reboundIds: [] };
  const ids = message.slice(at + REBOUND_IDS_SUFFIX.length).trim();
  return {
    message: message.slice(0, at),
    reboundIds: ids ? ids.split(/,\s*/) : [],
  };
}

// `null` when the failure carries no marker (an unmapped backend error, which
/// the dialog shows verbatim); otherwise the dialog state to apply.
function classifyRebindError(error, t) {
  const { message, reboundIds } = splitReboundIdsSuffix(String(error));
  if (message.startsWith(REBIND_OLD_ROOT_EXISTS)) {
    // Not an error state: the dialog switches to the strong warning and the
    // user confirms again.
    return { kind: 'old-root-exists', reboundIds };
  }
  if (message.startsWith(REBIND_SESSIONS_BUSY)) {
    const ids = message
      .slice(REBIND_SESSIONS_BUSY.length)
      .replace(/^:/, '')
      .trim();
    return {
      kind: 'sessions-busy',
      busySessionIds: ids ? ids.split(/,\s*/) : [],
      reboundIds,
    };
  }
  const marker = Object.keys(REBIND_MARKER_MESSAGE_KEYS).find((prefix) =>
    message.startsWith(prefix),
  );
  if (marker) {
    return { kind: 'copy', message: t.uiProjects[REBIND_MARKER_MESSAGE_KEYS[marker]], reboundIds };
  }
  return { kind: 'raw', message, reboundIds };
}

// Merge a roots-commit failure's moved ids into the post-busy carryover
// (review #463 round-15 Major 1): UNION + dedupe with the previous list —
// replacing would drop a still-busy carryover session from a prior run, and a
// converged to-lane session can never re-enter rebound_session_ids (the
// backend routes it to retry_evict_candidates only), so the final retry would
// close the dialog "up to date" while its old-cwd runtime stays resident and
// its next turn resurrects the vanished folder. The backend honors only the
// intersection with its own retry population, so the union cannot widen the
// eviction set.
function mergeRebindCarryoverIds(previousIds, reboundIds) {
  return [...new Set([...(previousIds || []), ...(reboundIds || [])])];
}

export {
  REBIND_MARKER_MESSAGE_KEYS,
  REBIND_OLD_ROOT_EXISTS,
  REBIND_SESSIONS_BUSY,
  classifyRebindError,
  mergeRebindCarryoverIds,
};
