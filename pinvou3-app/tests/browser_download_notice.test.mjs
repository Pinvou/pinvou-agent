import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import { browserEn, browserJa, browserZh } from '../src/shared/i18n/browser.js';
import {
  downloadFinishedNotice,
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
  }
});

test('only an unsaved download that is still identified offers another place', () => {
  for (const outcome of ['saved', 'cancelled', 'failed']) {
    assert.equal(downloadNoticeCanSaveAs(finished(outcome)), false, outcome);
  }
  assert.equal(downloadNoticeCanSaveAs(finished('unsaved')), true);
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
  const started = (downloadId) => ({
    type: 'report',
    notice: downloadStartedNotice(browserEn, { downloadId, fileName: `${downloadId}.bin` }),
  });
  const reported = (outcome, downloadId) => ({ type: 'report', notice: finished(outcome, browserEn, downloadId) });

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
  assert.match(
    browserView,
    /\{t\.browserDownloadSaveAs\}/,
  );
  // A failed retry reports through the shared error banner.
  assert.match(
    browserView,
    /invokeTauri\('browser_download_save_as'[\s\S]{0,160}catch \(e\) \{\s*reportErrorForOperation\(operationEpoch, e\);/,
  );
});
