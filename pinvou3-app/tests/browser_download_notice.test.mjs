import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import { browserEn, browserJa, browserZh } from '../src/shared/i18n/browser.js';
import {
  DOWNLOAD_NOT_RETRYABLE,
  downloadFinishedNotice,
  downloadNoticeCanDiscard,
  downloadNoticeCanSaveAs,
  downloadNoticesReducer,
  downloadStartedNotice,
  latestDownloadNotice,
} from '../src/features/browser/download-notice.mjs';

const projectRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const browserView = readFileSync(
  path.join(projectRoot, 'src', 'features', 'browser', 'BrowserView.jsx'),
  'utf8',
);

const DICTIONARIES = { zh: browserZh, en: browserEn, ja: browserJa };
const OUTCOMES = ['saved', 'cancelled', 'failed', 'unsaved'];

function finished(outcome, t = browserEn, downloadId = 'id1') {
  return downloadFinishedNotice(t, { downloadId, fileName: 'x.bin', outcome });
}

const started = (downloadId) => ({
  type: 'report',
  notice: downloadStartedNotice(browserEn, { downloadId, fileName: `${downloadId}.bin` }),
});
const reported = (outcome, downloadId) => ({ type: 'report', notice: finished(outcome, browserEn, downloadId) });

test('a started download tells the user to choose a place in the save dialog', () => {
  const notice = downloadStartedNotice(browserZh, { downloadId: 'id1', fileName: '报告.pdf' });
  assert.equal(notice.status, 'started');
  assert.equal(notice.downloadId, 'id1');
  assert.equal(notice.message, '正在下载「报告.pdf」。请在弹出的保存窗口中选择保存位置。');
  // Nothing is placed yet, so there is nothing to retry.
  assert.equal(downloadNoticeCanSaveAs(notice), false);
});

test('every reported outcome is described in all three languages', () => {
  for (const [language, dictionary] of Object.entries(DICTIONARIES)) {
    for (const outcome of OUTCOMES) {
      const notice = finished(outcome, dictionary);
      assert.equal(notice.status, outcome, `${language} ${outcome}`);
      assert.ok(notice.message.includes('x.bin'), `${language} ${outcome} must name the file`);
      assert.ok(!notice.message.includes('undefined'), `${language} ${outcome}`);
    }
  }
  // The unsaved notice asks for another place and names the button in copy
  // that exists in each language.
  assert.equal(
    finished('unsaved', browserZh).message,
    '「x.bin」已下载完成，但未能保存到所选位置。',
  );
  for (const [language, dictionary] of Object.entries(DICTIONARIES)) {
    assert.equal(typeof dictionary.browserDownloadSaveAs, 'string', `${language} button`);
    assert.ok(dictionary.browserDownloadSaveAs.length > 0, `${language} button`);
    // Giving up on a stranded download has its own copy in every language.
    assert.equal(typeof dictionary.browserDownloadDiscard, 'string', `${language} discard button`);
    assert.ok(dictionary.browserDownloadDiscard.length > 0, `${language} discard button`);
    // The live-cap refusal has its own copy in every language.
    assert.equal(typeof dictionary.browserDownloadBusy, 'function', `${language} limit notice`);
  }
});

test('only an unsaved download that is still identified offers another place', () => {
  for (const outcome of ['saved', 'cancelled', 'failed']) {
    assert.equal(downloadNoticeCanSaveAs(finished(outcome)), false, outcome);
    assert.equal(downloadNoticeCanDiscard(finished(outcome)), false, outcome);
  }
  assert.equal(downloadNoticeCanSaveAs(finished('unsaved')), true);
  // The discard affordance exists exactly where the retry does: both act on
  // the same staged file.
  assert.equal(downloadNoticeCanDiscard(finished('unsaved')), true);
  // Without an id the host could not find the staged transfer.
  assert.equal(
    downloadNoticeCanSaveAs(
      downloadFinishedNotice(browserEn, { fileName: 'x.bin', outcome: 'unsaved' }),
    ),
    false,
  );
  assert.equal(downloadNoticeCanSaveAs(null), false);
});

test('an unknown outcome or missing field never renders a broken notice', () => {
  assert.equal(finished('mystery'), null);
  assert.equal(downloadFinishedNotice(browserEn, { fileName: 'x.bin' }), null);
  assert.equal(downloadFinishedNotice(browserEn), null);
  const notice = downloadStartedNotice(browserEn, {});
  assert.equal(notice.downloadId, '');
  assert.equal(notice.message, 'Downloading “”. Choose where to save it in the Save dialog.');
});

