function itemId(item) {
  return String((item && (item.id || item.backendId || item.skillId)) || '').trim();
}

function isInstalled(items, id) {
  const wanted = String(id || '').trim();
  if (!wanted) return true;
  return (items || []).some((item) => itemId(item) === wanted && item.installed !== false);
}

// With switches defaulting to off (DenyAll), installed no longer means usable:
// the scene flow must read the plain scope's effective disabled set and
// explicitly move the scene packs out of it (the user-initiated scene action
// is itself the opt-in, review #455 R5-B3); otherwise the model receives no
// tools, the scene silently degrades, and the UI lies about being enabled.
// The pre-read only feeds the UI enabled flag; the write path goes through
// enable_marketplace_packages, the backend's single-critical-section RMW
// (review #455 R7-M3) — a whole-list read-modify-write across IPC is not lock
// protected, and a concurrent composer toggle's write would be overwritten by
// a stale snapshot.
async function listDisabledConnectors(invoke) {
  const disabled = await invoke('get_disabled_connectors', { scope: 'plain' });
  return new Set(Array.isArray(disabled) ? disabled.map((id) => String(id || '').trim()) : []);
}

// Returns the blocked list: non-empty = the plain scope is initialized and
// those ids sit in the user's explicit switch state — the backend enabled
// nothing and the caller must surface them (round-10 Major 2). The
// user-initiated scene action is an opt-in for *default*-off packs only;
// a deliberate opt-out is never silently overridden.
async function enablePackagesInPlainScope(invoke, packageIds) {
  const blocked = await invoke('enable_marketplace_packages', { packageIds, scope: 'plain' });
  return Array.isArray(blocked) ? blocked : [];
}

async function listMarketplaceTools(invoke) {
  const tools = await invoke('list_marketplace_tools');
  return Array.isArray(tools) ? tools : [];
}

async function listMarketplaceSkills(invoke) {
  const skills = await invoke('list_marketplace_skills');
  return Array.isArray(skills) ? skills : [];
}

// 用户可见文案由 UI 层按当前语言从 t.uiChatScenes[requirements.key] 取值，
// 模块本身只输出场景 key 与能力清单，不携带任何语言上下文。
const SCENE_CAPABILITY_DEFINITIONS = {
  'work:document-writing': {
    key: 'documentWriting',
    tools: ['gongwen'],
    skills: ['government-writing'],
  },
  'design:data-visualization': {
    key: 'dataVisualization',
    tools: [],
    skills: ['visualizer'],
  },
  'design:ppt': {
    key: 'pptDesign',
    tools: ['pptx'],
    skills: ['pptx'],
  },
};

function requiredCapabilitiesForMeta(meta) {
  if (!meta) return null;
  const definition = SCENE_CAPABILITY_DEFINITIONS[meta.pinvouScene];
  if (!definition) return null;
  return {
    key: definition.key,
    tools: [...definition.tools],
    skills: [...definition.skills],
  };
}

function canPrepareSceneCapabilities({ isWebHost, dependencyInstallAvailable } = {}) {
  return !isWebHost && dependencyInstallAvailable === true;
}

async function prepareSceneCapabilities(meta, invoke) {
  const requirements = requiredCapabilitiesForMeta(meta);
  if (!requirements) return { ok: true, requirements: null, installed: false };

  let installed = false;
  let tools = await listMarketplaceTools(invoke);
  let skills = await listMarketplaceSkills(invoke);

  for (const toolId of requirements.tools) {
    if (isInstalled(tools, toolId)) {
      continue;
    }

    await invoke('install_marketplace_tool', { toolId });
    installed = true;
    tools = await listMarketplaceTools(invoke);
    skills = await listMarketplaceSkills(invoke);
  }

  for (const skillId of requirements.skills) {
    if (isInstalled(skills, skillId)) {
      continue;
    }

    await invoke('install_marketplace_skill', { skillId });
    installed = true;
    skills = await listMarketplaceSkills(invoke);
  }

  const missingTools = requirements.tools.filter((toolId) => !isInstalled(tools, toolId));
  const missingSkills = requirements.skills.filter((skillId) => !isInstalled(skills, skillId));
  if (missingTools.length || missingSkills.length) {
    return {
      ok: false,
      requirements,
      installed,
      missing: [...missingTools, ...missingSkills],
    };
  }

  // Installed ≠ switched on: when the plain scope's effective disabled set
  // contains the scene packs, the user-initiated scene action is the explicit
  // opt-in — enable_marketplace_packages persists it and hot-refreshes the
  // running session's tool allowlist and skill-composition directory, taking
  // effect on the current turn.
  const requiredPackages = [...new Set([...requirements.tools, ...requirements.skills])];
  // Naming per R8 nit: true = a scene pack was default-gated and this send
  // completed the opt-in; future consumers must not misread it as availability.
  let optedIn;
  try {
    const disabledIds = await listDisabledConnectors(invoke);
    optedIn = requiredPackages.some((packageId) => disabledIds.has(packageId));
    if (optedIn) {
      const blocked = await enablePackagesInPlainScope(invoke, requiredPackages);
      if (blocked.length) {
        // Explicit user opt-out(s): refuse like the missing-install path —
        // the user re-enables from the composer tools list and resends.
        return {
          ok: false,
          requirements,
          installed,
          missing: [],
          blocked,
          error: String(blocked.join(', ')),
        };
      }
    }
  } catch (error) {
    return {
      ok: false,
      requirements,
      installed,
      missing: [],
      enableFailed: true,
      error: String((error && error.message) || error || ''),
    };
  }

  return { ok: true, requirements, installed, optedIn };
}

export {
  canPrepareSceneCapabilities,
  prepareSceneCapabilities,
  requiredCapabilitiesForMeta,
};
