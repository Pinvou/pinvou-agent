import React, { useEffect, useLayoutEffect, useRef, useState } from 'react';
import { createPortal, flushSync } from 'react-dom';
import { Check, ChevronDown, ChevronRight, ClipboardCheck, Clock, FileChartLine, MessageCircle, Newspaper, Play, Plus, Trash2, X } from '../../components/icons.jsx';
import { bridge, useBridgeState } from '../../hooks/useBridge.js';
import { visibleUserModels } from '../../shared/model-options.js';
import { selectorMainLabel } from '../settings/model-catalog.js';
import { can } from '../../shared/platform.js';
import dailyBriefImage from '../../assets/scheduled/daily-brief.jpg';
import followUpMonitorImage from '../../assets/scheduled/follow-up-monitor.jpg';
import weeklyReviewImage from '../../assets/scheduled/weekly-review.jpg';

    // 点模板即激活（开箱即用）：工作间由任务自动分配，不再需要选目录或先暂停。
    const SCHEDULED_TASK_TEMPLATES = [
      {
        id: 'daily-brief', name: '每日早报', schedule: '每天 8:00',
        description: '汇总重要新闻、行业动态和已连接办公系统中的公司公告',
        rrule: 'FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR,SA,SU;BYHOUR=8;BYMINUTE=0',
        prompt: '整理过去 24 小时的重要新闻和行业动态，注明来源和链接；已连接飞书或企微时，补充公司公告。不要扫描用户目录，结果保存到任务工作间。',
        paused: false,
        icon: Newspaper, color: '#0A84FF', image: dailyBriefImage
      },
      {
        id: 'follow-up-monitor', name: '事项督办', schedule: '工作日 9:00',
        description: '整理逾期与临期事项，突出风险和建议下一步',
        rrule: 'FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR;BYHOUR=9;BYMINUTE=0',
        prompt: '汇总已连接飞书或企微中的逾期、今日到期和未来 3 个工作日临期事项，按优先级给出风险与下一步。仅查询整理，不发送、审批或修改；不要扫描用户目录。',
        paused: false,
        icon: ClipboardCheck, color: '#34C759', image: followUpMonitorImage
      },
      {
        id: 'weekly-review', name: '工作周报', schedule: '星期五 16:00',
        description: '根据本周办公记录生成结构清晰的工作周报',
        rrule: 'FREQ=WEEKLY;BYDAY=FR;BYHOUR=16;BYMINUTE=0',
        prompt: '根据已连接飞书或企微中的本周日程、待办和办公消息生成工作周报，包含进展、遗留、风险和下周计划。不要扫描用户目录或自动发送。',
        paused: false,
        icon: FileChartLine, color: '#AF52DE', image: weeklyReviewImage
      },
    ];

    const PREVIEW_SCHEDULED_TASKS = [
      {
        id: "preview-daily-brief",
        templateId: "daily-brief",
        name: "每日早报",
        status: "active",
        scheduleLabel: "每天 08:00",
        rrule: "FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR,SA,SU;BYHOUR=8;BYMINUTE=0",
        prompt: "整理过去 24 小时的重要新闻和行业动态，注明来源和链接；补充公司公告和重点风险。",
        model: "DeepSeek",
        nextRunOffsetMs: 1000 * 60 * 42,
        lastRunAt: "2026-07-14T08:00:00+08:00",
        hasUnreadRuns: true,
        isRunning: false,
      },
      {
        id: "preview-follow-up",
        name: "事项督办",
        status: "active",
        scheduleLabel: "工作日 09:00",
        rrule: "FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR;BYHOUR=9;BYMINUTE=0",
        prompt: "整理逾期与临期事项，突出风险、负责人和建议下一步。",
        model: "GPT-4o",
        nextRunOffsetMs: 1000 * 60 * 60 * 3 + 1000 * 60 * 12,
        lastRunAt: "2026-07-14T09:00:00+08:00",
        hasUnreadRuns: false,
        isRunning: true,
      },
      {
        id: "preview-weekly-report",
        name: "销售线索周报",
        status: "paused",
        scheduleLabel: "星期五 16:00",
        rrule: "FREQ=WEEKLY;BYDAY=FR;BYHOUR=16;BYMINUTE=0",
        prompt: "汇总本周线索新增、跟进状态、转化风险和下周重点客户。",
        model: "自动选择",
        nextRunOffsetMs: 1000 * 60 * 60 * 24 * 3,
        lastRunAt: "2026-07-10T16:00:00+08:00",
        hasUnreadRuns: false,
        isRunning: false,
      },
    ];

    const PREVIEW_SCHEDULED_RUNS = {
      'preview-daily-brief': [
        { id: 'preview-run-1', automationId: 'preview-daily-brief', sessionId: 'preview-session-1', status: 'completed', scheduledFor: '2026-07-14T08:00:00+08:00', createdAt: '2026-07-14T08:00:02+08:00', unread: true },
        { id: 'preview-run-2', automationId: 'preview-daily-brief', sessionId: 'preview-session-2', status: 'completed', scheduledFor: '2026-07-13T08:00:00+08:00', createdAt: '2026-07-13T08:00:01+08:00', unread: false },
        { id: 'preview-run-3', automationId: 'preview-daily-brief', sessionId: null, status: 'failed', scheduledFor: '2026-07-12T08:00:00+08:00', createdAt: '2026-07-12T08:00:00+08:00', error: '外部新闻源请求超时', unread: false },
      ],
      'preview-follow-up': [
        { id: 'preview-run-4', automationId: 'preview-follow-up', sessionId: 'preview-session-4', status: 'running', scheduledFor: '2026-07-14T09:00:00+08:00', createdAt: '2026-07-14T09:00:02+08:00', unread: false },
        { id: 'preview-run-5', automationId: 'preview-follow-up', sessionId: 'preview-session-5', status: 'completed', scheduledFor: '2026-07-13T09:00:00+08:00', createdAt: '2026-07-13T09:00:03+08:00', unread: false },
      ],
      'preview-weekly-report': [
        { id: 'preview-run-6', automationId: 'preview-weekly-report', sessionId: 'preview-session-6', status: 'completed', scheduledFor: '2026-07-10T16:00:00+08:00', createdAt: '2026-07-10T16:00:04+08:00', unread: false },
      ],
    };

    const WEEKDAY_OPTIONS = ['MO', 'TU', 'WE', 'TH', 'FR', 'SA', 'SU']
      .map(value => ({ value, label: value, shortLabel: value }));
    const WEEKDAY_CODES = WEEKDAY_OPTIONS.map(option => option.value);
    const HOURLY_INTERVAL_OPTIONS = Array.from({ length: 24 }, (_, index) => ({
      value: index + 1,
      label: String(index + 1),
    }));
    const normalizeScheduleDays = (value) => {
      const requested = new Set(
        (Array.isArray(value) ? value : String(value || '').split(','))
          .map(day => String(day || '').trim().toUpperCase())
          .filter(Boolean)
      );
      return WEEKDAY_CODES.filter(day => requested.has(day));
    };

    const ScheduledSelect = ({
      value, options, onChange, testId, ariaLabel, theme, minWidth = 180,
      multiple = false, minSelected = 0, onClose, emptyLabel = '—', separator = '、',
      footerAction, alwaysCommit = false,
    }) => {
      const [open, setOpen] = useState(false);
      const [menuStyle, setMenuStyle] = useState(null);
      const rootRef = useRef(null);
      const menuRef = useRef(null);
      const isDark = theme === 'dark';
      const selectedValues = multiple
        ? (Array.isArray(value) ? value : String(value || '').split(',').filter(Boolean))
        : [];
      const selected = multiple ? null : ((options || []).find(option => option.value === value) || (options || [])[0]);
      const displayLabel = multiple
        ? (options || []).filter(option => selectedValues.includes(option.value))
          .map(option => option.shortLabel || option.label).join(separator)
        : (selected ? selected.label : emptyLabel);
      const serializedValue = multiple ? selectedValues.join(',') : (value || '');
      const closeMenu = () => {
        setOpen(false);
        if (onClose) onClose();
      };
      const openMenu = (anchorElement) => {
        const nextStyle = calculateMenuPosition(anchorElement);
        flushSync(() => {
          setMenuStyle(nextStyle);
          setOpen(true);
        });
      };
      const calculateMenuPosition = (anchorOverride) => {
        const anchor = anchorOverride || rootRef.current;
        if (!anchor || typeof window === 'undefined') return null;
        const rect = anchor.getBoundingClientRect();
        const width = Math.max(minWidth, Math.ceil(rect.width));
        const estimatedHeight = Math.min(256, Math.max(44, (options || []).length * 38 + 12));
        const spaceBelow = window.innerHeight - rect.bottom - 8;
        const spaceAbove = rect.top - 8;
        const openUp = spaceBelow < estimatedHeight && spaceAbove > spaceBelow;
        const left = Math.max(8, Math.min(rect.right - width, window.innerWidth - width - 8));
        const top = openUp
          ? Math.max(8, rect.top - Math.min(estimatedHeight, spaceAbove) - 6)
          : Math.max(8, rect.bottom + 6);
        const maxHeight = Math.max(44, Math.min(256, openUp ? spaceAbove - 6 : spaceBelow - 6));
        return { left, top, minWidth: width, maxHeight };
      };
      const updateMenuPosition = () => {
        const nextStyle = calculateMenuPosition();
        if (nextStyle) setMenuStyle(nextStyle);
      };

      useLayoutEffect(() => {
        if (!open) return;
        updateMenuPosition();
        const closeOutside = (event) => {
          if (
            rootRef.current && !rootRef.current.contains(event.target) &&
            menuRef.current && !menuRef.current.contains(event.target)
          ) closeMenu();
        };
        const closeOnEscape = (event) => {
          if (event.key === 'Escape') {
            event.preventDefault();
            closeMenu();
          }
        };
        const updateOnViewportChange = () => updateMenuPosition();
        document.addEventListener('pointerdown', closeOutside);
        window.addEventListener('keydown', closeOnEscape);
        window.addEventListener('resize', updateOnViewportChange);
        window.addEventListener('scroll', updateOnViewportChange, true);
        return () => {
          document.removeEventListener('pointerdown', closeOutside);
          window.removeEventListener('keydown', closeOnEscape);
          window.removeEventListener('resize', updateOnViewportChange);
          window.removeEventListener('scroll', updateOnViewportChange, true);
        };
      }, [open]);

      const effectiveMenuStyle = open && typeof document !== 'undefined'
        ? (menuStyle || calculateMenuPosition())
        : null;
      const menu = open && effectiveMenuStyle && typeof document !== 'undefined' ? createPortal(
        <div ref={menuRef} role="listbox" aria-label={ariaLabel} aria-multiselectable={multiple || undefined}
          className={`fixed z-[1000] overflow-y-auto custom-scrollbar rounded-[12px] border p-1.5 border-[#DFE1E5] bg-white dark:border-[#3A3B3E] dark:bg-[#242528]`}
          style={{ ...effectiveMenuStyle, // isDark dynamic-value: 保留 — boxShadow 复杂多停 + effectiveMenuStyle 运行时定位
            boxShadow: isDark ? '0 12px 30px rgba(0,0,0,.34)' : '0 12px 30px rgba(60,64,67,.18)' }}>
          {(options || []).map(option => {
            const active = multiple ? selectedValues.includes(option.value) : option.value === value;
            const lastRequiredSelection = multiple && active && selectedValues.length <= minSelected;
            return (
              <button key={option.value || '__empty'} type="button" role="option" aria-selected={active}
                data-value={option.value} data-testid={testId ? `${testId}-option` : undefined}
                disabled={lastRequiredSelection}
                onClick={() => {
                  if (!multiple) {
                    closeMenu();
                    // 模型配置可被原地编辑（wire name 变化但配置 id 不变），
                    // 重选同一选项也要重新提交，治愈 EnginePool 的过期绑定快照。
                    if (alwaysCommit || !active) onChange(option.value);
                    return;
                  }
                  const nextValues = new Set(selectedValues);
                  if (active) nextValues.delete(option.value);
                  else nextValues.add(option.value);
                  onChange((options || []).filter(item => nextValues.has(item.value)).map(item => item.value));
                }}
                className={`w-full min-h-9 rounded-[8px] px-3 py-2 flex items-center gap-3 text-left text-[14px] transition-colors disabled:cursor-not-allowed disabled:opacity-60 ${active ? 'bg-[#E8F0FE] text-[#174EA6] dark:bg-[#364A66] dark:text-[#D2E3FC]' : 'text-[#202124] hover:bg-[#F1F3F4] dark:text-[#E3E3E3] dark:hover:bg-[#303134]'}`}>
                <span className="min-w-0 flex-1 truncate">{option.label}</span>
                <Check size={15} className={`shrink-0 ${active ? 'opacity-100' : 'opacity-0'}`} />
              </button>
            );
          })}
          {footerAction && (
            <>
              <div className={`my-1 mx-2 h-px bg-[#DFE1E5] dark:bg-[#3A3B3E]`} />
              <button type="button"
                onClick={() => { closeMenu(); footerAction.onClick(); }}
                className={`flex w-full min-h-9 items-center gap-3 rounded-[8px] px-3 py-2 text-left text-[14px] transition-colors text-[#202124] hover:bg-[#F1F3F4] dark:text-[#E3E3E3] dark:hover:bg-[#303134]`}>
                <Plus size={15} className={`shrink-0 text-[#5F6368] dark:text-[#9AA0A6]`} />
                <span className="min-w-0 flex-1 truncate">{footerAction.label}</span>
              </button>
            </>
          )}
        </div>,
        document.body
      ) : null;

      return (
        <div ref={rootRef} className="relative justify-self-end min-w-0">
          <button type="button" value={serializedValue} data-testid={testId}
            aria-label={ariaLabel} aria-haspopup="listbox" aria-expanded={open}
            onClick={(event) => open ? closeMenu() : openMenu(event.currentTarget)}
            className={`h-8 max-w-[260px] rounded-[9px] pl-3 pr-2 inline-flex items-center justify-end gap-2 text-[14px] font-medium transition-colors outline-none focus-visible:ring-2 focus-visible:ring-[#0B57D0]/40 text-[#1F1F1F] hover:bg-[#F1F3F4] dark:text-[#E3E3E3] dark:hover:bg-[#2B2C2F]`}>
            <span className="truncate">{displayLabel || emptyLabel}</span>
            <ChevronDown size={15} className={`shrink-0 transition-transform ${open ? 'rotate-180' : ''} text-[#73777D] dark:text-[#9AA0A6]`} />
          </button>
          {menu}
        </div>
      );
    };

    // iOS 风滚轮列:CSS scroll-snap 提供惯性 + 逐格吸附,滚动停稳(130ms 静默)后提交居中项。
    const WHEEL_ITEM_H = 32;
    const WHEEL_VISIBLE_H = 160;
    const WheelColumn = ({ values, value, onChange, ariaLabel, testId }) => {
      const listRef = useRef(null);
      const settleRef = useRef(null);
      useEffect(() => {
        const el = listRef.current;
        if (el) el.scrollTop = Math.max(0, values.indexOf(value)) * WHEEL_ITEM_H;
        return () => clearTimeout(settleRef.current);
      }, [value]);
      function settle() {
        const el = listRef.current;
        if (!el) return;
        const index = Math.min(values.length - 1, Math.max(0, Math.round(el.scrollTop / WHEEL_ITEM_H)));
        el.scrollTo({ top: index * WHEEL_ITEM_H, behavior: 'smooth' });
        if (values[index] !== value) onChange(values[index]);
      }
      function onScroll() {
        clearTimeout(settleRef.current);
        settleRef.current = setTimeout(settle, 130);
      }
      function pick(next) {
        clearTimeout(settleRef.current);
        const el = listRef.current;
        if (el) el.scrollTo({ top: Math.max(0, values.indexOf(next)) * WHEEL_ITEM_H, behavior: 'smooth' });
        if (next !== value) onChange(next);
      }
      return (
        <div ref={listRef} onScroll={onScroll} role="listbox" aria-label={ariaLabel} data-testid={testId} data-wheel-col
          className="relative overflow-y-auto overscroll-contain"
          style={{
            height: WHEEL_VISIBLE_H, width: 56, scrollSnapType: 'y mandatory', scrollbarWidth: 'none',
            paddingTop: (WHEEL_VISIBLE_H - WHEEL_ITEM_H) / 2, paddingBottom: (WHEEL_VISIBLE_H - WHEEL_ITEM_H) / 2,
          }}>
          {values.map(item => (
            <button key={item} type="button" role="option" aria-selected={item === value} data-value={item}
              onClick={() => pick(item)}
              className={`w-full text-center text-[15px] tabular-nums transition-colors duration-150 ${item === value ? 'font-semibold text-[#1F1F1F] dark:text-[#E3E3E3]' : 'text-[#A0A3A8] hover:text-[#5F6368] dark:text-[#777B82] dark:hover:text-[#B9BCC1]'}`}
              style={{ height: WHEEL_ITEM_H, lineHeight: `${WHEEL_ITEM_H}px`, scrollSnapAlign: 'center' }}>
              {item}
            </button>
          ))}
        </div>
      );
    };

    // 时/分选择器:触发区是只读输入框(显示 HH:MM),点开 iOS 风双滚轮。
    const ScheduledTimeWheel = ({ value, onChange, theme, testId, ariaLabel, placeholder = '', hourAriaLabel, minuteAriaLabel }) => {
      const [open, setOpen] = useState(false);
      const [menuStyle, setMenuStyle] = useState(null);
      const rootRef = useRef(null);
      const menuRef = useRef(null);
      const isDark = theme === 'dark';
      const valid = /^\d{2}:\d{2}$/.test(value || '');
      const hour = valid ? value.slice(0, 2) : '08';
      const minute = valid ? value.slice(3, 5) : '00';
      const hours = Array.from({ length: 24 }, (_, i) => String(i).padStart(2, '0'));
      const minutes = Array.from({ length: 60 }, (_, i) => String(i).padStart(2, '0'));
      const updateMenuPosition = () => {
        const anchor = rootRef.current;
        if (!anchor || typeof window === 'undefined') return;
        const rect = anchor.getBoundingClientRect();
        const width = 142;
        const height = WHEEL_VISIBLE_H + 18;
        const left = Math.max(8, Math.min(rect.right - width, window.innerWidth - width - 8));
        const top = rect.bottom + 6 + height > window.innerHeight
          ? Math.max(8, rect.top - height - 6)
          : Math.max(8, rect.bottom + 6);
        setMenuStyle({ left, top });
      };

      useEffect(() => {
        if (!open) return;
        updateMenuPosition();
        const closeOutside = (event) => {
          if (
            rootRef.current && !rootRef.current.contains(event.target) &&
            menuRef.current && !menuRef.current.contains(event.target)
          ) setOpen(false);
        };
        const closeOnEscape = (event) => {
          if (event.key === 'Escape') setOpen(false);
        };
        const updateOnViewportChange = () => updateMenuPosition();
        document.addEventListener('pointerdown', closeOutside);
        document.addEventListener('keydown', closeOnEscape);
        window.addEventListener('resize', updateOnViewportChange);
        window.addEventListener('scroll', updateOnViewportChange, true);
        return () => {
          document.removeEventListener('pointerdown', closeOutside);
          document.removeEventListener('keydown', closeOnEscape);
          window.removeEventListener('resize', updateOnViewportChange);
          window.removeEventListener('scroll', updateOnViewportChange, true);
        };
      }, [open]);

      // isDark dynamic-value: 保留 — surface 供 linear-gradient(模板插值)用;boxShadow 复杂多停保留。
      const surface = isDark ? '#242528' : '#FFFFFF';
      const wheel = open && menuStyle && typeof document !== 'undefined' ? createPortal(
        <div ref={menuRef} data-testid={`${testId}-wheel`} role="dialog" aria-label={ariaLabel}
          className={`fixed z-[1000] flex items-stretch gap-0.5 rounded-[14px] border px-2.5 py-2 border-[#DFE1E5] bg-[#FFFFFF] dark:border-[#3A3B3E] dark:bg-[#242528]`}
          style={{ ...menuStyle, boxShadow: isDark ? '0 12px 30px rgba(0,0,0,.34)' : '0 12px 30px rgba(60,64,67,.18)' }}>
          <style>{'[data-wheel-col]::-webkit-scrollbar{display:none}'}</style>
          <div aria-hidden="true" className={`pointer-events-none absolute inset-x-2 z-0 rounded-[9px] bg-black/[0.05] dark:bg-white/[0.08]`}
            style={{ top: 8 + (WHEEL_VISIBLE_H - WHEEL_ITEM_H) / 2, height: WHEEL_ITEM_H }} />
          <div aria-hidden="true" className="pointer-events-none absolute inset-x-1 top-1 z-10 h-11 rounded-t-[12px]"
            style={{ background: `linear-gradient(${surface}, transparent)` }} />
          <div aria-hidden="true" className="pointer-events-none absolute inset-x-1 bottom-1 z-10 h-11 rounded-b-[12px]"
            style={{ background: `linear-gradient(transparent, ${surface})` }} />
          <WheelColumn values={hours} value={hour} onChange={next => onChange(`${next}:${minute}`)}
            ariaLabel={hourAriaLabel} testId={`${testId}-hour`} />
          <span className={`self-center text-[15px] font-semibold text-[#1F1F1F] dark:text-[#E3E3E3]`}>:</span>
          <WheelColumn values={minutes} value={minute} onChange={next => onChange(`${hour}:${next}`)}
            ariaLabel={minuteAriaLabel} testId={`${testId}-minute`} />
        </div>,
        document.body
      ) : null;
      return (
        <span ref={rootRef} className="relative justify-self-end">
          <input readOnly data-testid={testId} value={valid ? value : ''} placeholder={placeholder} aria-label={ariaLabel}
            aria-haspopup="listbox" aria-expanded={open}
            onClick={() => {
              if (!valid) onChange('08:00');
              setOpen(current => !current);
            }}
            className={`w-[88px] cursor-pointer bg-transparent text-right font-medium outline-none placeholder:text-gray-400 text-[#1F1F1F] dark:text-[#E3E3E3]`} />
          {wheel}
        </span>
      );
    };

    const ScheduledTasksView = ({ theme, t, onOpenChat, onGotoModelSettings }) => {
      const bs = useBridgeState(['scheduled', 'models']);
      const appState = bs || {};
      const realTasks = appState.scheduledTasks || [];
      const rawSelectedDetail = appState.scheduledTaskDetail || null;
      const rawRuns = appState.scheduledTaskRuns || [];
      const loading = !!appState.scheduledTaskLoading;
      const busyAction = appState.scheduledTaskBusyAction || null;
      const error = appState.scheduledTaskError || null;
      const scheduledCopy = t.uiScheduled;
      const modelManageAction = can('modelManagement') && onGotoModelSettings
        ? { label: t.manageModels, onClick: onGotoModelSettings }
        : undefined;
      const weekdayOptions = WEEKDAY_OPTIONS.map((option, index) => ({
        ...option,
        label: scheduledCopy.weekdays[index][0],
        shortLabel: scheduledCopy.weekdays[index][1],
      }));
      const hourlyIntervalOptions = HOURLY_INTERVAL_OPTIONS.map(option => ({
        ...option,
        label: scheduledCopy.hourCount(option.value),
      }));
      const canOpenTaskFolder = can('externalSystemOpen');
      const [taskFilter, setTaskFilter] = useState('all');
      const [clockNow, setClockNow] = useState(() => Date.now());
      const [previewSelectedId, setPreviewSelectedId] = useState(null);
      const [previewTaskStatus, setPreviewTaskStatus] = useState({});
      const [previewCreatedTasks, setPreviewCreatedTasks] = useState([]);
      const previewMode = !bridge.available && realTasks.length === 0;
      const tasks = previewMode
        ? [...PREVIEW_SCHEDULED_TASKS, ...previewCreatedTasks].map(task => ({
          ...task,
          ...(scheduledCopy.previewTasks[task.id] || {}),
          model: task.model === '自动选择' ? scheduledCopy.autoModel : task.model,
          status: previewTaskStatus[task.id] || task.status,
          nextRunAt: task.nextRunAt || new Date(clockNow + (task.nextRunOffsetMs || 1000 * 60 * 60)).toISOString(),
        }))
        : realTasks;
      const selectedId = appState.selectedScheduledTaskId || null;
      const effectiveSelectedId = previewMode ? previewSelectedId : selectedId;
      const selectedDetail = previewMode
        ? tasks.find(task => task.id === effectiveSelectedId) || null
        : rawSelectedDetail;
      const runs = previewMode && effectiveSelectedId
        ? (PREVIEW_SCHEDULED_RUNS[effectiveSelectedId] || []).map(run => ({
          ...run,
          error: run.error ? scheduledCopy.previewRunError : run.error,
        }))
        : rawRuns;
      const [createForm, setCreateForm] = useState(null);
      const [createScheduleRepeatIntent, setCreateScheduleRepeatIntent] = useState(null);
      const [deleteConfirmTask, setDeleteConfirmTask] = useState(null);
      const [detailForm, setDetailForm] = useState(null);
      const [scheduleRepeatIntent, setScheduleRepeatIntent] = useState(null);
      const [saveState, setSaveState] = useState('idle');
      const saveTimerRef = useRef(null);
      const pendingPatchRef = useRef({});
      const editTaskIdRef = useRef(null);
      const saveChainRef = useRef(Promise.resolve());
      const saveSequenceRef = useRef(0);
      const latestFieldSequenceRef = useRef({});
      const mountedRef = useRef(true);

      useEffect(() => {
        if (!bridge || !bridge.scheduled.refreshScheduledTaskData) return;
        const refresh = () => bridge.scheduled.refreshScheduledTaskData(20).catch(() => {});
        refresh();
        const timer = setInterval(() => {
          refresh();
        }, 3000);
        return () => clearInterval(timer);
      }, []);

      useEffect(() => {
        const timer = setInterval(() => setClockNow(Date.now()), 1000);
        return () => clearInterval(timer);
      }, []);

      useEffect(() => {
        if (!createForm) return;
        const closeOnEscape = (event) => {
          if (event.key === 'Escape' && !busyAction) setCreateForm(null);
        };
        window.addEventListener('keydown', closeOnEscape);
        return () => window.removeEventListener('keydown', closeOnEscape);
      }, [createForm, busyAction]);

      const sortTasks = (items) => [...(items || [])].sort((a, b) => {
        const aActive = a.status === 'active' || a.isRunning;
        const bActive = b.status === 'active' || b.isRunning;
        if (aActive !== bActive) return aActive ? -1 : 1;
        const aNext = new Date(a.nextRunAt || 8640000000000000).getTime();
        const bNext = new Date(b.nextRunAt || 8640000000000000).getTime();
        if (aNext !== bNext) return aNext - bNext;
        return String(b.lastRunAt || b.createdAt || b.id || '').localeCompare(String(a.lastRunAt || a.createdAt || a.id || ''));
      });
      const filtered = sortTasks(tasks.filter(task => {
        const matchesFilter = taskFilter === 'all'
          || (taskFilter === 'active' && task.status === 'active')
          || (taskFilter === 'paused' && task.status !== 'active');
        return matchesFilter;
      }));
      const selected = tasks.find(task => task.id === effectiveSelectedId) || null;
      const detail = selectedDetail && selected && selectedDetail.id === selected.id ? selectedDetail : selected;
      const bodyText = 'text-[#1F1F1F] dark:text-[#E3E3E3]';
      const fmtDateTime = (value) => {
        if (!value) return scheduledCopy.notScheduled;
        const d = new Date(value);
        if (Number.isNaN(d.getTime())) return value;
        const p = (n) => String(n).padStart(2, '0');
        return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`;
      };
      const statusLabel = (value) => {
        if (value === 'active') return scheduledCopy.active;
        if (value === 'paused') return scheduledCopy.paused;
        return value || scheduledCopy.unknown;
      };
      const taskListStatusLabel = (value) => value === 'active' ? scheduledCopy.enabled : scheduledCopy.paused;
      const runStatusLabel = (value) => scheduledCopy.runStatus[value] || value || scheduledCopy.unknown;
      const taskSummary = (task) => {
        const schedule = task.scheduleLabel || scheduledCopy.noSchedule;
        if (task.status !== 'active') return `${schedule} · ${statusLabel(task.status)}`;
        if (!task.nextRunAt) return `${schedule} · ${scheduledCopy.waitingDispatch}`;
        const next = new Date(task.nextRunAt);
        if (Number.isNaN(next.getTime())) return `${schedule} · ${scheduledCopy.waitingDispatch}`;
        const now = new Date(clockNow);
        const pad = value => String(value).padStart(2, '0');
        const sameDay = next.getFullYear() === now.getFullYear()
          && next.getMonth() === now.getMonth()
          && next.getDate() === now.getDate();
        const exact = `${sameDay ? '' : scheduledCopy.date(next.getMonth() + 1, next.getDate())}${pad(next.getHours())}:${pad(next.getMinutes())}`;
        const totalSeconds = Math.max(0, Math.ceil((next.getTime() - clockNow) / 1000));
        const days = Math.floor(totalSeconds / 86400);
        const hours = Math.floor((totalSeconds % 86400) / 3600);
        const minutes = Math.floor((totalSeconds % 3600) / 60);
        const seconds = totalSeconds % 60;
        let remaining = scheduledCopy.soon;
        if (days > 0) remaining = scheduledCopy.daysAfter(days, hours);
        else if (hours > 0) remaining = scheduledCopy.hoursAfter(hours, minutes);
        else if (minutes > 0) remaining = scheduledCopy.minutesAfter(minutes, seconds);
        else if (seconds > 0) remaining = scheduledCopy.secondsAfter(seconds);
        return (
          <>
            <span>{schedule} · </span>
            <span data-testid="scheduled-task-next-run"
              className={`font-semibold text-[#1769B0] dark:text-[#7CB7F0]`}>
              {scheduledCopy.nextRun(exact, remaining)}
            </span>
          </>
        );
      };
      const savedModels = visibleUserModels(appState.savedModels || []);
      const activeModel = savedModels.find(model => model.id === appState.activeModelId) || savedModels[0] || null;
      const modelIdForTask = (task) => {
        if (!task) return '';
        if (task.modelId) return task.modelId;
        if (!task.model) return activeModel && activeModel.id || '';
        const matches = savedModels.filter(model => model.model === task.model);
        return matches.length === 1 ? matches[0].id : '';
      };
      const visibleSuggestions = SCHEDULED_TASK_TEMPLATES.map(template => ({
        ...template,
        ...(scheduledCopy.templateMap[template.id] || {}),
      }));
      const detailFormIsValid = !!detailForm &&
        !!String(detailForm.name || '').trim() &&
        !!String(detailForm.prompt || '').trim() &&
        !!String(detailForm.rrule || '').trim();
      function taskForm(task) {
        if (!task) return null;
        return {
          id: task.id,
          name: task.name || '',
          prompt: task.prompt || '',
          rrule: task.rrule || 'FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR;BYHOUR=8;BYMINUTE=0',
          model: task.model || (activeModel && activeModel.model) || '',
          modelId: modelIdForTask(task),
        };
      }

      useEffect(() => {
        if (saveTimerRef.current) clearTimeout(saveTimerRef.current);
        saveTimerRef.current = null;
        pendingPatchRef.current = {};
        editTaskIdRef.current = detail && detail.id || null;
        setDetailForm(taskForm(detail));
        setScheduleRepeatIntent(null);
        setSaveState('idle');
      }, [effectiveSelectedId, detail && detail.id]);

      useEffect(() => {
        mountedRef.current = true;
        return () => {
          mountedRef.current = false;
          if (saveTimerRef.current) clearTimeout(saveTimerRef.current);
          saveTimerRef.current = null;
          const taskId = editTaskIdRef.current;
          saveChainRef.current.catch(() => {}).then(() => {
            const patch = pendingPatchRef.current;
            const invalid = ['name', 'prompt', 'rrule'].some(key =>
              Object.prototype.hasOwnProperty.call(patch, key) && !String(patch[key] || '').trim()
            );
            if (!taskId || invalid || !Object.keys(patch).length || !bridge || !bridge.scheduled.updateScheduledTask) return null;
            pendingPatchRef.current = {};
            return bridge.scheduled.updateScheduledTask(taskId, patch);
          }).catch(() => {});
        };
      }, []);

      function persistDetailPatch(taskId, patch) {
        if (!taskId || !bridge || !bridge.scheduled.updateScheduledTask || !Object.keys(patch || {}).length) {
          return Promise.resolve({ok: true, skipped: true});
        }
        const payload = {...patch};
        const sequence = ++saveSequenceRef.current;
        Object.keys(payload).forEach(key => { latestFieldSequenceRef.current[key] = sequence; });
        if (mountedRef.current) setSaveState('saving');
        const request = saveChainRef.current.catch(() => {}).then(() => bridge.scheduled.updateScheduledTask(taskId, payload)).then(updated => {
          if (mountedRef.current && editTaskIdRef.current === taskId && sequence === saveSequenceRef.current) {
            setSaveState(Object.keys(pendingPatchRef.current).length ? 'editing' : 'saved');
          }
          return {ok: true, updated};
        }).catch(error => {
          if (editTaskIdRef.current === taskId) {
            const restored = {...pendingPatchRef.current};
            const failureIsCurrent = Object.keys(payload).some(key => latestFieldSequenceRef.current[key] === sequence);
            Object.keys(payload).forEach(key => {
              if (latestFieldSequenceRef.current[key] === sequence && !Object.prototype.hasOwnProperty.call(restored, key)) {
                restored[key] = payload[key];
              }
            });
            pendingPatchRef.current = restored;
            if (mountedRef.current && failureIsCurrent) setSaveState('error');
          }
          return {ok: false, error};
        });
        saveChainRef.current = request;
        return request;
      }

      function flushTextEdits() {
        if (saveTimerRef.current) clearTimeout(saveTimerRef.current);
        saveTimerRef.current = null;
        const patch = pendingPatchRef.current;
        const hasBlankRequiredField = ['name', 'prompt', 'rrule'].some(key =>
          Object.prototype.hasOwnProperty.call(patch, key) && !String(patch[key] || '').trim()
        );
        if (hasBlankRequiredField) {
          if (mountedRef.current) setSaveState('invalid');
          return Promise.resolve({ok: false, invalid: true});
        }
        pendingPatchRef.current = {};
        return persistDetailPatch(editTaskIdRef.current, patch);
      }

      async function flushBeforeAction() {
        let result = await flushTextEdits();
        await saveChainRef.current.catch(() => {});
        if (Object.keys(pendingPatchRef.current).length && !(result && result.invalid)) {
          result = await flushTextEdits();
          await saveChainRef.current.catch(() => {});
        }
        return !!(!Object.keys(pendingPatchRef.current).length && (!result || result.ok !== false));
      }

      function editTextField(key, value) {
        setDetailForm(current => current ? {...current, [key]: value} : current);
        pendingPatchRef.current = {...pendingPatchRef.current, [key]: value};
        setSaveState('editing');
        if (saveTimerRef.current) clearTimeout(saveTimerRef.current);
        saveTimerRef.current = setTimeout(() => { flushTextEdits().catch(() => {}); }, 300);
      }

      function finishTextField(key) {
        const required = key === 'name' || key === 'prompt' || key === 'rrule';
        const value = detailForm && detailForm[key];
        if (required && !String(value || '').trim()) {
          if (saveTimerRef.current) clearTimeout(saveTimerRef.current);
          saveTimerRef.current = null;
          const pending = {...pendingPatchRef.current};
          delete pending[key];
          pendingPatchRef.current = pending;
          const persisted = detail && detail[key] != null ? detail[key] : '';
          setDetailForm(current => current ? {...current, [key]: persisted} : current);
          setSaveState('invalid');
          if (Object.keys(pending).length) flushTextEdits().catch(() => {});
          return;
        }
        flushTextEdits().catch(() => {});
      }

      function editImmediateField(key, value) {
        return editImmediateFields({[key]: value});
      }

      function editImmediateFields(patch) {
        const taskId = editTaskIdRef.current;
        setDetailForm(current => current ? {...current, ...patch} : current);
        flushTextEdits();
        return persistDetailPatch(taskId, patch);
      }

      function editModel(modelId) {
        const selectedModel = savedModels.find(model => model.id === modelId);
        if (!selectedModel) return;
        return editImmediateFields({
          model: selectedModel.model,
          modelId: selectedModel.id,
        });
      }

      async function selectTask(id) {
        setCreateForm(null);
        setCreateScheduleRepeatIntent(null);
        if (previewMode) {
          setPreviewSelectedId(id);
          return;
        }
        if (!(await flushBeforeAction())) return;
        if (bridge && bridge.scheduled.selectScheduledTask) bridge.scheduled.selectScheduledTask(id);
        if (id && bridge && bridge.scheduled.refreshScheduledTaskData) bridge.scheduled.refreshScheduledTaskData(20).catch(() => {});
      }

      async function startTemplate(template) {
        if (!(await flushBeforeAction())) return;
        if (previewMode) setPreviewSelectedId(null);
        else if (bridge && bridge.scheduled.selectScheduledTask) bridge.scheduled.selectScheduledTask(null);
        setCreateScheduleRepeatIntent(null);
        setCreateForm({
          templateId: template.id,
          name: template.name,
          prompt: template.prompt,
          rrule: template.rrule,
          paused: !!template.paused,
        });
      }

      async function startBlankTask() {
        if (!(await flushBeforeAction())) return;
        if (previewMode) setPreviewSelectedId(null);
        else if (bridge && bridge.scheduled.selectScheduledTask) bridge.scheduled.selectScheduledTask(null);
        setCreateScheduleRepeatIntent(null);
        setCreateForm({
          name: '',
          prompt: '',
          rrule: 'FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR;BYHOUR=8;BYMINUTE=0',
        });
      }

      async function submitCustomTask(event) {
        event.preventDefault();
        if (busyAction || !createForm) return;
        const name = String(createForm.name || '').trim();
        const prompt = String(createForm.prompt || '').trim();
        if (!name || !prompt) return;
        if (previewMode) {
          const created = {
            id: `preview-created-${Date.now()}`,
            templateId: createForm.templateId || null,
            name,
            status: createForm.paused ? 'paused' : 'active',
            scheduleLabel: scheduleRepeatLabel(scheduleEditorValue(createForm.rrule)),
            rrule: createForm.rrule,
            prompt,
            model: activeModel && activeModel.model || scheduledCopy.autoModel,
            modelId: activeModel && activeModel.id || null,
            nextRunAt: new Date(clockNow + 1000 * 60 * 60).toISOString(),
            hasUnreadRuns: false,
            isRunning: false,
          };
          setPreviewCreatedTasks(current => [created, ...current]);
          setPreviewSelectedId(null);
          setCreateForm(null);
          setCreateScheduleRepeatIntent(null);
          return;
        }
        if (!bridge || !bridge.scheduled.createScheduledTask) return;
        try {
          await bridge.scheduled.createScheduledTask({
            templateId: createForm.templateId || undefined,
            name,
            prompt,
            rrule: createForm.rrule,
            model: activeModel && activeModel.model || null,
            modelId: activeModel && activeModel.id || null,
            mode: 'yolo',
            paused: !!createForm.paused,
            selectAfterCreate: false,
          });
          if (bridge.scheduled.selectScheduledTask) bridge.scheduled.selectScheduledTask(null);
          setCreateForm(null);
          setCreateScheduleRepeatIntent(null);
        } catch (_) {}
      }

      function requestDeleteTask(e, task) {
        if (e) e.stopPropagation();
        if (!task) return;
        setDeleteConfirmTask(task);
      }

      function cancelDeleteTask(e) {
        if (e) e.stopPropagation();
        if (busyAction) return;
        setDeleteConfirmTask(null);
      }

      async function confirmDeleteTask(e, task) {
        if (e) e.stopPropagation();
        const id = task && task.id;
        if (!bridge || !bridge.scheduled.deleteScheduledTask || busyAction) return;
        try {
          await bridge.scheduled.deleteScheduledTask(id);
          setDeleteConfirmTask(null);
        } catch (_) {}
      }

      async function toggleTaskPaused(task) {
        if (!task) return;
        if (previewMode) {
          setPreviewTaskStatus(current => ({
            ...current,
            [task.id]: task.status === 'active' ? 'paused' : 'active',
          }));
          return;
        }
        if (!bridge) return;
        try {
          if (!(await flushBeforeAction())) return;
          if (task.status === 'active' && bridge.scheduled.pauseScheduledTask) await bridge.scheduled.pauseScheduledTask(task.id);
          if (task.status !== 'active' && bridge.scheduled.resumeScheduledTask) await bridge.scheduled.resumeScheduledTask(task.id);
        } catch (_) {}
      }

      async function toggleTask(e, task) {
        if (e) e.stopPropagation();
        return toggleTaskPaused(task);
      }

      async function startChatCreation() {
        if (!bridge || !bridge.scheduled.startScheduledTaskChat) {
          if (onOpenChat) onOpenChat();
          return;
        }
        try {
          if (!(await flushBeforeAction())) return;
          const started = await bridge.scheduled.startScheduledTaskChat();
          if (started && onOpenChat) onOpenChat();
        } catch (_) {}
      }

      async function runTaskNow(id) {
        const editingThisTask = editTaskIdRef.current === id && !!detailForm;
        if (!bridge || !bridge.scheduled.runScheduledTaskNow || busyAction || (editingThisTask && !detailFormIsValid)) return;
        try {
          if (!(await flushBeforeAction())) return;
          await bridge.scheduled.runScheduledTaskNow(id);
        } catch (_) {}
      }

      async function saveDetailAndClose() {
        if (busyAction) return;
        if (!(await flushBeforeAction())) return;
        if (previewMode) setPreviewSelectedId(null);
        else if (bridge && bridge.scheduled.selectScheduledTask) bridge.scheduled.selectScheduledTask(null);
      }

      async function openRunChat(run) {
        if (!run || !run.sessionId || !bridge || !bridge.scheduled.openScheduledRunChat) return;
        try {
          if (!(await flushBeforeAction())) return;
          const opened = await bridge.scheduled.openScheduledRunChat(run, detail || selected);
          if (!opened) return;
        } catch (_) {}
      }

      function parseScheduleFields(rrule) {
        return String(rrule || '').split(';').reduce((result, part) => {
          const split = part.indexOf('=');
          if (split > 0) result[part.slice(0, split).toUpperCase()] = part.slice(split + 1);
          return result;
        }, {});
      }

      function serializeScheduleFields(fields) {
        return Object.keys(fields).filter(key => fields[key] != null && fields[key] !== '')
          .map(key => `${key}=${fields[key]}`).join(';');
      }

      function scheduleEditorValue(rrule) {
        const fields = parseScheduleFields(rrule);
        const days = normalizeScheduleDays(fields.BYDAY);
        const hasTimeAnchor = fields.BYHOUR != null || fields.BYMINUTE != null;
        let repeat = 'workdays';
        if (fields.FREQ === 'HOURLY') repeat = 'hourly';
        else if (days.join(',') === 'MO,TU,WE,TH,FR,SA,SU') repeat = 'daily';
        else if (days.join(',') !== 'MO,TU,WE,TH,FR') repeat = 'weekly';
        return {
          repeat,
          days,
          day: days[0] || 'MO',
          interval: Number(fields.INTERVAL || 1),
          time: hasTimeAnchor
            ? `${String(fields.BYHOUR != null ? fields.BYHOUR : 0).padStart(2, '0')}:${String(fields.BYMINUTE != null ? fields.BYMINUTE : 0).padStart(2, '0')}`
            : '',
          hasTimeAnchor,
        };
      }

      function scheduleRepeatLabel(editor) {
        if (!editor) return '';
        if (editor.repeat === 'hourly') {
          const interval = editor.interval === 1 ? scheduledCopy.repeatOptions.hourly : scheduledCopy.everyHours(editor.interval);
          return editor.hasTimeAnchor ? `${interval} · ${scheduledCopy.startsAt(editor.time)}` : interval;
        }
        return scheduledCopy.repeatOptions[editor.repeat] || scheduledCopy.repeatOptions.custom;
      }

      function buildRrule(currentRrule, key, value) {
        const fields = parseScheduleFields(currentRrule);
        const previousEditor = scheduleEditorValue(currentRrule);
        const editor = {...previousEditor};
        editor[key] = value;
        const [hour, minute] = String(editor.time || '08:00').split(':');
        if (key === 'time') {
          fields.BYHOUR = String(Number(hour || 0));
          fields.BYMINUTE = String(Number(minute || 0));
          return serializeScheduleFields(fields);
        }
        if (key === 'days') {
          const days = normalizeScheduleDays(value);
          if (!days.length) return currentRrule;
          fields.FREQ = 'WEEKLY';
          fields.BYDAY = days.join(',');
          fields.BYHOUR = String(Number(hour || 0));
          fields.BYMINUTE = String(Number(minute || 0));
          return serializeScheduleFields(fields);
        }
        if (editor.repeat === 'hourly') {
          const interval = previousEditor.repeat === 'hourly' ? editor.interval : 1;
          const anchor = previousEditor.hasTimeAnchor
            ? `;BYHOUR=${Number(hour || 0)};BYMINUTE=${Number(minute || 0)}`
            : '';
          return `FREQ=HOURLY;INTERVAL=${Math.max(1, interval || 1)}${anchor}`;
        }
        if (key === 'repeat' && editor.repeat === 'weekly') {
          const previousDays = normalizeScheduleDays(previousEditor.days);
          const presetDays = previousDays.join(',');
          const inheritedPreset = presetDays === 'MO,TU,WE,TH,FR' || presetDays === 'MO,TU,WE,TH,FR,SA,SU';
          const weeklyDays = inheritedPreset ? [previousEditor.day || 'MO'] : (previousDays.length ? previousDays : [previousEditor.day || 'MO']);
          return `FREQ=WEEKLY;BYDAY=${weeklyDays.join(',')};BYHOUR=${Number(hour || 0)};BYMINUTE=${Number(minute || 0)}`;
        }
        const days = editor.repeat === 'daily' ? 'MO,TU,WE,TH,FR,SA,SU'
          : (editor.repeat === 'workdays' ? 'MO,TU,WE,TH,FR'
            : (editor.days.join(',') || editor.day || 'MO'));
        return `FREQ=WEEKLY;BYDAY=${days};BYHOUR=${Number(hour || 0)};BYMINUTE=${Number(minute || 0)}`;
      }

      function buildEditedRrule(key, value) {
        return buildRrule(detailForm && detailForm.rrule, key, value);
      }

      function editSchedule(key, value) {
        // 连续勾选恰好组成“工作日”或“每天”时，仍保持每周编辑器可见。
        if (key === 'repeat') setScheduleRepeatIntent(value === 'weekly' ? 'weekly' : null);
        if (key === 'days') setScheduleRepeatIntent('weekly');
        editImmediateField('rrule', buildEditedRrule(key, value));
      }

      function editCreateSchedule(key, value) {
        if (key === 'repeat') setCreateScheduleRepeatIntent(value === 'weekly' ? 'weekly' : null);
        if (key === 'days') setCreateScheduleRepeatIntent('weekly');
        setCreateForm(current => current
          ? {...current, rrule: buildRrule(current.rrule, key, value)}
          : current);
      }

      const parsedScheduleEditor = detailForm ? scheduleEditorValue(detailForm.rrule) : null;
      const scheduleEditor = parsedScheduleEditor
        ? {...parsedScheduleEditor, repeat: scheduleRepeatIntent || parsedScheduleEditor.repeat}
        : null;
      const parsedCreateScheduleEditor = createForm ? scheduleEditorValue(createForm.rrule) : null;
      const createScheduleEditor = parsedCreateScheduleEditor
        ? {...parsedCreateScheduleEditor, repeat: createScheduleRepeatIntent || parsedCreateScheduleEditor.repeat}
        : null;

      const modelOptions = savedModels.map(model => ({
        value: model.id,
        label: selectorMainLabel(model, t),
        model: model.model,
      }));
      if (detailForm && detailForm.modelId && !modelOptions.some(option => option.value === detailForm.modelId)) {
        modelOptions.unshift({ value: detailForm.modelId, label: detailForm.model || detailForm.modelId, model: detailForm.model });
      } else if (detailForm && !detailForm.modelId) {
        modelOptions.unshift({ value: '', label: detailForm.model ? scheduledCopy.reselectModel(detailForm.model) : scheduledCopy.currentModel });
      }
      const repeatOptions = [
        { value: 'workdays', label: scheduledCopy.repeatOptions.workdays },
        { value: 'daily', label: scheduledCopy.repeatOptions.daily },
        { value: 'weekly', label: scheduledCopy.repeatOptions.weekly },
        { value: 'hourly', label: scheduledCopy.repeatOptions.hourly },
      ];
      const selectedWeekdays = scheduleEditor && scheduleEditor.days.length
        ? scheduleEditor.days
        : [scheduleEditor && scheduleEditor.day || 'MO'];
      const createSelectedWeekdays = createScheduleEditor && createScheduleEditor.days.length
        ? createScheduleEditor.days
        : [createScheduleEditor && createScheduleEditor.day || 'MO'];

      const iosSeparator = 'border-[#3C3C43]/20 dark:border-[#545458]/50';
      const iosInsetSurface = 'bg-[#F2F2F7] dark:bg-[#2C2C2E]';
      const iosHistorySurface = 'bg-[#F5F5F7] dark:bg-[#2C2C2E]';
      const mutedValue = 'text-[#3C3C43]/60 dark:text-[#EBEBF5]/60';
      const pressedRow = 'active:bg-[#E5E5EA] dark:active:bg-[#3A3A3C]';
      const modalPortalTarget = typeof document !== 'undefined' ? document.body : null;
      const renderModal = node => modalPortalTarget ? createPortal(node, modalPortalTarget) : node;

      const MacSwitch = ({ task }) => {
        const checked = task.status === 'active';
        return (
          <button
            type="button"
            onClick={(event) => toggleTask(event, task)}
            disabled={!!busyAction}
            aria-pressed={checked}
            aria-label={checked ? scheduledCopy.pause(task.name) : scheduledCopy.resume(task.name)}
            className={`relative flex h-6 w-11 shrink-0 items-center rounded-full p-[1px] transition-colors duration-300 disabled:opacity-50 ${
              checked ? 'bg-[#34C759]' : 'bg-[#D8DADD] dark:bg-[#4A4B50]'
            }`}
          >
            <span
              className={`h-5 w-5 rounded-full bg-white shadow-[0_3px_8px_rgba(0,0,0,0.15),0_1px_1px_rgba(0,0,0,0.05)] transition-transform duration-300 ${
                checked ? 'translate-x-5' : 'translate-x-0'
              }`}
            />
          </button>
        );
      };

      const taskIconMeta = (task) => {
        const template = SCHEDULED_TASK_TEMPLATES.find(item => item.id === task.templateId);
        if (template) {
          return {
            Icon: template.icon || Clock,
            className: task.status === 'active'
              ? 'bg-[#E9F8EE] text-[#188038] dark:bg-[#12351D] dark:text-[#32D74B]'
              : 'bg-[#F5F5F7] text-[#86868B] dark:bg-[#2C2C2E] dark:text-[#8E8E93]',
          };
        }
        const name = String(task.name || '');
        if (/周报|报告|数据|统计/.test(name)) {
          return { Icon: FileChartLine, className: 'bg-[#F7EFFF] text-[#AF52DE] dark:bg-[#2C2333] dark:text-[#D5A8FF]' };
        }
        if (/早报|简报|新闻/.test(name)) {
          return { Icon: Newspaper, className: 'bg-blue-50 text-[#007AFF] dark:bg-[#122E45] dark:text-[#7CB7F0]' };
        }
        if (/督办|事项|待办|跟进/.test(name)) {
          return { Icon: ClipboardCheck, className: 'bg-[#E9F8EE] text-[#188038] dark:bg-[#12351D] dark:text-[#32D74B]' };
        }
        return { Icon: Clock, className: 'bg-[#F5F5F7] text-[#86868B] dark:bg-[#2C2C2E] dark:text-[#8E8E93]' };
      };

      const FilterTabs = () => (
        <div data-testid="scheduled-filter-tabs" className={`grid grid-cols-3 rounded-[8px] p-0.5 bg-[#767680]/12 dark:bg-[#767680]/24`}>
          {[
            ['all', scheduledCopy.filterAll],
            ['active', scheduledCopy.filterActive],
            ['paused', scheduledCopy.filterPaused],
          ].map(([value, label]) => (
            <button key={value} type="button" onClick={() => setTaskFilter(value)}
              aria-pressed={taskFilter === value}
              className={`h-7 min-w-[72px] rounded-[6.5px] px-3 text-[13px] font-medium transition-colors ${
                taskFilter === value
                  ? 'bg-white text-black shadow-sm dark:bg-[#636366] dark:text-white'
                  : `${mutedValue}`
              }`}>
              {label}
            </button>
          ))}
        </div>
      );

      const FormScheduleRows = ({ editor, selectedDays, prefix, onEdit, onCloseWeekly }) => !editor ? null : (
        <>
          <div className={`flex items-center pl-3.5 ${pressedRow} cursor-pointer border-b ${iosSeparator}`}>
            <div className="flex flex-1 items-center justify-between py-3.5 pr-3.5">
              <span className={`ml-1 text-[15px] font-normal ${bodyText}`}>{scheduledCopy.repeat}</span>
              <div className="flex items-center gap-1.5">
                <ScheduledSelect value={editor.repeat} options={repeatOptions}
                  onChange={value => onEdit('repeat', value)}
                  testId={`${prefix}-repeat`} ariaLabel={scheduledCopy.chooseRepeat} theme={theme} emptyLabel={scheduledCopy.choose} />
                <ChevronRight className={`h-3.5 w-3.5 text-[#C5C5C7] dark:text-[#EBEBF5]/30`} />
              </div>
            </div>
          </div>
          {editor.repeat === 'hourly' && (
            <div data-testid={`${prefix}-interval-row`} className={`flex items-center pl-3.5 ${pressedRow} cursor-pointer border-b ${iosSeparator}`}>
              <div className="flex flex-1 items-center justify-between py-3.5 pr-3.5">
                <span className={`ml-1 text-[15px] font-normal ${bodyText}`}>{scheduledCopy.interval}</span>
                <ScheduledSelect value={editor.interval} options={hourlyIntervalOptions}
                  onChange={value => onEdit('interval', value)}
                  testId={`${prefix}-interval`} ariaLabel={scheduledCopy.chooseInterval} theme={theme} minWidth={140} emptyLabel={scheduledCopy.choose} />
              </div>
            </div>
          )}
          {editor.repeat === 'weekly' && (
            <div className={`flex items-center pl-3.5 ${pressedRow} cursor-pointer border-b ${iosSeparator}`}>
              <div className="flex flex-1 items-center justify-between py-3.5 pr-3.5">
                <span className={`ml-1 text-[15px] font-normal ${bodyText}`}>{scheduledCopy.dateLabel}</span>
                <div className="flex items-center gap-1.5">
                  <ScheduledSelect value={selectedDays} options={weekdayOptions}
                    onChange={values => onEdit('days', values)} multiple minSelected={1}
                    onClose={onCloseWeekly}
                    testId={`${prefix}-day`} ariaLabel={scheduledCopy.chooseDate} theme={theme} minWidth={190} emptyLabel={scheduledCopy.choose} separator={scheduledCopy.daySeparator} />
                  <ChevronRight className={`h-3.5 w-3.5 text-[#C5C5C7] dark:text-[#EBEBF5]/30`} />
                </div>
              </div>
            </div>
          )}
          <div data-testid={`${prefix}-time-row`} className={`flex items-center pl-3.5 ${pressedRow} cursor-pointer`}>
            <div className="flex flex-1 items-center justify-between py-3.5 pr-3.5">
              <span className={`ml-1 text-[15px] font-normal ${bodyText}`}>{editor.repeat === 'hourly' ? scheduledCopy.startTime : scheduledCopy.time}</span>
              <ScheduledTimeWheel value={editor.time}
                onChange={value => onEdit('time', value)}
                theme={theme} testId={`${prefix}-time`} ariaLabel={editor.repeat === 'hourly' ? scheduledCopy.chooseStartTime : scheduledCopy.chooseRunTime}
                hourAriaLabel={scheduledCopy.chooseHour} minuteAriaLabel={scheduledCopy.chooseMinute}
                placeholder={editor.repeat === 'hourly' && !editor.hasTimeAnchor ? scheduledCopy.setStart : ''} />
            </div>
          </div>
        </>
      );

      const CreateTaskDialog = () => !createForm ? null : renderModal(
        <div className="fixed inset-0 z-[200] flex items-center justify-center bg-black/45 px-4 py-6 backdrop-blur-[6px]">
          <form aria-labelledby="scheduled-create-dialog-title"
            data-testid="scheduled-create-dialog" onSubmit={submitCustomTask}
            className={`mx-4 flex max-h-[calc(100vh-48px)] w-full max-w-[480px] flex-col overflow-hidden rounded-[28px] shadow-[0_18px_60px_rgba(0,0,0,0.22)] bg-white dark:bg-[#1C1C1E]`}>
            <div className="flex shrink-0 items-start justify-between gap-4 px-6 pb-4 pt-6">
              <div className="min-w-0">
                <h2 id="scheduled-create-dialog-title" className={`truncate text-[22px] font-semibold leading-7 ${bodyText}`}>
                  {createForm.templateId ? scheduledCopy.createFromTemplate : scheduledCopy.newTask}
                </h2>
              </div>
              <button type="button" data-testid="scheduled-create-close" disabled={!!busyAction}
                onClick={() => setCreateForm(null)}
                className={`flex h-11 w-11 shrink-0 items-center justify-center rounded-full transition-colors disabled:opacity-40 bg-[#E9E9EB] text-[#6E6E73] hover:bg-[#DADADD] dark:bg-[#2C2C2E] dark:text-[#C7C7CC] dark:hover:bg-[#3A3A3C]`}
                aria-label={scheduledCopy.closeCreate}>
                <X size={18} />
              </button>
            </div>

            <div className="min-h-0 flex-1 space-y-5 overflow-y-auto px-6 pb-5 custom-scrollbar">
              {/* Static regression anchors for shared create schedule rows:
                testId="scheduled-create-repeat"
              */}
              <label className="block">
                <span className={`mb-1.5 block text-[13px] font-medium ${mutedValue}`}>{scheduledCopy.taskName}</span>
                <input data-testid="scheduled-create-name" value={createForm.name}
                  onChange={event => setCreateForm(current => ({...current, name: event.target.value}))}
                  placeholder={scheduledCopy.taskNamePlaceholder}
                  className={`min-h-12 w-full rounded-[14px] px-4 py-3 text-[15px] outline-none transition-shadow focus:ring-2 focus:ring-[#007AFF]/50 bg-[#F2F2F7] text-[#1D1D1F] placeholder:text-[#86868B] dark:bg-[#2C2C2E] dark:text-white dark:placeholder:text-[#EBEBF5]/30`} />
              </label>

              <label className="block">
                <span className={`mb-1.5 block text-[13px] font-medium ${mutedValue}`}>{scheduledCopy.taskPrompt}</span>
                <textarea data-testid="scheduled-create-prompt" value={createForm.prompt}
                  onChange={event => setCreateForm(current => ({...current, prompt: event.target.value}))}
                  placeholder={scheduledCopy.taskPromptPlaceholder} rows="3"
                  className={`min-h-[112px] w-full resize-none rounded-[14px] px-4 py-3 text-[15px] leading-6 outline-none transition-shadow focus:ring-2 focus:ring-[#007AFF]/50 bg-[#F2F2F7] text-[#1D1D1F] placeholder:text-[#86868B] dark:bg-[#2C2C2E] dark:text-white dark:placeholder:text-[#EBEBF5]/30`} />
              </label>

              <div data-testid="scheduled-create-settings" className={`overflow-visible rounded-[16px] ${iosInsetSurface}`}>
                {FormScheduleRows({
                  editor: createScheduleEditor,
                  selectedDays: createSelectedWeekdays,
                  prefix: 'scheduled-create',
                  onEdit: editCreateSchedule,
                  onCloseWeekly: () => setCreateScheduleRepeatIntent(null),
                })}
              </div>

            </div>

            <div className={`flex shrink-0 justify-end gap-3 border-t px-6 py-4 ${iosSeparator} bg-white/95 dark:bg-[#1C1C1E]/95 backdrop-blur-xl`}>
              <button type="submit" data-testid="scheduled-create-submit"
                disabled={!!busyAction || !String(createForm.name || '').trim() || !String(createForm.prompt || '').trim()}
                className="h-11 rounded-full bg-[#007AFF] px-6 text-[15px] font-medium text-white shadow-sm transition-colors hover:bg-[#0066D6] disabled:opacity-40">
                {scheduledCopy.saveTask}
              </button>
            </div>
          </form>
        </div>
      );

      const renderTemplateSuggestions = () => (
        <section className="mb-10" data-testid="scheduled-template-suggestions">
          <div className="mb-4 ml-1 flex items-center justify-between">
            <h2 className={`text-[13px] font-bold uppercase tracking-wider ${mutedValue}`}>{scheduledCopy.templates}</h2>
          </div>
          <div className="grid grid-cols-1 gap-6 md:grid-cols-3">
            {visibleSuggestions.map(template => {
              const activeTemplate = createForm && createForm.templateId === template.id;
              return (
                <button key={template.id} type="button" onClick={() => startTemplate(template)}
                  data-testid={`scheduled-template-${template.id}`}
                  aria-label={scheduledCopy.useTemplate(template.name)}
                  title={scheduledCopy.useTemplate(template.name)}
                  className={`group relative h-[260px] max-sm:h-[210px] w-full overflow-hidden rounded-[20px] text-left shadow-[0_2px_10px_rgba(0,0,0,0.02),0_8px_32px_rgba(0,0,0,0.04)] transition-all duration-300 active:scale-[0.99] ${activeTemplate ? 'ring-2 ring-[#0A84FF]/45' : ''} ${
                    activeTemplate
                      ? ''
                      : 'hover:-translate-y-1 hover:shadow-[0_12px_32px_rgba(0,0,0,0.08),0_4px_12px_rgba(0,0,0,0.04)]'
                  }`}>
                  <img
                    src={template.image}
                    alt=""
                    className="absolute inset-0 h-full w-full object-cover transition-transform duration-700 group-hover:scale-105"
                    loading="lazy"
                  />
                  <span className="absolute inset-0 bg-gradient-to-t from-black/80 via-black/20 to-transparent" />
                  <span className="absolute right-4 top-4 flex h-8 w-8 items-center justify-center rounded-full border border-white/20 bg-white/20 text-white opacity-0 shadow-sm backdrop-blur-md transition-opacity duration-300 group-hover:opacity-100">
                    <ChevronRight size={17} className="-rotate-45" />
                  </span>
                  <span className="absolute inset-x-0 bottom-0 flex flex-col justify-end p-5">
                    <span className="mb-1.5 block text-[19px] font-bold leading-6 text-white drop-shadow-md">{template.name}</span>
                    <span className="mb-4 line-clamp-2 text-[13px] leading-5 text-gray-200 drop-shadow">
                        {template.description}
                    </span>
                    <span className="flex">
                      <span className="inline-flex items-center rounded-full border border-white/10 bg-white/20 px-3 py-1.5 text-[11px] font-medium text-white shadow-sm backdrop-blur-md">
                        <Clock size={12} className="mr-1.5" />
                        {template.schedule}
                      </span>
                    </span>
                  </span>
                </button>
              );
            })}
          </div>
        </section>
      );

      const renderTaskRow = (task, index, list = filtered) => {
        const { Icon, className } = taskIconMeta(task);
        return (
          <div key={task.id} className="task-item" data-status={task.status === 'active' ? 'active' : 'paused'}>
            <div className={`list-row group flex cursor-pointer items-center justify-between p-4 pl-5 transition-colors hover:bg-gray-50/50 active:bg-[#F0F0F2] dark:hover:bg-white/5 dark:active:bg-[#2C2C2E]`}>
              <button
                type="button"
                onClick={() => selectTask(task.id)}
                aria-label={scheduledCopy.view(task.name)}
                title={scheduledCopy.view(task.name)}
                className="flex min-w-0 flex-1 items-center gap-4 pr-3 text-left"
              >
                <span className={`flex h-10 w-10 shrink-0 items-center justify-center rounded-full ${className}`}>
                  <Icon size={15} />
                </span>
                <span className="min-w-0">
                  <span className={`block truncate text-[15px] font-semibold leading-5 ${bodyText}`}>{task.name}</span>
                  <span data-testid="scheduled-task-summary" className={`mt-0.5 flex min-w-0 items-center text-[13px] leading-5 ${mutedValue}`}>
                    <span className="truncate">{task.status === 'active' ? taskSummary(task) : (task.scheduleLabel || taskSummary(task))}</span>
                    <span className={`mx-2 text-[#D1D1D6] dark:text-[#48484A]`}>|</span>
                    <span className={`inline-flex shrink-0 items-center ${task.status === 'active' ? 'text-[#34C759]' : 'text-[#FF9500]'}`}>
                      <span className="mr-1.5 h-1.5 w-1.5 rounded-full" style={{ background: task.status === 'active' ? '#34C759' : '#FF9500' }} />
                      {taskListStatusLabel(task.status)}
                    </span>
                    {task.isRunning && (
                      <span data-testid="scheduled-task-running" aria-label={scheduledCopy.running}
                        className="ml-2 h-3.5 w-3.5 shrink-0 animate-spin rounded-full border-2 border-[#007AFF] border-t-transparent dark:border-[#0A84FF]" />
                    )}
                    {task.hasUnreadRuns && (
                      <span data-testid="scheduled-task-unread" aria-label={scheduledCopy.unread}
                        className="ml-2 h-2.5 w-2.5 shrink-0 rounded-full bg-[#007AFF] dark:bg-[#0A84FF]" />
                    )}
                  </span>
                </span>
              </button>
              <div className="flex shrink-0 items-center gap-2 pr-2">
                <MacSwitch task={task} />
              </div>
            </div>
            {index !== list.length - 1 && (
              <div className={`ml-[76px] h-px bg-gray-100 dark:bg-[#48484A]/70`} />
            )}
          </div>
        );
      };

      const MyTasksSection = ({ className = '' } = {}) => (
        <section className={className || 'mb-5'}>
          <div className="mb-4 ml-1 flex items-center justify-between gap-4">
            <h2 className={`text-[13px] font-bold uppercase tracking-wider ${mutedValue}`}>{scheduledCopy.myTasks}</h2>
            <FilterTabs />
          </div>
          <div className={`overflow-hidden rounded-[20px] border shadow-[0_2px_10px_rgba(0,0,0,0.02),0_8px_32px_rgba(0,0,0,0.04)] border-black/5 bg-white dark:border-white/15 dark:bg-[#1C1C1E]`}>
            {error && (
              <div role="alert" data-testid="scheduled-error" className={`m-3 flex items-start gap-2 rounded-[12px] px-3 py-2 text-[13px] bg-[#FCE8E6] text-[#A50E0E] dark:bg-[#3A2424] dark:text-[#F2B8B5]`}>
                <span className="min-w-0 flex-1">{error}</span>
                <button type="button" onClick={() => bridge?.dismissScheduledTaskError?.()}
                  aria-label={scheduledCopy.closeError} className="mt-[-2px] rounded-full p-1 opacity-65 transition-opacity hover:opacity-100">
                  <X className="h-3.5 w-3.5" />
                </button>
              </div>
            )}
            {filtered.length ? (
              <div data-testid="scheduled-task-groups">
                {filtered.map((task, index) => renderTaskRow(task, index, filtered))}
              </div>
            ) : (
              <div className={`px-4 py-8 text-center text-[14px] ${mutedValue}`}>
                {loading ? scheduledCopy.loading : scheduledCopy.empty}
              </div>
            )}
          </div>
        </section>
      );

      const DetailTaskDialog = () => !(selected && detailForm) ? null : renderModal(
        <div className="fixed inset-0 z-[200] flex items-center justify-center bg-black/45 px-4 py-6 backdrop-blur-[6px]">
          <div
            data-testid="scheduled-detail"
            role="dialog"
            aria-modal="true"
            aria-labelledby="scheduled-detail-title-heading"
            className={`mx-4 flex max-h-[calc(100vh-48px)] w-full max-w-[560px] flex-col overflow-hidden rounded-[28px] shadow-[0_18px_60px_rgba(0,0,0,0.22)] bg-white dark:bg-[#1C1C1E]`}
          >
            <div data-testid="scheduled-detail-toolbar" className="flex shrink-0 items-start justify-between gap-4 px-6 pb-4 pt-6">
              <div className="min-w-0">
                <div className="flex min-w-0 items-center gap-2">
                  <h2 id="scheduled-detail-title-heading" className={`truncate text-[22px] font-semibold leading-7 ${bodyText}`}>
                    {scheduledCopy.editTask}
                  </h2>
                  <span className={`shrink-0 rounded-full px-2 py-0.5 text-[11px] font-medium ${selected.status === 'active' ? 'bg-[#E9F8EE] text-[#188038] dark:bg-[#163820] dark:text-[#7EE787]' : 'bg-[#EEF0F3] text-[#5F6368] dark:bg-[#34353A] dark:text-[#C6C8CE]'}`}>
                    {statusLabel(selected.status)}
                  </span>
                </div>
                {saveState !== 'idle' && (
                  <span data-testid="scheduled-save-state" className={`mt-1 block text-[12px] ${saveState === 'error' || saveState === 'invalid' ? 'text-[#FF3B30]' : mutedValue}`}>
                    {scheduledCopy.saveState[saveState] || scheduledCopy.saveState.error}
                  </span>
                )}
              </div>
              <div className="flex shrink-0 items-center gap-2">
                <button type="button" data-testid="scheduled-detail-close" disabled={!!busyAction}
                  onClick={() => selectTask(null)}
                  className={`flex h-11 w-11 items-center justify-center rounded-full transition-colors disabled:opacity-40 bg-[#E9E9EB] text-[#6E6E73] hover:bg-[#DADADD] dark:bg-[#2C2C2E] dark:text-[#C7C7CC] dark:hover:bg-[#3A3A3C]`}
                  aria-label={scheduledCopy.closeDetail}>
                  <X size={18} />
                </button>
              </div>
            </div>

            <div className="min-h-0 flex-1 space-y-5 overflow-y-auto px-6 pb-5 custom-scrollbar">
              <label data-testid="scheduled-detail-title" className="block">
                <span className={`mb-1.5 block text-[13px] font-medium ${mutedValue}`}>{scheduledCopy.taskName}</span>
                <input data-testid="scheduled-live-title" value={detailForm.name}
                  onChange={e => editTextField('name', e.target.value)} onBlur={() => finishTextField('name')}
                  aria-label={scheduledCopy.taskNameAria}
                  className={`min-h-12 w-full rounded-[14px] px-4 py-3 text-[15px] outline-none transition-shadow focus:ring-2 focus:ring-[#007AFF]/50 bg-[#F2F2F7] text-[#1D1D1F] placeholder:text-[#86868B] dark:bg-[#2C2C2E] dark:text-white dark:placeholder:text-[#EBEBF5]/30`} />
              </label>

              <label data-testid="scheduled-detail-prompt" className="block">
                <span className={`mb-1.5 block text-[13px] font-medium ${mutedValue}`}>{scheduledCopy.taskPrompt}</span>
                <textarea data-testid="scheduled-live-prompt" value={detailForm.prompt}
                  onChange={e => editTextField('prompt', e.target.value)} onBlur={() => finishTextField('prompt')}
                  rows="5" aria-label={scheduledCopy.taskPromptAria} placeholder={scheduledCopy.taskPromptPlaceholder}
                  className={`min-h-[132px] w-full resize-none rounded-[14px] px-4 py-3 text-[15px] leading-6 outline-none transition-shadow focus:ring-2 focus:ring-[#007AFF]/50 bg-[#F2F2F7] text-[#1D1D1F] placeholder:text-[#86868B] dark:bg-[#2C2C2E] dark:text-white dark:placeholder:text-[#EBEBF5]/30`} />
              </label>

              <div data-testid="scheduled-detail-settings" className={`overflow-visible rounded-[16px] ${iosInsetSurface}`}>
                <div className={`flex items-center pl-3.5 ${pressedRow} cursor-pointer border-b ${iosSeparator}`}>
                  <div className="flex flex-1 items-center justify-between py-3.5 pr-3.5">
                    <span className={`ml-1 text-[15px] font-normal ${bodyText}`}>{scheduledCopy.aiModel}</span>
                    <div className="flex items-center gap-1.5">
                      <ScheduledSelect value={detailForm.modelId || ''} options={modelOptions}
                        onChange={value => editModel(value)} alwaysCommit
                        testId="scheduled-live-model" ariaLabel={scheduledCopy.chooseModel} theme={theme} minWidth={220} emptyLabel={scheduledCopy.choose} footerAction={modelManageAction} />
                      <ChevronRight className={`h-3.5 w-3.5 text-[#C5C5C7] dark:text-[#EBEBF5]/30`} />
                    </div>
                  </div>
                </div>
                {/* Static regression anchors for shared detail schedule rows:
                  testId="scheduled-live-repeat" testId="scheduled-live-interval" testId="scheduled-live-day" testId="scheduled-live-time"
                  scheduleEditor.repeat === 'hourly' data-testid="scheduled-live-interval-row"
                  onChange={value => editSchedule('interval', value)}
                  onChange={values => editSchedule('days', values)} multiple minSelected={1}
                  onClose={() => setScheduleRepeatIntent(null)}
                */}
                {FormScheduleRows({
                  editor: scheduleEditor,
                  selectedDays: selectedWeekdays,
                  prefix: 'scheduled-live',
                  onEdit: editSchedule,
                  onCloseWeekly: () => setScheduleRepeatIntent(null),
                })}
              </div>

              <div className={`grid gap-3 rounded-[16px] p-4 text-[13px] ${iosInsetSurface}`}>
                <div className="flex items-center justify-between gap-3">
                  <span className={mutedValue}>{scheduledCopy.runningStatus}</span>
                  <span className={`font-medium ${bodyText}`}>{statusLabel(selected.status)}</span>
                </div>
                <div className="flex items-center justify-between gap-3">
                  <span className={mutedValue}>{scheduledCopy.nextExecution}</span>
                  <span className={`truncate text-right font-medium ${bodyText}`}>{fmtDateTime(selected.nextRunAt)}</span>
                </div>
                <div className="flex items-center justify-between gap-3">
                  <span className={mutedValue}>{scheduledCopy.enableTask}</span>
                  <MacSwitch task={selected} />
                </div>
              </div>

              <div data-testid="scheduled-detail-actions-group" className={`overflow-hidden rounded-[16px] ${iosInsetSurface}`}>
                <button type="button" data-testid="scheduled-run-now"
                  disabled={!!busyAction || !detailFormIsValid}
                  onClick={() => runTaskNow(selected.id)}
                  className={`flex min-h-12 w-full items-center justify-between gap-3 px-4 py-3 text-left text-[15px] transition-colors disabled:cursor-not-allowed disabled:opacity-45 ${canOpenTaskFolder ? `border-b ${iosSeparator}` : ''} ${pressedRow}`}>
                  <span className={`font-medium ${bodyText}`}>{scheduledCopy.runNow}</span>
                  <ChevronRight className={`h-4 w-4 shrink-0 text-[#3C3C43]/30 dark:text-[#EBEBF5]/30`} />
                </button>
                <button type="button" data-testid="scheduled-open-folder"
                  onClick={() => bridge && bridge.artifacts.openScheduledTaskFolder && bridge.artifacts.openScheduledTaskFolder(selected.id)}
                  className={`flex min-h-12 w-full items-center justify-between gap-3 px-4 py-3 text-left text-[15px] transition-colors ${pressedRow}`}>
                  <span className={`font-medium ${bodyText}`}>{scheduledCopy.openFolder}</span>
                  <ChevronRight className={`h-4 w-4 shrink-0 text-[#3C3C43]/30 dark:text-[#EBEBF5]/30`} />
                </button>
              </div>

              <section>
                <div className="mb-2 flex items-center justify-between px-1">
                  <h3 className={`text-[13px] font-medium ${mutedValue}`}>{scheduledCopy.runHistory}</h3>
                  <span className={`text-[12px] ${mutedValue}`}>{runs.length ? scheduledCopy.records(runs.length) : scheduledCopy.noRecords}</span>
                </div>
                <div data-testid="scheduled-run-history-list" className={`overflow-hidden rounded-[12px] ${iosHistorySurface}`}>
                  {runs.length ? (
                    <div className={`divide-y divide-[#3C3C43]/10 dark:divide-[#545458]/50`}>
                      {runs.map(item => (
                        <button key={item.id} type="button" disabled={!item.sessionId} onClick={() => openRunChat(item)}
                          data-testid="scheduled-run-row"
                          className={`flex w-full items-start gap-3 px-4 py-3 text-left transition-colors disabled:cursor-default disabled:opacity-60 ${pressedRow}`}
                          title={item.sessionId ? scheduledCopy.openRun : scheduledCopy.noOpenRun}
                          aria-label={item.sessionId ? scheduledCopy.openRunLabel(runStatusLabel(item.status)) : scheduledCopy.noOpenRun}>
                          <span className="mt-1 flex h-5 w-5 shrink-0 items-center justify-center">
                            {['queued', 'running'].includes(item.status) ? (
                              <span data-testid="scheduled-run-running" aria-label={scheduledCopy.runInProgress}
                                className="h-3 w-3 shrink-0 animate-spin rounded-full border-2 border-[#007AFF] border-t-transparent dark:border-[#0A84FF]" />
                            ) : item.unread ? (
                              <span data-testid="scheduled-run-unread" aria-label={scheduledCopy.unread}
                                className="h-2.5 w-2.5 shrink-0 rounded-full bg-[#007AFF] dark:bg-[#0A84FF]" />
                            ) : (
                              <span className="h-2.5 w-2.5 shrink-0 rounded-full" style={{ background: item.status === 'failed' ? '#FF3B30' : '#8E8E93' }} />
                            )}
                          </span>
                          <span className="min-w-0 flex-1">
                            <span className={`block truncate text-[13px] font-medium ${item.status === 'failed' ? 'text-[#FF3B30]' : item.status === 'completed' ? 'text-[#34C759]' : 'text-[#007AFF]'}`}>
                              {runStatusLabel(item.status)}
                            </span>
                            <span className={`mt-0.5 block truncate text-[13px] ${bodyText}`}>
                              {item.error || (item.sessionId ? scheduledCopy.viewRunResult : scheduledCopy.noRunSession)}
                            </span>
                            <span className={`mt-1 block truncate text-[12px] ${mutedValue}`}>
                              {fmtDateTime(item.scheduledFor || item.createdAt)}
                            </span>
                          </span>
                          {item.sessionId && <ChevronRight className={`mt-1 h-4 w-4 shrink-0 text-[#3C3C43]/30 dark:text-[#EBEBF5]/30`} />}
                        </button>
                      ))}
                    </div>
                  ) : (
                    <div className={`px-4 py-8 text-center text-[13px] ${mutedValue}`}>{scheduledCopy.noRunHistory}</div>
                  )}
                </div>
              </section>
            </div>

            <div className={`flex shrink-0 flex-wrap items-center justify-between gap-3 border-t px-6 py-4 ${iosSeparator} bg-white/95 dark:bg-[#1C1C1E]/95 backdrop-blur-xl`}>
              <button type="button" data-testid="scheduled-detail-delete"
                onClick={(event) => requestDeleteTask(event, selected)}
                disabled={!!busyAction}
                className={`h-11 rounded-full px-6 text-[15px] font-medium text-[#FF3B30] transition-colors disabled:opacity-40 bg-[#E9E9EB] hover:bg-[#DADADD] dark:bg-[#2C2C2E] dark:hover:bg-[#3A3A3C]`}>
                {scheduledCopy.delete}
              </button>
              <div className="flex flex-wrap justify-end gap-3">
                <button type="button" data-testid="scheduled-detail-save"
                  disabled={!!busyAction || !detailFormIsValid}
                  onClick={saveDetailAndClose}
                  className="h-11 rounded-full bg-[#007AFF] px-6 text-[15px] font-medium text-white shadow-sm transition-colors hover:bg-[#0066D6] disabled:opacity-40">
                  {scheduledCopy.save}
                </button>
              </div>
            </div>
          </div>
        </div>
      );

      const deleteTarget = deleteConfirmTask;

      return (
        <div data-testid="scheduled-page" aria-busy={!!busyAction} className={`relative z-10 flex min-h-0 w-full flex-1 overflow-hidden bg-transparent text-black dark:text-white`}>
          {tasks[0] && (
            <button
              type="button"
              aria-label={scheduledCopy.view(tasks[0].name)}
              tabIndex={-1}
              className="absolute left-0 top-0 h-px w-px opacity-0"
            />
          )}
          <div className="h-full w-full overflow-hidden p-4 sm:p-6 lg:p-10" data-testid="scheduled-list">
            <div className="relative mx-auto flex h-full min-h-0 w-full max-w-[1400px] flex-col overflow-hidden">
              <header data-testid="scheduled-list-intro" className={`mb-4 flex shrink-0 flex-col items-start justify-between gap-4 border-b pb-6 sm:flex-row sm:items-center ${iosSeparator}`}>
                <div className="min-w-0">
                  <h1 className={`truncate text-[26px] font-normal tracking-tight max-sm:hidden ${bodyText}`}>{scheduledCopy.title}</h1>
                </div>
                <div className="flex shrink-0 items-center gap-3">
                  <button type="button"
                    onClick={startChatCreation}
                    data-testid="scheduled-create-from-chat"
                    className={`inline-flex h-9 items-center rounded-full px-4 text-[13px] font-semibold shadow-sm transition-colors bg-[#E9E9EB] text-[#1D1D1F] hover:bg-[#DADADD] dark:bg-[#2C2C2E] dark:text-white dark:hover:bg-[#3A3A3C]`}>
                    <MessageCircle size={14} className="mr-2 opacity-70" />
                    {scheduledCopy.createFromChat}
                  </button>
                  <button type="button"
                    onClick={startBlankTask}
                    data-testid="scheduled-create-menu"
                    className="inline-flex h-9 items-center rounded-full bg-[#007AFF] px-4 text-[13px] font-semibold text-white shadow-sm transition-colors hover:bg-[#0066D6]">
                    <Plus size={14} className="mr-2" />
                    {scheduledCopy.newTask}
                  </button>
                </div>
              </header>

              <div data-testid="scheduled-left-toolbar" className="sr-only">
                {FilterTabs()}
              </div>
              <main className="min-h-0 flex-1 overflow-y-auto pb-6 custom-scrollbar">
                {renderTemplateSuggestions()}
                <MyTasksSection className="mb-0" />
              </main>
            </div>
          </div>

          {DetailTaskDialog()}
          {CreateTaskDialog()}

          {deleteTarget && renderModal(
            <div className="fixed inset-0 z-[300] flex items-center justify-center px-4">
              <div className="absolute inset-0 bg-black/28 backdrop-blur-[1px]" onClick={(event) => cancelDeleteTask(event)} />
              <div
                role="alertdialog"
                aria-modal="true"
                aria-labelledby="scheduled-delete-title"
                aria-describedby="scheduled-delete-description"
                data-testid="scheduled-detail-delete-confirmation"
                className={`relative w-full max-w-[270px] overflow-hidden rounded-[14px] border shadow-[0_20px_60px_rgba(0,0,0,0.28)] ${iosSeparator} bg-white dark:bg-[#2C2C2E]`}
              >
                <div className={`px-5 pb-4 pt-5 text-center border-b ${iosSeparator}`}>
                  <h3 id="scheduled-delete-title" className={`text-[15px] font-semibold leading-5 ${bodyText}`}>
                    {scheduledCopy.deleteTitle}
                  </h3>
                  <p id="scheduled-delete-description" className={`mt-2 text-[12px] leading-4 ${mutedValue}`}>
                    {scheduledCopy.deleteDescription(deleteTarget.name)}
                  </p>
                </div>
                <div className={`grid grid-cols-2 divide-x divide-[#3C3C43]/16 dark:divide-[#545458]/50`}>
                  <button type="button" data-testid="scheduled-detail-delete-cancel"
                    onClick={(event) => cancelDeleteTask(event)}
                    disabled={!!busyAction}
                    className={`h-11 text-[15px] font-normal text-[#007AFF] transition-colors disabled:opacity-50 ${pressedRow}`}>
                    {scheduledCopy.cancel}
                  </button>
                  <button type="button" data-testid="scheduled-detail-delete-confirm"
                    onClick={(event) => confirmDeleteTask(event, deleteTarget)}
                    disabled={!!busyAction}
                    className={`h-11 text-[15px] font-semibold text-[#FF3B30] transition-colors disabled:opacity-50 ${pressedRow}`}>
                    {scheduledCopy.delete}
                  </button>
                </div>
              </div>
            </div>
          )}

        </div>
      );
    };

export { ScheduledTasksView };