test("one download's outcome cannot hide another that still waits for the user", () => {

  let state = downloadNoticesReducer(undefined, started('a'));
  state = downloadNoticesReducer(state, started('b'));
  assert.equal(latestDownloadNotice(state).downloadId, 'b');

  state = downloadNoticesReducer(state, reported('saved', 'a'));
  assert.equal(latestDownloadNotice(state).status, 'saved');
  // B's transfer was never replaced by A's outcome.
  assert.equal(state.b.status, 'started');

  state = downloadNoticesReducer(state, { type: 'dismiss', downloadId: 'a' });
  assert.equal(latestDownloadNotice(state).downloadId, 'b');

  // An update makes that download the latest notice again.
  state = downloadNoticesReducer(state, started('c'));
  state = downloadNoticesReducer(state, reported('cancelled', 'b'));
  assert.equal(latestDownloadNotice(state).downloadId, 'b');
  assert.deepEqual(Object.keys(state), ['c', 'b']);

  state = downloadNoticesReducer(state, { type: 'dismiss', downloadId: 'b' });
  state = downloadNoticesReducer(state, { type: 'dismiss', downloadId: 'c' });
  assert.equal(latestDownloadNotice(state), null);
});

test('an unsaved download keeps the notice slot until it is dismissed', () => {

  let state = downloadNoticesReducer(undefined, started('a'));
  state = downloadNoticesReducer(state, reported('unsaved', 'a'));
  // Later download events do not bury the transfer that still needs a place:
  // its Save as… affordance would otherwise be lost until the next start.
  state = downloadNoticesReducer(state, started('b'));
  state = downloadNoticesReducer(state, reported('saved', 'b'));
  assert.equal(latestDownloadNotice(state).downloadId, 'a');
  // Dismissing it returns the slot to the newest notice.
  state = downloadNoticesReducer(state, { type: 'dismiss', downloadId: 'a' });
  assert.equal(latestDownloadNotice(state).downloadId, 'b');
});

test('malformed reports and dismissals change nothing, and a stop clears every notice', () => {
  const notice = downloadStartedNotice(browserEn, { fileName: 'x.bin' });
  const state = downloadNoticesReducer({}, { type: 'report', notice });
  // A report without an id shares one slot instead of spreading keys.
  assert.deepEqual(Object.keys(state), ['unknown']);

  assert.equal(downloadNoticesReducer(state, { type: 'report' }), state);
  assert.equal(downloadNoticesReducer(state, { type: 'dismiss', downloadId: 'missing' }), state);
  assert.equal(downloadNoticesReducer(state), state);
  assert.deepEqual(downloadNoticesReducer(undefined, { type: 'reset' }), {});
  assert.deepEqual(downloadNoticesReducer(state, { type: 'reset' }), {});
});

