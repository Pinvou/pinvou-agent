import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

// Side-effect import: the chunked uploader is a classic script exposing
// itself as a global (same idiom as paste-image.js), so the test can derive
// the expected MB figure from the same MAX_FILE_BYTES constant the formatter
// injects into the localized copy.
import '../src/shared/chunked-file-upload.js';

import {
  ATTACHMENT_LIMIT_ERROR_CODES,
  attachmentLimitErrorCode,
  formatAttachmentLimitError,
} from '../src/features/attachments/attachment-limit-errors.js';
import { dictEn } from '../src/shared/i18n/en.js';
import { dictJa } from '../src/shared/i18n/ja.js';
import { dictZh } from '../src/shared/i18n/zh.js';

const { MAX_FILE_BYTES: ATTACHMENT_MAX_FILE_BYTES } = globalThis.PinvouChunkedFileUpload;
const MAX_FILE_MB = Math.round(ATTACHMENT_MAX_FILE_BYTES / (1024 * 1024));

test('normalizes browser and backend attachment limit failures', () => {
  const browserError = new Error('device upload exceeds limit');
  browserError.code = 'device_upload_too_large';
  assert.equal(
    attachmentLimitErrorCode(browserError),
    ATTACHMENT_LIMIT_ERROR_CODES.fileTooLarge,
  );

  for (const code of Object.values(ATTACHMENT_LIMIT_ERROR_CODES)) {
    assert.equal(attachmentLimitErrorCode(code), code);
    assert.equal(attachmentLimitErrorCode(new Error(code)), code);
  }
  assert.equal(attachmentLimitErrorCode('unrelated failure'), '');
});

test('renders every hard limit in all supported languages', () => {
  // The cap itself is still 20 MB — if MAX_FILE_BYTES moves, this fails until
  // the localized fileTooLarge copy and this pin are consciously re-checked.
  assert.equal(MAX_FILE_MB, 20, 'MAX_FILE_BYTES moved — re-check the localized fileTooLarge copy and this pin');
  for (const dictionary of [dictZh, dictEn, dictJa]) {
    // fileTooLarge must be an mb-function in every locale (r17): a static
    // "20 MB" string would silently lie once the shared MAX_FILE_BYTES cap
    // (pinned as a literal by the web_access contract test) moves. The
    // rendered figure below is matched boundary-aware against the value
    // COMPUTED from that constant, so a copy that bakes in a stale number
    // (e.g. "120 MB" satisfying a bare /20/) cannot pass.
    assert.equal(
      typeof dictionary.uiAttachments.fileTooLarge,
      'function',
      'uiAttachments.fileTooLarge must be an mb-function in every locale so the figure derives from the shared cap',
    );
    assert.match(
      formatAttachmentLimitError(ATTACHMENT_LIMIT_ERROR_CODES.fileTooLarge, dictionary.uiAttachments),
      new RegExp(`(^|\\D)${MAX_FILE_MB}(\\D|$)`),
    );
    // Track the backend cap (files/ingest_archive.rs MAX_ARCHIVE_ENTRIES,
    // raised 50→200 by the limits-audit batch — the r4 review found six user
    // copy strings still claiming 50). The number is matched exactly (not as
    // a bare /200/ substring, which a "1200 entries" copy regression would
    // also satisfy) — r5 review.
    assert.match(
      formatAttachmentLimitError(ATTACHMENT_LIMIT_ERROR_CODES.archiveTooManyEntries, dictionary.uiAttachments),
      /(^|\D)200(\D|$)/,
    );
    assert.match(
      formatAttachmentLimitError(ATTACHMENT_LIMIT_ERROR_CODES.archiveExpandedTooLarge, dictionary.uiAttachments),
      /100/,
    );
    assert.equal(
      formatAttachmentLimitError(ATTACHMENT_LIMIT_ERROR_CODES.archiveUnsafeEntry, dictionary.uiAttachments),
      dictionary.uiAttachments.archiveUnsafeEntry,
    );
    assert.ok(dictionary.uiAttachments.archiveUnsafeEntry);
    // The device cache codes introduced with the 50 MiB cap (issue #842)
    // must render trilingual copy on every chip path, not just the
    // upload/ingest system notices.
    for (const [code, key] of [
      [ATTACHMENT_LIMIT_ERROR_CODES.uploadCacheFull, 'deviceUploadCacheFull'],
      [ATTACHMENT_LIMIT_ERROR_CODES.parsedCacheTooLarge, 'deviceParsedTooLarge'],
      [ATTACHMENT_LIMIT_ERROR_CODES.attachmentCacheFull, 'deviceCacheFull'],
    ]) {
      assert.equal(
        formatAttachmentLimitError(code, dictionary.uiAttachments),
        dictionary.uiAttachments[key],
      );
      assert.ok(dictionary.uiAttachments[key]);
    }
  }
  assert.equal(formatAttachmentLimitError('unrelated failure', dictZh.uiAttachments), '');
});

test('ordinary chat and Codex paste entries use the shared limit formatter', () => {
  const root = fileURLToPath(new URL('../src/', import.meta.url));
  const chat = fs.readFileSync(`${root}features/chat/ChatView.jsx`, 'utf8');
  const codex = fs.readFileSync(`${root}features/codex/CodexAcpView.jsx`, 'utf8');
  const webBridge = fs.readFileSync(`${root}platform/web/bridge.js`, 'utf8');

  assert.match(
    chat,
    /addPasteImage\([\s\S]*?bytes,[\s\S]*?formatAttachmentError,[\s\S]*?\)/,
    'ordinary chat must pass its shared attachment formatter into the paste bridge',
  );
  assert.match(
    codex,
    /const limitError = formatAttachmentLimitError\(err, t\.uiAttachments\);[\s\S]*?setError\(limitError\);/,
    'Codex paste failures must render the shared localized attachment-limit copy',
  );
  assert.equal(
    webBridge.match(/archiveUnsafeEntry:/g)?.length,
    3,
    'the web bridge must define unsafe-archive copy in all three inline languages',
  );
  assert.match(
    webBridge,
    /raw === "attachment_archive_too_many_entries"[\s\S]*?bt\("archiveTooManyEntries"\)/,
    'the web bridge must normalize and render the too-many-entries code',
  );
  // The web user's 200 copy lives INLINE in the bridge (not in the shared
  // dictionaries the loop above pins), so the boundary-aware value pin must
  // cover the inline strings too — a silent revert of one language's copy
  // back to 50 stays green without this (r12 review).
  const inlineCopies = [
    ...webBridge.matchAll(/archiveTooManyEntries:\s*[`'"]([^`'"]*)[`'"]/g),
  ].map(match => match[1]);
  assert.equal(
    inlineCopies.length,
    3,
    'the web bridge must define too-many-entries copy in all three inline languages',
  );
  for (const copy of inlineCopies) {
    assert.match(
      copy,
      /(^|\D)200(\D|$)/,
      `the inline web copy must say 200 entries (the ingest_archive.rs cap), got: ${copy}`,
    );
  }
});
