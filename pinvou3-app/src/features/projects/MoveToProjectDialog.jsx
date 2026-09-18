// "Move to project" picker: searchable list with the current project marked,
// plus an ungrouped entry. Purely presentational — the container passes in
// projects/assignments and receives the chosen move as
// onMove(projectId | null, addWorkspaceRoot). When the target project's roots
// do not cover the session's workspace, the picker first shows the
// move-only confirmation instead of moving at once.
import { useEffect, useMemo, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { Check, Layers, Search, X } from '../../components/icons.jsx';
import { isImeComposing } from '../../shared/ime-guard.mjs';
import { useDialogFocusRestore } from '../../hooks/useDialogFocusRestore.js';
import { useDialogFocusTrap } from '../../hooks/useDialogFocusTrap.js';
import { hasProjectWorkspace, needsAddFolderConfirm, rootPath } from './projectGrouping.js';

const MoveToProjectDialog = ({
  session,
  projects,
  currentProjectId,
  presetProjectId,
  t,
  busy,
  restoreTargetRef,
  onClose,
  onMove,
}) => {
  const [query, setQuery] = useState('');
  // 拖拽落点直达:拖到 root 未覆盖会话目录的项目上时,直接以该目标预置
  // "仅移动"确认(刻意 move-only,绝不带 add_workspace_root);初始化器
  // 即可(对话框每次打开都重新挂载)。
  const [pendingMove, setPendingMove] = useState(() => {
    if (!presetProjectId || !session) return null;
    const target = (Array.isArray(projects) ? projects.filter(Boolean) : [])
      .find(project => project.id === presetProjectId);
    if (!target) return null;
    return needsAddFolderConfirm(session, target) ? target : null;
  });
  // onClose is an inline arrow at the call site; keeping it in a ref keeps the
  // key listeners subscribed once instead of per render.
  const onCloseRef = useRef(onClose);
  const dialogRef = useRef(null);
  const searchInputRef = useRef(null);
  const confirmPanelRef = useRef(null);
  const backdropPressRef = useRef(false);
  const pendingProjectRef = useRef(null);
  // Escape 的确认面板回退与 busy 门控读 ref 镜像,保持 key 监听不随每次
  // 状态变更重订阅(与 onCloseRef 同范式);busy 时关闭会毁掉「确认面板
  // 原地重试」刻意保留的上下文。
  const busyRef = useRef(busy);
  useEffect(() => {
    onCloseRef.current = onClose;
    busyRef.current = busy;
  });
  // Initial focus goes to the filter field; on unmount focus returns to the
  // row's always-rendered label button, which the move menu item focuses
  // before the portal unmounts (see NavigationComponents) so a live element
  // is captured (shared modal-dismiss recipe). A successful move regroups the
  // sidebar and re-parents that row — the container then stores a resolver
  // for the moved row's new node in restoreTargetRef, which the hook calls
  // at close, once the regroup commit has mounted it.
  useDialogFocusRestore(dialogRef, searchInputRef, restoreTargetRef);
  // Tab 循环走共享陷阱(含 busy 全禁用时的按住与 IME 守卫);这里只保留
  // Escape 的分级(确认面板先退回列表)。
  useDialogFocusTrap(dialogRef);

  useEffect(() => {
    const onKey = (e) => {
      if (e.key === 'Escape' && !isImeComposing(e)) {
        e.preventDefault();
        if (busyRef.current) return;
        // 确认面板态先退回列表,列表态才关窗(评审 #449 finding:确认框
        // Escape 不该直接关整个弹窗)。按派生值门控:目标项目在打开期间被删
        // 时视图已回落列表,原始 state 仍为真——若读它,第一次 Escape 会被
        // 静默吞掉。
        if (pendingProjectRef.current) { setPendingMove(null); return; }
        onCloseRef.current();
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, []);

  const projectList = useMemo(
    () => (Array.isArray(projects) ? projects.filter(Boolean) : []),
    [projects],
  );
  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return projectList;
    return projectList.filter((project) => {
      const name = String(project.name || '').toLowerCase();
      const roots = (project.roots || [])
        .map((root) => String(rootPath(root) || ''))
        .join(' ');
      return name.includes(q) || roots.toLowerCase().includes(q);
    });
  }, [projectList, query]);

  // 确认态以活列表为准:目标项目在子视图打开期间被删(如另一窗口)时,快照
  // 会把一个已死 id 反复送进 store 的 project not found,失败 toast + 面板
  // 重试构成死循环;从活列表派生,项目消失即回落列表视图。
  const pendingProject = pendingMove
    ? projectList.find(project => project.id === pendingMove.id) || null
    : null;

  // 子视图进出都把焦点带到位:进入确认面板读出其标签,退回列表回到过滤框;
  // 否则被卸载的行把焦点丢在 body,只能靠 Tab 陷阱兜底。Escape 的门控读
  // 这里的派生值(ref 镜像),不读原始 state。
  useEffect(() => {
    pendingProjectRef.current = pendingProject;
    if (pendingProject) {
      confirmPanelRef.current?.focus();
    } else {
      searchInputRef.current?.focus();
    }
  }, [pendingProject]);

  if (!session || typeof document === 'undefined') return null;

  // 显示用:确认框里向用户展示的目录(侧栏投影),实际添加以命令返回为准。
  const workspacePath = hasProjectWorkspace(session) ? String(session.workspacePath || '') : '';
  const choose = (project) => {
    if (busy || project.id === currentProjectId) return;
    if (needsAddFolderConfirm(session, project)) {
      setPendingMove(project);
      return;
    }
    onMove(project.id, false);
  };
  const commitPending = () => {
    // "仅移动"语义:确认框只确认这一笔移动,绝不加 root——加 root 是
    // 领地扩张:它占住目录、改变该项目内后续会话的自动归组,影响面超出
    // 这一笔移动本身,不该由一次看似只移动的确认顺带完成。
    // 不预清确认面板:提交后弹窗保持确认态(busy 禁用按钮),成功时由容器
    // 关闭整个对话框(卸载即复位);失败时确认面板留在原处供重试/取消——
    // 若先清 pendingMove,异步进行/失败期间会回落成"选择项目"列表,
    // 看起来像点击后又弹出了另一个弹窗(评审 #449 finding:失败清目标后
    // 用户被迫重新选择,本轮正面修复)。
    if (pendingProject && !busy) onMove(pendingProject.id, false);
  };
  // 背板关闭要求按下与松开两端都落在背板上:文本拖选无论从背板起手拖进
  // 弹窗,还是从弹窗起手拖到背板,合成的 click 都落在共同祖先(背板)上,
  // 只看 click 会把用户没打算关的确认态一起丢掉。
  const handleBackdropClick = (e) => {
    if (!backdropPressRef.current || e.target !== e.currentTarget) return;
    backdropPressRef.current = false;
    if (busyRef.current) return;
    onCloseRef.current();
  };

  const rowCls = 'w-full px-3.5 py-2.5 flex items-center gap-2.5 text-left text-[14px] rounded-2xl transition-colors text-[#1F1F1F] hover:bg-[#F1F3F4] dark:text-[#E3E3E3] dark:hover:bg-[#303134]';
  const projectLabel = (project) => {
    const firstRoot = (project.roots || [])[0];
    const path = firstRoot ? String(rootPath(firstRoot) || '') : '';
    return (
      <span className="min-w-0 flex-1">
        <span className="block truncate">{project.name}</span>
        {path && <span className="block truncate text-[12px] text-[#8A8F94] dark:text-[#9AA0A6]">{path}</span>}
      </span>
    );
  };

  return createPortal(
    // biome-ignore lint/a11y/noStaticElementInteractions: backdrop click-to-close; keyboard path is the Escape listener and the cancel button
    <div
      role="presentation"
      className="fixed inset-0 z-[200] flex items-center justify-center p-4"
      style={{ background: 'rgba(0,0,0,.34)', backdropFilter: 'blur(14px) saturate(140%)', WebkitBackdropFilter: 'blur(14px) saturate(140%)' }}
      onMouseDown={(e) => { backdropPressRef.current = e.target === e.currentTarget; }}
      onMouseUp={(e) => { if (backdropPressRef.current && e.target !== e.currentTarget) backdropPressRef.current = false; }}
      onClick={handleBackdropClick}
    >
      {/* biome-ignore lint/a11y/useKeyWithClickEvents: dialog body stops bubbling so backdrop close is not triggered accidentally; not interactive itself */}
      <div
        ref={dialogRef}
        role="dialog"
        aria-modal="true"
        aria-label={t.uiProjects.moveToProject}
        onClick={e => e.stopPropagation()}
        className="w-[360px] max-w-[calc(100vw-48px)] overflow-hidden rounded-[16px] shadow-2xl bg-[rgba(250,250,250,.96)] dark:bg-[rgba(44,44,46,.96)] text-[#000] dark:text-[#F2F2F7]"
        style={{ fontFamily: '-apple-system, BlinkMacSystemFont, "SF Pro Text", "PingFang SC", "Microsoft YaHei", sans-serif' }}
      >
        <div className="px-4 pt-4 pb-2 flex items-center justify-between gap-2">
          <div className="min-w-0">
            <div className="text-[15px] font-semibold truncate">{t.uiProjects.moveToProject}</div>
            <div className="text-[12px] text-[#8A8F94] dark:text-[#9AA0A6] truncate" title={session.title}>{session.title}</div>
          </div>
          <button
            type="button"
            title={t.cpCancel}
            disabled={busy}
            onClick={onClose}
            className="w-8 h-8 shrink-0 rounded-full flex items-center justify-center text-[#5F6368] hover:bg-[#D3D7DB] dark:text-[#C4C7C5] dark:hover:bg-[#444746] disabled:opacity-50"
          >
            <X size={16} />
          </button>
        </div>
        {!pendingProject && (
          <div className="px-4 pb-2">
            <div className="flex h-9 items-center gap-2 rounded-full px-3 bg-[#EAECEF] dark:bg-[#303134]">
              <Search size={14} className="shrink-0 text-[#5F6368] dark:text-[#9AA0A6]" />
              <input
                ref={searchInputRef}
                value={query}
                onChange={e => setQuery(e.target.value)}
                placeholder={t.uiProjects.searchPlaceholder}
                aria-label={t.uiProjects.searchPlaceholder}
                className="w-full bg-transparent border-0 outline-none text-[14px] placeholder:text-[#8A8F94] dark:placeholder:text-[#9AA0A6]"
              />
            </div>
          </div>
        )}
        {pendingProject ? (
          <div className="px-4 pb-4 pt-1">
            {/* biome-ignore lint/a11y/useSemanticElements: focus target announcing the confirm step; a <fieldset> would drag form semantics and default styling into a plain confirmation panel */}
            <div
              ref={confirmPanelRef}
              tabIndex={-1}
              role="group"
              aria-label={t.uiProjects.moveConfirmTitle}
              className="rounded-2xl bg-[#EAECEF] dark:bg-[#303134] px-3.5 py-3 outline-none"
            >
              <div className="text-[13px] font-semibold mb-1">{t.uiProjects.moveConfirmTitle}</div>
              <div className="text-[12px] text-[#5F6368] dark:text-[#C4C7C5] mb-3 break-all">
                {t.uiProjects.moveConfirmBody(pendingProject.name, workspacePath)}
              </div>
              <div className="flex gap-2">
                <button
                  type="button"
                  disabled={busy}
                  onClick={() => commitPending()}
                  className="flex-1 h-9 rounded-full bg-[#0B57D0] text-white text-[13px] font-medium hover:bg-[#0A4CB8] disabled:opacity-50"
                >
                  {t.uiProjects.moveConfirm}
                </button>
                <button
                  type="button"
                  disabled={busy}
                  onClick={onClose}
                  className="flex-1 h-9 rounded-full bg-[#D3D7DB] dark:bg-[#444746] text-[#1F1F1F] dark:text-[#E3E3E3] text-[13px] font-medium hover:opacity-90 disabled:opacity-50"
                >
                  {t.cpCancel}
                </button>
              </div>
            </div>
          </div>
        ) : (
          <div className="px-2 pb-3 max-h-[320px] overflow-y-auto">
            {filtered.length === 0 && (
              <div className="px-3.5 py-4 text-[13px] text-[#8A8F94] dark:text-[#9AA0A6]">
                {projectList.length === 0 ? t.uiProjects.noProjects : t.uiProjects.noMatchProject}
              </div>
            )}
            {filtered.map(project => (
              <button
                key={project.id}
                type="button"
                aria-disabled={busy || project.id === currentProjectId}
                onClick={() => choose(project)}
                className={`${rowCls} ${busy || project.id === currentProjectId ? 'opacity-60 cursor-default' : ''}`}
              >
                <Layers size={15} className="shrink-0 text-[#5F6368] dark:text-[#9AA0A6]" />
                {projectLabel(project)}
                {project.id === currentProjectId && (
                  <span className="shrink-0 flex items-center gap-1 text-[11px] text-[#0B57D0] dark:text-[#A8C7FA]">
                    <Check size={12} />
                    {t.uiProjects.currentProject}
                  </span>
                )}
              </button>
            ))}
            <div className="my-1 h-px bg-black/10 dark:bg-white/10" />
            <button
              type="button"
              aria-disabled={busy || !currentProjectId}
              onClick={() => { if (!busy && currentProjectId) onMove(null, false); }}
              className={`${rowCls} ${busy || !currentProjectId ? 'opacity-40 cursor-default' : ''}`}
            >
              <X size={15} className="shrink-0 text-[#5F6368] dark:text-[#9AA0A6]" />
              <span className="min-w-0 flex-1 truncate">{t.uiProjects.moveToUngrouped}</span>
              {!currentProjectId && (
                <span className="sr-only">{t.uiProjects.alreadyUngrouped}</span>
              )}
            </button>
          </div>
        )}
      </div>
    </div>,
    document.body,
  );
};

export { MoveToProjectDialog };
