import { companionPackageMap } from '../../shared/companion-packages.js';

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

// 场景子标签只存在于普通会话（work 车道；bridge 侧非 code 会话一律映射 plain
// scope），因此可用性检查与显式开启都固定落在 plain scope。
const SCENE_SCOPE = 'plain';

// companion 技能 → 所属包 id 由 shared/companion-packages.js 单一真源提供
// （与 ToolStoreView 的技能卡路由同源）。

// 场景要求 id（含 companion 映射后的包 id）落在开关禁用集或可见性隐藏集里 →
// 会话侧组合目录与工具白名单都会把它排除（unavailable = disabled ∪ hidden），
// 强制场景路由必然落空。用户在场景子标签里主动发送即显式选择该能力，与安装
// 同一口径就地开启：从两个集合移除后整集写回（后端命令是整集覆盖语义）。
// 返回是否实际改写了用户的开关/可见性集合——这是对用户治理状态的变更，
// 调用方必须给出可见提示，不得静默改写。
async function ensureSceneAvailability(requirements, tools, invoke) {
  const map = companionPackageMap(tools);
  const wanted = new Set();
  const add = (id) => {
    const raw = String(id || '').trim();
    if (!raw) return;
    wanted.add(raw);
    const pkg = map[raw];
    if (pkg) wanted.add(pkg);
  };
  requirements.tools.forEach(add);
  requirements.skills.forEach(add);
  if (!wanted.size) return false;

  const [disabled, hidden] = await Promise.all([
    invoke('get_disabled_connectors', { scope: SCENE_SCOPE }),
    invoke('get_bundle_visibility', { scope: SCENE_SCOPE }),
  ]);
  const disabledList = Array.isArray(disabled) ? disabled : [];
  const hiddenList = Array.isArray(hidden) ? hidden : [];
  const blockedIn = (list) => list.filter((id) => wanted.has(id));
  const nextDisabled = blockedIn(disabledList);
  const nextHidden = blockedIn(hiddenList);
  if (!nextDisabled.length && !nextHidden.length) return false;

  // 未被场景点名的条目原样保留，避免整集覆盖语义误伤用户其他开关配置。
  if (nextDisabled.length) {
    await invoke('set_disabled_connectors', {
      connectorIds: disabledList.filter((id) => !wanted.has(id)),
      scope: SCENE_SCOPE,
    });
  }
  if (nextHidden.length) {
    await invoke('set_bundle_visibility', {
      bundleIds: hiddenList.filter((id) => !wanted.has(id)),
      scope: SCENE_SCOPE,
    });
  }
  return true;
}

async function prepareSceneCapabilities(meta, invoke) {
  const requirements = requiredCapabilitiesForMeta(meta);
  if (!requirements) return { ok: true, requirements: null, installed: false, reEnabled: false };

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

  // 装上 ≠ 会话可见：开关/可见性任一关闭都会让会话侧排除该包，强制场景
  // 路由因此必然失败（PPT 场景实测：pptx 在 plain 隐藏集残留，装了也调不到）。
  const reEnabled = await ensureSceneAvailability(requirements, tools, invoke);

  const missingTools = requirements.tools.filter((toolId) => !isInstalled(tools, toolId));
  const missingSkills = requirements.skills.filter((skillId) => !isInstalled(skills, skillId));
  if (missingTools.length || missingSkills.length) {
    return {
      ok: false,
      requirements,
      installed,
      reEnabled,
      missing: [...missingTools, ...missingSkills],
    };
  }

  return { ok: true, requirements, installed, reEnabled };
}

export {
  canPrepareSceneCapabilities,
  prepareSceneCapabilities,
  requiredCapabilitiesForMeta,
};
