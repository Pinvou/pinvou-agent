// Shared builtin-plugin judgement (docs/builtin-toolset-contract.md §3.1/§3.2):
// a list_marketplace_tools entry counts as builtin when builtin === true, or
// when the manifest declares visibility: "system" (the backend fills that field
// only for builtin plugins). Missing fields (old backend / regular plugins)
// pass through as regular plugins (undefined !== true / !== 'system').
const isBuiltinPlugin = (tool) => !!tool && (tool.builtin === true || tool.visibility === 'system');

// Full tool name → display short name: strip the `mcp_<pluginId>_` prefix (e.g.
// mcp_session-reader_read_session → read_session); names without the prefix are
// returned as-is, and the card title/hover always shows the full name.
const builtinToolShortName = (fullName, pluginId) => {
  const name = String(fullName || '');
  const prefix = `mcp_${pluginId}_`;
  return pluginId && name.startsWith(prefix) ? name.slice(prefix.length) : name;
};

export { isBuiltinPlugin, builtinToolShortName };


/** Builtin skills whose availability is session-mode-controlled (backend
 * MODE_TABLE delta): the builtin page's audit badge and the composer menu
 * must agree with the store card for these instead of claiming "always on".
 * Lives here (tools feature) so both the settings menu logic and the tool
 * cards can import it without a cross-feature back-edge. */
export const DEFAULT_BUILTIN_SKILLS = [
  {
    id: 'visual-design',
    title: '视觉设计',
    // Design-time delta (mirrors the backend MODE_TABLE): the skill is not
    // offered in these modes, so the switch renders read-only there.
    unavailableIn: ['code'],
  },
];

export const MODE_CONTROLLED_BUILTIN_SKILL_IDS = DEFAULT_BUILTIN_SKILLS
  .filter((skill) => Array.isArray(skill.unavailableIn) && skill.unavailableIn.length)
  .map((skill) => skill.id);
