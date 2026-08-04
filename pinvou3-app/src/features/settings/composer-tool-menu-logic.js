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
  serviceStates = [],
  activeSkill = null,
  builtinSkills = DEFAULT_BUILTIN_SKILLS,
  scope = 'plain',
} = {}) {
  const disabled = new Set(disabledIds || []);
  // code scope: 代码会话已整体禁用技能加载,技能行只读展示且不计入启用数。
  const skillsUnavailable = scope === 'code';
  const installedTools = (marketplaceTools || []).filter(tool => tool && tool.installed);
  const companionSkillIds = new Set(installedTools.flatMap(tool => tool.companion_skills || []));

  const connectedServices = asArray(serviceStates)
    .filter(service => service && service.connected)
    .map(service => ({
      id: service.id,
      kind: 'service',
      title: service.title || service.name || service.id,
      description: service.description || '',
      enabled: service.enabled !== false,
      connected: true,
      switchable: false,
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
        enabled: !disabled.has(rowId),
        active: activeSkill === skill.id || activeSkill === rowId,
        switchable: !skillsUnavailable,
        unavailable: skillsUnavailable,
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
    unavailable: skillsUnavailable,
  }));

  const allSkillRows = [...skillRows, ...builtinRows];
  const enabledCount =
    connectedServices.filter(row => row.enabled).length +
    toolRows.filter(row => row.enabled).length +
    (skillsUnavailable ? 0 : allSkillRows.filter(row => row.enabled).length);

  return {
    connectedServices,
    toolRows,
    skillRows: allSkillRows,
    enabledCount,
  };
}

export { DEFAULT_BUILTIN_SKILLS, buildComposerToolMenuState };
