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
