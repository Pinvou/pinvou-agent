// ProjectGroupHeader 的菜单门控与内联编辑提交判定,抽成纯函数以便 node
// 侧单测(评审 finding 42:#16 的 convert 静默失败与 web 空菜单两轮回归
// 都发生在这段此前未测的逻辑上)。

// 「更多」菜单按实际可用的动作渲染:web 没有 projects 后端,onConvert 等
// 回调缺席时不渲染按钮,避免点开一个零项菜单。
function groupHeaderHasMenu(kind, handlers) {
  const { onConvert, onRename, onDelete } = handlers || {};
  return (kind === 'folder' && !!onConvert)
    || (kind === 'project' && (!!onRename || !!onDelete));
}

// 提交内联编辑:返回 { action: 'convert' | 'rename', value } 或 null(取消)。
// 「值未变 = 取消」只对重命名成立:convert 把目录名预填为默认项目名,直接
// 回车必须按预填值创建,否则默认路径静默无操作(评审 finding 16)。
function resolveGroupHeaderEdit({ mode, value, label, busy }) {
  const trimmed = String(value || '').trim();
  if (!trimmed || busy) return null;
  if (mode === 'rename' && trimmed === label) return null;
  if (mode === 'convert') return { action: 'convert', value: trimmed };
  if (mode === 'rename') return { action: 'rename', value: trimmed };
  return null;
}

export { groupHeaderHasMenu, resolveGroupHeaderEdit };
