#!/usr/bin/env node
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';

const __dirname = path.dirname(fileURLToPath(import.meta.url));

const logicPath = path.join(__dirname, '..', 'src', 'features', 'chat', 'scene-capabilities.js');
const code = fs.readFileSync(logicPath, 'utf8')
  .replace(/\bexport\s+\{[^}]+\};?/g, '')
  .replace(/\bexport\s+/g, '');

const ctx = {};
vm.createContext(ctx);
vm.runInContext(`${code}
this.canPrepareSceneCapabilities = canPrepareSceneCapabilities;
this.requiredCapabilitiesForMeta = requiredCapabilitiesForMeta;`, ctx, { filename: logicPath });

const { canPrepareSceneCapabilities, requiredCapabilitiesForMeta } = ctx;

assert.strictEqual(
  canPrepareSceneCapabilities({ isWebHost: true, dependencyInstallAvailable: true }),
  false,
  'Web host must not call desktop marketplace install APIs',
);
assert.strictEqual(
  canPrepareSceneCapabilities({ isWebHost: false, dependencyInstallAvailable: false }),
  false,
  'Desktop host without dependency install capability must not call install APIs',
);
assert.strictEqual(
  canPrepareSceneCapabilities({ isWebHost: false, dependencyInstallAvailable: true }),
  true,
  'Desktop host with dependency install capability may prepare scene capabilities',
);

const dataVisualizationRequirements = requiredCapabilitiesForMeta({ pinvouScene: 'design:data-visualization' });
assert.strictEqual(dataVisualizationRequirements.key, 'dataVisualization');
assert.deepStrictEqual([...dataVisualizationRequirements.tools], []);
assert.deepStrictEqual([...dataVisualizationRequirements.skills], ['visualizer']);

const documentWritingRequirements = requiredCapabilitiesForMeta({ pinvouScene: 'work:document-writing' });
assert.strictEqual(documentWritingRequirements.key, 'documentWriting');
assert.deepStrictEqual([...documentWritingRequirements.tools], ['gongwen']);
assert.deepStrictEqual([...documentWritingRequirements.skills], ['government-writing']);

const pptDesignRequirements = requiredCapabilitiesForMeta({ pinvouScene: 'design:ppt' });
assert.strictEqual(pptDesignRequirements.key, 'pptDesign');
assert.deepStrictEqual([...pptDesignRequirements.tools], ['pptx']);
assert.deepStrictEqual([...pptDesignRequirements.skills], ['pptx']);

assert.strictEqual(requiredCapabilitiesForMeta(null), null);
assert.strictEqual(requiredCapabilitiesForMeta({ pinvouScene: 'design:poster' }), null);
// User-visible copy is resolved by the UI layer from t.uiChatScenes[requirements.key]; the module must not carry copy fields.
assert.strictEqual('label' in dataVisualizationRequirements, false);
assert.strictEqual('preparingText' in dataVisualizationRequirements, false);

console.log('scene_capabilities_logic: ok');

// --- DenyAll adaptation (review #455 R5-B3): installed ≠ usable; a scene action must complete the explicit opt-in ---
vm.runInContext('this.prepareSceneCapabilities = prepareSceneCapabilities;', ctx, { filename: logicPath });
const { prepareSceneCapabilities } = ctx;

