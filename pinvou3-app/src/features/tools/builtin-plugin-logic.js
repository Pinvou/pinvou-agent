// 内置插件共享判定（《内置工具集长期契约》§3.1/§3.2）：list_marketplace_tools
// 条目的 builtin === true 才视为内置插件；字段缺省（旧后端/普通插件）一律按
// 普通插件放行（undefined !== true 自然通过）。
const isBuiltinPlugin = (tool) => !!tool && tool.builtin === true;

// 工具全名 → 展示短名：剥 `mcp_<pluginId>_` 前缀（如
// mcp_session-reader_read_session → read_session）；前缀不匹配时原样返回，
// 全名始终由卡片 title/hover 展示。
const builtinToolShortName = (fullName, pluginId) => {
  const name = String(fullName || '');
  const prefix = `mcp_${pluginId}_`;
  return pluginId && name.startsWith(prefix) ? name.slice(prefix.length) : name;
};

export { isBuiltinPlugin, builtinToolShortName };
