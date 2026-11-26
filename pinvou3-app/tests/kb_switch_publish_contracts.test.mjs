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

test('review P2 stale rows: rows paint only for the collection that actually landed', () => {
  // Loaded identity (set only when a reset page lands) gates the render —
  // the in-flight stamp alone would admit the old rows under the new
  // heading (stamp B while docs holds A) and a rejected B request would
  // leave them there indefinitely.
  assert.match(
    knowledgeView,
    /const rows = activeColl\s*\n\s*\? \(docsLoadedCollId === activeColl\.id \? docs : \[\]\)\s*\n\s*: allDocs;/,
    'foreign rows must never paint under the focused heading (failed switch shows the honest empty state)',
  );
  assert.match(
    knowledgeView,
    /setDocsLoadedCollId\(cid\);/,
    'the loaded identity moves only with landed rows',
  );
});

test('review P2 publish enumeration: exhausting the page cap fails loudly', () => {
  assert.match(
    remoteView,
    /if \(capped\) \{\s*\n\s*throw new Error\(t\.remoteKbPublishEnumerationCapped/,
    'hitting the page cap must raise the explicit overflow error instead of preparing a supposedly complete publish',
  );
});

test('review P2 full enumeration: preparePublish pages kb_documents until a short page', () => {
  assert.match(
    remoteView,
    /for \(let pages = 0; pages < KB_PUBLISH_PAGE_CAP; pages \+= 1\) \{[\s\S]{0,400}?invokeTauri\('kb_documents', \{[\s\S]{0,200}?offset,\s*\n\s*\}\) \|\| \[\];[\s\S]{0,200}?if \(page\.length < KB_PUBLISH_PAGE_SIZE\) \{\s*\n\s*capped = false;\s*\n\s*break;/,
    'publish preparation must enumerate with an advancing offset until a CONFIRMED short page (limit: 0 is backend-clamped to an implicit 500-row page)',
  );
  assert.match(
    remoteView,
    /let capped = true;/,
    'a cap-exhausted loop must default to overflow (only a confirmed short page proves completeness)',
  );
  assert.doesNotMatch(
    remoteView,
    /invokeTauri\('kb_documents', \{\s*\n\s*collectionId: localCollection\.id,\s*\n\s*limit: 0,\s*\n\s*\}\)/,
    'the single-shot limit: 0 call must be gone (it silently omitted documents beyond the implicit page)',
  );
});