function makeInvoke({ tools = [], skills = [], disabled = [], hidden = [], blockedOnEnable = [], notAppliedOnEnable = [] } = {}) {
  const state = {
    tools: new Set(tools),
    skills: new Set(skills),
    disabled: new Set(disabled),
    hidden: new Set(hidden),
    blockedOnEnable: new Set(blockedOnEnable),
    notAppliedOnEnable: new Set(notAppliedOnEnable),
    enableCalls: [],
  };
  const toolList = () => [...state.tools].map((id) => ({ id, installed: true }));
  const skillList = () => [...state.skills].map((id) => ({ id, installed: true }));
  const invoke = async (command, args) => {
    if (command === 'list_marketplace_tools') return toolList();
    if (command === 'list_marketplace_skills') return skillList();
    if (command === 'install_marketplace_tool') { state.tools.add(args.toolId); return null; }
    if (command === 'install_marketplace_skill') { state.skills.add(args.skillId); return null; }
    if (command === 'get_disabled_connectors') return [...state.disabled];
    if (command === 'get_bundle_visibility') return [...state.hidden];
    // Backend single-critical-section RMW semantics of enable_marketplace_packages:
    // the explicit outcome shape (round-11 m11) — a blocked id (explicit user
    // opt-out) refuses the batch; otherwise the ids leave the disabled set AND
    // the hidden set (availability is disabled ∪ hidden, round-11 m9).
    if (command === 'enable_marketplace_packages') {
      if (args.scope !== 'plain') throw new Error('scene opt-in must target plain scope');
      state.enableCalls.push([...args.packageIds]);
      const blocked = args.packageIds.filter((id) => state.blockedOnEnable.has(id));
      if (blocked.length) return { enabled: false, blocked };
      // Round-13 m3: ids absent from the DenyAll expansion match nothing —
      // nothing is applied for them and the outcome reports not_applied.
      const notApplied = args.packageIds.filter((id) => state.notAppliedOnEnable.has(id));
      for (const id of args.packageIds) {
        if (state.notAppliedOnEnable.has(id)) continue;
        state.disabled.delete(id);
        state.hidden.delete(id);
      }
      return { enabled: notApplied.length === 0, blocked: [], not_applied: notApplied };
    }
    throw new Error(`unexpected command ${command}`);
  };
  return { invoke, state };
}

