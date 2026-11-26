import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

const read = (...segments) =>
  readFileSync(new URL(`../${segments.join('/')}`, import.meta.url), 'utf8');

const knowledgeView = read('src', 'features', 'knowledge', 'KnowledgeView.jsx');
const remoteView = read('src', 'features', 'remote-knowledge', 'RemoteKnowledgeView.jsx');

test('review P2 stale rows: loadDocs stamps the in-flight target at entry', () => {
  assert.match(
    knowledgeView,
    /const loadDocs = async \(cid\) => \{\s*\n\s*const generation = \+\+docsGenerationRef\.current;\s*\n\s*\/\/ Stamp the in-flight target at entry[\s\S]{0,600}?setDocsPaging\(\(p\) => \(\{ \.\.\.p, gen: generation, collId: cid \}\)\);/,
    'loadDocs must stamp gen+collId at entry so a failed switch cannot leave the previous collection\'s rows under the new heading',
  );
});

test('review P2 stale rows: the rows render gate admits only the stamped collection', () => {
  assert.match(
    knowledgeView,
    /const rows = activeColl\s*\n\s*\? \(docsPaging\.collId === activeColl\.id \? docs : \[\]\)\s*\n\s*: allDocs;/,
    'foreign rows must never paint under the focused heading (failed switch shows the honest empty state)',
  );
});

test('review P2 full enumeration: preparePublish pages kb_documents until a short page', () => {
  assert.match(
    remoteView,
    /for \(let pages = 0; pages < KB_PUBLISH_PAGE_CAP; pages \+= 1\) \{[\s\S]{0,400}?invokeTauri\('kb_documents', \{[\s\S]{0,200}?offset,\s*\n\s*\}\) \|\| \[\];[\s\S]{0,200}?if \(page\.length < KB_PUBLISH_PAGE_SIZE\) break;\s*\n\s*offset \+= page\.length;/,
    'publish preparation must enumerate with an advancing offset (limit: 0 is backend-clamped to an implicit 500-row page and silently truncated large collections)',
  );
  assert.doesNotMatch(
    remoteView,
    /invokeTauri\('kb_documents', \{\s*\n\s*collectionId: localCollection\.id,\s*\n\s*limit: 0,\s*\n\s*\}\)/,
    'the single-shot limit: 0 call must be gone (it silently omitted documents beyond the implicit page)',
  );
});
