/** Cross-lane voice-mode parity pin (round-9 review): normalizeVoiceMode is
 * declared in BOTH bridge lanes (classic scripts, no shared import) and in
 * the canonical feature policy. The web copy drifted for months — it mapped
 * voice_edit/draft_edit to dictation while the policy and the desktop lane
 * mapped them to edit — so this pin requires all three spellings to accept
 * the alias set on every lane. Source-shape level: the lanes cannot import
 * the .mjs policy, so the alias literals must stay in lockstep by pin. */
import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(here, '..');

const SOURCES = {
  policy: 'src/features/voice-composer/voice-ui-policy.mjs',
  tauri: 'src/platform/tauri/bridge/voice.js',
  web: 'src/platform/web/bridge.js',
};

function read(rel) {
  return fs.readFileSync(path.join(root, rel), 'utf8');
}

test('every normalizeVoiceMode copy maps the edit aliases to edit', () => {
  for (const [name, rel] of Object.entries(SOURCES)) {
    const source = read(rel);
    const fn = source.match(/function normalizeVoiceMode\([^)]*\)\s*\{[\s\S]*?\n\s*\}/);
    assert.ok(fn, `${name}: normalizeVoiceMode declaration must stay discoverable`);
    for (const alias of ['voice_edit', 'draft_edit']) {
      assert.ok(
        fn[0].includes(`'${alias}'`) || fn[0].includes(`"${alias}"`),
        `${name}: normalizeVoiceMode must keep the ${alias} alias mapped to edit`,
      );
    }
  }
});
