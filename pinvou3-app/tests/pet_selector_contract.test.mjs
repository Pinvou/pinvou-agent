import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

function source(relativePath) {
  return readFileSync(new URL(`../${relativePath}`, import.meta.url), 'utf8');
}

const selectedPetRust = source('src-tauri/src/features/pet/selected_pet.rs');
const petCommands = source('src-tauri/src/app/commands/pet.rs');
const rustLib = source('src-tauri/src/lib.rs');
const bridge = source('src/platform/tauri/bridge.js') + source('src/platform/tauri/bridge/settings.js');
const domainAdapter = source('src/platform/web/bridge/domain-adapter.js');
const petWindow = source('src/features/pet/PetWindow.jsx');
const manifest = JSON.parse(source('src/features/pet/pet-manifest.json'));

const builtinIdsDeclaration = selectedPetRust.match(
  /const\s+BUILTIN_PET_IDS\s*:\s*\[&str;\s*\d+\]\s*=\s*\[([^\]]*)\]/s,
);
assert.ok(builtinIdsDeclaration, 'Rust must declare BUILTIN_PET_IDS');
const rustBuiltinIds = Array.from(
  builtinIdsDeclaration[1].matchAll(/"([^"]+)"/g),
  (match) => match[1],
);
const manifestIds = manifest.map((pet) => pet.id);
assert.deepEqual(
  rustBuiltinIds,
  manifestIds,
  'Rust selected-pet whitelist order must exactly equal the visible manifest order',
);
assert.deepEqual(manifestIds, ['lingling', 'langlang', 'ace-taffy', 'vivi']);

assert.match(
  petCommands,
  /sync_command_passthrough!\(selected_pet_domain,\s*get_selected_pet/,
);
assert.match(
  petCommands,
  /sync_command_passthrough!\(selected_pet_domain,\s*set_selected_pet/,
);
assert.match(rustLib, /commands::pet::get_selected_pet/);
assert.match(rustLib, /commands::pet::set_selected_pet/);
assert.match(selectedPetRust, /["']pet:selected_changed["']/);
assert.match(petCommands, /set_pet_temporarily_hidden/);
assert.match(rustLib, /commands::pet::set_pet_temporarily_hidden/);
assert.match(bridge, /["']pet:selected_changed["']/);

assert.match(bridge, /selectedPet:\s*["']lingling["']/);
assert.match(bridge, /listen\(["']pet:selected_changed["']/);
assert.match(bridge, /window\.addEventListener\(["']focus["'][\s\S]*?loadSettings\(\)/);
assert.match(bridge, /async function setSelectedPet\(id\)/);
assert.match(bridge, /invoke\(["']set_selected_pet["'],\s*\{\s*id:\s*id\s*\}\)/);
assert.match(bridge, /setSelectedPet:\s*setSelectedPet/);
assert.match(bridge, /setPetEnabled:\s*setPetEnabled/);
assert.match(domainAdapter, /settings:\s*domain\(\[[^\]]*["']setPetEnabled["']/);
assert.match(
  bridge,
  /state\.settings\.pet\s*=\s*Object\.assign\(\{\},\s*state\.settings\.pet\s*\|\|\s*\{\}/,
);
assert.doesNotMatch(
  bridge,
  /state\.settings\.pet\s*=\s*\{\s*enabled\s*:/,
  'pet:enabled_changed must preserve the other pet settings fields',
);

assert.match(petWindow, /listen\(["']pet:selected_changed["']/);
assert.match(petWindow, /invoke\(["']get_selected_pet["']/);

console.log('pet selector contract tests passed');
