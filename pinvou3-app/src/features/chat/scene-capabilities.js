function itemId(item) {
  return String((item && (item.id || item.backendId || item.skillId)) || '').trim();
}

function isInstalled(items, id) {
  const wanted = String(id || '').trim();
  if (!wanted) return true;
  return (items || []).some((item) => itemId(item) === wanted && item.installed !== false);
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

  return { ok: true, requirements, installed };
}

export {
  canPrepareSceneCapabilities,
  prepareSceneCapabilities,
  requiredCapabilitiesForMeta,
};
