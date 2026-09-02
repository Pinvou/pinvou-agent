import assert from 'node:assert/strict';
import test from 'node:test';

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
  }
  assert.equal(formatAttachmentLimitError('unrelated failure', dictZh.uiAttachments), '');
});