async function runDenyAllOptInScenarios() {
  // All installed, all switched off: the scene packs sit in the DenyAll
  // extension of the set (the effective disabled set contains the pack ids),
  // and the scene action must move them out of the disabled set — otherwise
  // the model receives no tools and the ready copy would be lying.
  {
    const { invoke, state } = makeInvoke({
      tools: ['gongwen'],
      skills: ['government-writing'],
      disabled: ['gongwen', 'government-writing', 'feishu'],
    });
    const prepared = await prepareSceneCapabilities({ pinvouScene: 'work:document-writing' }, invoke);
    assert.strictEqual(prepared.ok, true);
    assert.strictEqual(prepared.installed, false, 'nothing was installed');
    assert.strictEqual(prepared.optedIn, true, 'scene packages must be opted in');
    assert.strictEqual(state.disabled.has('gongwen'), false);
    assert.strictEqual(state.disabled.has('government-writing'), false);
    assert.strictEqual(state.disabled.has('feishu'), true, 'unrelated packs stay disabled');
    // Exact snapshot: a single batched call, exactly the scene packs, no extra ids (R7 minor)
    assert.deepStrictEqual(state.enableCalls, [['gongwen', 'government-writing']]);
  }

  // Not installed + scope uninitialized (DenyAll default-off): the explicit opt-in is still required after install.
  {
    const { invoke, state } = makeInvoke({ disabled: ['pptx'] });
    const prepared = await prepareSceneCapabilities({ pinvouScene: 'design:ppt' }, invoke);
    assert.strictEqual(prepared.ok, true);
    assert.strictEqual(prepared.installed, true);
    assert.strictEqual(prepared.optedIn, true);
    assert.strictEqual(state.disabled.has('pptx'), false);
    assert.deepStrictEqual(state.enableCalls, [['pptx']]);
  }

  // Installed and not in the disabled set: zero switch writes, enabled=false (the UI no longer pops ready).
  {
    const { invoke, state } = makeInvoke({
      tools: ['gongwen'],
      skills: ['government-writing'],
      disabled: ['feishu'],
    });
    const prepared = await prepareSceneCapabilities({ pinvouScene: 'work:document-writing' }, invoke);
    assert.strictEqual(prepared.ok, true);
    assert.strictEqual(prepared.installed, false);
    assert.strictEqual(prepared.optedIn, false);
    assert.strictEqual(state.enableCalls.length, 0, 'no switch write when already enabled');
  }

  // Switch read failure: fail must be visible (ok=false + error) and must not pass as "ready".
  {
    const { invoke } = makeInvoke({ tools: ['gongwen'], skills: ['government-writing'] });
    const failing = async (command, args) => {
      if (command === 'get_disabled_connectors') throw new Error('scope file unreadable');
      return invoke(command, args);
    };
    const prepared = await prepareSceneCapabilities({ pinvouScene: 'work:document-writing' }, failing);
    assert.strictEqual(prepared.ok, false, 'gate-read failure must not pass as ready');
    assert.strictEqual(prepared.enableFailed, true);
    assert.match(prepared.error, /scope file unreadable/);
  }

  // The enable command itself fails: fail-visible (ok=false + error), must not pass as "ready".
  {
    const { invoke } = makeInvoke({ tools: ['gongwen'], skills: ['government-writing'], disabled: ['gongwen'] });
    const failing = async (command, args) => {
      if (command === 'enable_marketplace_packages') throw new Error('backend locked');
      return invoke(command, args);
    };
    const prepared = await prepareSceneCapabilities({ pinvouScene: 'work:document-writing' }, failing);
    assert.strictEqual(prepared.ok, false, 'enable failure must not pass as ready');
    assert.strictEqual(prepared.enableFailed, true);
    assert.match(prepared.error, /backend locked/);
  }

  // Round-11 m9: switch-ON but hidden — availability is disabled ∪ hidden,
  // so a hidden scene pack must still trigger the enable call (the only
  // hidden-set cleaner on this path); otherwise the model never sees the
  // tool while the UI may report ready.
  {
    const { invoke, state } = makeInvoke({
      tools: ['gongwen'],
      skills: ['government-writing'],
      disabled: ['feishu'],
      hidden: ['gongwen'],
    });
    const prepared = await prepareSceneCapabilities({ pinvouScene: 'work:document-writing' }, invoke);
    assert.strictEqual(prepared.ok, true);
    assert.strictEqual(prepared.optedIn, true, 'a hidden scene pack must complete the opt-in (un-hide)');
    assert.deepStrictEqual(state.enableCalls, [['gongwen', 'government-writing']]);
    assert.strictEqual(state.hidden.has('gongwen'), false, 'enable un-hides the pack');
    assert.strictEqual(state.disabled.has('feishu'), true, 'unrelated packs stay disabled');
  }

  // Round-11 m10: the blocked branch — an explicit user opt-out refuses the
  // batch (ok:false + blocked ids, nothing moved). Previously zero coverage:
  // the enable mock always succeeded.
  {
    const { invoke, state } = makeInvoke({
      tools: ['gongwen'],
      skills: ['government-writing'],
      disabled: ['gongwen', 'government-writing'],
      blockedOnEnable: ['gongwen'],
    });
    const prepared = await prepareSceneCapabilities({ pinvouScene: 'work:document-writing' }, invoke);
    assert.strictEqual(prepared.ok, false, 'an explicit opt-out must refuse the scene send');
    assert.deepStrictEqual([...prepared.blocked], ['gongwen']);
    assert.strictEqual(prepared.enableFailed, undefined, 'a refusal is not a failure');
    assert.strictEqual(state.disabled.has('gongwen'), true, 'blocked pack stays disabled');
    assert.strictEqual(
      state.disabled.has('government-writing'),
      true,
      'the refusal is wholesale — unblocked batch mates stay disabled too',
    );
  }

  // Round-13 m3: not_applied — an id that matched nothing in the DenyAll
  // expansion (concurrent install not yet committed) must abort the send via
  // the missing-copy path instead of proceeding without the tool.
  {
    const { invoke, state } = makeInvoke({
      tools: ['gongwen'],
      skills: ['government-writing'],
      disabled: ['gongwen'],
      notAppliedOnEnable: ['gongwen'],
    });
    const prepared = await prepareSceneCapabilities({ pinvouScene: 'work:document-writing' }, invoke);
    assert.strictEqual(prepared.ok, false, 'a not-applied opt-in must not pass as ready');
    assert.deepStrictEqual([...prepared.missing], ['gongwen']);
    assert.strictEqual(state.disabled.has('gongwen'), true, 'not-applied pack stays disabled');
  }

  console.log('scene_capabilities_logic deny-all opt-in: ok');
}

try {
  await runDenyAllOptInScenarios();
} catch (error) {
  console.error(error);
  process.exit(1);
}
