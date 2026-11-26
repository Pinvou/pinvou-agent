// Download notices for the embedded browser.
//
// The host reports a page download by file name and outcome only: the URL and
// every local path stay in Rust. `unsaved` means the transfer finished but
// could not be placed at the chosen path, so its staged file waits for the
// user to pick another location.

const OUTCOME_NOTICES = Object.freeze({
  saved: 'browserDownloadSaved',
  cancelled: 'browserDownloadCancelled',
  failed: 'browserDownloadFailed',
  unsaved: 'browserDownloadUnsaved',
});

const UNKNOWN_DOWNLOAD = 'unknown';

// The backend rejects a retry with this bare code (see `ask_again` in
// downloads.rs); the shared constant pins both sides of the wire string.
export const DOWNLOAD_NOT_RETRYABLE = 'download_not_retryable';

// Longest notice history kept: every download id would otherwise leave one
// entry behind for the process lifetime, and the whole map is re-read on
// each render to pick the visible notice.
const MAX_DOWNLOAD_NOTICES = 64;

export function downloadStartedNotice(t, payload) {
  return {
    status: 'started',
    downloadId: downloadIdOf(payload),
    message: t.browserDownloadStarted(downloadFileName(payload)),
  };
}

// Returns null for an outcome this UI cannot describe.
export function downloadFinishedNotice(t, payload) {
  const status = payload?.outcome;
  const message = OUTCOME_NOTICES[status];
  if (!message) return null;
  return {
    status,
    downloadId: downloadIdOf(payload),
    message: t[message](downloadFileName(payload)),
  };
}

// Only a finished-but-unplaced transfer still has a staged file to save.
export function downloadNoticeCanSaveAs(notice) {
  return notice?.status === 'unsaved' && !!notice.downloadId;
}

// The same transfer can also be given up on: its staged file is deleted and
// the notice reads as cancelled. This is the only way out of an `unsaved`
// state that the user does not want to retry.
export function downloadNoticeCanDiscard(notice) {
  return downloadNoticeCanSaveAs(notice);
}

// Notices are keyed by download so that one transfer's outcome cannot hide
// another transfer that is still waiting for the user.
export function downloadNoticesReducer(state, action) {
  switch (action?.type) {
    case 'report': {
      const notice = action.notice;
      if (!notice) return state || {};
      const key = notice.downloadId || UNKNOWN_DOWNLOAD;
      // Copy, remove, and re-insert so the most recently updated download is
      // shown last.
      const next = { ...state };
      delete next[key];
      next[key] = notice;
      return trimNotices(next);
    }
    // Mount-time backfill of the backend's live `unsaved` records. An id the
    // UI already knows keeps its (fresher) local entry, so a stale snapshot
    // can never resurrect a record the user already dismissed or that
    // reported after the query ran.
    case 'backfill': {
      const notices = Array.isArray(action.notices) ? action.notices : [];
      let next = state || {};
      let changed = false;
      for (const notice of notices) {
        if (!notice) continue;
        const key = notice.downloadId || UNKNOWN_DOWNLOAD;
        if (key in next) continue;
        next = { ...next, [key]: notice };
        changed = true;
      }
      return changed ? next : state || {};
    }
    case 'dismiss': {
      const key = action.downloadId || UNKNOWN_DOWNLOAD;
      if (!(key in (state || {}))) return state || {};
      const next = { ...state };
      delete next[key];
      return next;
    }
    case 'reset':
      return {};
    default:
      return state || {};
  }
}

// Keeps the newest MAX_DOWNLOAD_NOTICES entries; key order is insertion
// order, and `report` re-inserts on every update, so the oldest entries are
// the least recently updated ones.
function trimNotices(notices) {
  const keys = Object.keys(notices);
  if (keys.length <= MAX_DOWNLOAD_NOTICES) return notices;
  const trimmed = { ...notices };
  for (const key of keys.slice(0, keys.length - MAX_DOWNLOAD_NOTICES)) {
    delete trimmed[key];
  }
  return trimmed;
}

// A finished-but-unplaced transfer needs the user's action, so it keeps the
// single notice slot even when later download events arrive. Until it is
// saved or discarded, its record keeps holding one of the live download
// slots, and its staged file is lost when the app closes. Everything else
// shows the most recently updated download.
export function latestDownloadNotice(state) {
  const notices = Object.values(state || {});
  if (!notices.length) return null;
  const actionable = notices.filter((notice) => notice.status === 'unsaved');
  return actionable.length
    ? actionable[actionable.length - 1]
    : notices[notices.length - 1];
}

function downloadIdOf(payload) {
  return typeof payload?.downloadId === 'string' ? payload.downloadId : '';
}

function downloadFileName(payload) {
  return typeof payload?.fileName === 'string' ? payload.fileName : '';
}
