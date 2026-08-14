const DEFAULT_BUILTIN_SKILLS = [
  {
    id: 'visual-design',
    title: '视觉设计',
    description: '设计系统直出网页/banner/海报/简历...',
  },
];

function asArray(value) {
  if (Array.isArray(value)) return value;
  if (!value || typeof value !== 'object') return [];
  return Object.entries(value).map(([id, state]) => ({ id, ...(state || {}) }));
}

function buildComposerToolMenuState({
  marketplaceTools = [],
  marketplaceSkills = [],
  disabledIds = [],
  disabledSkillIds = [],
  serviceStates = [],
  activeSkill = null,
  builtinSkills = DEFAULT_BUILTIN_SKILLS,
  scope = 'plain',
} = {}) {
  const disabled = new Set(disabledIds || []);
  const disabledSkills = new Set(disabledSkillIds || []);
  // skill 双 scope 治理后,技能行在两个 scope 都可写（各自 scope 独立持久化,
  // code scope 未初始化时后端默认全禁已装技能）。`scope` 参数由调用方用于
  // 区分读写目标（get/set_disabled_skills、get/set_disabled_connectors）。
  void scope;
  const installedTools = (marketplaceTools || []).filter(tool => tool && tool.installed);
  const companionSkillIds = new Set(installedTools.flatMap(tool => tool.companion_skills || []));

  const connectedServices = asArray(serviceStates)
    .filter(service => service && service.connected)
    .map(service => ({
      id: service.id,
      kind: 'service',
      title: service.title || service.name || service.id,
      description: service.description || '',
      // scope 门禁开关：enabled = 全局未停用（marker）且本 scope 未禁用
      // （disabled_connectors.json）。「已连接」徽章表达连接状态，开关表达门禁。
      enabled: service.enabled !== false && !disabled.has(service.id),
      connected: true,
      switchable: true,
    }));

  const toolRows = installedTools.map(tool => ({
    id: tool.id,
    kind: 'tool',
    title: tool.name || tool.title || tool.id,
    description: tool.description || tool.subtitle || '',
    enabled: !disabled.has(tool.id),
    switchable: true,
  }));

  const skillRows = (marketplaceSkills || [])
    .filter(skill => skill && skill.installed && !companionSkillIds.has(skill.id))
    .map(skill => {
      const rowId = `skill:${skill.id}`;
      return {
        id: rowId,
        skillId: skill.id,
        kind: 'skill',
        title: skill.title || skill.name || skill.id,
        description: skill.description || skill.subtitle || '',
        enabled: !disabledSkills.has(skill.id),
        active: activeSkill === skill.id || activeSkill === rowId,
        switchable: true,
        unavailable: false,
      };
    });

  const builtinRows = (builtinSkills || []).map(skill => ({
    id: `builtin-skill:${skill.id}`,
    skillId: skill.id,
    kind: 'builtin-skill',
    title: skill.title || skill.name || skill.id,
    description: skill.description || skill.desc || '',
    enabled: true,
    active: activeSkill === skill.id,
    switchable: false,
    unavailable: false,
  }));

  const allSkillRows = [...skillRows, ...builtinRows];
  const enabledCount =
    connectedServices.filter(row => row.enabled).length +
    toolRows.filter(row => row.enabled).length +
    allSkillRows.filter(row => row.enabled).length;

  return {
    connectedServices,
    toolRows,
    skillRows: allSkillRows,
    enabledCount,
  };
}

export { DEFAULT_BUILTIN_SKILLS, buildComposerToolMenuState };
