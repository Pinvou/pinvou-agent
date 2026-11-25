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
      // Re-insert so the most recently updated download is shown last.
      const { [key]: _replaced, ...rest } = state || {};
      return { ...rest, [key]: notice };
    }
    case 'dismiss': {
      const key = action.downloadId || UNKNOWN_DOWNLOAD;
      if (!(key in (state || {}))) return state || {};
      const { [key]: _dismissed, ...rest } = state;
      return rest;
    }
    case 'reset':
      return {};
    default:
      return state || {};
  }
}

// A finished-but-unplaced transfer needs the user's action, so it keeps the
// single notice slot even when later download events arrive; the staged file
// is otherwise swept at the next start with its Save as… affordance lost.
// Everything else shows the most recently updated download.
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
