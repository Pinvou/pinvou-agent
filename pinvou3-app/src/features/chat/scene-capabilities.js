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

function requiredCapabilitiesForMeta(meta) {
  if (!meta) return null;
  if (meta.pinvouScene === 'work:document-writing') {
    return {
      key: 'document-writing',
      label: '公文写作',
      preparingText: '正在准备公文写作能力...',
      readyText: '已启用公文写作，开始生成',
      failureText: '公文写作能力准备失败，请稍后重试。',
      tools: ['gongwen'],
      skills: ['government-writing'],
    };
  }
  if (meta.pinvouScene === 'design:data-visualization') {
    return {
      key: 'data-visualization',
      label: '数据可视化',
      preparingText: '正在准备数据可视化能力...',
      readyText: '已启用数据可视化，开始生成',
      failureText: '数据可视化能力准备失败，请稍后重试。',
      tools: [],
      skills: ['visualizer'],
    };
  }
  return null;
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
    if (!isInstalled(tools, toolId)) {
      await invoke('install_marketplace_tool', { toolId });
      installed = true;
      tools = await listMarketplaceTools(invoke);
      skills = await listMarketplaceSkills(invoke);
    }
  }

  for (const skillId of requirements.skills) {
    if (!isInstalled(skills, skillId)) {
      await invoke('install_marketplace_skill', { skillId });
      installed = true;
      skills = await listMarketplaceSkills(invoke);
    }
  }

  const missingTools = requirements.tools.filter((toolId) => !isInstalled(tools, toolId));
  const missingSkills = requirements.skills.filter((skillId) => !isInstalled(skills, skillId));
  if (missingTools.length || missingSkills.length) {
    return {
      ok: false,
      requirements,
      installed,
      error: `缺少能力：${[...missingTools, ...missingSkills].join(', ')}`,
    };
  }

  return { ok: true, requirements, installed };
}

export {
  canPrepareSceneCapabilities,
  prepareSceneCapabilities,
  requiredCapabilitiesForMeta,
};
