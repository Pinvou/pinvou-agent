import React, { useEffect, useRef, useState } from 'react';
import { Check, ChevronDown, ClipboardList, Clock, FolderOpen, Pause, Play, Search, StopCircle, Trash2, X } from '../../components/icons.jsx';
import { bridge, useBridge } from '../../hooks/useBridge.js';
    const SCHEDULED_TASK_TEMPLATES = [
      {
        id: 'daily-brief', name: '每日简报', schedule: '工作日 8:00',
        rrule: 'FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR;BYHOUR=8;BYMINUTE=0',
        prompt: '汇总选定项目或目录的近期变化、待办事项和风险，给出今天最值得关注的优先级与下一步。',
        workspace: [], allowShell: false, trustMode: false, autoApprove: false, paused: true,
        icon: Clock, color: '#0A84FF'
      },
      {
        id: 'weekly-review', name: '每周回顾', schedule: '星期五 16:00',
        rrule: 'FREQ=WEEKLY;BYDAY=FR;BYHOUR=16;BYMINUTE=0',
        prompt: '回顾选定项目或目录最近一周的变化，整理已完成进展、关键决定、未完成待办、风险和下周优先级。',
        workspace: [], allowShell: false, trustMode: false, autoApprove: false, paused: true,
        icon: ClipboardList, color: '#8B5CF6'
      },
      {
        id: 'follow-up-monitor', name: '跟进监控', schedule: '工作日 9:00',
        rrule: 'FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR;BYHOUR=9;BYMINUTE=0',
        prompt: '检查选定项目或目录的最近变化与工作记录，标记仍需跟进、回复、确认或做决定的未完成事项和风险。',
        workspace: [], allowShell: false, trustMode: false, autoApprove: false, paused: true,
        icon: Search, color: '#00A86B'
      },
    ];

    const ScheduledSelect = ({ value, options, onChange, testId, ariaLabel, theme, minWidth = 180 }) => {
      const [open, setOpen] = useState(false);
      const rootRef = useRef(null);
      const isDark = theme === 'dark';
      const selected = (options || []).find(option => option.value === value) || (options || [])[0];

      useEffect(() => {
        if (!open) return;
        const closeOutside = (event) => {
          if (rootRef.current && !rootRef.current.contains(event.target)) setOpen(false);
        };
        const closeOnEscape = (event) => {
          if (event.key === 'Escape') {
            event.preventDefault();
            setOpen(false);
          }
        };
        document.addEventListener('pointerdown', closeOutside);
        window.addEventListener('keydown', closeOnEscape);
        return () => {
          document.removeEventListener('pointerdown', closeOutside);
          window.removeEventListener('keydown', closeOnEscape);
        };
      }, [open]);

      return (
        <div ref={rootRef} className="relative justify-self-end min-w-0">
          <button type="button" value={value || ''} data-testid={testId}
            aria-label={ariaLabel} aria-haspopup="listbox" aria-expanded={open}
            onClick={() => setOpen(current => !current)}
            className={`h-8 max-w-[260px] rounded-[9px] pl-3 pr-2 inline-flex items-center justify-end gap-2 text-[14px] font-medium transition-colors outline-none focus-visible:ring-2 focus-visible:ring-[#0B57D0]/40 ${isDark ? 'text-[#E3E3E3] hover:bg-[#2B2C2F]' : 'text-[#1F1F1F] hover:bg-[#F1F3F4]'}`}>
            <span className="truncate">{selected ? selected.label : '请选择'}</span>
            <ChevronDown size={15} className={`shrink-0 transition-transform ${open ? 'rotate-180' : ''} ${isDark ? 'text-[#9AA0A6]' : 'text-[#73777D]'}`} />
          </button>
          {open && (
            <div role="listbox" aria-label={ariaLabel}
              className={`absolute right-0 top-full z-50 mt-1.5 max-h-64 overflow-y-auto custom-scrollbar rounded-[12px] border p-1.5 ${isDark ? 'border-[#3A3B3E] bg-[#242528]' : 'border-[#DFE1E5] bg-white'}`}
              style={{ minWidth, boxShadow: isDark ? '0 12px 30px rgba(0,0,0,.34)' : '0 12px 30px rgba(60,64,67,.18)' }}>
              {(options || []).map(option => {
                const active = option.value === value;
                return (
                  <button key={option.value || '__empty'} type="button" role="option" aria-selected={active}
                    data-value={option.value} data-testid={testId ? `${testId}-option` : undefined}
                    onClick={() => { setOpen(false); if (!active) onChange(option.value); }}
                    className={`w-full min-h-9 rounded-[8px] px-3 py-2 flex items-center gap-3 text-left text-[14px] transition-colors ${active ? (isDark ? 'bg-[#364A66] text-[#D2E3FC]' : 'bg-[#E8F0FE] text-[#174EA6]') : (isDark ? 'text-[#E3E3E3] hover:bg-[#303134]' : 'text-[#202124] hover:bg-[#F1F3F4]')}`}>
                    <span className="min-w-0 flex-1 truncate">{option.label}</span>
                    <Check size={15} className={`shrink-0 ${active ? 'opacity-100' : 'opacity-0'}`} />
                  </button>
                );
              })}
            </div>
          )}
        </div>
      );
    };

    const ScheduledTasksView = ({ theme, t, onOpenChat }) => {
      const bs = useBridge();
      const appState = bs || (bridge?.getState ? bridge.getState() : {});
      const tasks = appState.scheduledTasks || [];
      const selectedDetail = appState.scheduledTaskDetail || null;
      const runs = appState.scheduledTaskRuns || [];
      const loading = !!appState.scheduledTaskLoading;
      const busyAction = appState.scheduledTaskBusyAction || null;
      const error = appState.scheduledTaskError || null;
      const isDark = theme === 'dark';
      const [query, setQuery] = useState('');
      const [taskFilter, setTaskFilter] = useState('all');
      const [clockNow, setClockNow] = useState(() => Date.now());
      const selectedId = appState.selectedScheduledTaskId || null;
      const [createMenuOpen, setCreateMenuOpen] = useState(false);
      const [deleteConfirmId, setDeleteConfirmId] = useState(null);
      const [detailForm, setDetailForm] = useState(null);
      const [saveState, setSaveState] = useState('idle');
      const saveTimerRef = useRef(null);
      const pendingPatchRef = useRef({});
      const editTaskIdRef = useRef(null);
      const saveChainRef = useRef(Promise.resolve());
      const saveSequenceRef = useRef(0);
      const latestFieldSequenceRef = useRef({});
      const mountedRef = useRef(true);

      useEffect(() => {
        if (!bridge || !bridge.refreshScheduledTaskData) return;
        const refresh = () => bridge.refreshScheduledTaskData(20).catch(() => {});
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

      const filtered = tasks.filter(task => {
        const q = query.trim().toLowerCase();
        const matchesQuery = !q || (
          (task.name || '') + ' ' +
          (task.scheduleLabel || '') + ' ' +
          (task.prompt || '') + ' ' +
          ((task.cwds || []).join(' '))
        ).toLowerCase().includes(q);
        const matchesFilter = taskFilter === 'all'
          || (taskFilter === 'active' && task.status === 'active')
          || (taskFilter === 'paused' && task.status !== 'active');
        return matchesQuery && matchesFilter;
      });
      const selected = tasks.find(task => task.id === selectedId) || null;
      const detail = selectedDetail && selected && selectedDetail.id === selected.id ? selectedDetail : selected;
      const hasAnyTask = tasks.length > 0;
      const hasQuery = query.trim().length > 0;
      const accent = isDark ? '#0A84FF' : '#007AFF';
      const subtleText = isDark ? 'text-[#9AA0A6]' : 'text-[#85888D]';
      const bodyText = isDark ? 'text-[#E3E3E3]' : 'text-[#1F1F1F]';
      const border = isDark ? 'border-[#2C2D30]' : 'border-[#ECEEF1]';
      const panelBg = isDark ? 'bg-[#17181A]' : 'bg-white';
      const rowHover = isDark ? 'hover:bg-[#242528]' : 'hover:bg-[#F5F5F6]';
      const fmtDateTime = (value) => {
        if (!value) return '未安排';
        const d = new Date(value);
        if (Number.isNaN(d.getTime())) return value;
        const p = (n) => String(n).padStart(2, '0');
        return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`;
      };
      const statusLabel = (value) => {
        if (value === 'active') return '活跃';
        if (value === 'paused') return '已暂停';
        return value || '未知';
      };
      const runStatusLabel = (value) => ({
        queued: '等待中', running: '运行中', completed: '已完成', failed: '失败', canceled: '已取消'
      }[value] || value || '未知');
      const taskSummary = (task) => {
        const schedule = task.scheduleLabel || '暂无计划';
        if (task.status !== 'active') return `${schedule} · ${statusLabel(task.status)}`;
        if (!task.nextRunAt) return `${schedule} · 等待调度`;
        const next = new Date(task.nextRunAt);
        if (Number.isNaN(next.getTime())) return `${schedule} · 等待调度`;
        const now = new Date(clockNow);
        const pad = value => String(value).padStart(2, '0');
        const sameDay = next.getFullYear() === now.getFullYear()
          && next.getMonth() === now.getMonth()
          && next.getDate() === now.getDate();
        const exact = `${sameDay ? '' : `${next.getMonth() + 1}月${next.getDate()}日 `}${pad(next.getHours())}:${pad(next.getMinutes())}`;
        const totalSeconds = Math.max(0, Math.ceil((next.getTime() - clockNow) / 1000));
        const days = Math.floor(totalSeconds / 86400);
        const hours = Math.floor((totalSeconds % 86400) / 3600);
        const minutes = Math.floor((totalSeconds % 3600) / 60);
        const seconds = totalSeconds % 60;
        let remaining = '即将执行';
        if (days > 0) remaining = `${days}天${hours ? `${hours}小时` : ''}后`;
        else if (hours > 0) remaining = `${hours}小时${minutes ? `${minutes}分` : ''}后`;
        else if (minutes > 0) remaining = `${minutes}分${seconds}秒后`;
        else if (seconds > 0) remaining = `${seconds}秒后`;
        return `${schedule} · 下次 ${exact}（${remaining}）`;
      };
      const activeModel = (appState.savedModels || []).find(model => model.id === appState.activeModelId);
      const visibleSuggestions = SCHEDULED_TASK_TEMPLATES.filter(template => {
        const representedByTask = tasks.some(task => {
          const sameTemplateSource = task.templateId === template.id;
          const sameNameAndSchedule = String(task.name || '').trim() === template.name
            && task.rrule === template.rrule;
          const sameDefinition = task.rrule === template.rrule && task.prompt === template.prompt;
          return sameTemplateSource || sameNameAndSchedule || sameDefinition;
        });
        return !representedByTask;
      });
      const detailFormIsValid = !!detailForm &&
        !!String(detailForm.name || '').trim() &&
        !!String(detailForm.prompt || '').trim() &&
        !!String(detailForm.rrule || '').trim();
      const detailHasWorkspace = !!detailForm && (detailForm.cwds || []).some(path => String(path || '').trim());
      function taskForm(task) {
        if (!task) return null;
        return {
          id: task.id,
          name: task.name || '',
          prompt: task.prompt || '',
          rrule: task.rrule || 'FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR;BYHOUR=8;BYMINUTE=0',
          cwds: Array.isArray(task.cwds) ? [...task.cwds] : [],
          model: task.model || (activeModel && activeModel.model) || '',
          allowShell: !!task.allowShell,
          trustMode: !!task.trustMode,
          autoApprove: !!task.autoApprove,
        };
      }

      useEffect(() => {
        if (saveTimerRef.current) clearTimeout(saveTimerRef.current);
        saveTimerRef.current = null;
        pendingPatchRef.current = {};
        editTaskIdRef.current = detail && detail.id || null;
        setDetailForm(taskForm(detail));
        setSaveState('idle');
      }, [selectedId, detail && detail.id]);

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
            if (!taskId || invalid || !Object.keys(patch).length || !bridge || !bridge.updateScheduledTask) return null;
            pendingPatchRef.current = {};
            return bridge.updateScheduledTask(taskId, patch);
          }).catch(() => {});
        };
      }, []);

      function persistDetailPatch(taskId, patch) {
        if (!taskId || !bridge || !bridge.updateScheduledTask || !Object.keys(patch || {}).length) {
          return Promise.resolve({ok: true, skipped: true});
        }
        const payload = {...patch};
        const sequence = ++saveSequenceRef.current;
        Object.keys(payload).forEach(key => { latestFieldSequenceRef.current[key] = sequence; });
        if (mountedRef.current) setSaveState('saving');
        const request = saveChainRef.current.catch(() => {}).then(() => bridge.updateScheduledTask(taskId, payload)).then(updated => {
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
        const taskId = editTaskIdRef.current;
        setDetailForm(current => current ? {...current, [key]: value} : current);
        flushTextEdits();
        return persistDetailPatch(taskId, {[key]: value});
      }

      async function selectTask(id) {
        if (!(await flushBeforeAction())) return;
        if (bridge && bridge.selectScheduledTask) bridge.selectScheduledTask(id);
        if (id && bridge && bridge.refreshScheduledTaskData) bridge.refreshScheduledTaskData(20).catch(() => {});
      }

      async function startTemplate(template) {
        const workspace = Array.isArray(template.workspace) ? template.workspace : [];
        if (!bridge || !bridge.createScheduledTask) return;
        if (!(await flushBeforeAction())) return;
        setCreateMenuOpen(false);
        try {
          await bridge.createScheduledTask({
          templateId: template.id, name: template.name, prompt: template.prompt, rrule: template.rrule,
          cwds: [...workspace],
          model: activeModel && activeModel.model || null,
          mode: 'yolo',
          allowShell: !!template.allowShell,
          trustMode: !!template.trustMode,
          autoApprove: !!template.autoApprove,
          paused: !!template.paused,
          });
        } catch (_) {}
      }

      async function startBlankTask() {
        if (!bridge || !bridge.createScheduledTask) return;
        if (!(await flushBeforeAction())) return;
        setCreateMenuOpen(false);
        try {
          await bridge.createScheduledTask({
            name: '新任务',
            prompt: '请描述这个定时任务每次运行时要完成的工作。',
            rrule: 'FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR;BYHOUR=8;BYMINUTE=0',
            cwds: [], model: activeModel && activeModel.model || null,
            mode: 'yolo', allowShell: false, trustMode: false, autoApprove: false,
            paused: true,
          });
        } catch (_) {}
      }

      function requestDeleteTask(e, id) {
        e.stopPropagation();
        setDeleteConfirmId(id);
      }

      function cancelDeleteTask(e) {
        e.stopPropagation();
        if (busyAction) return;
        setDeleteConfirmId(null);
      }

      async function confirmDeleteTask(e, id) {
        e.stopPropagation();
        if (!bridge || !bridge.deleteScheduledTask || busyAction) return;
        try {
          await bridge.deleteScheduledTask(id);
          setDeleteConfirmId(null);
        } catch (_) {}
      }

      async function toggleTask(e, task) {
        e.stopPropagation();
        if (!bridge) return;
        try {
          if (!(await flushBeforeAction())) return;
          if (task.status === 'active' && bridge.pauseScheduledTask) await bridge.pauseScheduledTask(task.id);
          if (task.status !== 'active' && bridge.resumeScheduledTask) await bridge.resumeScheduledTask(task.id);
        } catch (_) {}
      }

      async function startChatCreation() {
        if (!bridge || !bridge.startScheduledTaskChat) return;
        try {
          if (!(await flushBeforeAction())) return;
          setCreateMenuOpen(false);
          const started = await bridge.startScheduledTaskChat();
          if (started && onOpenChat) onOpenChat();
        } catch (_) {}
      }

      async function runTaskNow(id) {
        if (!bridge || !bridge.runScheduledTaskNow || busyAction || !detailFormIsValid || !detailHasWorkspace) return;
        try {
          if (!(await flushBeforeAction())) return;
          await bridge.runScheduledTaskNow(id);
        } catch (_) {}
      }

      async function openRunChat(run) {
        if (!run || !run.sessionId || !bridge || !bridge.openScheduledRunChat) return;
        try {
          if (!(await flushBeforeAction())) return;
          const opened = await bridge.openScheduledRunChat(run, detail || selected);
          if (!opened) return;
        } catch (_) {}
      }

      async function chooseDetailFolder() {
        if (!bridge || !bridge.pickFolder) return;
        try {
          const selectedFolder = await bridge.pickFolder();
          if (selectedFolder) editImmediateField('cwds', [selectedFolder]);
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
        const days = String(fields.BYDAY || '').split(',').filter(Boolean);
        let repeat = 'workdays';
        if (fields.FREQ === 'MINUTELY') repeat = 'minutely';
        else if (fields.FREQ === 'HOURLY') repeat = 'hourly';
        else if (days.join(',') === 'MO,TU,WE,TH,FR,SA,SU') repeat = 'daily';
        else if (days.join(',') !== 'MO,TU,WE,TH,FR') repeat = 'weekly';
        return {
          repeat,
          days,
          day: days[0] || 'MO',
          interval: Number(fields.INTERVAL || 1),
          time: `${String(fields.BYHOUR != null ? fields.BYHOUR : 8).padStart(2, '0')}:${String(fields.BYMINUTE != null ? fields.BYMINUTE : 0).padStart(2, '0')}`,
        };
      }

      function scheduleRepeatLabel(editor) {
        if (!editor) return '';
        if (editor.repeat === 'minutely') {
          return editor.interval === 1 ? '每分钟' : `每 ${editor.interval} 分钟`;
        }
        if (editor.repeat === 'hourly') {
          return editor.interval === 1 ? '每小时' : `每 ${editor.interval} 小时`;
        }
        return {workdays: '工作日', daily: '每天', weekly: '每周'}[editor.repeat] || '自定义';
      }

      function buildEditedRrule(key, value) {
        const currentRrule = detailForm && detailForm.rrule;
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
        if (key === 'day') {
          fields.FREQ = 'WEEKLY';
          fields.BYDAY = value;
          fields.BYHOUR = String(Number(hour || 0));
          fields.BYMINUTE = String(Number(minute || 0));
          return serializeScheduleFields(fields);
        }
        if (editor.repeat === 'minutely') {
          const interval = previousEditor.repeat === 'minutely' ? editor.interval : 10;
          return `FREQ=MINUTELY;INTERVAL=${Math.max(1, interval || 10)}`;
        }
        if (editor.repeat === 'hourly') {
          const interval = previousEditor.repeat === 'hourly' ? editor.interval : 1;
          return `FREQ=HOURLY;INTERVAL=${Math.max(1, interval || 1)}`;
        }
        const days = editor.repeat === 'daily' ? 'MO,TU,WE,TH,FR,SA,SU'
          : (editor.repeat === 'workdays' ? 'MO,TU,WE,TH,FR'
            : (previousEditor.repeat === 'weekly' ? (editor.days.join(',') || editor.day) : editor.day));
        return `FREQ=WEEKLY;BYDAY=${days};BYHOUR=${Number(hour || 0)};BYMINUTE=${Number(minute || 0)}`;
      }

      function editSchedule(key, value) {
        editImmediateField('rrule', buildEditedRrule(key, value));
      }

      const scheduleEditor = detailForm ? scheduleEditorValue(detailForm.rrule) : null;

      function scheduleDayLabel(days) {
        const labels = {MO:'星期一', TU:'星期二', WE:'星期三', TH:'星期四', FR:'星期五', SA:'星期六', SU:'星期日'};
        return (days || []).map(day => labels[day] || day).join('、');
      }

      const modelOptions = (appState.savedModels || []).map(model => ({
        value: model.model,
        label: model.name || model.model,
      }));
      if (detailForm && detailForm.model && !modelOptions.some(option => option.value === detailForm.model)) {
        modelOptions.unshift({ value: detailForm.model, label: detailForm.model });
      } else if (detailForm && !detailForm.model) {
        modelOptions.unshift({ value: '', label: '当前模型' });
      }
      const repeatOptions = scheduleEditor ? [
        { value: 'workdays', label: '工作日' },
        { value: 'daily', label: '每天' },
        { value: 'weekly', label: '每周' },
        { value: 'hourly', label: scheduleEditor.repeat === 'hourly' ? scheduleRepeatLabel(scheduleEditor) : '每小时' },
        { value: 'minutely', label: scheduleEditor.repeat === 'minutely' ? scheduleRepeatLabel(scheduleEditor) : '每 10 分钟' },
      ] : [];
      const weekdayOptions = [
        ['MO','星期一'], ['TU','星期二'], ['WE','星期三'], ['TH','星期四'],
        ['FR','星期五'], ['SA','星期六'], ['SU','星期日'],
      ].map(([value, label]) => ({ value, label }));
      const currentDaysValue = scheduleEditor ? (scheduleEditor.days.join(',') || scheduleEditor.day) : 'MO';
      const dayOptions = scheduleEditor && scheduleEditor.days.length > 1
        ? [{ value: currentDaysValue, label: scheduleDayLabel(scheduleEditor.days) }, ...weekdayOptions]
        : weekdayOptions;

      const TemplateSuggestions = () => (
        <section className="mt-5" data-testid="scheduled-template-suggestions">
          <div className={`pb-3 mb-2 border-b text-[15px] font-semibold ${subtleText} ${border}`}>建议</div>
          <div className="space-y-1">
            {visibleSuggestions.map(template => {
              const TemplateIcon = template.icon;
              return (
                <button key={template.id} type="button" onClick={() => startTemplate(template)}
                  data-testid={`scheduled-template-${template.id}`}
                  aria-label={`使用${template.name}模板`}
                  title={`使用${template.name}模板`}
                  className={`w-full rounded-[10px] px-3 py-3 flex items-start gap-3 text-left transition-colors ${rowHover}`}>
                  <span className="mt-0.5 w-7 h-7 shrink-0 rounded-[8px] flex items-center justify-center" style={{ color: template.color, background: `${template.color}14` }}>
                    <TemplateIcon size={16} />
                  </span>
                  <span className="min-w-0 flex-1">
                    <span className="flex flex-wrap items-baseline gap-x-2">
                      <span className={`text-[15px] font-semibold ${bodyText}`}>{template.name}</span>
                      <span className={`text-[13px] ${subtleText}`}>{template.schedule}</span>
                    </span>
                    <span className={`mt-1 block text-[13px] leading-5 ${subtleText}`}>{template.prompt}</span>
                  </span>
                </button>
              );
            })}
          </div>
          {!visibleSuggestions.length && (
            <div className={`py-4 text-[13px] ${subtleText}`}>所有建议模板都已添加</div>
          )}
        </section>
      );

      const renderTaskRow = (task) => {
        const selectedRow = selectedId === task.id;
        const canToggle = task.status === 'active' || (task.cwds || []).some(path => String(path || '').trim());
        return (
          <div
            key={task.id}
            className={`group w-full min-h-[60px] rounded-[9px] px-3 py-2.5 flex items-center gap-3 transition-colors ${selectedRow ? (isDark ? 'bg-[#242528]' : 'bg-[#F1F1F2]') : rowHover}`}
          >
            <button
              type="button"
              onClick={() => selectTask(task.id)}
              aria-label={`查看定时任务：${task.name}`}
              title={`查看定时任务：${task.name}`}
              className="min-w-0 flex-1 flex items-center gap-3 text-left"
            >
              <span className="relative w-5 h-5 shrink-0">
                {task.isRunning ? (
                  <span data-testid="scheduled-task-running" aria-label="任务正在运行"
                    className="absolute left-[2px] top-[3px] w-4 h-4 rounded-full border-2 animate-spin"
                    style={{ borderColor: accent, borderTopColor: 'transparent' }} />
                ) : (
                  <span className="absolute left-[3px] top-[4px] w-[14px] h-[14px] rounded-full border-2" style={{ borderColor: isDark ? '#A1A1A6' : '#8E8E93' }} />
                )}
                {task.hasUnreadRuns && (
                  <span data-testid="scheduled-task-unread" aria-label="有未查看的运行对话"
                    className="absolute right-[1px] top-[1px] w-2.5 h-2.5 rounded-full border-2"
                    style={{ background: accent, borderColor: isDark ? '#17181A' : '#ffffff' }} />
                )}
              </span>
              <span className="min-w-0 flex-1">
                <span className={`block truncate text-[15px] font-semibold ${bodyText}`}>{task.name}</span>
                <span data-testid="scheduled-task-summary" className={`mt-0.5 block truncate text-[13px] ${subtleText}`}>{taskSummary(task)}</span>
              </span>
            </button>
            <span className={`shrink-0 flex items-center gap-1 transition-opacity ${deleteConfirmId === task.id ? 'opacity-100' : 'opacity-0 group-hover:opacity-100 focus-within:opacity-100'}`}>
              {deleteConfirmId === task.id ? (
                <span data-testid="scheduled-delete-confirmation" className={`h-9 pl-3 pr-1 rounded-full inline-flex items-center gap-1 text-[12px] ${isDark ? 'bg-[#3A2424] text-[#F2B8B5]' : 'bg-[#FCE8E6] text-[#A50E0E]'}`}>
                  <span className="whitespace-nowrap">确认删除？</span>
                  <button type="button" data-testid="scheduled-delete-cancel" onClick={cancelDeleteTask}
                    disabled={!!busyAction} aria-label="取消删除定时任务" title="取消删除"
                    className={`h-7 px-2 rounded-full disabled:opacity-50 ${isDark ? 'hover:bg-white/10' : 'hover:bg-black/5'}`}>取消</button>
                  <button type="button" data-testid="scheduled-delete-confirm" onClick={(e) => confirmDeleteTask(e, task.id)}
                    disabled={!!busyAction} aria-label={`确认删除${task.name}`} title="确认删除"
                    className="h-7 px-2 rounded-full bg-[#C5221F] text-white disabled:opacity-50">删除</button>
                </span>
              ) : (<>
                <button
                  type="button"
                  title={task.status === 'active' ? '暂停' : (canToggle ? '运行' : '请先选择项目')}
                  aria-label={task.status === 'active' ? `暂停${task.name}` : `恢复${task.name}`}
                  disabled={!!busyAction || !canToggle}
                  onClick={(e) => toggleTask(e, task)}
                  className={`w-8 h-8 rounded-full flex items-center justify-center transition-colors disabled:opacity-50 ${isDark ? 'text-[#C4C7C5] hover:bg-[#333537]' : 'text-[#72757A] hover:bg-[#E8EAED]'}`}
                >
                  {task.status === 'active' ? <StopCircle size={16} /> : <Play size={16} />}
                </button>
                <button
                  type="button"
                  title="删除"
                  aria-label={`删除${task.name}`}
                  disabled={!!busyAction}
                  data-testid="scheduled-list-delete"
                  onClick={(e) => requestDeleteTask(e, task.id)}
                  className={`w-8 h-8 rounded-full flex items-center justify-center transition-colors disabled:opacity-50 ${isDark ? 'text-[#C4C7C5] hover:text-[#F28B82] hover:bg-[#5c2b29]' : 'text-[#72757A] hover:text-[#C5221F] hover:bg-[#FAD2CF]'}`}
                >
                  <Trash2 size={16} />
                </button>
              </>)}
            </span>
          </div>
        );
      };

      return (
        <div data-testid="scheduled-page" aria-busy={!!busyAction} className={`flex-1 min-h-0 w-full h-full relative z-10 ${panelBg}`}>
          <div className={`h-full flex flex-col ${isDark ? 'text-[#E3E3E3]' : 'text-[#1F1F1F]'}`}>
            <div className={`h-14 shrink-0 grid border-b ${border} ${selected ? 'grid-cols-[minmax(420px,0.96fr)_minmax(420px,1.04fr)]' : 'grid-cols-1'}`}>
              <div data-testid="scheduled-left-toolbar" className="min-w-0 px-5 flex items-center justify-between">
                <div data-testid="scheduled-filter-tabs" className="flex items-center gap-1">
                  {[
                    ['all', '全部'],
                    ['active', '已开启'],
                    ['paused', '已暂停'],
                  ].map(([value, label]) => (
                    <button key={value} type="button" onClick={() => setTaskFilter(value)}
                      aria-pressed={taskFilter === value}
                      className={`h-8 px-3 rounded-[9px] text-[14px] transition-colors ${taskFilter === value ? (isDark ? 'bg-[#2B2C2F] text-white' : 'bg-[#F0F0F1] text-[#1F1F1F]') : `${subtleText} ${rowHover}`}`}>
                      {label}
                    </button>
                  ))}
                </div>
                <div className="relative flex items-center gap-2">
                  <button type="button" onClick={() => setCreateMenuOpen(value => !value)}
                    data-testid="scheduled-create-menu" aria-label="创建定时任务" title="创建定时任务"
                    disabled={!!busyAction}
                    className={`h-9 px-4 rounded-full border flex items-center gap-2 text-[15px] font-semibold transition-colors ${isDark ? 'border-[#333537] hover:bg-[#242528]' : 'border-[#E3E5E8] hover:bg-[#F5F5F6]'}`}>
                    创建 <ChevronDown size={16} className={subtleText} />
                  </button>
                  {createMenuOpen && (
                    <div className={`absolute right-0 top-11 z-30 w-48 overflow-hidden rounded-[12px] border p-1 shadow-lg ${border} ${isDark ? 'bg-[#242528]' : 'bg-white'}`}>
                      <button type="button" onClick={startBlankTask} className={`w-full rounded-[8px] px-3 py-2 text-left text-[14px] ${rowHover}`}>自定义任务</button>
                      <button type="button" onClick={startChatCreation} className={`w-full rounded-[8px] px-3 py-2 text-left text-[14px] ${rowHover}`}>通过聊天创建</button>
                    </div>
                  )}
                </div>
              </div>
              {selected ? (
                <div data-testid="scheduled-detail-toolbar" className={`min-w-0 border-l px-6 flex items-center justify-between ${border}`}>
                  <span className="flex items-center gap-3 text-[13px] font-semibold" style={{ color: accent }}>
                    {statusLabel(detail.status)}
                    {saveState === 'saving' && <span data-testid="scheduled-save-state" className={subtleText}>正在保存…</span>}
                    {saveState === 'saved' && <span data-testid="scheduled-save-state" className={subtleText}>已保存</span>}
                    {saveState === 'error' && <span data-testid="scheduled-save-state" className="text-[#C5221F]">保存失败</span>}
                    {saveState === 'invalid' && <span data-testid="scheduled-save-state" className="text-[#C5221F]">名称和说明不能为空</span>}
                  </span>
                  <div className="flex items-center gap-1">
                    <button type="button" onClick={() => runTaskNow(selected.id)} disabled={!!busyAction || !detailFormIsValid || !detailHasWorkspace}
                      data-testid="scheduled-run-now" aria-label="立即运行定时任务" title="立即运行"
                      className={`w-8 h-8 rounded-full flex items-center justify-center disabled:opacity-50 ${rowHover} ${subtleText}`}>
                      <Play size={15} />
                    </button>
                    <button type="button" onClick={() => toggleTask({ stopPropagation() {} }, selected)} disabled={!!busyAction || (selected.status !== 'active' && !detailHasWorkspace)}
                      aria-label={selected.status === 'active' ? '暂停定时任务' : '恢复定时任务'}
                      title={selected.status === 'active' ? '暂停' : '恢复'}
                      className={`w-8 h-8 rounded-full flex items-center justify-center disabled:opacity-50 ${rowHover} ${subtleText}`}>
                      {selected.status === 'active' ? <Pause size={15} /> : <Play size={15} />}
                    </button>
                    <button type="button" onClick={() => selectTask(null)} title="关闭详情" aria-label="关闭定时任务详情"
                      className={`w-8 h-8 rounded-full flex items-center justify-center ${rowHover} ${subtleText}`}>
                      <X size={15} />
                    </button>
                  </div>
                </div>
              ) : null}
            </div>

            <div className="flex-1 min-h-0 overflow-hidden">
              <div className={`h-full grid transition-[grid-template-columns] duration-200 ${selected ? 'grid-cols-[minmax(420px,0.96fr)_minmax(420px,1.04fr)]' : 'grid-cols-1'}`}>
                <div data-testid="scheduled-list" className="min-w-0 h-full overflow-y-auto custom-scrollbar px-6 pb-16">
                  <div className={`${selected ? 'max-w-[760px]' : 'max-w-[860px] mx-auto'} pt-4 transition-all`}>
                    <div className={`h-10 rounded-full border px-4 flex items-center gap-3 ${isDark ? 'border-[#333537] bg-[#131314]' : 'border-[#E0E2E5] bg-white'}`}>
                      <Search size={19} className={subtleText} />
                      <input
                        value={query}
                        onChange={e => setQuery(e.target.value)}
                        placeholder="搜索定时任务"
                        aria-label="搜索定时任务"
                        className={`w-full min-w-0 bg-transparent outline-none text-[15px] ${bodyText} ${isDark ? 'placeholder:text-[#777B82]' : 'placeholder:text-[#A0A3A8]'}`}
                      />
                    </div>

                    {error && (
                      <div role="alert" data-testid="scheduled-error" className={`mt-4 rounded-[8px] border px-4 py-3 text-[13px] ${border} ${isDark ? 'bg-[#1F1416] text-[#F2B8B5]' : 'bg-[#FCE8E6] text-[#A50E0E]'}`}>
                        {error}
                      </div>
                    )}

                    {hasAnyTask ? (
                      <>
                        {filtered.length ? (
                          <div className="mt-5 space-y-2">
                            {filtered.map(renderTaskRow)}
                          </div>
                        ) : (
                          <div className={`mt-5 py-6 text-center text-[14px] ${subtleText}`}>没有匹配的任务</div>
                        )}
                        <TemplateSuggestions />
                      </>
                    ) : (
                      <>
                        <TemplateSuggestions />
                        {(loading || hasQuery) && (
                          <div className={`mt-8 rounded-[10px] border px-6 py-6 text-center text-[14px] ${border} ${subtleText}`}>
                            {loading ? '正在读取定时任务…' : '没有匹配的定时任务。'}
                          </div>
                        )}
                      </>
                    )}
                  </div>
                </div>

                {selected && detailForm && (
                  <aside data-testid="scheduled-detail" className={`min-w-0 h-full overflow-y-auto custom-scrollbar border-l px-6 py-5 ${border}`}>
                    <div className="max-w-[720px] pb-10">
                      <div data-testid="scheduled-detail-title">
                        <input data-testid="scheduled-live-title" value={detailForm.name}
                          onChange={e => editTextField('name', e.target.value)} onBlur={() => finishTextField('name')}
                          aria-label="定时任务名称"
                          className={`w-full bg-transparent outline-none text-[19px] font-semibold tracking-normal ${bodyText}`} />
                      </div>
                      <div data-testid="scheduled-detail-prompt" className={`mt-5 rounded-[14px] border px-4 py-3 ${border}`}>
                        <textarea data-testid="scheduled-live-prompt" value={detailForm.prompt}
                          onChange={e => editTextField('prompt', e.target.value)} onBlur={() => finishTextField('prompt')}
                          rows="3" aria-label="定时任务说明" placeholder="描述每次运行时要完成的工作"
                          className={`w-full resize-y bg-transparent outline-none text-[14px] leading-6 ${bodyText}`} />
                      </div>

                      <div className="mt-7 space-y-7">
                        <section data-testid="scheduled-detail-settings">
                          <div className={`mb-2 px-1 text-[13px] font-medium ${subtleText}`}>详情</div>
                          <div className={`relative rounded-[14px] border divide-y ${border} ${isDark ? 'divide-[#2C2D30]' : 'divide-[#ECEEF1]'}`}>
                            <div className="grid grid-cols-[96px_minmax(0,1fr)] items-center gap-4 px-4 py-3 text-[14px]">
                              <span className={bodyText}>运行于</span>
                              <span className={`justify-self-end font-medium ${bodyText}`}>独立会话</span>
                            </div>
                            <div className="grid grid-cols-[96px_minmax(0,1fr)] items-center gap-4 px-4 py-2 text-[14px]">
                              <span className={bodyText}>项目</span>
                              <span className="min-w-0 flex items-center justify-end gap-2">
                                <input data-testid="scheduled-live-project" value={(detailForm.cwds && detailForm.cwds[0]) || ''}
                                  onChange={e => editTextField('cwds', e.target.value ? [e.target.value] : [])}
                                  onBlur={() => finishTextField('cwds')} placeholder="无" aria-label="定时任务工作目录"
                                  className={`min-w-0 flex-1 bg-transparent text-right outline-none ${bodyText}`} />
                                <button type="button" data-testid="scheduled-detail-pick-folder"
                                  onClick={chooseDetailFolder} aria-label="选择定时任务工作目录" title="选择文件夹"
                                  className={`w-8 h-8 shrink-0 rounded-full flex items-center justify-center ${rowHover} ${subtleText}`}>
                                  <FolderOpen size={16} />
                                </button>
                              </span>
                            </div>
                            {!detailHasWorkspace && (
                              <div data-testid="scheduled-workspace-required" className={`px-4 py-2 text-[12px] ${subtleText}`}>
                                请选择项目后再启用或立即运行任务。
                              </div>
                            )}
                            <div className="grid grid-cols-[96px_minmax(0,1fr)] items-center gap-4 px-4 py-2 text-[14px]">
                              <span className={bodyText}>模型</span>
                              <ScheduledSelect value={detailForm.model || ''} options={modelOptions}
                                onChange={value => editImmediateField('model', value)}
                                testId="scheduled-live-model" ariaLabel="选择定时任务模型" theme={theme} minWidth={220} />
                            </div>
                            <div className="grid grid-cols-[96px_minmax(0,1fr)] items-center gap-4 px-4 py-3 text-[14px]">
                              <span className={bodyText}>权限</span>
                              <span className="justify-self-end flex flex-wrap justify-end gap-x-4 gap-y-2">
                                {[
                                  ['allowShell', 'Shell'],
                                  ['trustMode', '信任模式'],
                                  ['autoApprove', '自动批准'],
                                ].map(([key, label]) => (
                                  <label key={key} className={`group inline-flex h-8 items-center gap-2 rounded-[9px] px-2 cursor-pointer transition-colors ${isDark ? 'hover:bg-[#2B2C2F]' : 'hover:bg-[#F1F3F4]'} ${subtleText}`}>
                                    <input type="checkbox" checked={!!detailForm[key]} className="sr-only"
                                      onChange={e => editImmediateField(key, e.target.checked)}
                                    />
                                    <span aria-hidden="true"
                                      className={`w-4 h-4 shrink-0 rounded-[5px] border flex items-center justify-center transition-colors ${detailForm[key] ? 'border-transparent text-white' : (isDark ? 'border-[#777B82] bg-transparent' : 'border-[#A8ADB3] bg-white')}`}
                                      style={detailForm[key] ? { background: accent } : undefined}>
                                      {detailForm[key] && <Check size={11} strokeWidth={3} />}
                                    </span>
                                    <span>{label}</span>
                                  </label>
                                ))}
                              </span>
                            </div>
                          </div>
                        </section>

                        <section data-testid="scheduled-detail-frequency">
                          <div className={`mb-2 px-1 text-[13px] font-medium ${subtleText}`}>频率</div>
                          <div className={`relative rounded-[14px] border divide-y ${border} ${isDark ? 'divide-[#2C2D30]' : 'divide-[#ECEEF1]'}`}>
                            <div className="grid grid-cols-[96px_minmax(0,1fr)] items-center gap-4 px-4 py-2 text-[14px]">
                              <span className={bodyText}>重复</span>
                              <ScheduledSelect value={scheduleEditor.repeat} options={repeatOptions}
                                onChange={value => editSchedule('repeat', value)}
                                testId="scheduled-live-repeat" ariaLabel="选择重复频率" theme={theme} />
                            </div>
                            {scheduleEditor.repeat === 'weekly' && (
                              <div className="grid grid-cols-[96px_minmax(0,1fr)] items-center gap-4 px-4 py-2 text-[14px]">
                                <span className={bodyText}>日期</span>
                                <ScheduledSelect value={currentDaysValue} options={dayOptions}
                                  onChange={value => editSchedule('day', value)}
                                  testId="scheduled-live-day" ariaLabel="选择运行日期" theme={theme} />
                              </div>
                            )}
                            {scheduleEditor.repeat !== 'hourly' && scheduleEditor.repeat !== 'minutely' && (
                              <label data-testid="scheduled-live-time-row" className="grid grid-cols-[96px_minmax(0,1fr)] items-center gap-4 px-4 py-3 text-[14px]">
                                <span className={bodyText}>时间</span>
                                <input data-testid="scheduled-live-time" type="time" value={scheduleEditor.time}
                                  onChange={e => editSchedule('time', e.target.value)}
                                  className={`justify-self-end bg-transparent text-right font-medium outline-none ${bodyText}`} />
                              </label>
                            )}
                          </div>
                        </section>

                        <section>
                          <div className={`flex items-center justify-between mb-2 px-1 ${subtleText}`}>
                            <span className="text-[13px] font-medium">运行历史记录</span>
                            <span className="text-[12px]">{runs.length ? `${runs.length} 条` : ''}</span>
                          </div>
                          {runs.length ? (
                            <div className={`overflow-hidden rounded-[12px] border divide-y ${border} ${isDark ? 'divide-[#2C2D30]' : 'divide-[#ECEEF1]'}`}>
                              {runs.map(item => (
                                <button key={item.id} type="button" disabled={!item.sessionId} onClick={() => openRunChat(item)}
                                  data-testid="scheduled-run-row"
                                  className={`w-full grid grid-cols-[16px_minmax(0,1fr)_116px] items-center gap-3 px-4 py-3 text-left text-[14px] transition-colors ${item.sessionId ? rowHover : 'cursor-default opacity-70'}`}
                                  title={item.sessionId ? '打开运行对话' : '此运行记录还没有可打开的会话'}
                                  aria-label={item.sessionId ? `打开运行记录：${runStatusLabel(item.status)}` : '此运行记录还没有可打开的会话'}>
                                  {['queued', 'running'].includes(item.status) ? (
                                    <span data-testid="scheduled-run-running" aria-label="运行正在进行"
                                      className="w-3 h-3 rounded-full border-2 animate-spin"
                                      style={{ borderColor: accent, borderTopColor: 'transparent' }} />
                                  ) : item.unread ? (
                                    <span data-testid="scheduled-run-unread" aria-label="未查看的运行对话"
                                      className="w-2.5 h-2.5 rounded-full" style={{ background: accent }} />
                                  ) : (
                                    <span className="w-2.5 h-2.5 rounded-full" style={{ background: item.status === 'failed' ? '#FF3B30' : '#8E8E93' }} />
                                  )}
                                  <span className={`truncate ${bodyText}`}>{runStatusLabel(item.status)}{item.error ? ` · ${item.error}` : ''}</span>
                                  <span className={`justify-self-end truncate text-[12px] ${subtleText}`}>{fmtDateTime(item.scheduledFor || item.createdAt)}</span>
                                </button>
                              ))}
                            </div>
                          ) : (
                            <div className={`rounded-[12px] border px-4 py-5 text-[13px] ${border} ${subtleText}`}>还没有运行记录</div>
                          )}
                        </section>
                      </div>
                    </div>
                  </aside>
                )}
              </div>
            </div>
          </div>
        </div>
      );
    };

export { ScheduledTasksView };
