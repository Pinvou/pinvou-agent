#!/usr/bin/env node
import assert from 'node:assert/strict';
import { createPetActivationState, loadActivePet } from '../src/features/pet/pet-active.js';

const pets = Object.freeze({
  lingling: Object.freeze({ id: 'lingling', name: '灵灵' }),
  langlang: Object.freeze({ id: 'langlang', name: '浪浪' }),
  'ace-taffy': Object.freeze({ id: 'ace-taffy', name: 'Ace Taffy' }),
});

function deferred() {
  let resolve;
  let reject;
  const promise = new Promise((accept, decline) => { resolve = accept; reject = decline; });
  return { promise, reject, resolve };
}

function harness({ activePet = null, loadAtlas, decodeImage = async () => {} } = {}) {
  const state = createPetActivationState(activePet);
  const commits = [];
  const errors = [];
  const activationFailures = [];
  const options = {
    state,
    normalizeId: (id) => (Object.hasOwn(pets, id) ? id : 'lingling'),
    resolvePet: (id) => pets[id],
    loadAtlas: loadAtlas || (async (pet) => `asset://${pet.id}.webp`),
    decodeImage,
    commit: (pet) => commits.push(pet),
    onActivationFailed: (failed) => activationFailures.push(failed),
    onError: (error, context) => errors.push({ error, context }),
  };
  return { state, commits, errors, activationFailures, options };
}

{
  const decoded = [];
  const h = harness({ decodeImage: async (url) => decoded.push(url) });
  const result = await loadActivePet('ace-taffy', h.options);
  assert.equal(result.id, 'ace-taffy');
  assert.equal(result.sheetUrl, 'asset://ace-taffy.webp');
  assert.deepEqual(decoded, ['asset://ace-taffy.webp']);
  assert.deepEqual(h.commits, [result]);
  assert.deepEqual(h.activationFailures, [false]);
}

{
  const atlas = { 'ace-taffy': deferred(), langlang: deferred() };
  const h = harness({ loadAtlas: (pet) => atlas[pet.id].promise });
  const older = loadActivePet('ace-taffy', h.options);
  const latest = loadActivePet('langlang', h.options);
  atlas.langlang.resolve('asset://langlang.webp');
  assert.equal((await latest).id, 'langlang');
  atlas['ace-taffy'].resolve('asset://ace-taffy.webp');
  await older;
  assert.deepEqual(h.commits.map((pet) => pet.id), ['langlang']);
  assert.equal(h.state.activePet.id, 'langlang');
  assert.deepEqual(h.activationFailures, [false]);
}

{
  const previous = { ...pets.lingling, sheetUrl: 'asset://lingling.webp' };
  const h = harness({
    activePet: previous,
    loadAtlas: async () => { throw new Error('atlas unavailable'); },
  });
  assert.equal(await loadActivePet('ace-taffy', h.options), previous);
  assert.equal(h.state.activePet, previous);
  assert.equal(h.commits.length, 0);
}

{
  const attempts = [];
  const h = harness({
    loadAtlas: async (pet) => {
      attempts.push(pet.id);
      if (pet.id === 'langlang') throw new Error('langlang unavailable');
      return 'asset://lingling.webp';
    },
  });
  const result = await loadActivePet('langlang', { ...h.options });
  assert.equal(result.id, 'lingling');
  assert.deepEqual(attempts, ['langlang', 'lingling']);
  assert.deepEqual(h.commits.map((pet) => pet.id), ['lingling']);
}

{
  const h = harness({
    loadAtlas: async () => { throw new Error('all atlases unavailable'); },
  });
  const result = await loadActivePet('langlang', { ...h.options });
  assert.equal(result, null);
  assert.equal(h.state.activePet, null);
  assert.equal(h.commits.length, 0);
  assert.deepEqual(h.errors.map(({ context }) => context.petId), ['langlang', 'lingling']);
  assert.deepEqual(h.activationFailures, [true]);
}

{
  const atlas = { 'ace-taffy': deferred(), langlang: deferred() };
  const h = harness({ loadAtlas: (pet) => atlas[pet.id].promise });
  const older = loadActivePet('ace-taffy', h.options);
  const latest = loadActivePet('langlang', h.options);
  atlas['ace-taffy'].reject(new Error('old request failed'));
  await older;
  assert.deepEqual(h.activationFailures, [], 'a stale failure must not expose the fallback shell');
  atlas.langlang.resolve('asset://langlang.webp');
  await latest;
  assert.deepEqual(h.activationFailures, [false]);
}

{
  const atlas = deferred();
  let loads = 0;
  const h = harness({
    loadAtlas: () => {
      loads += 1;
      return atlas.promise;
    },
  });
  const first = loadActivePet('ace-taffy', h.options);
  assert.equal(await loadActivePet('ace-taffy', h.options), null);
  assert.equal(loads, 1);
  atlas.resolve('asset://ace-taffy.webp');
  await first;
  assert.equal((await loadActivePet('ace-taffy', h.options)).id, 'ace-taffy');
  assert.equal(loads, 1);
  assert.equal(h.commits.length, 1);
}

console.log('pet active logic tests passed');
