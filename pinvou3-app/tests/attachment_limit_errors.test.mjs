import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

import {
  ATTACHMENT_LIMIT_ERROR_CODES,
  attachmentLimitErrorCode,
  formatAttachmentLimitError,
} from '../src/features/attachments/attachment-limit-errors.js';
import { dictEn } from '../src/shared/i18n/en.js';
import { dictJa } from '../src/shared/i18n/ja.js';
import { dictZh } from '../src/shared/i18n/zh.js';

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
  for (const dictionary of [dictZh, dictEn, dictJa]) {
    assert.match(
      formatAttachmentLimitError(ATTACHMENT_LIMIT_ERROR_CODES.fileTooLarge, dictionary.uiAttachments),
      /20/,
    );
    assert.match(
      formatAttachmentLimitError(ATTACHMENT_LIMIT_ERROR_CODES.archiveTooManyEntries, dictionary.uiAttachments),
      /50/,
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
    /raw === "attachment_archive_unsafe_entry"[\s\S]*?bt\("archiveUnsafeEntry"\)/,
    'the web bridge must normalize the unsafe-archive wire code',
  );
});
