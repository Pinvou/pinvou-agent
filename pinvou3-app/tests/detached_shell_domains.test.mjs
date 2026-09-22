import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

// Static contract: the detached chat window's bridge domain list must be a
// subset of the app's registered domains, and must include `computerUse` —
// the consent-carrying domain. Scope note: this pins the computerUse slice
// (the PR #468 round-10 M1 fix — missing here made the consent banner and
// grant/confirm dialogs unrenderable in detached windows while the agent
// controlled the machine); it does not derive the full list from ChatView's
// usages, so other ChatView-consumed domains still rely on the subset check
// against main.

const source = (relative) =>
  readFileSync(new URL(`../src/${relative}`, import.meta.url), 'utf8');

const detachedBlock = source('app/DetachedShell.jsx').match(
  /const bs = useBridgeState\(\[([^\]]*)\]\)/,
);
assert.ok(detachedBlock, 'useDetachedBase domain list must exist');
const detachedDomains = detachedBlock[1]
  .split(',')
  .map((entry) => entry.trim().replace(/^'|'$/g, ''))
  .filter(Boolean);

const mainBlock = source('app/main.jsx').match(
  /const APP_BRIDGE_STATE_DOMAINS = \[([^\]]*)\]/,
);
assert.ok(mainBlock, 'APP_BRIDGE_STATE_DOMAINS must exist');
const mainDomains = mainBlock[1]
  .split(',')
  .map((entry) => entry.trim().replace(/^'|'$/g, ''))
  .filter(Boolean);

assert.ok(
  detachedDomains.includes('computerUse'),
  'detached windows must subscribe to the computerUse domain for consent surfaces',
);
for (const domain of detachedDomains) {
  assert.ok(
    mainDomains.includes(domain),
    `detached domain "${domain}" must be declared in APP_BRIDGE_STATE_DOMAINS`,
  );
}