test('BrowserView renders the notice from copy and wires the retry to the host', () => {
  // The retry is only offered for an unsaved transfer and always sends its id.
  assert.match(
    browserView,
    /if \(!downloadNoticeCanSaveAs\(downloadNotice\)\) return;/,
  );
  assert.match(
    browserView,
    /invokeTauri\('browser_download_save_as', \{ downloadId: downloadNotice\.downloadId \}\)/,
  );
  // Giving up is offered for the same transfer and sends its id too.
  assert.match(
    browserView,
    /if \(!downloadNoticeCanDiscard\(downloadNotice\)\) return;/,
  );
  assert.match(
    browserView,
    /invokeTauri\('browser_download_discard', \{ downloadId: downloadNotice\.downloadId \}\)/,
  );
  assert.match(browserView, /data-testid="browser-download-discard"/);
  assert.match(browserView, /\{t\.browserDownloadDiscard\}/);
  assert.match(
    browserView,
    /\{t\.browserDownloadSaveAs\}/,
  );
  // A failed retry reports through the shared error banner, keyed by the
  // shared wire constant rather than a copy of the string.
  assert.match(
    browserView,
    /String\(e\)\.includes\(DOWNLOAD_NOT_RETRYABLE\) \? t\.browserDownloadNotRetryable : e,/,
  );
  // An `unsaved` notice renders no dismiss button: dismissing it would
  // orphan the record with no way back to its Save as / Discard actions.
  assert.match(
    browserView,
    /\{downloadNotice\.status !== 'unsaved' && \(\s*<button\s+type="button"\s+data-testid="browser-download-notice-dismiss"/,
  );
  // Mount-time backfill re-reports the backend's live `unsaved` records once
  // the listeners have settled, and keeps only this session's records — the
  // same scoping the listeners enforce, so another session's download never
  // renders here with actionable buttons.
  assert.match(browserView, /invokeTauri\('browser_download_pending'\)\.then/);
  assert.match(
    browserView,
    /\.filter\(\(payload\) => payload\?\.sessionId === sessionId\)\s*\.map\(\(payload\) => downloadFinishedNotice\(t, payload\)\)/,
  );
  assert.match(browserView, /type: 'backfill', notices/);
  // `limit` and `unavailable` keep their dedicated copy; `staging` keeps the
  // staging copy; an unknown future reason gets the generic one instead of a
  // wrong staging claim.
  assert.match(
    browserView,
    /payload\.reason === 'limit'[\s\S]{0,200}?payload\.reason === 'unavailable'[\s\S]{0,200}?payload\.reason === 'staging'[\s\S]{0,200}?: t\.browserDownloadBlockedGeneric\(payload\.source \|\| ''\)\);/,
  );
});

test('the not-retryable wire code is exported once and described in every language', () => {
  assert.equal(DOWNLOAD_NOT_RETRYABLE, 'download_not_retryable');
  for (const [language, dictionary] of Object.entries(DICTIONARIES)) {
    assert.equal(typeof dictionary.browserDownloadNotRetryable, 'string', language);
    assert.ok(dictionary.browserDownloadNotRetryable.length > 0, language);
    // The generic blocked copy exists everywhere for unknown future reasons.
    assert.equal(typeof dictionary.browserDownloadBlockedGeneric, 'function', language);
    assert.ok(!dictionary.browserDownloadBlockedGeneric('').includes('undefined'), language);
  }
});

test('the unavailable-service and out-of-flow copies exist in every language', () => {
  for (const [language, dictionary] of Object.entries(DICTIONARIES)) {
    // The degraded-service refusal is a distinct copy, not a fallback.
    assert.equal(typeof dictionary.browserDownloadUnavailable, 'function', language);
    const unavailable = dictionary.browserDownloadUnavailable('site.invalid');
    assert.ok(unavailable.includes('site.invalid'), language);
    assert.ok(!unavailable.includes('undefined'), language);
    // The staging-refusal copy names a temporary file, not the service.
    const staging = dictionary.browserDownloadBlocked('site.invalid');
    assert.ok(staging.includes('site.invalid'), language);
    assert.ok(!staging.includes('undefined'), language);
    // The started notice names the file in every language, so a dropped key
    // cannot surface only at runtime inside the listener.
    const startedNotice = downloadStartedNotice(dictionary, { downloadId: 'k', fileName: 'x.bin' });
    assert.ok(startedNotice.message.includes('x.bin'), language);
    assert.ok(!startedNotice.message.includes('undefined'), language);
  }
});

test('a mount-time backfill fills only the notices this session does not know', () => {
  // An `unsaved` notice cannot be dismissed (the UI hides the X), so a key
  // missing from the state always means "never seen here" and is filled.
  // Records this session already knows keep their fresher local entry.
  let state = downloadNoticesReducer(undefined, reported('cancelled', 'a'));
  const snapshot = [finished('unsaved', browserEn, 'b')];
  state = downloadNoticesReducer(state, { type: 'backfill', notices: snapshot });
  assert.equal(state.a.status, 'cancelled', 'known records keep their local entry');
  assert.equal(state.b.status, 'unsaved', 'unseen records are filled');
  assert.equal(latestDownloadNotice(state).status, 'unsaved');

  // A record that reported after the query ran keeps its fresher entry.
  state = downloadNoticesReducer(undefined, reported('cancelled', 'x'));
  state = downloadNoticesReducer(state, { type: 'backfill', notices: [finished('unsaved', browserEn, 'x')] });
  assert.equal(state.x.status, 'cancelled');

  // Empty, malformed, and all-known snapshots change nothing (same reference).
  const known = downloadNoticesReducer(undefined, reported('unsaved', 'k'));
  assert.equal(downloadNoticesReducer(known, { type: 'backfill', notices: [] }), known);
  assert.equal(downloadNoticesReducer(known, { type: 'backfill' }), known);
  assert.equal(
    downloadNoticesReducer(known, { type: 'backfill', notices: [finished('unsaved', browserEn, 'k')] }),
    known,
  );
  assert.deepEqual(downloadNoticesReducer(undefined, { type: 'backfill', notices: [] }), {});
});

test('the notice history stays bounded so one map entry per download cannot accumulate', () => {
  let state;
  for (let index = 0; index < 80; index += 1) {
    state = downloadNoticesReducer(state, started(`d${index}`));
  }
  // The exact bound is pinned: a silently tightened bound would hide notices
  // the UI promises to keep.
  assert.equal(Object.keys(state).length, 64);
  // The most recent download survives trimming.
  assert.ok('d79' in state);
  // Updating a notice re-inserts it, so trimming never drops the freshest.
  state = downloadNoticesReducer(state, reported('saved', 'd40'));
  assert.ok('d40' in state);
  assert.equal(latestDownloadNotice(state).downloadId, 'd40');
});

test('trimming never drops an unsaved notice that still waits for the user', () => {
  // The unsaved record holds a live download slot and its staged file; its
  // notice is the only route to the Save as / Discard actions, so it must
  // survive an otherwise full history while newer reports stream in.
  let state = downloadNoticesReducer(undefined, reported('unsaved', 'held'));
  for (let index = 0; index < 80; index += 1) {
    state = downloadNoticesReducer(state, started(`d${index}`));
  }
  assert.ok('held' in state, 'the unsaved notice survives trimming');
  assert.ok(Object.keys(state).length <= 65, 'the rest stays bounded');
  assert.equal(latestDownloadNotice(state).downloadId, 'held');
  // Once it leaves `unsaved`, it trims like any other notice.
  state = downloadNoticesReducer(state, reported('cancelled', 'held'));
  for (let index = 80; index < 160; index += 1) {
    state = downloadNoticesReducer(state, started(`d${index}`));
  }
  assert.ok(!('held' in state), 'the resolved notice trims away');
  assert.equal(Object.keys(state).length, 64);
});
