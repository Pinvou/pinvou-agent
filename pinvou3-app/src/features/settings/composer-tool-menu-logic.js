const DEFAULT_BUILTIN_SKILLS = [
  {
    id: 'visual-design',
    title: '视觉设计',
    // 设计期差量（后端 MODE_TABLE）：该技能在这些模式不提供，开关只读。
    unavailableIn: ['code'],
  },
];

function asArray(value) {
  if (Array.isArray(value)) return value;
  if (!value || typeof value !== 'object') return [];
  return Object.entries(value).map(([id, state]) => ({ id, ...state }));
}

function buildComposerToolMenuState({
  marketplaceTools = [],
  marketplaceSkills = [],
  disabledIds = [], // 开关（on/off）
  hiddenIds = [], // 可见性（显隐，预过滤）
  serviceStates = [],
  activeSkill = null,
  builtinSkills = DEFAULT_BUILTIN_SKILLS,
  scope = 'plain',
} = {}) {
  // 开关与可见性正交：hidden 决定是否出现在列表，disabled 决定 on/off。
  const disabled = new Set(disabledIds || []);
  const hidden = new Set(hiddenIds || []);
  const installedTools = (marketplaceTools || []).filter(tool => tool && tool.installed);
  const companionSkillIds = new Set(installedTools.flatMap(tool => tool.companion_skills || []));

  const connectedServices = asArray(serviceStates)
    .filter(service => service && service.connected && !hidden.has(service.id))
    .map(service => ({
      id: service.id,
      kind: 'service',
      title: service.title || service.name || service.id,
      enabled: service.enabled !== false && !disabled.has(service.id),
      switchable: true,
    }));

  const toolRows = installedTools
    .filter(tool => tool && !hidden.has(tool.id))
    .map(tool => ({
      id: tool.id,
      kind: 'tool',
      title: tool.name || tool.title || tool.id,
      enabled: !disabled.has(tool.id),
      switchable: true,
    }));

  const installedSkills = (marketplaceSkills || [])
    .filter(skill => skill && skill.installed && !companionSkillIds.has(skill.id));
  const skillRows = installedSkills
    .filter(skill => !hidden.has(skill.id))
    .map(skill => ({
      id: skill.id,
      skillId: skill.id,
      kind: 'skill',
      title: skill.title || skill.name || skill.id,
      enabled: !disabled.has(skill.id),
      active: activeSkill === skill.id,
      switchable: true,
      unavailable: false,
    }));

  const builtinRows = (builtinSkills || [])
    .filter(skill => !(skill.unavailableIn || []).includes(scope))
    .map(skill => ({
      id: `builtin-skill:${skill.id}`,
      skillId: skill.id,
      kind: 'builtin-skill',
      title: skill.title || skill.name || skill.id,
      // 该模式提供（不可用者已在上方过滤）：权限只读开关恒为开，不可手动切换。
      enabled: true,
      active: activeSkill === skill.id,
      switchable: false,
      readonly: true,
      unavailable: false,
    }));

  const allSkillRows = [...skillRows, ...builtinRows];
  const enabledCount =
    connectedServices.filter(row => row.enabled).length +
    toolRows.filter(row => row.enabled).length +
    allSkillRows.filter(row => row.enabled).length;

  // 「所有技能已关闭」应覆盖三类携带技能的行：独立技能 + 带 companion_skills 的
  // MCP 工具（如 pptx、combo-demo）+ CLI 服务（如 feishu）。配套技能跟随所属包开关。
  const skillCarryingToolIds = installedTools
    .filter(tool => (tool.companion_skills || []).length > 0)
    .map(tool => tool.id);
  const allSkillsDisabled =
    skillRows.length > 0 &&
    skillRows.every(row => !row.enabled) &&
    skillCarryingToolIds.every(id => hidden.has(id) || disabled.has(id)) &&
    connectedServices.every(row => !row.enabled);

  return {
    connectedServices,
    toolRows,
    skillRows: allSkillRows,
    enabledCount,
    allSkillsDisabled,
  };
}

// 治理写代数门：每个控件（包开关 / 项目技能）各自记录写请求代数，begin 发号、
// 完成时对号。快速连点时较早的写可能较晚才失败（out-of-order completion），
// 对不上最新代数的完成不得回滚乐观态或提示失败——控件结局只由其最新一次写决定。
function createToggleWriteGate() {
  const generations = new Map();
  return {
    begin(key) {
      const generation = (generations.get(key) || 0) + 1;
      generations.set(key, generation);
      return generation;
    },
    isCurrent(key, generation) {
      return generations.get(key) === generation;
    },
  };
}

// 项目技能开关在代数门里的键：加前缀与包 id 空间隔离，避免同名包撞键。
const TOGGLE_WRITE_KEY_PROJECT_SKILLS = '__project_skills__';

export { buildComposerToolMenuState, createToggleWriteGate, TOGGLE_WRITE_KEY_PROJECT_SKILLS };
