// 「选择工作区」统一选择器(设计 §2/§3/§9.3/§9.4):项目为唯一组织单位,
// 物理文件夹被吸收为"单根项目";浏览 = 新建项目的一种方式;临时会话是
// 显式选项。热视图(最近使用排序 + 冷项目隐藏)由 ./workspacePickerState.js
// 计算,本组件纯展示;所有后果动作经 props 回调交给容器(main.jsx)。
// Web 宿主(§9.8)只渲染"临时会话"一个选项,由容器经 webOnly 传入。
import { useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { AlertTriangle, ChevronDown, FolderOpen, Layers, Search, Sparkles, X } from '../../components/icons.jsx';
import { isImeComposing } from '../../shared/ime-guard.mjs';
import { formatSessionDate } from '../../shared/date-utils.js';
import { workspaceName } from '../../shared/workspace-recents.js';
import { pickerPrimaryRoot, pickerProjectRoots, workspaceNoticeTone } from './workspacePickerState.js';

const WorkspacePickerDialog = ({
  open,
  rows,
  mode,
  language,
  busy,
  webOnly,
  excludedFolder,
  t,
  onClose,
  onSelectProject,
  onTemporary,
  onBrowse,
  onBrowseExcluded,
  onDismissExcluded,
}) => {
  const [query, setQuery] = useState('');
  // 多根项目行内展开(改选根 + 权限告知);一次只展开一行。
  const [expandedId, setExpandedId] = useState(null);
  const onCloseRef = useRef(onClose);
  const dialogRef = useRef(null);
  useEffect(() => {
    onCloseRef.current = onClose;
  });

  useEffect(() => {
    if (!open) return () => {};
    const onKey = (e) => {
      if (e.key === 'Escape' && !isImeComposing(e)) {
        e.preventDefault();
        onCloseRef.current();
      } else if (e.key === 'Tab' && dialogRef.current) {
        // Minimal focus trap(与 MoveToProjectDialog 同 idiom):Tab 在对话框
        // 内循环,不落到遮罩后的页面。
        const focusables = dialogRef.current.querySelectorAll(
          'button:not([disabled]), input:not([disabled]), [tabindex]:not([tabindex="-1"])',
        );
        if (!focusables.length) return;
        const first = focusables[0];
        const last = focusables[focusables.length - 1];
        if (e.shiftKey && document.activeElement === first) {
          e.preventDefault();
          last.focus();
        } else if (!e.shiftKey && document.activeElement === last) {
          e.preventDefault();
          first.focus();
        }
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [open]);

  if (!open || typeof document === 'undefined') return null;

  const copy = t.uiWorkspacePicker;
  const rowCls = 'w-full px-3.5 py-2.5 flex items-center gap-2.5 text-left text-[14px] rounded-2xl transition-colors text-[#1F1F1F] hover:bg-[#F1F3F4] dark:text-[#E3E3E3] dark:hover:bg-[#303134]';

  const filtered = (() => {
    const q = query.trim().toLowerCase();
    const list = Array.isArray(rows) ? rows : [];
    if (!q) return list;
    return list.filter((row) => {
      const name = String(row.project.name || '').toLowerCase();
      const roots = pickerProjectRoots(row.project).join(' ').toLowerCase();
      return name.includes(q) || roots.includes(q);
    });
  })();

  const chooseProject = (project) => {
    if (busy) return;
    const roots = pickerProjectRoots(project);
    if (roots.length === 0) return; // 纯标签项目:无根可绑定,行禁用态由渲染侧保证
    if (roots.length > 1) {
      // 多根:展开改选根 + 分模式权限告知(§9.4 告知在选中一刻)。
      setExpandedId(prev => (prev === project.id ? null : project.id));
      return;
    }
    onSelectProject(project, roots[0]);
  };

  const projectRow = ({ project, lastActivity }) => {
    const roots = pickerProjectRoots(project);
    if (roots.length === 0) return null;
    const primary = pickerPrimaryRoot(project);
    const multi = roots.length > 1;
    const expanded = expandedId === project.id;
    return (
      <div key={project.id}>
        <button
          type="button"
          disabled={busy}
          onClick={() => chooseProject(project)}
          title={primary}
          className={`${rowCls} disabled:opacity-60`}
        >
          {multi
            ? <Layers size={15} className="shrink-0 text-[#5F6368] dark:text-[#9AA0A6]" />
            : <FolderOpen size={15} className="shrink-0 text-[#0B57D0] dark:text-[#A8C7FA]" />}
          <span className="min-w-0 flex-1">
            <span className="block truncate">{project.name}</span>
            <span className="block truncate text-[12px] text-[#8A8F94] dark:text-[#9AA0A6]">
              {multi
                ? copy.multiRootSummary(roots.length, primary)
                : primary}
            </span>
          </span>
          <span className="shrink-0 text-[11px] text-[#8A8F94] dark:text-[#9AA0A6]">
            {formatSessionDate(lastActivity, language)}
          </span>
          {multi && (
            <ChevronDown
              size={13}
              className={`shrink-0 text-[#8A8F94] transition-transform ${expanded ? 'rotate-180' : ''}`}
            />
          )}
        </button>
        {multi && expanded && (
          <div className="mx-2 mb-1 rounded-2xl bg-[#EAECEF] dark:bg-[#303134] px-3.5 py-2.5">
            {/* 分模式权限告知(§9.4):受限=授权语义;YOLO=可见性语义。 */}
            <div className="mb-2 flex items-start gap-1.5 text-[12px] text-[#5F6368] dark:text-[#C4C7C5]">
              <AlertTriangle size={13} className="shrink-0 mt-0.5" />
              <span>
                {workspaceNoticeTone(mode) === 'restricted'
                  ? copy.noticeRestricted(roots.length)
                  : copy.noticeVisibility(roots.length)}
              </span>
            </div>
            {roots.map(root => (
              <button
                key={root}
                type="button"
                disabled={busy}
                onClick={() => !busy && onSelectProject(project, root)}
                title={root}
                className="w-full rounded-lg px-2.5 py-1.5 flex items-center gap-2 text-left hover:bg-black/[0.04] dark:hover:bg-white/[0.06] disabled:opacity-60"
              >
                <FolderOpen size={13} className="shrink-0 text-gray-400" />
                <span className="min-w-0 flex-1 truncate text-[12px]">{root}</span>
                {root === primary && (
                  <span className="shrink-0 text-[11px] text-[#0B57D0] dark:text-[#A8C7FA]">
                    {copy.primaryRootBadge}
                  </span>
                )}
              </button>
            ))}
          </div>
        )}
      </div>
    );
  };

  return createPortal(
    // biome-ignore lint/a11y/noStaticElementInteractions: backdrop click-to-close; keyboard path is the Escape listener and the close button
    <div
      role="presentation"
      className="fixed inset-0 z-[200] flex items-center justify-center p-4"
      style={{ background: 'rgba(0,0,0,.34)', backdropFilter: 'blur(14px) saturate(140%)', WebkitBackdropFilter: 'blur(14px) saturate(140%)' }}
      onClick={onClose}
    >
      {/* biome-ignore lint/a11y/useKeyWithClickEvents: dialog body stops bubbling so backdrop close is not triggered accidentally; not interactive itself */}
      <div
        ref={dialogRef}
        role="dialog"
        aria-modal="true"
        aria-label={copy.title}
        onClick={e => e.stopPropagation()}
        className="w-[380px] max-w-[calc(100vw-48px)] overflow-hidden rounded-[16px] shadow-2xl bg-[rgba(250,250,250,.96)] dark:bg-[rgba(44,44,46,.96)] text-[#000] dark:text-[#F2F2F7]"
        style={{ fontFamily: '-apple-system, BlinkMacSystemFont, "SF Pro Text", "PingFang SC", "Microsoft YaHei", sans-serif' }}
      >
        <div className="px-4 pt-4 pb-2 flex items-center justify-between gap-2">
          <div className="text-[15px] font-semibold truncate">{copy.title}</div>
          <button
            type="button"
            title={t.cpCancel}
            onClick={onClose}
            className="w-8 h-8 shrink-0 rounded-full flex items-center justify-center text-[#5F6368] hover:bg-[#D3D7DB] dark:text-[#C4C7C5] dark:hover:bg-[#444746]"
          >
            <X size={16} />
          </button>
        </div>
        {!webOnly && filtered.length > 6 && (
          <div className="px-4 pb-2">
            <div className="flex h-9 items-center gap-2 rounded-full px-3 bg-[#EAECEF] dark:bg-[#303134]">
              <Search size={14} className="shrink-0 text-[#5F6368] dark:text-[#9AA0A6]" />
              {/* biome-ignore lint/a11y/noAutofocus: modal opens for a single purpose; focus belongs in the filter field immediately */}
              <input autoFocus
                value={query}
                onChange={e => setQuery(e.target.value)}
                placeholder={copy.searchPlaceholder}
                className="w-full bg-transparent border-0 outline-none text-[14px] placeholder:text-[#8A8F94] dark:placeholder:text-[#9AA0A6]"
              />
            </div>
          </div>
        )}
        {excludedFolder ? (
          /* 浏览通道撞上排除列表(§3):如实告知 + 仍可以纯文件夹会话开始。 */
          <div className="px-4 pb-4 pt-1">
            <div className="rounded-2xl bg-[#EAECEF] dark:bg-[#303134] px-3.5 py-3">
              <div className="text-[13px] font-semibold mb-1">{copy.excludedTitle}</div>
              <div className="text-[12px] text-[#5F6368] dark:text-[#C4C7C5] mb-3 break-all">
                {copy.excludedBody(workspaceName(excludedFolder, copy.unknownDirectory))}
              </div>
              <div className="flex gap-2">
                <button
                  type="button"
                  disabled={busy}
                  onClick={() => !busy && onBrowseExcluded(excludedFolder)}
                  className="flex-1 h-9 rounded-full bg-[#0B57D0] text-white text-[13px] font-medium hover:bg-[#0A4CB8] disabled:opacity-50"
                >
                  {copy.excludedProceed}
                </button>
                <button
                  type="button"
                  disabled={busy}
                  onClick={onDismissExcluded}
                  className="flex-1 h-9 rounded-full bg-[#D3D7DB] dark:bg-[#444746] text-[#1F1F1F] dark:text-[#E3E3E3] text-[13px] font-medium hover:opacity-90 disabled:opacity-50"
                >
                  {t.cpCancel}
                </button>
              </div>
            </div>
          </div>
        ) : (
          <div className="px-2 pb-3 max-h-[340px] overflow-y-auto">
            {!webOnly && filtered.length === 0 && (
              <div className="px-3.5 py-4 text-[13px] text-[#8A8F94] dark:text-[#9AA0A6]">
                {copy.empty}
              </div>
            )}
            {!webOnly && filtered.map(projectRow)}
            <button
              type="button"
              disabled={busy}
              onClick={() => !busy && onTemporary()}
              className={`${rowCls} disabled:opacity-60`}
            >
              <Sparkles size={15} className="shrink-0 text-emerald-500" />
              <span className="min-w-0 flex-1">
                <span className="block truncate">{copy.temporary}</span>
                <span className="block truncate text-[12px] text-[#8A8F94] dark:text-[#9AA0A6]">{copy.temporaryDesc}</span>
              </span>
            </button>
            {!webOnly && (
              <button
                type="button"
                disabled={busy}
                onClick={() => !busy && onBrowse()}
                className={`${rowCls} disabled:opacity-60`}
              >
                <FolderOpen size={15} className="shrink-0 text-[#5F6368] dark:text-[#9AA0A6]" />
                <span className="min-w-0 flex-1">
                  <span className="block truncate">{copy.browse}</span>
                  <span className="block truncate text-[12px] text-[#8A8F94] dark:text-[#9AA0A6]">{copy.browseDesc}</span>
                </span>
              </button>
            )}
          </div>
        )}
      </div>
    </div>,
    document.body,
  );
};

export { WorkspacePickerDialog };
