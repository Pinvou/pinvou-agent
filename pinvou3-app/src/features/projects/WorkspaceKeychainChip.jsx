// 会话工作区钥匙串 chip(§6):主目录名 + 附加根计数("+N"),点击弹出
// 根列表 + 「对齐到项目」动作(§9.7 会话级显式动作)。纯展示:对齐调用与
// 结果反馈(toast)由容器经 onAlign/onNotify 处理;无绑定/临时会话由
// 容器决定不渲染(canAlign=false 时只读,无动作)。
// 交互形态照抄 ComposerWorkspaceSelector(底栏小按钮 + 上弹菜单)。
import { useRef, useState } from 'react';
import { ChevronDown, FolderOpen, RefreshCw } from '../../components/icons.jsx';
import { useOutsidePointerClose } from '../../components/ComposerPopover.jsx';
import { workspaceName } from '../../shared/workspace-recents.js';

export function WorkspaceKeychainChip({ copy, primary, additionalCount, roots, canAlign, busy, onAlign }) {
  const [open, setOpen] = useState(false);
  const triggerRef = useRef(null);
  const panelRef = useRef(null);
  useOutsidePointerClose(open, () => setOpen(false), [panelRef, triggerRef]);

  const label = workspaceName(primary, copy.unknownDirectory);
  const list = Array.isArray(roots) ? roots : [];

  return (
    <div className="relative min-w-0">
      <button
        type="button"
        ref={triggerRef}
        data-testid="workspace-keychain-chip"
        disabled={busy}
        onClick={() => setOpen(value => !value)}
        className="h-7 max-w-[200px] rounded-lg px-2 inline-flex items-center gap-1.5 text-[11px] text-gray-500 dark:text-gray-400 hover:bg-black/[0.05] dark:hover:bg-white/[0.07] disabled:opacity-60"
        title={primary || ''}
      >
        <FolderOpen size={13} className="shrink-0" />
        <span className="truncate">
          {label}
          {additionalCount > 0 ? ` +${additionalCount}` : ''}
        </span>
        <ChevronDown size={12} className="shrink-0" />
      </button>
      {open && (
        <div ref={panelRef} className="absolute z-40 bottom-9 left-0 w-[300px] max-w-[calc(100vw-32px)] rounded-2xl border border-black/[0.08] dark:border-white/10 bg-white/95 dark:bg-[#202124]/95 backdrop-blur-xl shadow-xl p-2">
          <div className="px-3 pb-1 text-[10px] uppercase tracking-wider text-gray-400">
            {copy.accessibleFolders(list.length)}
          </div>
          {list.map(path => (
            <div key={path} title={path}
              className="rounded-lg px-3 py-1.5 flex items-center gap-2 text-[11px]">
              <FolderOpen size={13} className="shrink-0 text-gray-400" />
              <span className="truncate">{workspaceName(path, copy.unknownDirectory)}</span>
              {path === primary && (
                <span className="shrink-0 text-[10px] text-[#0B57D0] dark:text-[#A8C7FA]">{copy.primaryBadge}</span>
              )}
            </div>
          ))}
          {canAlign && (
            <button type="button" disabled={busy}
              onClick={() => { setOpen(false); onAlign(); }}
              className="mt-1 w-full rounded-xl px-3 py-2.5 flex items-center gap-3 text-left border-t border-black/[0.05] dark:border-white/[0.06] hover:bg-black/[0.04] dark:hover:bg-white/[0.06] disabled:opacity-60">
              <RefreshCw size={15} className="text-blue-500 shrink-0" />
              <span>
                <span className="block text-[12px] font-semibold">{copy.alignAction}</span>
                <span className="block text-[10px] text-gray-400 mt-0.5">{copy.alignActionDesc}</span>
              </span>
            </button>
          )}
        </div>
      )}
    </div>
  );
}
