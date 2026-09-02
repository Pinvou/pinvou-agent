import { useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { Check, Filter, PinIcon, Search, X } from '../../components/icons.jsx';
import { ArchivedDeleteConfirmDialog, RecentItem } from '../../components/layout/NavigationComponents.jsx';
import { SessionAttachmentTitle } from '../attachments/SessionAttachmentTitle.jsx';
import {
  sessionTitlePlainText,
  sessionTitlePresentation,
} from '../attachments/attachment-message.js';
import { formatDateGroupLabel, formatSessionDate, localDateKey } from '../../shared/date-utils.js';
import { sessionRoute } from '../../shared/session-management.js';

// 对话管理页:上方搜索框,工具行「对话|已收纳」切换 + 批量管理,左侧日期栏,右侧对话列表。
// 已收纳与在线对话共用同一套日期分组/搜索/多选管线,仅数据源与行操作不同
// (在线:置顶/重命名/收纳/删除;已收纳:恢复/永久删除,按对话自身更新时间分组)。
// 无搜索词:右侧显示所选日期的对话;有搜索词:右侧按日期分组显示全部匹配项,
// 左侧只保留有匹配的日期,点击日期平滑滚动到右侧对应分组。
// eslint-disable-next-line sonarjs/cognitive-complexity -- session management page: filter/sort/group/batch-select form one cohesive pipeline; splitting it would introduce a lot of pass-through state
export const SearchView = ({ theme, history, t, language, archived = [], showArchived: showArchivedProp, onShowArchivedConsumed, onSelect, onOpenCodex, onOpenScheduledRun, onRename, onDelete, onTogglePinned, onOpenFolder, onArchive, onArchiveMany, onDeleteMany, onRestoreArchived, onRestoreMany }) => {
  const [query, setQuery] = useState('');
  const [selectedDate, setSelectedDate] = useState(null);
  const [showArchived, setShowArchived] = useState(false);
  const [batchMode, setBatchMode] = useState(false);
  const [selectedIds, setSelectedIds] = useState(() => new Set());
  const [batchDeleteConfirming, setBatchDeleteConfirming] = useState(false);
  const [archivedDeleteConfirm, setArchivedDeleteConfirm] = useState(null);
  const [listFilter, setListFilter] = useState('all'); // all | pinned | scheduled(仅对话面板生效,与侧栏任务列表同款)
  const [listSort, setListSort] = useState('pinned_first'); // pinned_first | recent
  const [filterOpen, setFilterOpen] = useState(false);
  const filterRef = useRef(null);
  const inputRef = useRef(null);
  const listRef = useRef(null);
  const groupRefs = useRef({});

  // 收纳 toast「前往查看」的一次性信号:展开「已收纳」面板后立刻消费,避免下次进页又自动打开
  useEffect(() => {
    if (showArchivedProp) {
      // eslint-disable-next-line react-hooks/set-state-in-effect -- one-shot signal: mirror and consume it immediately on receipt to avoid repeated expansion
      setShowArchived(true);
      onShowArchivedConsumed && onShowArchivedConsumed();
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps -- trigger only on the one-shot signal prop; adding the callback dep would consume the signal repeatedly
  }, [showArchivedProp]);

  const archivedList = archived || [];
  // 已收纳复用同一套日期分组管线:归一成 history 形状(updatedAt 取对话自身更新时间),
  // 原始 DTO 挂 raw 供恢复/删除按钮取 archived_at 等信息
  const archivedHistory = archivedList.map(s => {
    const titlePresentation = sessionTitlePresentation(s.title || t.newChat, s.title_attachment_names);
    return {
      id: s.id,
      title: sessionTitlePlainText(titlePresentation),
      titleContent: titlePresentation.attachments.length
        ? <SessionAttachmentTitle presentation={titlePresentation} />
        : null,
      date: formatSessionDate(s.updated_at || s.created_at, language),
      updatedAt: s.updated_at || s.created_at || '',
      raw: s,
    };
  });
  // 筛选/排序与左侧任务列表同款(仅对话面板):全部/置顶/定时任务 + 置顶优先/最近更新
  const searchFilterOptions = [
    { id: 'all', label: t.sidebarTaskFilterAll },
    { id: 'pinned', label: t.sidebarTaskFilterPinned },
    { id: 'scheduled', label: t.sidebarTaskFilterScheduled },
  ];
  const searchSortOptions = [
    { id: 'pinned_first', label: t.sidebarTaskSortPinnedFirst },
    { id: 'recent', label: t.sidebarTaskSortRecent },
  ];
  const sourceHistory = (showArchived ? archivedHistory : (history || []))
    .filter(c => {
      if (showArchived || listFilter === 'all') return true;
      if (listFilter === 'pinned') return !!c.pinned;
      if (listFilter === 'scheduled') return c.taskKind === 'scheduled';
      return true;
    })
    .sort((a, b) => {
      if (showArchived || listSort === 'recent') {
        return String(b.updatedAt || b.pinnedAt || '').localeCompare(String(a.updatedAt || a.pinnedAt || ''));
      }
      if (!!a.pinned !== !!b.pinned) return a.pinned ? -1 : 1;
      const aTime = a.pinned ? (a.pinnedAt || a.updatedAt) : (a.updatedAt || a.pinnedAt);
      const bTime = b.pinned ? (b.pinnedAt || b.updatedAt) : (b.updatedAt || b.pinnedAt);
      return String(bTime || '').localeCompare(String(aTime || ''));
    });

  // 筛选菜单:点外部/Escape 关闭(与侧栏筛选菜单同款交互)
  useEffect(() => {
    if (!filterOpen) return;
    const closeOnPointerDown = (event) => {
      if (filterRef.current && !filterRef.current.contains(event.target)) setFilterOpen(false);
    };
    const closeOnEscape = (event) => {
      if (event.key === 'Escape') {
        event.preventDefault();
        setFilterOpen(false);
      }
    };
    document.addEventListener('pointerdown', closeOnPointerDown);
    window.addEventListener('keydown', closeOnEscape);
    return () => {
      document.removeEventListener('pointerdown', closeOnPointerDown);
      window.removeEventListener('keydown', closeOnEscape);
    };
  }, [filterOpen]);

  // 按本地日历日分组:组内时间倒序,组间日期倒序,无时间戳落 'unknown' 沉底
  const dateGroups = [];
  {
    const byDate = new Map();
    // sourceHistory 已按当前面板排好序(置顶优先/最近更新),组内顺序即排序结果
    sourceHistory.forEach(chat => {
      const key = localDateKey(chat.updatedAt);
      if (!byDate.has(key)) byDate.set(key, []);
      byDate.get(key).push(chat);
    });
    byDate.forEach((rows, key) => { dateGroups.push({ key, rows }); });
    dateGroups.sort((a, b) => {
      if (a.key === 'unknown') return 1;
      if (b.key === 'unknown') return -1;
      return b.key.localeCompare(a.key);
    });
  }
  // 'all' = 全部日期;未选或所选日期已不存在时,默认落在最近一天
  const activeDate = selectedDate === 'all'
    ? 'all'
    : dateGroups.some(g => g.key === selectedDate)
      ? selectedDate
      : (dateGroups[0] ? dateGroups[0].key : null);

  const q = query.trim().toLowerCase();
  const searching = q.length > 0;
  const matchedGroups = searching
    ? dateGroups
        .map(g => ({ key: g.key, rows: g.rows.filter(c => c.title.toLowerCase().includes(q)) }))
        .filter(g => g.rows.length > 0)
    : dateGroups;
  // 左侧日期栏:搜索时只保留有匹配项的日期
  const railGroups = searching ? matchedGroups : dateGroups;
  const railTotal = railGroups.reduce((n, g) => n + g.rows.length, 0);
  const activeGroup = dateGroups.find(g => g.key === activeDate) || null;

  // 当前右侧可见的会话 id(全选范围):搜索态=全部匹配,否则=所选日期(或「全部」)的分组
  const visibleGroups = searching ? matchedGroups : (activeDate === 'all' ? dateGroups : (activeGroup ? [activeGroup] : []));
  const visibleIds = visibleGroups.flatMap(g => g.rows.map(c => c.id));
  const allVisibleSelected = visibleIds.length > 0 && visibleIds.every(id => selectedIds.has(id));

  const exitBatch = () => { setBatchMode(false); setSelectedIds(new Set()); setBatchDeleteConfirming(false); };
  const toggleSelect = (id) => setSelectedIds(prev => {
    const next = new Set(prev);
    if (next.has(id)) next.delete(id); else next.add(id);
    return next;
  });
  const toggleSelectAll = () => setSelectedIds(allVisibleSelected ? new Set() : new Set(visibleIds));
  const switchPanel = (toArchived) => { setShowArchived(toArchived); exitBatch(); };
  const runBatchArchive = () => { if (onArchiveMany) { onArchiveMany([...selectedIds]); } exitBatch(); };
  const runBatchDelete = () => { if (onDeleteMany) { onDeleteMany([...selectedIds]); } exitBatch(); };
  const runBatchRestore = () => { if (onRestoreMany) { onRestoreMany([...selectedIds]); } exitBatch(); };

  const handlePickDate = (key) => {
    setSelectedDate(key);
    if (!searching) return;
    // 搜索态:右侧是全部匹配的长列表,点击日期滚动到对应分组,「全部」回顶部
    const container = listRef.current;
    if (!container) return;
    if (key === 'all') { container.scrollTo({ top: 0, behavior: 'smooth' }); return; }
    const el = groupRefs.current[key];
    if (el) container.scrollTo({ top: el.offsetTop, behavior: 'smooth' });
  };

  const renderChatRow = (chat) => {
    if (batchMode) {
      const selected = selectedIds.has(chat.id);
      return (
        // biome-ignore lint/a11y/useSemanticElements: the batch-select row uses a custom-drawn dot checkbox style; a button would break the existing layout
        <div
          key={chat.id}
          role="button"
          tabIndex={0}
          onClick={() => toggleSelect(chat.id)}
          onKeyDown={(event) => { if (event.key === 'Enter' || event.key === ' ') { event.preventDefault(); toggleSelect(chat.id); } }}
          className="flex items-center gap-3 px-4 py-[10px] cursor-pointer rounded-[16px] transition-colors hover:bg-[#F0F4F9] dark:hover:bg-[#1E1F20]"
        >
          <span className={`w-5 h-5 shrink-0 rounded-full border flex items-center justify-center transition-colors ${
            selected
              ? 'bg-[#0B57D0] border-[#0B57D0] text-white'
              : 'border-[#C4C7C5] dark:border-[#5F6368]'
          }`}>
            {selected && <Check size={13} />}
          </span>
          {chat.pinned && <PinIcon size={12} className="shrink-0 rotate-45 text-[#8A8F94] dark:text-[#9AA0A6]" />}
          <span className="flex-1 min-w-0 truncate text-[15px] text-[#1F1F1F] dark:text-[#E3E3E3]">{chat.titleContent || chat.title}</span>
          <span className="text-[13px] shrink-0 text-[#444746] dark:text-[#C4C7C5]">{chat.date}</span>
        </div>
      );
    }
    if (showArchived) {
      // 已收纳行:恢复 / 永久删除(删除走 ArchivedDeleteConfirmDialog 二次确认)
      const s = chat.raw || chat;
      return (
        <div
          key={chat.id}
          className="flex items-center gap-2 px-4 py-[12px] rounded-[16px] transition-colors hover:bg-[#F0F4F9] dark:hover:bg-[#1E1F20]"
        >
          <span className="flex-1 min-w-0 pr-2">
            <span className="block truncate text-[15px] text-[#1F1F1F] dark:text-[#E3E3E3]">{chat.titleContent || chat.title}</span>
            <span className="block truncate text-[12px] text-[#8A8F94] dark:text-[#9AA0A6]">
              {t.searchArchivedAt(formatSessionDate(s.archived_at || s.updated_at || s.created_at, language))}
            </span>
          </span>
          <button
            type="button"
            onClick={() => onRestoreArchived && onRestoreArchived(chat.id)}
            className="shrink-0 h-8 px-3 rounded-full text-[13px] font-medium transition-colors bg-[#D3E3FD] text-[#041E49] dark:bg-[#A8C7FA]"
          >
            {t.searchRestore}
          </button>
          <button
            type="button"
            onClick={() => setArchivedDeleteConfirm(s)}
            className="shrink-0 h-8 px-3 rounded-full text-[13px] font-medium transition-colors text-[#C5221F] hover:bg-[#FAD2CF] dark:text-[#F28B82] dark:hover:bg-[#5c2b29]"
          >
            {t.cpDelete}
          </button>
        </div>
      );
    }
    // 与左侧任务列表同款行项目:置顶/重命名/删除/收纳/打开文件夹/右键菜单;
    // 定时任务运行项点按进入运行会话(与侧栏一致)。
    // 管理页只是会话清单,不高亮当前会话(active 恒 false),避免打开过的对话残留"选中"背景。
    const route = sessionRoute(chat);
    return (
      <RecentItem
        key={chat.id}
        chat={chat}
        theme={theme}
        t={t}
        active={false}
        personaTarget={false}
        onSelect={route === 'codex'
          ? () => onOpenCodex && onOpenCodex(chat.id)
          : route === 'scheduled'
            ? () => onOpenScheduledRun && onOpenScheduledRun(chat.scheduledRun)
            : onSelect}
        onRename={onRename}
        onDelete={onDelete}
        onTogglePinned={onTogglePinned}
        onOpenFolder={onOpenFolder}
        onArchive={onArchive}
      />
    );
  };

  const renderDateHeader = (key) => (
    <div className="px-4 pt-4 pb-1 text-[13px] font-medium text-[#8A8F94] dark:text-[#9AA0A6]">
      {formatDateGroupLabel(key, language)}
    </div>
  );

  return (
    <div className="flex-1 flex flex-col w-full h-full relative z-10 animate-in fade-in duration-300">
      <div className="flex-1 min-h-0 flex flex-col px-6 pt-16 pb-6">
        <div className="max-w-[960px] w-full mx-auto flex flex-col flex-1 min-h-0 relative">

          {/* Centered Search Bar */}
          <div className="shrink-0 flex items-center gap-3 px-6 py-4 rounded-full mb-4 transition-colors bg-[#F0F4F9] text-[#1F1F1F] dark:bg-[#1E1F20] dark:text-[#E3E3E3]">
            <Search size={22} className="text-[#444746] dark:text-[#C4C7C5]" />
            <input
              ref={inputRef}
              type="text"
              placeholder={t.searchPlaceholder}
              value={query}
              onChange={e => setQuery(e.target.value)}
              className="flex-1 bg-transparent border-none outline-none text-[16px] placeholder:text-[16px] placeholder:text-[#444746] dark:placeholder:text-[#C4C7C5]"
            />
            {query ? (
              <button
                type="button"
                aria-label={t.clearSearch}
                title={t.clearSearch}
                onClick={() => { setQuery(''); inputRef.current && inputRef.current.focus(); }}
                className="w-8 h-8 shrink-0 rounded-full flex items-center justify-center transition-colors text-[#444746] hover:bg-[#DDE3EA] dark:text-[#C4C7C5] dark:hover:bg-[#333537]"
              >
                <X size={16} />
              </button>
            ) : null}
          </div>

          {/* 工具行:「对话|已收纳」切换(批量模式下换成全选) + 批量管理开关;与搜索框同宽对齐 */}
          <div className="shrink-0 h-9 mb-2 flex items-center justify-between">
            {batchMode ? (
              <button
                type="button"
                onClick={toggleSelectAll}
                className="h-8 px-2 flex items-center gap-2 rounded-full text-[13px] transition-colors text-[#444746] hover:bg-[#F0F4F9] dark:text-[#C4C7C5] dark:hover:bg-[#1E1F20]"
              >
                <span className={`w-4 h-4 shrink-0 rounded-full border flex items-center justify-center transition-colors ${
                  allVisibleSelected
                    ? 'bg-[#0B57D0] border-[#0B57D0] text-white'
                    : 'border-[#C4C7C5] dark:border-[#5F6368]'
                }`}>
                  {allVisibleSelected && <Check size={11} />}
                </span>
                <span>{t.searchSelectAll} · {t.searchSelectedCount(selectedIds.size)}</span>
              </button>
            ) : (
              <div className="flex items-center gap-0.5 p-0.5 rounded-full bg-[#F0F4F9] dark:bg-[#1E1F20]">
                {[{ key: false, label: t.searchPanelChats }, { key: true, label: `${t.searchArchivedEntry} (${archivedList.length})` }].map(tab => (
                  <button
                    key={String(tab.key)}
                    type="button"
                    onClick={() => switchPanel(tab.key)}
                    className={`h-7 px-3 rounded-full text-[13px] font-medium transition-colors ${
                      showArchived === tab.key
                        ? 'bg-white text-[#1F1F1F] shadow-sm dark:bg-[#A8C7FA] dark:text-[#041E49]'
                        : 'text-[#444746] dark:text-[#C4C7C5]'
                    }`}
                  >
                    {tab.label}
                  </button>
                ))}
              </div>
            )}
            <div className="flex items-center gap-1">
              {!showArchived && (
                <div ref={filterRef} className="relative">
                  <button
                    type="button"
                    title={t.sidebarTaskFilter}
                    onClick={() => setFilterOpen(v => !v)}
                    className={`w-8 h-8 shrink-0 rounded-full flex items-center justify-center transition-colors ${
                      filterOpen || listFilter !== 'all' || listSort !== 'pinned_first'
                        ? 'bg-[#E1E5EA] text-[#444746] dark:bg-[#333537] dark:text-[#E3E3E3]'
                        : 'text-[#444746] hover:bg-[#F0F4F9] dark:text-[#C4C7C5] dark:hover:bg-[#1E1F20]'
                    }`}
                  >
                    <Filter size={15} />
                  </button>
                  {filterOpen && (
                    <div className="absolute right-0 top-9 z-50 w-44 overflow-hidden rounded-2xl border p-1.5 shadow-xl border-black/10 bg-white dark:border-white/10 dark:bg-[#202124]">
                      <div className="px-2.5 pb-1 pt-1 text-[11px] font-semibold text-[#8A8A8E] dark:text-[#8E8E93]">
                        {t.sidebarTaskFilter}
                      </div>
                      {searchFilterOptions.map(option => (
                        <button
                          key={option.id}
                          type="button"
                          onClick={() => setListFilter(option.id)}
                          className="w-full px-2.5 py-1.5 flex items-center gap-2 rounded-xl text-left text-[13px] leading-5 transition-colors text-[#1F1F1F] hover:bg-[#F1F3F4] dark:text-[#E3E3E3] dark:hover:bg-[#303134]"
                        >
                          <span className="w-4 shrink-0">{listFilter === option.id && <Check size={13} />}</span>
                          <span className="truncate">{option.label}</span>
                        </button>
                      ))}
                      <div className="my-1 h-px bg-black/10 dark:bg-white/10" />
                      <div className="px-2.5 pb-1 pt-1 text-[11px] font-semibold text-[#8A8A8E] dark:text-[#8E8E93]">
                        {t.sidebarTaskSort}
                      </div>
                      {searchSortOptions.map(option => (
                        <button
                          key={option.id}
                          type="button"
                          onClick={() => setListSort(option.id)}
                          className="w-full px-2.5 py-1.5 flex items-center gap-2 rounded-xl text-left text-[13px] leading-5 transition-colors text-[#1F1F1F] hover:bg-[#F1F3F4] dark:text-[#E3E3E3] dark:hover:bg-[#303134]"
                        >
                          <span className="w-4 shrink-0">{listSort === option.id && <Check size={13} />}</span>
                          <span className="truncate">{option.label}</span>
                        </button>
                      ))}
                    </div>
                  )}
                </div>
              )}
              <button
                type="button"
                onClick={() => (batchMode ? exitBatch() : setBatchMode(true))}
                className={`h-8 px-3 rounded-full text-[13px] font-medium transition-colors ${
                  batchMode
                    ? 'bg-[#D3E3FD] text-[#041E49] dark:bg-[#A8C7FA]'
                    : 'text-[#444746] hover:bg-[#F0F4F9] dark:text-[#C4C7C5] dark:hover:bg-[#1E1F20]'
                }`}
              >
                {batchMode ? t.searchBatchDone : t.searchBatchManage}
              </button>
            </div>
          </div>

          {/* 主体:左日期栏 + 右对话列表 */}
          <div className="flex-1 min-h-0 flex gap-2">
            {/* 日期栏(跟随「对话|已收纳」切换的数据集) */}
            <div className="w-[150px] max-sm:w-[112px] shrink-0 overflow-y-auto custom-scrollbar pr-1 flex flex-col gap-0.5">
              <button
                type="button"
                onClick={() => handlePickDate('all')}
                className={`w-full h-9 px-3 shrink-0 flex items-center justify-between gap-2 rounded-[12px] text-[13px] transition-colors ${
                  activeDate === 'all'
                    ? 'bg-[#F0F4F9] text-[#1F1F1F] font-medium dark:bg-[#1E1F20] dark:text-[#E3E3E3]'
                    : 'text-[#444746] hover:bg-[#F0F4F9] dark:text-[#C4C7C5] dark:hover:bg-[#1E1F20]'
                }`}
              >
                <span className="truncate">{t.searchDateAll}</span>
                <span className="shrink-0 text-[12px] text-[#8A8F94] dark:text-[#9AA0A6]">{railTotal}</span>
              </button>
              {railGroups.map(g => {
                const active = g.key === activeDate;
                return (
                  <button
                    key={g.key}
                    type="button"
                    onClick={() => handlePickDate(g.key)}
                    className={`w-full h-9 px-3 shrink-0 flex items-center justify-between gap-2 rounded-[12px] text-[13px] transition-colors ${
                      active
                        ? 'bg-[#F0F4F9] text-[#1F1F1F] font-medium dark:bg-[#1E1F20] dark:text-[#E3E3E3]'
                        : 'text-[#444746] hover:bg-[#F0F4F9] dark:text-[#C4C7C5] dark:hover:bg-[#1E1F20]'
                    }`}
                  >
                    <span className="truncate">{formatDateGroupLabel(g.key, language)}</span>
                    <span className="shrink-0 text-[12px] text-[#8A8F94] dark:text-[#9AA0A6]">{g.rows.length}</span>
                  </button>
                );
              })}
            </div>

            {/* 对话列表:「对话|已收纳」共用日期分组渲染,仅行项目不同 */}
            <div ref={listRef} className="relative flex-1 min-w-0 overflow-y-auto custom-scrollbar">
              {showArchived && archivedList.length === 0 && !searching ? (
                <div className="px-4 py-10 text-center text-[14px] text-[#8A8F94] dark:text-[#9AA0A6]">
                  {t.searchArchivedEmpty}
                </div>
              ) : searching ? (
                matchedGroups.length > 0 ? matchedGroups.map(g => (
                  <div key={g.key} ref={el => { if (el) groupRefs.current[g.key] = el; else delete groupRefs.current[g.key]; }}>
                    {renderDateHeader(g.key)}
                    {g.rows.map(renderChatRow)}
                  </div>
                )) : (
                  <div className="px-4 py-10 text-center text-[14px] text-[#8A8F94] dark:text-[#9AA0A6]">
                    {t.searchNoResults}
                  </div>
                )
              ) : (
                activeDate === 'all' ? dateGroups.map(g => (
                  <div key={g.key}>
                    {renderDateHeader(g.key)}
                    {g.rows.map(renderChatRow)}
                  </div>
                )) : (
                  activeGroup && (
                    <div>
                      {renderDateHeader(activeGroup.key)}
                      {activeGroup.rows.map(renderChatRow)}
                    </div>
                  )
                )
              )}
            </div>
          </div>

          {/* 批量操作条:多选模式下吸附底部(在线=收纳/删除,已收纳=恢复/删除) */}
          {batchMode && (
            <div className="absolute bottom-4 left-1/2 -translate-x-1/2 z-20 flex items-center gap-2 pl-4 pr-2 py-2 rounded-full shadow-xl border bg-white border-black/10 dark:bg-[#202124] dark:border-white/10">
              <span className="text-[13px] whitespace-nowrap text-[#444746] dark:text-[#C4C7C5]">
                {t.searchSelectedCount(selectedIds.size)}
              </span>
              {batchDeleteConfirming ? (
                // biome-ignore lint/a11y/useKeyWithClickEvents: click-bubbling stop layer; keyboard events don't need bubbling here
                // biome-ignore lint/a11y/noStaticElementInteractions: click-bubbling stop layer; non-interactive container
                <span className="flex items-center gap-1" onClick={e => e.stopPropagation()}>
                  <span className="text-[13px] whitespace-nowrap text-[#C5221F] dark:text-[#F28B82]">{t.riDelQ}</span>
                  <button
                    type="button"
                    title={t.riDelConfirm}
                    disabled={selectedIds.size === 0}
                    onClick={runBatchDelete}
                    className="w-8 h-8 rounded-full flex items-center justify-center transition-colors text-[#C5221F] hover:bg-[#FAD2CF] dark:text-[#F28B82] dark:hover:bg-[#5c2b29]"
                  >
                    <Check size={15} />
                  </button>
                  <button
                    type="button"
                    title={t.cpCancel}
                    onClick={() => setBatchDeleteConfirming(false)}
                    className="w-8 h-8 rounded-full flex items-center justify-center transition-colors text-[#5F6368] hover:bg-[#D3D7DB] dark:text-[#C4C7C5] dark:hover:bg-[#444746]"
                  >
                    <X size={14} />
                  </button>
                </span>
              ) : (
                <>
                  <button
                    type="button"
                    disabled={selectedIds.size === 0}
                    onClick={showArchived ? runBatchRestore : runBatchArchive}
                    className="h-8 px-3 rounded-full text-[13px] font-medium transition-colors disabled:opacity-40 bg-[#D3E3FD] text-[#041E49] dark:bg-[#A8C7FA]"
                  >
                    {showArchived ? t.searchRestore : t.archiveSession}
                  </button>
                  <button
                    type="button"
                    disabled={selectedIds.size === 0}
                    onClick={() => setBatchDeleteConfirming(true)}
                    className="h-8 px-3 rounded-full text-[13px] font-medium transition-colors disabled:opacity-40 text-[#C5221F] hover:bg-[#FAD2CF] dark:text-[#F28B82] dark:hover:bg-[#5c2b29]"
                  >
                    {t.cpDelete}
                  </button>
                </>
              )}
            </div>
          )}

          {/* 永久删除已收纳对话(二次确认,沿用原设置页同款弹窗) */}
          {archivedDeleteConfirm && createPortal(
            <ArchivedDeleteConfirmDialog
              theme={theme}
              t={t}
              onCancel={() => setArchivedDeleteConfirm(null)}
              onConfirm={() => {
                const id = archivedDeleteConfirm.id;
                setArchivedDeleteConfirm(null);
                if (id && onDelete) onDelete(id);
              }}
            />,
            document.body
          )}

        </div>
      </div>
    </div>
  );
};
