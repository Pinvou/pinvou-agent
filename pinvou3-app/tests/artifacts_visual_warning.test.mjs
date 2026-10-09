import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import {
  VISUAL_WARNING_LABEL_KEYS,
  localizedVisualWarning,
  visualWarningNeedsDependencyCheck,
} from '../src/features/artifacts/visual-warning.js';

const labels = {
  visualWarningTruncatedPages: 'Preview truncated to the first 30 pages.',
  visualWarningConvertFailed: 'Document conversion failed.',
};

test('known warning codes map to the localized label', () => {
  assert.equal(
    localizedVisualWarning({ warning: '中文原始文案', warning_code: 'truncatedPages' }, labels),
    labels.visualWarningTruncatedPages,
  );
  assert.equal(
    localizedVisualWarning({ warning: '中文原始文案', warning_code: 'convertFailed' }, labels),
    labels.visualWarningConvertFailed,
  );
});

test('unknown or missing code falls back, never to the raw string', () => {
  assert.equal(localizedVisualWarning({ warning: '中文原始文案', warning_code: 'futureCode' }, labels), null);
  assert.equal(localizedVisualWarning({ warning: '中文原始文案' }, labels), null);
  assert.equal(localizedVisualWarning(null, labels), null);
  assert.equal(localizedVisualWarning({ warning_code: 'futureCode' }, labels, 'fallback'), 'fallback');
});

test('only the LibreOffice conversion path offers the dependency shortcut', () => {
  assert.equal(visualWarningNeedsDependencyCheck({ warning_code: 'convertFailed' }), true);
  assert.equal(visualWarningNeedsDependencyCheck({ warning_code: 'truncatedPages' }), false);
  assert.equal(visualWarningNeedsDependencyCheck({ warning: 'LibreOffice 转换失败' }), false);
  assert.equal(visualWarningNeedsDependencyCheck(null), false);
});

test('mapped label keys exist in all three locales', () => {
  for (const locale of ['en', 'ja', 'zh']) {
    const source = readFileSync(fileURLToPath(new URL(`../src/shared/i18n/${locale}.js`, import.meta.url)), 'utf8');
    for (const key of Object.values(VISUAL_WARNING_LABEL_KEYS)) {
      assert.match(source, new RegExp(`${key}\\s*:`), `${locale} is missing artifactPreview key ${key}`);
    }
  }
});
