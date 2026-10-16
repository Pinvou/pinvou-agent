import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';

const knowledgeSource = await readFile(new URL('../src/features/knowledge/KnowledgeView.jsx', import.meta.url), 'utf8');
const remoteKnowledgeSource = await readFile(new URL('../src/features/remote-knowledge/RemoteKnowledgeView.jsx', import.meta.url), 'utf8');

test('shared knowledge dialogs portal their full-viewport overlay to document.body', () => {
  assert.match(remoteKnowledgeSource, /import \{ createPortal \} from 'react-dom';/);
  assert.match(
    remoteKnowledgeSource,
    /return typeof document === 'undefined' \? dialog : createPortal\(dialog, document\.body\);/,
  );
  assert.match(remoteKnowledgeSource, /className="fixed inset-0 z-\[80\]/);
  assert.match(remoteKnowledgeSource, /role="dialog"[\s\S]*?aria-modal="true"/);
  assert.match(remoteKnowledgeSource, /onMouseDown=\{event => event\.stopPropagation\(\)\}/);
});

test('BGE download uses an accessible numeric circular progress indicator', () => {
  const indicator = knowledgeSource.slice(
    knowledgeSource.indexOf('function ModelProgressIndicator'),
    knowledgeSource.indexOf('const KnowledgeView'),
  );
  assert.match(indicator, /role="progressbar"/);
  assert.match(indicator, /aria-valuenow=\{percent\}/);
  assert.match(indicator, /strokeDasharray=\{MODEL_PROGRESS_CIRCUMFERENCE\}/);
  assert.match(indicator, /strokeDashoffset=\{progressOffset\}/);
  assert.match(indicator, /\{percent\}<span[\s\S]*?>%<\/span>/);
  assert.doesNotMatch(indicator, /style=\{\{ width:/);
});

test('BGE model loading keeps a distinct indeterminate status', () => {
  const indicator = knowledgeSource.slice(
    knowledgeSource.indexOf('function ModelProgressIndicator'),
    knowledgeSource.indexOf('const KnowledgeView'),
  );
  assert.match(indicator, /if \(!downloading\)/);
  assert.match(indicator, /role="status" aria-live="polite"/);
  assert.match(indicator, /animate-spin motion-reduce:animate-none/);
  assert.match(knowledgeSource, /<ModelProgressIndicator[\s\S]*?downloading=\{downloading\}[\s\S]*?percent=\{dlPct\}/);
});

test('deliverables index discloses unscannable session records instead of rendering a silent partial list', async () => {
  // refreshOutputs consumes the command's { rows, skipped } shape: the
  // skipped list (session records over the scan cap, unreadable, or
  // unparseable) is the only signal that the listing is incomplete —
  // defaulting the result to an array hid it (round-4 review).
  assert.match(knowledgeSource, /Array\.isArray\(res\.rows\)/);
  assert.match(knowledgeSource, /Array\.isArray\(res\.skipped\)/);
  assert.match(knowledgeSource, /setOutputsSkipped\(nextSkipped\)/);
  // The notice renders on the outputs tab whenever loaded work has skips —
  // including above the "empty" state, which must not read as a clean bill.
  assert.match(knowledgeSource, /outputsLoaded && outputsSkipped\.length > 0/);
  assert.match(knowledgeSource, /t\.kbOutSkippedNotice\(outputsSkipped\.length\)/);
  // The copy is tri-language via the shared i18n module (no zh-only literal).
  for (const lang of ['zh', 'en', 'ja']) {
    const source = await readFile(new URL(`../src/shared/i18n/${lang}.js`, import.meta.url), 'utf8');
    assert.match(source, /kbOutSkippedNotice: \(n\) =>/);
  }
});
