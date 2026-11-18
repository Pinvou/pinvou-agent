#!/usr/bin/env node
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const source = readFileSync(
  path.join(here, '..', 'src', 'features', 'settings', 'SettingsView.jsx'),
  'utf8',
);

test('the memory save-refusal surface keeps its verbatim-detail mapping and trilingual fallback', () => {
  // prettier can reflow the JSX catch block; the pins match on the
  // whitespace-squeezed text so a re-layout cannot silence them.
  const squeezedSource = source.replace(/\s+/g, ' ');

  // Cross-language contract: the backend's wipe-shaped-label guard refuses
  // saves with a `memory_profile_label_rejected`-prefixed message whose
  // detail names the fix. memory.rs's own Rust pins (the
  // memory_profile_label refusal fixtures) hold the producer side of that
  // prefix; nothing held the consumer side — dropping the startsWith branch
  // or the verbatim pass-through shipped every suite green, and dropping
  // the trilingual `memorySaveFailed` else arm regressed zh/ja users back
  // to raw English backend strings (r12 review). This file pins the
  // frontend side of the contract.
  assert.match(
    squeezedSource,
    /const refusal = detail\.startsWith\('memory_profile_label_rejected'\);/,
    'the save lane must keep detecting the crafted refusal via the backend memory_profile_label_rejected prefix (the producer side is pinned in memory.rs)',
  );
  // The refusal branch renders the backend detail VERBATIM (it names the
  // fix); both error banners route through the same ternary.
  assert.match(
    squeezedSource,
    /setMemoryEditorError\(refusal \? detail : settingsCopy\.memorySaveFailed\);/,
    'the editor banner must render the backend refusal detail verbatim in the refusal branch',
  );
  assert.match(
    squeezedSource,
    /setProfileSaveError\(refusal \? detail : settingsCopy\.memorySaveFailed\);/,
    'the profile banner must render the backend refusal detail verbatim and keep the trilingual memorySaveFailed else arm',
  );
});
