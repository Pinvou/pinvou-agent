function itemId(item) {
  return String((item && (item.id || item.backendId || item.skillId)) || '').trim();
}

function isInstalled(items, id) {
  const wanted = String(id || '').trim();
  if (!wanted) return true;
  return (items || []).some((item) => itemId(item) === wanted && item.installed !== false);
}

// 开关默认全关（DenyAll）后，安装不再等于可用：场景流程必须读取 plain
// scope 的有效禁用集，把场景包显式移出（用户发起场景动作本身就是 opt-in，
// 评审 #455 R5-B3），否则模型收不到工具、场景静默降级而 UI 谎称已启用。
async function listDisabledConnectors(invoke) {
  const disabled = await invoke('get_disabled_connectors', { scope: 'plain' });
  return new Set(Array.isArray(disabled) ? disabled.map((id) => String(id || '').trim()) : []);
}

async function enablePackageInPlainScope(invoke, disabledIds, packageId) {
  if (!disabledIds.has(packageId)) return;
  disabledIds.delete(packageId);
  await invoke('set_disabled_connectors', { connectorIds: [...disabledIds], scope: 'plain' });
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

  // 安装完成 ≠ 开关打开：plain scope 有效禁用集含场景包时，用户发起的场景
  // 动作即显式 opt-in——移出禁用集并落盘（set_disabled_connectors 落盘后
  // 热刷在跑会话的工具白名单与技能组合目录，本轮即生效）。
  const requiredPackages = [...requirements.tools, ...requirements.skills];
  let enabled = false;
  try {
    const disabledIds = await listDisabledConnectors(invoke);
    for (const packageId of requiredPackages) {
      if (!disabledIds.has(packageId)) continue;
      await enablePackageInPlainScope(invoke, disabledIds, packageId);
      enabled = true;
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

  return { ok: true, requirements, installed, enabled };
}

export {
  canPrepareSceneCapabilities,
  prepareSceneCapabilities,
  requiredCapabilitiesForMeta,
};
