#!/usr/bin/env node
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const source = readFileSync(
  path.join(here, '..', 'src', 'features', 'knowledge', 'KnowledgeView.jsx'),
  'utf8',
);

// r15 review: the docs continuation is stamped with the collection that
// produced its offset. The indexing-completion poll used to reload docs
// through the effect's captured collection; if the user switched
// collections mid-indexing, that stale reset landed as the newest
// generation — the generation guard passes by construction — and the next
// load-more click sent the LIVE collection id against the STALE
// collection's offset, making that collection's first page unreachable
// with no error and no repair until the next reset. Three parts hold the
// fix: the reset stamps collId, the click refuses an identity mismatch,
// and the request itself carries the stamped id (the r5 search-continuation
// fix, ported to the per-collection docs table). The poll reads the live
// ref, not the closure.
assert.match(
  source,
  /setDocsPaging\(\{ gen: generation, collId: cid, offset: page\.length, hasMore: page\.length >= KB_DOCS_PAGE_SIZE, loading: false \}\);/,
  'loadDocs reset must stamp the collection id into docsPaging',
);
assert.match(
  source,
  /if \(docsPaging\.collId !== cid\) return;/,
  'load-more must refuse a continuation whose stamped collection differs from the focused one',
);
assert.match(
  source,
  /const page = await invokeTauri\('kb_documents', \{ collectionId: collId, limit: KB_DOCS_PAGE_SIZE, offset: docsPaging\.offset \}\)/,
  'the continuation request must target the stamped collection, not the live one',
);
assert.match(
  source,
  /setDocsPaging\(\{ gen: generation, collId, offset: docsPaging\.offset \+ page\.length, hasMore: page\.length >= KB_DOCS_PAGE_SIZE, loading: false \}\);/,
  'the landed continuation page must re-stamp the collection id',
);
// The completion poll's stale-collection hazard is closed at the source:
// latest-ref alone does NOT cover an in-flight invocation — the callback
// re-syncs only BETWEEN invocations, and the status await can span a
// collection switch (review-verified A→B→A request order: the stale
// continuation then takes a generation later than the new collection's
// and its rows overwrite the live selection while the paging state still
// belongs to the stale one). The poll must therefore capture the
// selection at call time and re-check the live ref after the await;
// the gated kb_smoke case ⑫ pins the behavior end to end.
assert.match(
  source,
  /const selectedId = activeColl && activeColl\.id;[\s\S]{0,600}if \(selectedId && activeCollRef\.current\?\.id === selectedId\) loadDocs\(selectedId\);/,
  'the completion poll must capture the selection at call time and re-check the live ref after the await',
);
