import { companionPackageMap } from '../../shared/companion-packages.js';

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

// Availability is disabled ∪ hidden (round-11 m9): a switch-ON-but-hidden
// pack would otherwise skip the enable call — the only hidden-set cleaner on
// this path — and the send would proceed with the model never seeing the
// tool while the UI reports ready.
async function listHiddenBundles(invoke) {
  const hidden = await invoke('get_bundle_visibility', { scope: 'plain' });
  return new Set(Array.isArray(hidden) ? hidden.map((id) => String(id || '').trim()) : []);
}

// Returns the explicit outcome shape (round-11 m11, extended round-13 m3):
// blocked non-empty = the plain scope is initialized and those ids sit in the
// user's explicit switch state — the backend enabled nothing and the caller
// must surface them (round-10 Major 2). not_applied non-empty = those ids
// matched no entry in the DenyAll expansion (concurrent install not yet
// committed, or unknown id) — nothing was applied for them; the caller must
// not present their opt-in as done. Install-default offs lift freely
// (round-11 B2); a deliberate opt-out is never silently overridden.
async function enablePackagesInPlainScope(invoke, packageIds) {
  const outcome = await invoke('enable_marketplace_packages', { packageIds, scope: 'plain' });
  return {
    blocked: Array.isArray(outcome && outcome.blocked) ? outcome.blocked : [],
    notApplied: Array.isArray(outcome && outcome.not_applied) ? outcome.not_applied : [],
  };
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

  // 装上 ≠ 会话可见：开关/可见性任一关闭都会让会话侧排除该包（PPT 场景实测：
  // pptx 在 plain 隐藏集残留，装了也调不到）——下方的可用性预读 + 显式开启
  // （enable_marketplace_packages）就地处理 disabled 与 hidden 两个集合。

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

  // Installed ≠ switched on: when the plain scope's effective disabled set —
  // or the hidden set (availability is disabled ∪ hidden, round-11 m9) —
  // contains the scene packs, the user-initiated scene action is the explicit
  // opt-in — enable_marketplace_packages persists it (and un-hides) and
  // hot-refreshes the running session's tool allowlist and skill-composition
  // directory, taking effect on the current turn.
  // Round-16 minor 13, closed by the shared companion map (main's #563
  // extracted it so the scene path and ToolStoreView cannot drift): the
  // backend's disabled/hidden sets and the DenyAll expansion carry OWNER pack
  // ids, so a bare companion skill id must opt in for its owner pack.
  const ownerMap = companionPackageMap(tools);
  const requiredPackages = [...new Set([
    ...requirements.tools,
    ...requirements.skills,
    ...requirements.skills.flatMap((id) => (ownerMap[id] ? [ownerMap[id]] : [])),
  ])];
  // Naming per R8 nit: true = a scene pack was default-gated (or hidden) and
  // this send completed the opt-in; future consumers must not misread it as
  // availability.
  let optedIn;
  try {
    const [disabledIds, hiddenIds] = await Promise.all([
      listDisabledConnectors(invoke),
      listHiddenBundles(invoke),
    ]);
    optedIn = requiredPackages.some((packageId) => disabledIds.has(packageId) || hiddenIds.has(packageId));
    if (optedIn) {
      const { blocked, notApplied } = await enablePackagesInPlainScope(invoke, requiredPackages);
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
      if (notApplied.length) {
        // Round-13 m3: those ids matched nothing in the expansion (likely a
        // concurrent install that had not committed) — abort the send, but
        // NOT under the missing-install copy (round-16 minor 13): the packs
        // are installed, so a reinstall invitation would not help. The
        // dedicated notApplied shape renders the retry-inviting copy instead.
        return {
          ok: false,
          requirements,
          installed,
          missing: [],
          blocked: [],
          notApplied: [...notApplied],
          error: String(notApplied.join(', ')),
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
