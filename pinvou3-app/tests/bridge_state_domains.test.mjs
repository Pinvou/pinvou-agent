// Contract: a desktop bridge feature that writes bridge-state fields must be
// wired through the whole chain, or the feature is dead at runtime:
//
//   bridge/<feature>.js writes state.<field>
//     -> STATE_SLICE_FIELDS declares the domain with those fields
//        (snapshotStateSlice throws on unknown domains)
//     -> APP_BRIDGE_STATE_DOMAINS subscribes the domain in the app
//        (useBridgeState never surfaces unsubscribed domains)
//
// The projects feature shipped the feature file without either registration
// and every lane stayed green — this file locks the mapping so that cannot
// recur. Source-regex based on purpose: the constants live in classic scripts
// (bridge.js) and the app bundle (main.jsx) with no module boundary to import.
//
// Scope note: older features still write ephemeral fields that no slice
// declares (state.activeTurnTimelineId, state.modeLane, state.modeDefaults,
// state.pendingDraftMultiAgent, state.pinvouSceneEvents, state.steeredMessages,
// state.remoteControl in updater.js). None of them is consumed through
// state.getMany today; registering or removing them is its own cleanup, not
// silently bundled here.
import assert from 'node:assert/strict';
import test from 'node:test';
import { readFileSync } from 'node:fs';

const read = (relative) =>
  readFileSync(new URL(`../src/${relative}`, import.meta.url), 'utf8');

function parseSliceFields(source) {
  const match = source.match(/const STATE_SLICE_FIELDS = \{([\s\S]*?)\n {2}\};/);
  assert.ok(match, 'STATE_SLICE_FIELDS not found in bridge.js');
  const domains = {};
  for (const entry of match[1].matchAll(/(\w+): \[([^\]]*)\]/g)) {
    domains[entry[1]] = entry[2]
      .split(',')
      .map((field) => field.trim().replace(/^"|"$/g, ''))
      .filter(Boolean);
  }
  return domains;
}

function parseAppDomains(source) {
  const match = source.match(/const APP_BRIDGE_STATE_DOMAINS = \[([^\]]*)\]/);
  assert.ok(match, 'APP_BRIDGE_STATE_DOMAINS not found in main.jsx');
  return match[1]
    .split(',')
    .map((domain) => domain.trim().replace(/^'|'$/g, ''))
    .filter(Boolean);
}

test('STATE_SLICE_FIELDS domains and app subscription domains match', () => {
  const sliceDomains = Object.keys(parseSliceFields(read('platform/tauri/bridge.js')));
  const appDomains = parseAppDomains(read('app/main.jsx'));
  assert.deepEqual(
    [...appDomains].sort((a, b) => a.localeCompare(b)),
    [...sliceDomains].sort((a, b) => a.localeCompare(b)),
    'every subscribed domain must be registered in STATE_SLICE_FIELDS and vice versa',
  );
});

test('projects domain is fully wired (feature -> slice -> app)', () => {
  const feature = read('platform/tauri/bridge/projects.js');
  const sliceFields = parseSliceFields(read('platform/tauri/bridge.js'));
  const appDomains = parseAppDomains(read('app/main.jsx'));

  assert.ok(
    feature.includes('state.projectsList'),
    'projects feature must publish its snapshot on state.projectsList',
  );
  const featureWrites = [...feature.matchAll(/state\.(\w+)\s*=/g)].map((entry) => entry[1]);
  assert.deepEqual(
    [...new Set(featureWrites)].sort((a, b) => a.localeCompare(b)),
    ['projectsList'],
    'projects feature must only write the fields its domain declares',
  );
  assert.ok(
    sliceFields.projects.includes('projectsList'),
    'STATE_SLICE_FIELDS.projects must declare projectsList',
  );
  assert.ok(
    appDomains.includes('projects'),
    'APP_BRIDGE_STATE_DOMAINS must subscribe the projects domain',
  );
});
