import React, { useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { Archive, Briefcase, Check, ChevronDown, Cpu, Database, Edit2, FileText, Lightbulb, MessageSquare, MoreHorizontal, Paperclip, Plus, RefreshCw, Search, Smartphone, Sparkles, Store, Trash2, User, Video, Wrench, X, Zap } from '../../components/icons.jsx';
import { ArchivedDeleteConfirmDialog } from '../../components/layout/NavigationComponents.jsx';
import { VllmSetupProgress } from '../../components/VllmSetupProgress.jsx';
import PetSettingsSection from '../pet/PetSettingsSection.jsx';
import { DEFAULT_PET_ID } from '../pet/pet-registry.js';
import { bridge } from '../../hooks/useBridge.js';
import { formatSessionDate } from '../../shared/date-utils.js';
import { visibleUserModels } from '../../shared/model-options.js';
import { buildComposerToolMenuState } from './composer-tool-menu-logic.js';
import { notifyComposerToolsChanged } from '../tools/tool-events.js';
import deepseekIcon from '../../brand-icons/deepseek.svg';
import doubaoIcon from '../../brand-icons/doubao.svg';
import glmIcon from '../../brand-icons/glm.svg';
import kimiIcon from '../../brand-icons/kimi.svg';
import mimoIcon from '../../brand-icons/mimo.svg';
import minimaxIcon from '../../brand-icons/minimax.svg';
import openaiIcon from '../../brand-icons/openai.svg';
import qwenIcon from '../../brand-icons/qwen.svg';
import { invokeTauri } from '../../platform/tauri/client.js';

function isReadonlyModel(model) {
  return !!(model && (model.readonly || model.system));
}

function isBuiltinLlmApiModel(model) {
  return !!(model && (model.kind === 'builtin_llmapi' || model.id === 'builtin_llmapi'));
}

function hasLlmApiBackendUser(bs) {
  const status = bs && bs.llmApiStatus;
  if (!status || status.backend_user_state === 'not_exists') return false;
  return status.backend_user_state === 'exists' || !!status.backend_user_exists;
}

function visibleSortedModels(models, bs) {
  const allowBuiltin = hasLlmApiBackendUser(bs);
  return (models || [])
    .filter(model => model && model.id && (!isBuiltinLlmApiModel(model) || allowBuiltin))
    .slice()
    .sort((a, b) => Number(isBuiltinLlmApiModel(b)) - Number(isBuiltinLlmApiModel(a)));
}

const SCard = React.forwardRef(({ isDark, title, titleAdornment, children, id, style }, ref) => (
      <section ref={ref} id={id} style={style} className={`rounded-[24px] p-6 ${isDark ? 'bg-[#1E1F20]' : 'bg-[#F0F4F9]'}`}>
        <h2 className="text-[18px] font-medium mb-6 flex items-center gap-2">
          <span>{title}</span>
          {titleAdornment}
        </h2>
        {children}
      </section>
    ));

    const SRow = ({ isDark, label, desc, children }) => (
      <div className="flex items-center justify-between gap-8">
        <div className="min-w-0">
          <span className="text-[16px] block mb-1">{label}</span>
          {desc && <span className={`text-[13px] block ${isDark ? 'text-[#C4C7C5]' : 'text-[#444746]'}`}>{desc}</span>}
        </div>
        <div className="shrink-0">{children}</div>
      </div>
    );

    const SField = ({ isDark, label, ...inputProps }) => (
      <div>
        <span className={`text-[14px] block mb-2 ${isDark ? 'text-[#C4C7C5]' : 'text-[#444746]'}`}>{label}</span>
        <input
          {...inputProps}
          className={`w-full px-4 py-2 rounded-lg text-[14px] outline-none transition-colors ${
            isDark ? 'bg-[#131314] text-[#E3E3E3] border border-[#444746] focus:border-[#A8C7FA]'
                   : 'bg-white text-[#1F1F1F] border border-[#C4C7C5] focus:border-[#0B57D0]'
          }`}
        />
      </div>
    );

    const SSegmented = ({ isDark, options, value, onChange }) => (
      <div className={`p-1 rounded-full flex flex-wrap justify-end gap-1 max-w-full ${isDark ? 'bg-[#131314]' : 'bg-[#E1E5EA]'}`}>
        {options.map(o => (
          <button
            key={o.key}
            onClick={() => onChange(o.key)}
            className={`min-w-[72px] px-4 py-2 rounded-full text-[14px] font-medium transition-colors ${
              value === o.key ? (isDark ? 'bg-[#A8C7FA] text-[#041E49]' : 'bg-white text-[#0B57D0] shadow-sm') : ''
            }`}
          >{o.label}</button>
        ))}
      </div>
    );

    // 「需重启」统一表达：改动后才出现，一句说明 + 一个动作，替代常驻大按钮和斜体小字
    const SActionBar = ({ isDark, message, actionLabel, onAction }) => (
      <div className={`flex items-center justify-between gap-4 px-4 py-3 rounded-xl ${isDark ? 'bg-[#131314]' : 'bg-white'}`}>
        <span className={`text-[13px] ${isDark ? 'text-[#C4C7C5]' : 'text-[#444746]'}`}>{message}</span>
        <button
          onClick={onAction}
          className={`text-[13px] font-medium px-4 py-2 rounded-full whitespace-nowrap transition-colors ${
            isDark ? 'bg-[#A8C7FA] text-[#041E49] hover:bg-[#C2D7FB]'
                   : 'bg-[#0B57D0] text-white hover:bg-[#1967D2]'
          }`}
        >{actionLabel}</button>
      </div>
    );

    const MemorySettingsCard = ({ isDark, bs, memoryEnabled, onMemoryEnabledChange }) => {
      const memory = (bs && bs.memory) || {};
      const profile = memory.profile || {};
      const identity = profile.identity || {};
      const preferences = memory.preferences || [];
      const workContext = memory.work_context || [];
      const currentFocus = (memory.current_focus || []).filter(item => item.status === 'active');
      const recentActivity = (memory.recent_activity || []).filter(item => item.status === 'active');
      const [open, setOpen] = useState(false);
      const [tab, setTab] = useState('long_term');
      const [query, setQuery] = useState('');
      const [menuFor, setMenuFor] = useState(null);
      const [draft, setDraft] = useState({
        call_name: identity.call_name || '',
        assistant_alias: identity.assistant_alias || '',
      });
      const [editing, setEditing] = useState(null);
      const [saving, setSaving] = useState(false);
      const subText = isDark ? 'text-[#C4C7C5]' : 'text-[#444746]';
      const faintText = isDark ? 'text-[#8F969E]' : 'text-[#6B7280]';
      const border = isDark ? 'border-[#333537]' : 'border-[#DDE3EA]';
      const itemBg = isDark ? 'bg-[#131314]' : 'bg-white';
      const cardBg = isDark ? 'bg-[#17191D] border-white/[0.08]' : 'bg-white border-[#DDE3EA]';
      const panelBg = isDark ? 'bg-[#1F2023] text-[#E8EAED]' : 'bg-[#F8FAFD] text-[#1F1F1F]';
      const inputBg = isDark ? 'bg-[#131314] border-[#3C4043] text-[#E8EAED] placeholder:text-[#777D86]' : 'bg-white border-[#DDE3EA] text-[#1F1F1F] placeholder:text-[#8A9099]';
      const ghostBtn = isDark ? 'bg-white/[0.07] text-[#E3E3E3] hover:bg-white/[0.11]' : 'bg-[#E1E5EA] text-[#1F1F1F] hover:bg-[#D3D9E0]';
      const dangerBtn = isDark ? 'text-[#F28B82] hover:bg-[#3A2425]' : 'text-[#C5221F] hover:bg-[#FCE8E6]';
      const primaryBtn = isDark ? 'bg-[#A8C7FA] text-[#041E49] hover:bg-[#C2D7FB]' : 'bg-[#0B57D0] text-white hover:bg-[#1967D2]';
      const selectedTab = isDark
        ? 'bg-[rgba(43,119,255,0.16)] border-[rgba(70,145,255,0.35)] text-[#D8E8FF]'
        : 'bg-[#E8F0FE] border-[#B8D1FF] text-[#0B57D0]';
      const profileCount = (identity.call_name ? 1 : 0) + (identity.assistant_alias ? 1 : 0);
      const profileSummary = [
        identity.call_name ? `称呼：${identity.call_name}` : '',
        identity.assistant_alias ? `助手昵称：${identity.assistant_alias}` : '',
      ].filter(Boolean).join(' · ');
      const total = preferences.length + workContext.length + currentFocus.length + recentActivity.length;
      const longTermItems = [
        ...preferences.map(item => ({ ...item, kind: 'preference' })),
        ...workContext.map(item => ({ ...item, kind: 'work_context' })),
      ];
      const recentItems = [
        ...currentFocus.map(item => ({ ...item, kind: 'current_focus' })),
        ...recentActivity.map(item => ({ ...item, kind: 'recent_activity' })),
      ];
      const longTermCount = profileCount + longTermItems.length;
      const recentCount = recentItems.length;
      const tabs = [
        { key: 'long_term', label: '长期记忆', count: longTermCount, icon: Database },
        { key: 'recent', label: '近期记忆', count: recentCount, icon: RefreshCw },
      ];
      const tabMeta = tabs.find(x => x.key === tab) || tabs[0];
      const memoryTypeLabel = kind => kind === 'current_focus' ? '当前关注'
        : kind === 'recent_activity' ? '近期动态'
        : kind === 'work_context' ? '工作背景'
        : '长期偏好';
      const memoryTypeIcon = kind => kind === 'current_focus' ? Lightbulb
        : kind === 'recent_activity' ? RefreshCw
        : kind === 'work_context' ? Briefcase
        : kind === 'profile' ? User
        : Sparkles;
      const memoryTypeTone = kind => kind === 'work_context' ? 'text-[#8AB4F8] bg-[#1A73E8]/[0.13]'
        : kind === 'current_focus' ? 'text-[#FDD663] bg-[#FDD663]/[0.12]'
        : kind === 'recent_activity' ? 'text-[#81C995] bg-[#34A853]/[0.12]'
        : kind === 'profile' ? 'text-[#C58AF9] bg-[#A142F4]/[0.12]'
        : 'text-[#A8C7FA] bg-[#A8C7FA]/[0.12]';
      const normalizedQuery = query.trim().toLowerCase();
      const searchMatch = text => !normalizedQuery || String(text || '').toLowerCase().includes(normalizedQuery);

      useEffect(() => {
        if (!bridge.available || !bridge.memory.loadMemoryOverview) return;
        bridge.memory.loadMemoryOverview();
      }, [bs && bs.activeSessionId]);
      useEffect(() => {
        setDraft({
          call_name: identity.call_name || '',
          assistant_alias: identity.assistant_alias || '',
        });
      }, [identity.call_name, identity.assistant_alias]);
      useEffect(() => {
        setMenuFor(null);
        setQuery('');
      }, [tab, open]);

      const reload = () => bridge.available && bridge.memory.loadMemoryOverview && bridge.memory.loadMemoryOverview();
      const saveProfile = async () => {
        if (!bridge.available || !bridge.memory.saveMemoryProfilePatch) return;
        setSaving(true);
        try {
          await bridge.memory.saveMemoryProfilePatch({
            call_name: draft.call_name,
            assistant_alias: draft.assistant_alias,
          });
        } finally {
          setSaving(false);
        }
      };
      const startEdit = item => {
        setMenuFor(null);
        setEditing({
          kind: item.kind,
          id: item.id,
          text: item.text || item.content || '',
        });
      };
      const saveEdit = async () => {
        if (!editing || !bridge.memory.updateMemoryItem) return;
        setSaving(true);
        try {
          await bridge.memory.updateMemoryItem(editing.kind, editing.id, {
            text: editing.text,
          });
          setEditing(null);
        } finally {
          setSaving(false);
        }
      };
      const deleteItem = async item => {
        setMenuFor(null);
        if (!item || !bridge.memory.deleteMemoryItem) return;
        if (!window.confirm('删除后这条记忆不会再被使用，确定删除吗？')) return;
        await bridge.memory.deleteMemoryItem(item.kind, item.id);
      };
      const archiveItem = async item => {
        setMenuFor(null);
        if (!item || !bridge.memory.archiveRecentWorkMemory) return;
        await bridge.memory.archiveRecentWorkMemory(item.id);
      };
      const activeList = tab === 'recent' ? recentItems : longTermItems;
      const filteredList = activeList.filter(item => searchMatch(item.text || item.content));

      const formatMemoryTime = item => {
        const raw = item.updated_at || item.created_at || item.last_seen_at || item.last_used_at;
        if (!raw) return '已记住';
        const date = new Date(raw);
        if (Number.isNaN(date.getTime())) return '已记住';
        const diff = Date.now() - date.getTime();
        const day = 24 * 60 * 60 * 1000;
        if (diff >= 0 && diff < day) return '今天更新';
        if (diff >= day && diff < 7 * day) return `${Math.floor(diff / day)} 天前更新`;
        return `${date.getMonth() + 1}月${date.getDate()}日更新`;
      };
      const confidenceText = item => {
        const n = Number(item.confidence);
        if (!Number.isFinite(n)) return '自动整理';
        if (n >= 0.85) return '置信度高';
        if (n >= 0.65) return '置信度中';
        return '置信度低';
      };

      const MemoryRow = ({ item }) => {
        const Icon = memoryTypeIcon(item.kind);
        const rowKey = `${item.kind}:${item.id}`;
        return (
          <div className={`group relative rounded-2xl border px-4 py-4 ${cardBg} shadow-[0_12px_34px_rgba(0,0,0,0.16)]`}>
            <div className="flex items-start justify-between gap-4">
              <div className="min-w-0 flex-1">
                <div className="flex items-center gap-2 mb-3">
                  <span className={`w-7 h-7 rounded-full flex items-center justify-center ${memoryTypeTone(item.kind)}`}><Icon size={14} /></span>
                  <span className="text-[13px] font-medium">{memoryTypeLabel(item.kind)}</span>
                  <span className={`ml-auto text-[11px] ${faintText}`}>{formatMemoryTime(item)}</span>
                </div>
                <div className="text-[14px] leading-relaxed break-words">{item.text}</div>
                <div className={`mt-3 text-[12px] ${faintText}`}>
                  来源：对话识别 · {confidenceText(item)}
                </div>
              </div>
              <button
                title="更多操作"
                onClick={(e) => {
                  e.stopPropagation();
                  setMenuFor(menuFor === rowKey ? null : rowKey);
                }}
                className={`shrink-0 w-8 h-8 rounded-full flex items-center justify-center transition-colors ${isDark ? 'text-[#AEB4BC] hover:bg-white/[0.08] hover:text-[#F2F3F5]' : 'text-[#5F6368] hover:bg-black/[0.06]'}`}
              >
                <MoreHorizontal size={17} />
              </button>
            </div>
            {menuFor === rowKey && (
              <div onClick={(e) => e.stopPropagation()} className={`absolute right-4 top-12 z-10 min-w-[118px] rounded-xl border ${border} ${isDark ? 'bg-[#24262B] text-[#E8EAED]' : 'bg-white text-[#1F1F1F]'} shadow-2xl overflow-hidden`}>
                <button onClick={() => startEdit(item)} className={`w-full flex items-center gap-2 px-3 py-2 text-left text-[13px] ${isDark ? 'hover:bg-white/[0.07]' : 'hover:bg-black/[0.04]'}`}><Edit2 size={14} />编辑</button>
                {(item.kind === 'current_focus' || item.kind === 'recent_activity') && (
                  <button onClick={() => archiveItem(item)} className={`w-full flex items-center gap-2 px-3 py-2 text-left text-[13px] ${isDark ? 'hover:bg-white/[0.07]' : 'hover:bg-black/[0.04]'}`}><Archive size={14} />归档</button>
                )}
                <button onClick={() => deleteItem(item)} className={`w-full flex items-center gap-2 px-3 py-2 text-left text-[13px] ${dangerBtn}`}><Trash2 size={14} />删除</button>
              </div>
            )}
          </div>
        );
      };

      return (
        <>
          <SCard isDark={isDark} title="记忆">
            <div className="flex items-center justify-between gap-4">
              <div className="min-w-0">
                <div className={`text-[14px] font-medium ${isDark ? 'text-[#E8EAED]' : 'text-[#1F1F1F]'}`}>
                  {memoryEnabled ? '已启用' : '已关闭'}
                </div>
                <div className={`mt-1 text-[13px] leading-relaxed ${subText}`}>
                  {memoryEnabled
                    ? (memory.loading ? '正在读取记忆' : (profileSummary ? `${profileSummary} · ${total} 条有效记忆。` : `PINVOU 会记住你的偏好、工作背景和近期事项，让后续对话更容易接上上下文。已记录 ${total} 条有效记忆。`))
                    : '开启后，PINVOU 可以记住你的称呼、稳定偏好、工作背景和近期事项，减少重复说明。'}
                </div>
                {memory.error && <div className="mt-2 text-[13px] text-[#EA4335]">{memory.error}</div>}
              </div>
              <div className="shrink-0 flex items-center gap-2">
                <button
                  onClick={() => onMemoryEnabledChange && onMemoryEnabledChange(!memoryEnabled)}
                  role="switch"
                  aria-checked={!!memoryEnabled}
                  title={memoryEnabled ? '关闭记忆' : '开启记忆'}
                  className={`w-12 h-7 rounded-full p-1 flex items-center transition-colors ${memoryEnabled ? 'justify-end bg-[#0B57D0]' : `justify-start ${isDark ? 'bg-[#3C4043]' : 'bg-[#DADCE0]'}`}`}
                >
                  <span className="block w-5 h-5 rounded-full bg-white shadow" />
                </button>
                {memoryEnabled && (
                  <button onClick={() => { setOpen(true); reload(); }} className={`text-[13px] font-medium px-4 py-2 rounded-full transition-colors ${primaryBtn}`}>
                    查看和管理
                  </button>
                )}
              </div>
            </div>
          </SCard>

          {open && (
            <div className="fixed inset-0 z-[80] flex items-center justify-center px-4 py-6">
              <div className="absolute inset-0 bg-black/55" onClick={() => setOpen(false)} />
              <div className={`relative w-full max-w-[980px] max-h-[88vh] overflow-hidden rounded-[22px] border ${border} ${panelBg} shadow-2xl`}>
                <div className={`flex items-center justify-between gap-4 px-6 py-4 border-b ${border}`}>
                  <div>
                    <div className="text-[19px] font-semibold">记忆中心</div>
                    <div className={`text-[12px] mt-1 ${subText}`}>记忆由 AI 自动整理，非必要无需手动管理。</div>
                  </div>
                  <div className="flex items-center gap-2">
                    <button onClick={reload} disabled={!!memory.loading} className={`inline-flex items-center gap-1.5 text-[12px] px-3 py-1.5 rounded-full ${ghostBtn}`}><RefreshCw size={13} className={memory.loading ? 'animate-spin' : ''} />{memory.loading ? '同步中' : '同步记忆'}</button>
                    <button onClick={() => setOpen(false)} className={`w-8 h-8 rounded-full flex items-center justify-center ${ghostBtn}`}><X size={15} /></button>
                  </div>
                </div>
                <div className="grid grid-cols-1 md:grid-cols-[190px_1fr] min-h-[420px] max-h-[calc(88vh-73px)]">
                  <div className={`border-b md:border-b-0 md:border-r ${border} p-3 overflow-auto`}>
                    <div className="space-y-1">
                      {tabs.map(({ key, label, count, icon: TabIcon }) => (
                        <button
                          key={key}
                          onClick={() => setTab(key)}
                          className={`w-full flex items-center gap-2 text-left px-3 py-2 rounded-full border text-[13px] transition-colors ${tab === key ? selectedTab : `border-transparent ${isDark ? 'hover:bg-white/[0.06]' : 'hover:bg-black/[0.04]'}`}`}
                        >
                          <TabIcon size={15} className="shrink-0" />
                          <span className="min-w-0 flex-1 truncate">{label}</span>
                          <span className="text-[11px] opacity-75">{count}</span>
                        </button>
                      ))}
                    </div>
                  </div>
                  <div className="p-5 overflow-auto" onClick={() => setMenuFor(null)}>
                    {!memoryEnabled && (
                      <div className={`mb-4 rounded-2xl border px-4 py-3 ${isDark ? 'bg-white/[0.04] border-white/[0.08]' : 'bg-white border-[#DDE3EA]'}`}>
                        <div className={`text-[13px] leading-relaxed ${subText}`}>开启记忆后，PINVOU 会在对话中使用这些信息，并自动整理新的长期记忆与近期记忆。</div>
                      </div>
                    )}
                    <div className="flex flex-col md:flex-row md:items-center justify-between gap-3 mb-5">
                      <div>
                        <div className="text-[16px] font-semibold">{tabMeta.label}</div>
                        <div className={`text-[12px] mt-1 ${faintText}`}>{tab === 'long_term' ? '称呼、长期偏好与工作背景' : '当前关注与近期动态'} · {tabMeta.count} 条</div>
                      </div>
                      <div className={`h-10 min-w-0 md:w-[260px] flex items-center gap-2 rounded-full border px-3 ${inputBg}`}>
                        <Search size={15} className={faintText} />
                        <input value={query} onChange={e => setQuery(e.target.value)} onClick={e => e.stopPropagation()} placeholder="搜索记忆" className="w-full bg-transparent outline-none text-[13px]" />
                      </div>
                    </div>

                    {tab === 'long_term' ? (
                      <div className="space-y-4">
                        <div className="grid grid-cols-1 md:grid-cols-2 gap-3">
                          <SField isDark={isDark} label="助手称呼我" value={draft.call_name} onChange={e => setDraft({ ...draft, call_name: e.target.value })} placeholder="例如：欣哥" />
                          <SField isDark={isDark} label="我称呼助手" value={draft.assistant_alias} onChange={e => setDraft({ ...draft, assistant_alias: e.target.value })} placeholder="例如：小猪" />
                        </div>
                        <div className="flex justify-end">
                          <button onClick={saveProfile} disabled={saving} className={`text-[12px] font-medium px-4 py-2 rounded-full ${primaryBtn} ${saving ? 'opacity-50' : ''}`}>{saving ? '保存中' : '保存'}</button>
                        </div>
                        {filteredList.length === 0 ? (
                          <div className={`text-[13px] ${subText}`}>{query.trim() ? '没有匹配的长期记忆。' : '暂无长期偏好或工作背景。'}</div>
                        ) : (
                          <div className="space-y-3">{filteredList.map(item => <MemoryRow key={`${item.kind}:${item.id}`} item={item} />)}</div>
                        )}
                        <div className={`rounded-2xl border px-4 py-3 ${isDark ? 'bg-white/[0.03] border-white/[0.06]' : 'bg-white/70 border-[#DDE3EA]'}`}>
                          <div className={`text-[12px] leading-relaxed ${faintText}`}>长期记忆会长期保留，用来理解你的稳定偏好、工作背景和称呼习惯。它不会自动过期，你可以随时编辑或删除。</div>
                        </div>
                      </div>
                    ) : filteredList.length === 0 ? (
                      <div className={`text-[13px] ${subText}`}>{query.trim() ? '没有匹配的近期记忆。' : '暂无当前关注或近期动态。'}</div>
                    ) : (
                      <div className="space-y-3">
                        {filteredList.map(item => <MemoryRow key={`${item.kind}:${item.id}`} item={item} />)}
                        <div className={`rounded-2xl border px-4 py-3 ${isDark ? 'bg-white/[0.03] border-white/[0.06]' : 'bg-white/70 border-[#DDE3EA]'}`}>
                          <div className={`text-[12px] leading-relaxed ${faintText}`}>近期记忆会记录最近正在推进和刚完成的事情，用来帮助 PINVOU 接上上下文。它会随时间自动淡出，你也可以手动归档或删除。</div>
                        </div>
                      </div>
                    )}
                  </div>
                </div>
              </div>
            </div>
          )}

          {editing && (
            <div className="fixed inset-0 z-[90] flex items-center justify-center px-4">
              <div className="absolute inset-0 bg-black/60" onClick={() => setEditing(null)} />
              <div className={`relative w-full max-w-[560px] rounded-[18px] border ${border} ${panelBg} p-5 shadow-2xl`}>
                <div className="flex items-center justify-between gap-3 mb-4">
                  <div>
                    <div className="text-[16px] font-semibold">编辑{memoryTypeLabel(editing.kind)}</div>
                    <div className={`text-[12px] mt-1 ${subText}`}>修改后会立即影响后续记忆注入。</div>
                  </div>
                  <button onClick={() => setEditing(null)} className={`w-8 h-8 rounded-full flex items-center justify-center ${ghostBtn}`}><X size={15} /></button>
                </div>
                <div className="space-y-3">
                  <label className="block">
                    <span className={`block text-[12px] mb-1.5 ${subText}`}>内容</span>
                    <textarea value={editing.text} onChange={e => setEditing({ ...editing, text: e.target.value })} rows={5} className={`w-full rounded-xl border px-3 py-2 text-[14px] outline-none resize-none ${inputBg}`} />
                  </label>
                </div>
                <div className="mt-5 flex justify-end gap-2">
                  <button onClick={() => setEditing(null)} className={`text-[13px] px-4 py-2 rounded-full ${ghostBtn}`}>取消</button>
                  <button onClick={saveEdit} disabled={saving || !editing.text.trim()} className={`text-[13px] font-medium px-4 py-2 rounded-full ${primaryBtn} ${(saving || !editing.text.trim()) ? 'opacity-50' : ''}`}>{saving ? '保存中' : '保存'}</button>
                </div>
              </div>
            </div>
          )}

        </>
      );
    };

    // ── 「添加模型」方案:模型快切 chip + 添加/编辑弹窗 ─────────────────
    // 各预设默认 baseUrl/model 模板(与 bridge/prefs.rs 对齐),添加模型时自动填充。
    const MODEL_PRESET_DEFS = {
      local_vllm:  { baseUrl: 'http://127.0.0.1:8000/v1',                model: 'qwen36_35b_256k' },
      deepseek:    { baseUrl: 'https://api.deepseek.com',                model: 'deepseek-v4-pro' },
      kimi:        { baseUrl: 'https://api.moonshot.cn/v1',              model: 'kimi-k3' },
      openai_compatible: { baseUrl: 'https://api.openai.com/v1',        model: 'gpt-5.6-terra' },
      qwen:        { baseUrl: 'https://dashscope.aliyuncs.com/compatible-mode/v1', model: 'qwen3.7-plus' },
      doubao:      { baseUrl: 'https://ark.cn-beijing.volces.com/api/v3', model: 'doubao-seed-evolving' },
      minimax:     { baseUrl: 'https://api.minimax.chat/v1',            model: 'MiniMax-M3' },
      glm:         { baseUrl: 'https://open.bigmodel.cn/api/paas/v4',   model: 'glm-5.2' },
      mimo:        { baseUrl: 'https://api.xiaomimimo.com/v1',          model: 'mimo-v2.5-pro' },
    };
    function presetOptionsI18n(t) {
      return [
        { key: 'local_vllm', label: t.modelPresetLocalVllm },
        { key: 'deepseek', label: t.modelPresetDeepseek },
        { key: 'kimi', label: t.modelPresetKimi },
        { key: 'openai_compatible', label: t.modelPresetOpenaiCompatible },
        { key: 'qwen', label: t.modelPresetQwen },
        { key: 'doubao', label: t.modelPresetDoubao },
        { key: 'minimax', label: t.modelPresetMinimax },
        { key: 'glm', label: t.modelPresetGlm },
        { key: 'mimo', label: t.modelPresetMimo },
      ];
    }
    function presetProviderLabel(preset, t) {
      const m = {};
      presetOptionsI18n(t).forEach(o => { m[o.key] = o.label; });
      return m[preset] || preset;
    }

    const BRAND_ICON_BY_PRESET = {
      deepseek: deepseekIcon,
      kimi: kimiIcon,
      glm: glmIcon,
      qwen: qwenIcon,
      doubao: doubaoIcon,
      minimax: minimaxIcon,
      mimo: mimoIcon,
      openai_compatible: openaiIcon,
      local_vllm: qwenIcon,
    };

    const MODEL_CATALOG = {
      local: [
        {
          key: 'local',
          title: '本地 vLLM',
          preset: 'local_vllm',
          items: [
            { model: 'qwen36_35b_256k', title: 'qwen36_35b_256k', desc: '本地服务默认模型' },
            { model: '', title: '自定义本地模型', desc: '填写本地服务暴露的模型 ID', custom: true },
          ],
        },
      ],
      cloud: [
        {
          key: 'deepseek',
          title: 'DeepSeek',
          preset: 'deepseek',
          items: [
            { model: 'deepseek-v4-pro', title: 'deepseek-v4-pro', desc: '高能力模型' },
            { model: 'deepseek-v4-flash', title: 'deepseek-v4-flash', desc: '快速响应' },
            { model: '', title: '自定义 DeepSeek 模型', desc: '手动填写模型 ID', custom: true },
          ],
        },
        {
          key: 'kimi',
          title: 'Kimi',
          preset: 'kimi',
          items: [
            { model: 'kimi-k3', title: 'kimi-k3', desc: '最新通用模型' },
            { model: 'kimi-k2.7-code', title: 'kimi-k2.7-code', desc: '代码场景' },
            { model: 'kimi-k2.7-code-highspeed', title: 'kimi-k2.7-code-highspeed', desc: '高速代码场景' },
            { model: 'kimi-k2.6', title: 'kimi-k2.6', desc: '稳定可用' },
            { model: '', title: '自定义 Kimi 模型', desc: '手动填写模型 ID', custom: true },
          ],
        },
        {
          key: 'glm',
          title: 'GLM',
          preset: 'glm',
          items: [
            { model: 'glm-5.2', title: 'glm-5.2', desc: '最新推荐' },
            { model: 'glm-5-turbo', title: 'glm-5-turbo', desc: '高性价比' },
            { model: 'glm-4.7', title: 'glm-4.7', desc: '通用能力' },
            { model: 'glm-5.1', title: 'glm-5.1', desc: '兼容保留' },
            { model: '', title: '自定义 GLM 模型', desc: '手动填写模型 ID', custom: true },
          ],
        },
        {
          key: 'minimax',
          title: 'MiniMax',
          preset: 'minimax',
          items: [
            { model: 'MiniMax-M3', title: 'MiniMax-M3', desc: '最新推荐' },
            { model: 'MiniMax-M2.7', title: 'MiniMax-M2.7', desc: '通用能力' },
            { model: 'MiniMax-M2.7-highspeed', title: 'MiniMax-M2.7-highspeed', desc: '高速响应' },
            { model: 'MiniMax-M2.5', title: 'MiniMax-M2.5', desc: '兼容保留' },
            { model: 'MiniMax-M2.5-highspeed', title: 'MiniMax-M2.5-highspeed', desc: '兼容高速' },
            { model: '', title: '自定义 MiniMax 模型', desc: '手动填写模型 ID', custom: true },
          ],
        },
        {
          key: 'mimo',
          title: 'MiMo',
          preset: 'mimo',
          items: [
            { model: 'mimo-v2.5-pro', title: 'mimo-v2.5-pro', desc: '最新推荐' },
            { model: 'mimo-v2.5', title: 'mimo-v2.5', desc: '通用能力' },
            { model: '', title: '自定义 MiMo 模型', desc: '手动填写模型 ID', custom: true },
          ],
        },
        {
          key: 'qwen',
          title: '通义千问',
          preset: 'qwen',
          items: [
            { model: 'qwen3.7-plus', title: 'qwen3.7-plus', desc: '最新推荐' },
            { model: 'qwen3.6-flash', title: 'qwen3.6-flash', desc: '快速响应' },
            { model: '', title: '自定义通义模型', desc: '手动填写模型 ID', custom: true },
          ],
        },
        {
          key: 'doubao',
          title: '豆包',
          preset: 'doubao',
          items: [
            { model: 'doubao-seed-evolving', title: 'doubao-seed-evolving', desc: '最新推荐' },
            { model: 'doubao-seed-2.1-pro', title: 'doubao-seed-2.1-pro', desc: '高能力模型' },
            { model: 'doubao-seed-2.1-turbo', title: 'doubao-seed-2.1-turbo', desc: '快速响应' },
            { model: 'doubao-seed-2.0-pro', title: 'doubao-seed-2.0-pro', desc: '稳定通用' },
            { model: 'doubao-seed-2.0-lite', title: 'doubao-seed-2.0-lite', desc: '轻量模型' },
            { model: '', title: '自定义豆包模型', desc: '手动填写模型 ID', custom: true },
          ],
        },
        {
          key: 'openai_compatible',
          title: 'OpenAI Compatible',
          preset: 'openai_compatible',
          items: [
            { model: 'gpt-5.6-terra', title: 'gpt-5.6-terra', desc: '兼容端点示例' },
            { model: 'gpt-5.6-luna', title: 'gpt-5.6-luna', desc: '兼容端点示例' },
            { model: 'gpt-5.6-sol', title: 'gpt-5.6-sol', desc: '兼容端点示例' },
            { model: '', title: '自定义兼容模型', desc: '手动填写模型 ID 和服务地址', custom: true },
          ],
        },
      ],
    };

    const ProviderIcon = ({ preset, isDark, compact = false }) => {
      const src = BRAND_ICON_BY_PRESET[preset];
      if (!src) return null;
      const darkBacked = preset === 'kimi';
      return (
        <span className={`${compact ? 'h-8 w-8 rounded-[9px]' : 'h-9 w-9 rounded-[10px]'} shrink-0 flex items-center justify-center overflow-hidden ${darkBacked ? 'bg-[#111827]' : (isDark ? 'bg-white' : 'bg-white border border-black/[0.08]')}`}>
          <img src={src} alt="" className={`${compact ? 'h-6 w-6' : 'h-7 w-7'} object-contain`} />
        </span>
      );
    };

    // 聊天输入框上方:当前会话模型 chip + 下拉热切。
    const ModelChip = ({ isDark, t, bs, onGotoSettings }) => {
      const [open, setOpen] = useState(false);
      const savedModels = visibleUserModels((bs && bs.savedModels) || []);
      const activeSessionId = bs ? bs.activeSessionId : null;
      const activeModelId = bs && bs.activeModelId;
      const currentSessionModelId = bs && bs.currentSessionModelId;
      const busy = bs ? bs.busy : false;
      const effectiveId = currentSessionModelId || activeModelId;
      const current = savedModels.find(m => m.id === effectiveId);
      if (!savedModels.length) return null;
      function pick(id) {
        setOpen(false);
        if (id === effectiveId) return;
        if (bridge.available) bridge.models.switchModel(activeSessionId, id);
      }
      return (
        <div className="relative px-2 mb-2">
          <button onClick={() => { if (!busy) setOpen(o => !o); }} disabled={busy}
            title={busy ? t.modelSwitchBusy : t.switchModelTitle}
            className={`inline-flex items-center gap-1.5 pl-3 pr-2 py-1 rounded-full text-[12px] font-medium transition-colors disabled:opacity-50 ${isDark ? 'bg-[#2A2B2D] text-[#E3E3E3] hover:bg-[#333537]' : 'bg-[#EAEDF1] text-[#1F1F1F] hover:bg-[#E0E3E7]'}`}>
            <span className="w-1.5 h-1.5 rounded-full bg-[#34A853]"></span>
            <span className="max-w-[220px] truncate">{current ? current.name : t.modelNonePick}</span>
            <ChevronDown size={13} />
          </button>
          {open && (
            <div>
              <div className="fixed inset-0 z-40" onClick={() => setOpen(false)}></div>
              <div className={`absolute bottom-full left-2 mb-1 z-50 min-w-[240px] max-h-[340px] overflow-y-auto rounded-xl border shadow-lg py-1 ${isDark ? 'bg-[#1E1F20] border-[#333537]' : 'bg-white border-[#E0E3E7]'}`}>
                {savedModels.map(m => (
                  <button key={m.id} onClick={() => pick(m.id)}
                    className={`w-full flex items-center gap-2 px-3 py-2 text-left transition-colors ${isDark ? 'hover:bg-[#2A2B2D]' : 'hover:bg-[#F0F4F9]'}`}>
                    <span className={`shrink-0 w-1.5 h-1.5 rounded-full ${m.id === effectiveId ? 'bg-[#34A853]' : 'bg-transparent'}`}></span>
                    <span className="flex-1 min-w-0">
                      <span className={`block text-[13px] truncate ${isDark ? 'text-[#E3E3E3]' : 'text-[#1F1F1F]'}`}>{m.name}</span>
                      <span className={`block text-[11px] truncate ${isDark ? 'text-[#9AA0A6]' : 'text-[#5F6368]'}`}>{m.model}</span>
                    </span>
                    {m.id === activeModelId && <span className={`shrink-0 text-[10px] px-1.5 py-0.5 rounded ${isDark ? 'bg-[#37393B] text-[#9AA0A6]' : 'bg-[#E8EAED] text-[#5F6368]'}`}>{t.modelActiveTag}</span>}
                  </button>
                ))}
                <div className={`border-t mt-1 pt-1 ${isDark ? 'border-[#333537]' : 'border-[#E8EAED]'}`}>
                  <button onClick={() => { setOpen(false); if (onGotoSettings) onGotoSettings(); }}
                    className={`w-full px-3 py-1.5 text-left text-[12px] ${isDark ? 'text-[#9AA0A6] hover:bg-[#2A2B2D]' : 'text-[#5F6368] hover:bg-[#F0F4F9]'}`}>
                    {t.manageModels}
                  </button>
                </div>
              </div>
            </div>
          )}
        </div>
      );
    };

    // 输入框底栏:模型选择器(iOS 化,复用 ModelChip 的 switchModel 逻辑;darkMode:'class' 故用 dark: 变体)。
    const ComposerModelSelector = ({ t, bs, onGotoSettings, compact }) => {
      const [open, setOpen] = useState(false);
      const savedModels = visibleUserModels((bs && bs.savedModels) || []);
      const activeSessionId = bs ? bs.activeSessionId : null;
      const activeModelId = bs && bs.activeModelId;
      const currentSessionModelId = bs && bs.currentSessionModelId;
      const busy = bs ? bs.busy : false;
      const effectiveId = currentSessionModelId || activeModelId;
      const current = savedModels.find(m => m.id === effectiveId);
      if (!savedModels.length) return null;
      function pick(id) { setOpen(false); if (id !== effectiveId && bridge.available) bridge.models.switchModel(activeSessionId, id); }
      return (
        <div className="relative min-w-0">
          <button onClick={() => { if (!busy) setOpen(o => !o); }} disabled={busy}
            title={(current ? current.name : t.modelNonePick) + (busy ? ' · ' + t.modelSwitchBusy : '')}
            className={`relative shrink-0 flex items-center justify-center text-gray-700 dark:text-gray-200 transition-colors border disabled:opacity-50 ${compact ? 'w-9 h-9 rounded-full bg-transparent hover:bg-black/5 dark:hover:bg-white/10 border-transparent' : 'gap-1.5 px-2.5 py-1.5 rounded-xl text-[13px] font-semibold min-w-0 max-w-full bg-gray-100 dark:bg-white/5 hover:bg-gray-200 dark:hover:bg-white/10 border-black/[0.04] dark:border-white/5'}`}>
            {compact ? (
              <>
                <Cpu size={18} className="opacity-80" />
                <span className="absolute top-1 right-1 w-1.5 h-1.5 rounded-full bg-[#34C759] ring-2 ring-white dark:ring-[#161618]"></span>
              </>
            ) : (
              <>
                <span className="w-1.5 h-1.5 shrink-0 rounded-full bg-[#34C759]"></span>
                <span className="max-w-[128px] truncate">{t.composerModelLabel(current ? current.name : t.modelNonePick)}</span>
                <ChevronDown size={14} className="opacity-50 shrink-0" />
              </>
            )}
          </button>
          {open && (
            <>
              <div className="fixed inset-0 z-40" onClick={() => setOpen(false)}></div>
              <div className="absolute bottom-full left-0 mb-2 z-50 w-64 max-h-[340px] overflow-y-auto bg-white/95 dark:bg-[#1E1E20]/95 backdrop-blur-xl border border-black/5 dark:border-white/10 rounded-2xl shadow-xl p-1.5">
                {savedModels.map(m => (
                  <button key={m.id} onClick={() => pick(m.id)}
                    className="w-full flex items-center justify-between px-3 py-2.5 text-[13px] text-gray-700 dark:text-gray-200 hover:bg-[#007AFF] hover:text-white rounded-xl transition-colors group">
                    <span className="flex items-center gap-2.5 min-w-0">
                      <Cpu size={15} className="shrink-0 text-gray-400 group-hover:text-white/90" />
                      <span className="truncate">{m.name}</span>
                    </span>
                    {m.id === effectiveId && <Check size={15} className="shrink-0 text-[#007AFF] group-hover:text-white" />}
                  </button>
                ))}
                <div className="h-px bg-black/5 dark:bg-white/10 my-1.5 mx-2" />
                <button onClick={() => { setOpen(false); if (onGotoSettings) onGotoSettings(); }}
                  className="w-full flex items-center gap-2.5 px-3 py-2.5 text-[13px] text-gray-700 dark:text-gray-200 hover:bg-[#007AFF] hover:text-white rounded-xl transition-colors group">
                  <Plus size={15} className="text-gray-400 group-hover:text-white/90" />
                  {t.manageModels}
                </button>
              </div>
            </>
          )}
        </div>
      );
    };

    const RemoteControlModal = ({ theme, bs, onClose }) => {
      const isDark = theme === 'dark';
      const [refreshConfirmOpen, setRefreshConfirmOpen] = useState(false);
      const [actionBusy, setActionBusy] = useState(false);
      const remoteControl = (bs && bs.remoteControl) || {};
      const remotePairing = remoteControl.pairing || remoteControl;
      const remoteActive = !!remoteControl.active;
      const statusKey = remoteControl.starting ? 'starting' : (remoteControl.status || 'idle');
      const statusMeta = {
        idle: { label: '未开启', detail: '点击手机控制图标后才会开启远程控制。', color: '#8A9097' },
        starting: { label: '正在开启', detail: '正在创建远程控制连接和二维码。', color: '#F9AB00' },
        connecting_relay: { label: '正在连接', detail: '正在连接云端中继，请稍候。', color: '#F9AB00' },
        waiting_mobile: { label: '等待手机连接', detail: '用手机扫码，或在手机上打开链接。', color: '#F9AB00' },
        mobile_connected: { label: '手机已连接', detail: '当前手机可以查看和控制远程会话。', color: '#34A853' },
        mobile_disconnected: { label: '手机已断开', detail: '原二维码仍然有效，手机可随时重新连接。', color: '#F9AB00' },
        expired: { label: '连接已失效', detail: '请刷新二维码后重新连接。', color: '#EA4335' },
        stopped: { label: '已停止', detail: '再次点击手机控制图标可重新开启。', color: '#8A9097' },
        error: { label: '连接异常', detail: remoteControl.last_error || '远程控制暂时不可用，请重试。', color: '#EA4335' },
      }[statusKey] || { label: String(statusKey), detail: '远程控制状态已更新。', color: '#8A9097' };

      useEffect(() => {
        if (!remoteActive && bridge.available) {
          bridge.remoteControl.startRemoteControl(null).catch(() => {});
        }
      }, []);

      async function handleRefreshRemoteControl() {
        if (!bridge.available) return;
        setActionBusy(true);
        try {
          await bridge.remoteControl.refreshRemoteControlQr(null);
          setRefreshConfirmOpen(false);
        } catch (_) {
        } finally {
          setActionBusy(false);
        }
      }

      async function handleStopRemoteControl() {
        if (!bridge.available) return;
        setActionBusy(true);
        try {
          await bridge.remoteControl.stopRemoteControl();
          onClose();
        } finally {
          setActionBusy(false);
        }
      }

      async function handleRetryRemoteControl() {
        if (!bridge.available) return;
        setActionBusy(true);
        try { await bridge.remoteControl.startRemoteControl(null); }
        catch (_) {}
        finally { setActionBusy(false); }
      }

      return (
        <div className="fixed inset-0 z-[90] flex items-center justify-center p-4 bg-black/45" onClick={onClose}>
          <div onClick={e => e.stopPropagation()} className={`relative w-full max-w-[420px] rounded-[22px] shadow-2xl p-5 ${isDark ? 'bg-[#1E1F20] text-[#E3E3E3]' : 'bg-white text-[#1F1F1F]'}`}>
            <div className="flex items-start justify-between gap-3 mb-4">
              <div>
                <div className="text-[17px] font-semibold">移动端远程控制</div>
                <div className={`text-[12px] mt-1 ${isDark ? 'text-[#AEB4BC]' : 'text-[#5F6368]'}`}>扫码或在手机上打开链接，即可远程控制当前工作区。</div>
              </div>
              <button onClick={onClose} className={`w-8 h-8 rounded-full flex items-center justify-center ${isDark ? 'hover:bg-white/10' : 'hover:bg-black/5'}`}><X size={17} /></button>
            </div>
            <div className={`rounded-[16px] border p-3 mb-4 ${isDark ? 'border-white/10 bg-white/[0.035]' : 'border-black/10 bg-[#F8F9FA]'}`}>
              <div className="flex items-start justify-between gap-3">
                <div className="min-w-0 flex items-start gap-3">
                  <div className={`mt-0.5 w-9 h-9 rounded-xl flex items-center justify-center shrink-0 ${isDark ? 'bg-white/5 text-[#C4C7C5]' : 'bg-white text-[#5F6368]'}`}><Smartphone size={17} /></div>
                  <div className="min-w-0">
                    <div className="text-[14px] font-medium">手机扫码连接</div>
                    <div className={`text-[12px] mt-1 leading-relaxed ${isDark ? 'text-[#9AA0A6]' : 'text-[#6F7378]'}`}>{statusMeta.detail}</div>
                  </div>
                </div>
                <div className="flex items-center gap-2 shrink-0">
                  <span className={`inline-flex items-center gap-1.5 px-2 py-1 rounded-full text-[11px] ${isDark ? 'bg-white/5 text-[#C4C7C5]' : 'bg-white text-[#5F6368]'}`}>
                    <span className="w-1.5 h-1.5 rounded-full" style={{ background: statusMeta.color }}></span>{statusMeta.label}
                  </span>
                  {remoteActive && <button disabled={actionBusy} onClick={handleStopRemoteControl}
                    className={`px-3 py-1.5 rounded-lg text-[12px] disabled:opacity-50 ${isDark ? 'border border-white/10 hover:bg-white/10' : 'border border-black/10 hover:bg-black/5'}`}>停止</button>}
                </div>
              </div>
            </div>
            {remotePairing && remotePairing.qr_data_url ? (
              <div className="flex flex-col items-center">
                <div className="p-3 rounded-[16px] bg-white">
                  <img src={remotePairing.qr_data_url} alt="Remote control QR" className="w-[220px] h-[220px]" />
                </div>
                <div className={`mt-3 w-full text-[12px] leading-relaxed break-all px-3 py-2 rounded-xl ${isDark ? 'bg-white/5 text-[#C4C7C5]' : 'bg-[#F1F3F4] text-[#3C4043]'}`}>{remotePairing.url || remoteControl.url}</div>
              </div>
            ) : (
              <div className={`text-[13px] px-3 py-4 rounded-xl ${isDark ? 'bg-white/5 text-[#C4C7C5]' : 'bg-[#F1F3F4] text-[#3C4043]'}`}>
                {remoteControl.starting ? '正在生成二维码...' : (remoteControl.last_error || '当前会话还未开启远程控制。')}
              </div>
            )}
            {remoteControl.last_error && <div className="mt-3 text-[12px] text-[#EA4335] break-all">{remoteControl.last_error}</div>}
            <div className="mt-4 flex items-center justify-end gap-2">
              <button onClick={() => navigator.clipboard && navigator.clipboard.writeText(remotePairing.url || remoteControl.url || '')}
                disabled={!(remotePairing.url || remoteControl.url)}
                className={`px-3.5 py-2 rounded-full text-[13px] ${isDark ? 'bg-white/10 hover:bg-white/15 disabled:opacity-40' : 'bg-black/5 hover:bg-black/10 disabled:opacity-40'}`}>复制链接</button>
              {remoteActive ? <button disabled={actionBusy} onClick={() => setRefreshConfirmOpen(true)}
                className={`px-3.5 py-2 rounded-full text-[13px] disabled:opacity-50 ${isDark ? 'bg-white/10 hover:bg-white/15' : 'bg-black/5 hover:bg-black/10'}`}>刷新二维码</button>
                : <button disabled={actionBusy} onClick={handleRetryRemoteControl}
                  className="px-3.5 py-2 rounded-full text-[13px] bg-[#0B57D0] text-white hover:bg-[#0842A0] disabled:opacity-50">重新开启</button>}
            </div>
            {refreshConfirmOpen && (
              <div className="absolute inset-0 z-10 flex items-center justify-center p-4 rounded-[22px] bg-black/55" onClick={() => !actionBusy && setRefreshConfirmOpen(false)}>
                <div onClick={e => e.stopPropagation()} className={`w-full max-w-[330px] rounded-[18px] p-5 shadow-2xl ${isDark ? 'bg-[#2A2B2D]' : 'bg-white'}`}>
                  <div className="text-[16px] font-semibold">刷新二维码？</div>
                  <div className={`text-[13px] leading-relaxed mt-2 ${isDark ? 'text-[#B7BBC0]' : 'text-[#5F6368]'}`}>刷新后，之前复制或扫码得到的远程控制链接将失效，已连接的手机需要重新扫码。</div>
                  <div className="mt-5 flex justify-end gap-2">
                    <button disabled={actionBusy} onClick={() => setRefreshConfirmOpen(false)} className={`px-4 py-2 rounded-lg text-[13px] ${isDark ? 'bg-white/5 hover:bg-white/10' : 'bg-black/5 hover:bg-black/10'}`}>取消</button>
                    <button disabled={actionBusy} onClick={handleRefreshRemoteControl} className="px-4 py-2 rounded-lg text-[13px] font-medium bg-white text-[#202124] hover:bg-[#F1F3F4] disabled:opacity-60">{actionBusy ? '正在刷新…' : '刷新二维码'}</button>
                  </div>
                </div>
              </div>
            )}
          </div>
        </div>
      );
    };

    // 输入框底栏:工具菜单(只展示已装工具 + 跳工具商店;无会话级开关——后端无此概念)。
    // 产物 HTML 预览：测内容自然尺寸，比面板宽就整体等比缩小铺满（只缩不放）。
    // 治"固定尺寸 banner 在窄预览面板里溢出、出滚动条、只露一角"。响应式整页缩放比≈1、不受影响。
    const ScaledHtmlPreview = ({ html }) => {
      const wrapRef = useRef(null);
      const frameRef = useRef(null);
      const [box, setBox] = useState(null); // { w, h, scale }
      const [ready, setReady] = useState(false);
      const measure = () => {
        try {
          const fr = frameRef.current, wrap = wrapRef.current;
          if (!fr || !wrap || !fr.contentWindow) return;
          const doc = fr.contentWindow.document;
          const de = doc.documentElement, bd = doc.body;
          const w = Math.max(de ? de.scrollWidth : 0, bd ? bd.scrollWidth : 0);
          const h = Math.max(de ? de.scrollHeight : 0, bd ? bd.scrollHeight : 0);
          const panelW = wrap.clientWidth;
          const scale = (w > panelW && w > 0) ? panelW / w : 1;
          setBox({ w, h, scale });
        } catch (e) { /* 未就绪/跨域，忽略 */ }
      };
      useEffect(() => { setReady(false); setBox(null); }, [html]);
      useEffect(() => {
        if (!wrapRef.current || typeof ResizeObserver === 'undefined') return;
        const ro = new ResizeObserver(() => measure());
        ro.observe(wrapRef.current);
        return () => ro.disconnect();
      }, []);
      const scaled = box && box.scale < 1;
      return (
        <div ref={wrapRef} className="relative w-full bg-[#15171a]" style={scaled ? { height: Math.ceil(box.h * box.scale) } : { minHeight: 480, height: '100%' }}>
          {!ready && <div className="h-[480px] bg-[#15171a]"></div>}
          <iframe ref={frameRef} sandbox="allow-same-origin allow-scripts" onLoad={() => { measure(); setTimeout(() => setReady(true), 80); }}
            className={`border-0 block bg-[#15171a] transition-opacity duration-300 ${ready ? 'opacity-100' : 'opacity-0 absolute pointer-events-none'}`}
            style={scaled
              ? { width: box.w + 'px', height: box.h + 'px', transform: 'scale(' + box.scale + ')', transformOrigin: 'top left', colorScheme: 'dark' }
              : { width: '100%', height: '100%', minHeight: '480px', colorScheme: 'dark' }}
            srcDoc={"<script>"
              + "document.addEventListener('contextmenu',function(e){e.preventDefault();});"
              // 预览内拦截 <a> 导航：srcDoc 的 base 是父文档(app)，放任会把 iframe 跳成 app 首页。
              // 阻止默认导航；页内 #锚点 改为在预览内滚动；外链/# 不跳走。<button> 的 onclick 不受影响。
              + "document.addEventListener('click',function(e){var a=e.target&&e.target.closest?e.target.closest('a'):null;if(!a)return;e.preventDefault();var h=a.getAttribute('href')||'';if(h.charAt(0)==='#'&&h.length>1){var el=document.getElementById(h.slice(1));if(el)el.scrollIntoView({behavior:'smooth'});}},true);"
              + "document.addEventListener('submit',function(e){e.preventDefault();},true);"
              + "<\/script><style>html,body{background:#15171a;margin:0;}</style>" + (html || '')} />
        </div>
      );
    };

    // 输入框「技能」入口：⚡ 药丸 + popover。视觉设计=内置自动技能（只读，模型 load_skill 时显"使用中"
    // 并高亮药丸）。activeSkill 由 bridge 检测 load_skill 设，纯只读指示。
    const ComposerModeMenu = ({ t, bs, compact }) => {
      const [open, setOpen] = useState(false);
      const SKILLS = [
        { id: 'visual-design', name: '视觉设计', desc: '设计系统直出网页/banner/海报/简历…', kind: 'auto' },
      ];
      const activeId = bs && bs.activeSkill;
      const cur = SKILLS.find(s => s.id === activeId && s.kind === 'auto');
      return (
        <div className="relative">
          <button onClick={() => setOpen(o => !o)} title={cur ? cur.name : t.composerMode}
            className={`flex items-center shrink-0 font-semibold transition-colors border ${compact ? 'justify-center w-9 h-9 rounded-full' : 'gap-1.5 px-2.5 py-1.5 rounded-xl text-[13px] whitespace-nowrap'} ${cur
              ? 'bg-[#007AFF]/[0.1] dark:bg-[#0A84FF]/20 text-[#007AFF] dark:text-[#5AC8FA] border-[#007AFF]/20 dark:border-[#0A84FF]/30'
              : 'bg-gray-100 dark:bg-white/5 hover:bg-gray-200 dark:hover:bg-white/10 text-gray-700 dark:text-gray-200 border-black/[0.04] dark:border-white/5'}`}>
            <Zap size={14} className={cur ? '' : 'opacity-70'} />
            {!compact && (cur ? cur.name : t.composerMode)}
            {!compact && <ChevronDown size={14} className="opacity-50 shrink-0" />}
          </button>
          {open && (
            <>
              <div className="fixed inset-0 z-40" onClick={() => setOpen(false)}></div>
              <div className="absolute bottom-full left-0 mb-2 z-50 w-64 bg-white/95 dark:bg-[#1E1E20]/95 backdrop-blur-xl border border-black/5 dark:border-white/10 rounded-2xl shadow-xl p-1.5">
                <div className="px-3 py-2 text-[11px] font-bold text-gray-400 dark:text-gray-500 uppercase tracking-wider">{t.composerModeTitle}</div>
                {SKILLS.map(s => {
                  const soon = s.kind === 'soon';
                  const inUse = s.kind === 'auto' && activeId === s.id;
                  return (
                    <div key={s.id} className={`flex items-start justify-between gap-2 px-3 py-2.5 rounded-xl ${soon ? 'opacity-50' : ''}`}>
                      <span className="min-w-0">
                        <span className="block text-[13px] font-medium text-gray-800 dark:text-gray-100 truncate">{s.name}</span>
                        <span className="block text-[11px] text-gray-400 dark:text-gray-500 truncate">{s.desc}</span>
                      </span>
                      {soon
                        ? <span className="shrink-0 text-[10px] font-semibold text-gray-400 dark:text-gray-500 bg-black/[0.04] dark:bg-white/10 px-2 py-0.5 rounded-full leading-none mt-0.5">{t.composerSkillSoon}</span>
                        : inUse
                          ? <span className="shrink-0 inline-flex items-center gap-1 text-[10px] font-semibold text-[#34C759] bg-[#34C759]/10 px-2 py-0.5 rounded-full leading-none mt-0.5"><span className="w-1.5 h-1.5 rounded-full bg-[#34C759]" />{t.composerSkillInUse}</span>
                          : <span className="shrink-0 text-[10px] font-semibold text-[#007AFF] dark:text-[#5AC8FA] bg-[#007AFF]/10 dark:bg-[#0A84FF]/15 px-2 py-0.5 rounded-full leading-none mt-0.5">{t.composerSkillAuto}</span>}
                    </div>
                  );
                })}
              </div>
            </>
          )}
        </div>
      );
    };

    const ComposerToolMenu = ({ t, onGotoTools, compact, activeSkill }) => {
      const [open, setOpen] = useState(false);
      const [marketplaceTools, setMarketplaceTools] = useState([]);
      const [marketplaceSkills, setMarketplaceSkills] = useState([]);
      const [disabled, setDisabled] = useState(() => new Set()); // 被关掉的连接器 id(全局持久)
      const [feishuOn, setFeishuOn] = useState(false); // 飞书是否已连接(CLI 路线)
      const [feishuEnabled, setFeishuEnabled] = useState(true); // 飞书技能是否启用(未手动停用)
      const [wecomOn, setWecomOn] = useState(false); // 企微是否已连接(CLI 路线)
      const [wecomEnabled, setWecomEnabled] = useState(true); // 企微技能是否启用(未手动停用)
      const [dingtalkOn, setDingtalkOn] = useState(false); // 钉钉是否已连接(CLI 路线)
      const [dingtalkEnabled, setDingtalkEnabled] = useState(true); // 钉钉技能是否启用(未手动停用)
      const [eipOn, setEipOn] = useState(false); // EIP 是否已连接(SSO 路线)
      const [zhidaoOn, setZhidaoOn] = useState(false); // 知道是否已连接(SSO 路线)
      // 启动时加载已装工具 + 全局持久的禁用列表(持久语义:新窗口/新对话都继承)
      async function refreshToolsMenu(isAlive) {
        try {
          const list = await invokeTauri('list_marketplace_tools');
          if (isAlive()) setMarketplaceTools(Array.isArray(list) ? list : []);
        } catch (e) { /* ignore */ }
        try {
          const skills = await invokeTauri('list_marketplace_skills');
          if (isAlive()) setMarketplaceSkills(Array.isArray(skills) ? skills : []);
        } catch (e) { /* ignore */ }
        try {
          const dis = await invokeTauri('get_disabled_connectors');
          if (isAlive()) setDisabled(new Set(dis || []));
        } catch (e) { /* ignore */ }
        try {
          const fs = await invokeTauri('feishu_skills_state');
          if (isAlive()) { setFeishuOn(!!(fs && fs.connected)); setFeishuEnabled(!fs || fs.enabled !== false); }
        } catch (e) { /* ignore */ }
        try {
          const ws = await invokeTauri('wecom_skills_state');
          if (isAlive()) { setWecomOn(!!(ws && ws.connected)); setWecomEnabled(!ws || ws.enabled !== false); }
        } catch (e) { /* ignore */ }
        try {
          const ds = await invokeTauri('dingtalk_skills_state');
          if (isAlive()) { setDingtalkOn(!!(ds && ds.connected)); setDingtalkEnabled(!ds || ds.enabled !== false); }
        } catch (e) { /* ignore */ }
        try {
          const es = await invokeTauri('eip_status');
          if (isAlive()) setEipOn(!!(es && es.connected));
        } catch (e) { /* ignore */ }
        try {
          const zs = await invokeTauri('zhidao_status');
          if (isAlive()) setZhidaoOn(!!(zs && zs.connected));
        } catch (e) { /* ignore */ }
      }
      useEffect(() => {
        let alive = true;
        const isAlive = () => alive;
        const onChanged = () => refreshToolsMenu(isAlive);
        refreshToolsMenu(isAlive);
        window.addEventListener('pinvou:tools-changed', onChanged);
        return () => { alive = false; window.removeEventListener('pinvou:tools-changed', onChanged); };
      }, []);
      function toggleTool(id) {
        const next = new Set(disabled);
        next.has(id) ? next.delete(id) : next.add(id);
        setDisabled(next);
        // 全局持久:落盘 + 广播给所有在跑引擎,关一次所有新对话/新窗口都继承。
        if (bridge.available) {
          invokeTauri('set_disabled_connectors',
            { connectorIds: Array.from(next) }).catch(() => {});
        }
      }
      const menuState = buildComposerToolMenuState({
        marketplaceTools,
        marketplaceSkills,
        disabledIds: Array.from(disabled),
        activeSkill,
        serviceStates: [
          { id: 'feishu', title: '飞书（Lark）', connected: feishuOn, enabled: feishuEnabled },
          { id: 'wecom', title: '企业微信', connected: wecomOn, enabled: wecomEnabled },
          { id: 'dingtalk', title: '钉钉', connected: dingtalkOn, enabled: dingtalkEnabled },
          { id: 'eip', title: 'H3C 员工门户（EIP）', connected: eipOn },
          { id: 'zhidao', title: 'H3C 知道', connected: zhidaoOn },
        ],
      });
      const { connectedServices, toolRows, skillRows, enabledCount } = menuState;
      const statusBadge = (label, tone = 'green') => {
        const cls = tone === 'blue'
          ? 'text-[#007AFF] dark:text-[#5AC8FA] bg-[#007AFF]/10 dark:bg-[#0A84FF]/15'
          : 'text-[#34C759] bg-[#34C759]/10';
        return <span className={`shrink-0 inline-flex items-center gap-1 text-[10px] font-semibold ${cls} px-2 py-0.5 rounded-full leading-none`}><span className={`w-1.5 h-1.5 rounded-full ${tone === 'blue' ? 'bg-[#007AFF] dark:bg-[#5AC8FA]' : 'bg-[#34C759]'}`} />{label}</span>;
      };
      const switchRow = (row) => (
        <div key={row.id} className="flex items-center justify-between gap-2 px-3 py-2.5 rounded-xl font-medium">
          <span className="min-w-0">
            <span className="block text-[13px] text-gray-700 dark:text-gray-200 truncate">{row.title}</span>
          </span>
          <button onClick={() => toggleTool(row.id)} aria-label={row.id}
            className={`relative inline-flex h-5 w-[34px] shrink-0 items-center rounded-full transition-colors ${row.enabled ? 'bg-[#34C759]' : 'bg-[#E5E5EA] dark:bg-[#39393D]'}`}>
            <span className={`inline-block h-4 w-4 rounded-full bg-white shadow transition-transform ${row.enabled ? 'translate-x-[16px]' : 'translate-x-[2px]'}`} />
          </button>
        </div>
      );
      const readonlyRow = (row, label, tone = 'green') => (
        <div key={row.id} className="flex items-center justify-between gap-2 px-3 py-2.5 rounded-xl font-medium">
          <span className="min-w-0">
            <span className="block text-[13px] text-gray-700 dark:text-gray-200 truncate">{row.title}</span>
          </span>
          {statusBadge(label, tone)}
        </div>
      );
      return (
        <div className="relative shrink-0">
          <button onClick={() => setOpen(o => !o)} title={t.composerTools}
            className={`relative shrink-0 flex items-center justify-center text-gray-700 dark:text-gray-200 transition-colors border ${compact ? 'w-9 h-9 rounded-full bg-transparent hover:bg-black/5 dark:hover:bg-white/10 border-transparent' : 'gap-1.5 px-2.5 py-1.5 rounded-xl text-[13px] font-semibold whitespace-nowrap bg-gray-100 dark:bg-white/5 hover:bg-gray-200 dark:hover:bg-white/10 border-black/[0.04] dark:border-white/5'}`}>
            <Wrench size={compact ? 18 : 14} className="opacity-80" />
            {!compact && t.composerTools}
            {enabledCount > 0 && (compact
              ? <span className="absolute -top-1 -right-1 min-w-[16px] h-4 px-1 text-[10px] leading-4 text-center font-bold bg-[#007AFF] text-white rounded-full">{enabledCount}</span>
              : <span className="text-[11px] bg-[#007AFF] text-white px-1.5 py-0.5 rounded-full leading-none font-bold shrink-0">{enabledCount}</span>)}
            {!compact && <ChevronDown size={14} className="opacity-50 shrink-0" />}
          </button>
          {open && (
            <>
              <div className="fixed inset-0 z-40" onClick={() => setOpen(false)}></div>
              <div className="absolute bottom-full left-0 mb-2 z-50 w-72 max-h-[420px] overflow-y-auto custom-scrollbar bg-white/95 dark:bg-[#1E1E20]/95 backdrop-blur-xl border border-black/5 dark:border-white/10 rounded-2xl shadow-xl p-1.5">
                {connectedServices.map(row => readonlyRow(row, t.composerConnected, 'green'))}
                {toolRows.map(switchRow)}
                {skillRows.length === 0 ? (
                  <div className="px-3 py-2 text-[13px] text-gray-400 dark:text-gray-500">{t.composerModeNone}</div>
                ) : skillRows.map(row => row.switchable
                  ? switchRow(row)
                  : readonlyRow(row, row.active ? t.composerSkillInUse : t.composerBuiltinAuto, row.active ? 'green' : 'blue'))}
                <div className="h-px bg-black/5 dark:bg-white/10 my-1.5 mx-2" />
                <button onClick={() => { setOpen(false); if (onGotoTools) onGotoTools(); }}
                  className="w-full flex items-center gap-2.5 px-3 py-2.5 text-[13px] text-gray-700 dark:text-gray-200 hover:bg-[#007AFF] hover:text-white rounded-xl transition-colors group">
                  <Store size={15} className="text-gray-400 group-hover:text-white/90" />
                  {t.composerManageTools}
                </button>
              </div>
            </>
          )}
        </div>
      );
    };

    // 添加/编辑模型模态弹窗。
    const ModelFormModal = ({ isDark, t, initial, onCancel, onSave, bs }) => {
      const modelScope = initial.__scope || (initial.preset === 'local_vllm' ? 'local' : 'cloud');
      const initialCatalogGroups = MODEL_CATALOG[modelScope] || MODEL_CATALOG.cloud;
      const initialCatalogMatch = initialCatalogGroups.some(group =>
        group.preset === initial.preset && group.items.some(item => !item.custom && item.model === initial.model)
      );
      const [name, setName] = useState(initial.name || '');
      const [preset, setPreset] = useState(initial.preset || 'local_vllm');
      const [model, setModel] = useState(initial.model || '');
      const [baseUrl, setBaseUrl] = useState(initial.base_url || '');
      const [contextWindow, setContextWindow] = useState(initial.context_window_tokens ? String(initial.context_window_tokens) : '');
      const [maxOutput, setMaxOutput] = useState(initial.max_output_tokens ? String(initial.max_output_tokens) : '');
      const [apiKey, setApiKey] = useState('');
      const [keyAction, setKeyAction] = useState(initial.__new ? 'replace' : 'keep_existing');
      const [showKey, setShowKey] = useState(false);
      const [pickerOpen, setPickerOpen] = useState(!!initial.__new && initial.preset !== 'local_vllm');
      const [customModel, setCustomModel] = useState(!!initial.__custom || (!initial.__new && initial.preset !== 'local_vllm' && !initialCatalogMatch));
      const [keyRevealError, setKeyRevealError] = useState('');
      const [testing, setTesting] = useState(false);
      const [testResult, setTestResult] = useState(null);
      const [detecting, setDetecting] = useState(false);
      const [detectResult, setDetectResult] = useState(null); // { candidates } | { error } | null
      // 本机预装大模型「再入口」:检测无运行实例但有预装时,提示启用;走同一 bootstrap。
      const [offerSetup, setOfferSetup] = useState(false);   // 检测到预装,显示启用提示
      const [bootstrapHere, setBootstrapHere] = useState(false); // 从本页发起了 bootstrap(隔离全局态,避免开机引导的成功态串到这里)
      const baseCatalogGroups = MODEL_CATALOG[modelScope] || MODEL_CATALOG.cloud;
      const catalogGroups = !initial.__new && modelScope === 'cloud'
        ? baseCatalogGroups.filter(group => group.preset === initial.preset)
        : baseCatalogGroups;
      function applyCatalogItem(group, item) {
        const p = group.preset;
        setPreset(p);
        const defs = MODEL_PRESET_DEFS[p] || MODEL_PRESET_DEFS.local_vllm;
        const nextModel = item.custom ? '' : (item.model || defs.model);
        setBaseUrl(defs.baseUrl);
        setModel(nextModel);
        setName(p === 'local_vllm' ? (nextModel ? `本地 ${nextModel}` : '本地模型') : group.title);
        setContextWindow(p === 'local_vllm' ? '262144' : '');
        setMaxOutput(p === 'local_vllm' ? '24576' : '');
        if (p !== 'local_vllm') {
          setApiKey('');
          setKeyAction(initial.__new ? 'replace' : 'keep_existing');
        } else {
          setApiKey('');
          setKeyAction(initial.__new ? 'replace' : 'keep_existing');
        }
        setCustomModel(!!item.custom);
        setPickerOpen(false);
      }
      async function handleTest() {
        if (!bridge.available) return;
        setTesting(true); setTestResult(null);
        try { const msg = await bridge.models.testModelConnection(baseUrl.trim(), keyAction === 'replace' ? apiKey.trim() : '', initial.__new ? null : initial.id); setTestResult({ ok: true, msg: String(msg) }); }
        catch (e) { setTestResult({ ok: false, msg: String(e) }); }
        finally { setTesting(false); }
      }
      // 探测本机 vLLM：只扫 127.0.0.1/localhost 的 8000-8002，探到唯一可用实例直接自动填充。
      function applyCandidate(c) {
        if (!c) return;
        if (c.base_url) setBaseUrl(c.base_url);
        if (c.model) { setModel(c.model); if (!name.trim()) setName(c.model); }
        setApiKey('');
        setKeyAction(initial.__new ? 'replace' : 'keep_existing');
      }
      async function handleDetect() {
        if (!bridge.available || detecting) return;
        setDetecting(true); setDetectResult(null); setTestResult(null); setOfferSetup(false); setBootstrapHere(false);
        try {
          const result = await bridge.vllm.discoverLocalVllm({
            currentBaseUrl: baseUrl.trim() || null,
            savedBaseUrl: initial.base_url || null,
          });
          const online = ((result && result.candidates) || []).filter(c => c.status !== 'offline');
          setDetectResult({ candidates: online });
          if (online.length === 1) applyCandidate(online[0]); // 唯一可用实例直接填充
          else if (online.length === 0) {
            // 没探到运行中的实例:看本机是否有预装大模型,有则提示一键启用(走同一 bootstrap)。
            const setup = await bridge.vllm.detectLocalVllmSetup();
            const canStart = setup && setup.has_packages &&
              (setup.engine_state ? ['stopped', 'failed'].includes(setup.engine_state) : !setup.vllm_online);
            if (canStart) setOfferSetup(true);
            if (setup && setup.engine_state === 'starting') {
              setDetectResult({ candidates: [], engineState: 'starting' });
            }
          }
        } catch (e) {
          setDetectResult({ error: String(e) });
        } finally {
          setDetecting(false);
        }
      }
      function vllmStatusLabel(status) {
        if (status === 'busy') return t.vllmDetectBusy;
        if (status === 'ready') return t.vllmDetectReady;
        if (status === 'mismatch') return t.vllmDetectMismatch;
        return t.vllmDetectOffline;
      }
      const isLocalPreset = preset === 'local_vllm';
      const showModelIdField = isLocalPreset || customModel;
      const showBaseUrlField = isLocalPreset || (customModel && preset === 'openai_compatible');
      const showCustomCloudKeyField = !isLocalPreset && customModel;
      const showConfigFields = showModelIdField || showBaseUrlField || showCustomCloudKeyField;
      const selectedProvider = presetProviderLabel(preset, t);
      const selectedModelLabel = model || '自定义模型';
      const saveName = name.trim() || (isLocalPreset ? (model.trim() ? `本地 ${model.trim()}` : '本地模型') : selectedProvider);
      const credentialState = initial.credential_state || (initial.has_secret ? 'configured' : 'missing');
      const hasSavedKey = !!initial.has_secret || credentialState === 'configured' || credentialState === 'env_override';
      const keyStatusText = credentialState === 'env_override' ? t.credEnvOverride
        : credentialState === 'unavailable' ? t.credUnavailable
        : hasSavedKey ? t.credConfigured
        : t.credNotConfigured;
      const hasUsableApiKey = isLocalPreset || hasSavedKey || !!apiKey.trim();
      const canSave = !!(saveName && model.trim() && baseUrl.trim() && hasUsableApiKey);
      async function toggleApiKeyVisibility() {
        const nextVisible = !showKey;
        if (nextVisible && hasSavedKey && !apiKey.trim() && credentialState !== 'env_override' && initial.id && bridge.models.revealModelApiKey) {
          try {
            setKeyRevealError('');
            const savedKey = await bridge.models.revealModelApiKey(initial.id);
            if (savedKey) setApiKey(savedKey);
          } catch (error) {
            setKeyRevealError(String(error || '读取 API Key 失败'));
          }
        }
        setShowKey(nextVisible);
      }
      function doSave() {
        if (!canSave) return;
        const id = initial.__new ? ('m_' + Date.now().toString(36) + Math.random().toString(36).slice(2, 7)) : initial.id;
        const contextTokens = Number.parseInt(contextWindow, 10);
        const outputTokens = Number.parseInt(maxOutput, 10);
        const nextKeyAction = isLocalPreset
          ? 'keep_existing'
          : (apiKey.trim() ? 'replace' : (initial.__new || !hasSavedKey ? 'replace' : 'keep_existing'));
        onSave({
          id: id, name: saveName, preset: preset,
          context_window_tokens: Number.isFinite(contextTokens) && contextTokens > 0 ? contextTokens : null,
          max_output_tokens: Number.isFinite(outputTokens) && outputTokens > 0 ? outputTokens : null,
          model: model.trim(), base_url: baseUrl.trim(),
          api_key: !isLocalPreset && apiKey.trim() ? apiKey.trim() : '', credential_action: nextKeyAction,
        });
      }
      const catalogSectionTitleClass = `px-1 mb-2 text-[12px] leading-4 font-semibold ${isDark ? 'text-[#8E8E93]' : 'text-[#8A8A8E]'}`;
      const catalogGroupClass = `overflow-hidden rounded-[16px] ${isDark ? 'bg-[#2C2C2E]' : 'bg-[#F2F2F7]'}`;
      const formSectionTitle = `px-1 mb-1.5 text-[12px] leading-4 font-semibold ${isDark ? 'text-[#8E8E93]' : 'text-[#8A8A8E]'}`;
      const formGroup = `overflow-hidden rounded-[16px] ${isDark ? 'bg-[#2C2C2E]' : 'bg-[#F2F2F7]'}`;
      const formDivider = isDark ? 'border-white/[0.10]' : 'border-black/[0.10]';
      const renderInlineField = ({ label, value, onChange, placeholder, type = 'text', trailing, readOnly = false }) => (
        <div className={`min-h-[54px] flex items-center gap-3 px-4 py-2.5 border-b last:border-b-0 ${formDivider}`}>
          <label className={`shrink-0 text-[14px] leading-5 ${isDark ? 'text-[#F2F2F7]' : 'text-[#1C1C1E]'}`}>{label}</label>
          <input
            type={type}
            value={value}
            onChange={onChange}
            readOnly={readOnly}
            placeholder={placeholder}
            className={`min-w-0 flex-1 bg-transparent text-right text-[14px] leading-5 outline-none ${isDark ? 'text-[#F2F2F7] placeholder:text-[#636366]' : 'text-[#1C1C1E] placeholder:text-[#8A8A8E]'} ${readOnly ? 'cursor-default' : ''}`}
          />
          {trailing}
        </div>
      );
      const renderCatalogPicker = () => (
        <div className="space-y-4">
          {catalogGroups.map(group => (
            <section key={group.key}>
              <div className={catalogSectionTitleClass}>{group.title}</div>
              <div className={catalogGroupClass}>
                {group.items.map(item => {
                  const active = preset === group.preset && model === item.model && !item.custom;
                  return (
                    <button
                      type="button"
                      key={`${group.key}-${item.title}`}
                      onClick={() => applyCatalogItem(group, item)}
                      className={`w-full min-h-[56px] px-3.5 py-2.5 flex items-center gap-3 text-left border-b last:border-b-0 ${active ? 'bg-[#007AFF]/10' : ''} ${isDark ? 'border-white/[0.10] hover:bg-white/[0.06]' : 'border-black/[0.10] hover:bg-black/[0.035]'}`}
                    >
                      <ProviderIcon preset={group.preset} isDark={isDark} compact />
                      <span className="min-w-0 flex-1">
                        <span className={`block text-[15px] leading-5 font-normal truncate ${isDark ? 'text-[#F2F2F7]' : 'text-[#1C1C1E]'}`}>{item.title}</span>
                        <span className={`block mt-0.5 text-[12px] leading-[17px] truncate ${isDark ? 'text-[#98989D]' : 'text-[#8A8A8E]'}`}>{item.desc}</span>
                      </span>
                      {active ? <Check size={16} className="shrink-0 text-[#007AFF]" /> : <ChevronDown size={16} className={`-rotate-90 shrink-0 ${isDark ? 'text-[#636366]' : 'text-[#C7C7CC]'}`} />}
                    </button>
                  );
                })}
              </div>
            </section>
          ))}
        </div>
      );
      if (initial.__new && pickerOpen) {
        return (
          <div data-testid="model-form-backdrop" className="fixed inset-0 z-[100] flex items-center justify-center bg-black/45 px-4 animate-in fade-in duration-150">
            <div data-testid="model-form-dialog" role="dialog" aria-modal="true"
              className={`w-[440px] max-w-[90vw] max-h-[76vh] overflow-y-auto custom-scrollbar rounded-[22px] shadow-2xl ${isDark ? 'bg-[#1C1C1E] text-[#F2F2F7]' : 'bg-white text-[#1C1C1E]'}`}>
              <div className={`px-5 py-4 flex items-start justify-between gap-4 border-b ${isDark ? 'border-white/[0.10]' : 'border-black/[0.10]'}`}>
                <div>
                  <h2 className="text-[20px] leading-6 font-semibold">{t.modelFormAddTitle}</h2>
                  <p className={`mt-1 text-[13px] leading-[18px] ${isDark ? 'text-[#98989D]' : 'text-[#8A8A8E]'}`}>选择模型后再填写必要凭据</p>
                </div>
                <button data-testid="model-form-cancel" onClick={onCancel} className={`h-9 w-9 shrink-0 rounded-full flex items-center justify-center ${isDark ? 'bg-white/[0.08] text-[#C7C7CC]' : 'bg-[#E5E5EA] text-[#636366]'}`}><X size={18} /></button>
              </div>
              <div className="px-5 py-4">{renderCatalogPicker()}</div>
            </div>
          </div>
        );
      }
      return (
        <div data-testid="model-form-backdrop" className="fixed inset-0 z-[100] flex items-center justify-center bg-black/50 animate-in fade-in duration-150" onClick={onCancel}>
          <div data-testid="model-form-dialog" role="dialog" aria-modal="true" onClick={e => e.stopPropagation()}
            className={`w-[430px] max-w-[90vw] max-h-[76vh] overflow-y-auto custom-scrollbar rounded-[22px] shadow-2xl ${isDark ? 'bg-[#1C1C1E] text-[#F2F2F7]' : 'bg-white text-[#1C1C1E]'}`}>
            <div className={`px-5 py-4 flex items-start justify-between gap-4 border-b ${formDivider}`}>
              <div>
                <h2 className="text-[20px] leading-6 font-semibold">{initial.__new ? t.modelFormAddTitle : t.modelFormEditTitle}</h2>
                <p className={`mt-1 text-[13px] leading-[18px] ${isDark ? 'text-[#98989D]' : 'text-[#8A8A8E]'}`}>{isLocalPreset ? selectedModelLabel : `${selectedProvider} · ${selectedModelLabel}`}</p>
              </div>
              <button data-testid="model-form-cancel" onClick={onCancel} className={`h-9 w-9 shrink-0 rounded-full flex items-center justify-center ${isDark ? 'bg-white/[0.08] text-[#C7C7CC]' : 'bg-[#E5E5EA] text-[#636366]'}`}><X size={18} /></button>
            </div>
            <div className="space-y-4 px-5 py-4">
              <div className={`overflow-hidden rounded-[18px] border ${isDark ? 'border-white/[0.10] bg-[#2C2C2E]' : 'border-black/[0.08] bg-white'}`}>
                {isLocalPreset ? (
                  <div className="w-full min-h-[62px] px-4 py-3 flex items-center gap-3 text-left">
                    <ProviderIcon preset={preset} isDark={isDark} compact />
                    <span className="min-w-0 flex-1">
                      <span className="block text-[15px] leading-5 font-normal truncate">{selectedProvider}</span>
                      <span className={`block mt-0.5 text-[12px] leading-[17px] truncate ${isDark ? 'text-[#98989D]' : 'text-[#8A8A8E]'}`}>{selectedModelLabel}</span>
                    </span>
                  </div>
                ) : (
                  <button
                    type="button"
                    onClick={() => setPickerOpen(v => !v)}
                    className={`w-full min-h-[62px] px-4 py-3 flex items-center gap-3 text-left ${isDark ? 'hover:bg-white/[0.05]' : 'hover:bg-black/[0.035]'}`}
                  >
                    <ProviderIcon preset={preset} isDark={isDark} compact />
                    <span className="min-w-0 flex-1">
                      <span className="block text-[15px] leading-5 font-normal truncate">{selectedProvider}</span>
                      <span className={`block mt-0.5 text-[12px] leading-[17px] truncate ${isDark ? 'text-[#98989D]' : 'text-[#8A8A8E]'}`}>{selectedModelLabel}</span>
                    </span>
                    <span className="shrink-0 text-[14px] text-[#007AFF]">{pickerOpen ? '收起' : '更换'}</span>
                  </button>
                )}
                {pickerOpen && !isLocalPreset && (
                  <div className={`border-t px-4 py-4 ${isDark ? 'border-white/[0.10]' : 'border-black/[0.12]'}`}>
                    {renderCatalogPicker()}
                  </div>
                )}
              </div>
              {!isLocalPreset && !customModel && (
                <section>
                  <div className={formGroup}>
                    <div className="min-h-[54px] flex items-center gap-3 px-4 py-2.5">
                      <label className={`shrink-0 text-[14px] leading-5 ${isDark ? 'text-[#F2F2F7]' : 'text-[#1C1C1E]'}`}>API Key</label>
                      <input type="text" value={apiKey} onChange={e => { setApiKey(e.target.value); if (e.target.value.trim()) setKeyAction('replace'); }}
                        placeholder={hasSavedKey ? '••••••••' : '输入 API Key'}
                        style={showKey ? undefined : { WebkitTextSecurity: 'disc' }}
                        className={`min-w-0 flex-1 bg-transparent text-right text-[14px] leading-5 outline-none ${isDark ? 'text-[#F2F2F7] placeholder:text-[#636366]' : 'text-[#1C1C1E] placeholder:text-[#8A8A8E]'}`} />
                      <button type="button" onClick={toggleApiKeyVisibility} className="shrink-0 text-[14px] text-[#007AFF]">{showKey ? '隐藏' : '显示'}</button>
                    </div>
                  </div>
                  {keyRevealError && <div className="px-1 mt-1.5 text-[12px] leading-4 text-[#FF3B30]">{keyRevealError}</div>}
                </section>
              )}
              {showConfigFields && (
                <section>
                  <div className={formGroup}>
                    {showModelIdField && renderInlineField({ label: isLocalPreset ? '本地模型 ID' : '模型 ID', value: model, onChange: e => setModel(e.target.value), placeholder: isLocalPreset ? '' : '输入模型 ID' })}
                    {showCustomCloudKeyField && (
                      <div className={`min-h-[54px] flex items-center gap-3 px-4 py-2.5 border-b last:border-b-0 ${formDivider}`}>
                        <label className={`shrink-0 text-[14px] leading-5 ${isDark ? 'text-[#F2F2F7]' : 'text-[#1C1C1E]'}`}>API Key</label>
                        <input type="text" value={apiKey} onChange={e => { setApiKey(e.target.value); if (e.target.value.trim()) setKeyAction('replace'); }}
                          placeholder={hasSavedKey ? '••••••••' : '输入 API Key'}
                          style={showKey ? undefined : { WebkitTextSecurity: 'disc' }}
                          className={`min-w-0 flex-1 bg-transparent text-right text-[14px] leading-5 outline-none ${isDark ? 'text-[#F2F2F7] placeholder:text-[#636366]' : 'text-[#1C1C1E] placeholder:text-[#8A8A8E]'}`} />
                        <button type="button" onClick={toggleApiKeyVisibility} className="shrink-0 text-[14px] text-[#007AFF]">{showKey ? '隐藏' : '显示'}</button>
                      </div>
                    )}
                    {showBaseUrlField && renderInlineField({ label: t.customBaseUrl, value: baseUrl, onChange: e => setBaseUrl(e.target.value) })}
                  </div>
                  {keyRevealError && <div className="px-1 mt-1.5 text-[12px] leading-4 text-[#FF3B30]">{keyRevealError}</div>}
                </section>
              )}
              {preset === 'local_vllm' && detectResult && (
                <div className={`rounded-xl border p-3 space-y-2 ${isDark ? 'border-[#333537] bg-[#131314]' : 'border-[#E0E3E7] bg-[#F8F9FB]'}`}>
                  {detectResult.error ? (
                    <span className={`text-[12px] ${isDark ? 'text-[#F28B82]' : 'text-[#C5221F]'}`}>{t.vllmDetectError(detectResult.error)}</span>
                  ) : detectResult.engineState === 'starting' ? (
                    <span className={`text-[12px] ${isDark ? 'text-[#A8C7FA]' : 'text-[#0B57D0]'}`}>{t.vllmDetectStarting}</span>
                  ) : detectResult.candidates.length === 0 ? (
                    <span className={`text-[12px] ${isDark ? 'text-[#9AA0A6]' : 'text-[#5F6368]'}`}>{t.vllmDetectNone}</span>
                  ) : (
                    <>
                      <span className={`text-[12px] ${isDark ? 'text-[#93D5A6]' : 'text-[#137333]'}`}>{t.vllmDetectFound(detectResult.candidates.length)}</span>
                      {detectResult.candidates.map(c => (
                        <button key={c.base_url} onClick={() => applyCandidate(c)}
                          className={`w-full text-left rounded-lg border px-3 py-2 transition-colors ${isDark ? 'border-[#333537] hover:bg-[#2A2B2D]' : 'border-[#E0E3E7] hover:bg-[#F0F4F9]'}`}>
                          <div className={`text-[13px] truncate ${isDark ? 'text-[#E3E3E3]' : 'text-[#1F1F1F]'}`}>{c.base_url}</div>
                          <div className={`text-[11px] truncate ${isDark ? 'text-[#9AA0A6]' : 'text-[#5F6368]'}`}>
                            {vllmStatusLabel(c.status)}
                            {c.model ? ` · ${t.vllmDetectedModel}: ${c.model}` : ''}
                            {c.max_model_len ? ` · ${t.vllmDetectedContext}: ${c.max_model_len}` : ''}
                          </div>
                        </button>
                      ))}
                    </>
                  )}
                  <span className={`text-[11px] block ${isDark ? 'text-[#5F6368]' : 'text-[#9AA0A6]'}`}>{t.vllmDetectHint}</span>
                </div>
              )}
              {preset === 'local_vllm' && (offerSetup || bootstrapHere) && (
                <div className={`rounded-xl border p-3 ${isDark ? 'border-[#333537] bg-[#131314]' : 'border-[#E0E3E7] bg-[#F8F9FB]'}`}>
                  {bootstrapHere ? (
                    bs && bs.vllmBootstrapDone ? (
                      <div>
                        <div className="text-[13px] leading-relaxed mb-3">{t.vllmSetupDone}</div>
                        <div className="flex justify-end">
                          <button onClick={() => bridge.available && bridge.updater.restartApp()}
                            className="h-8 px-4 rounded-lg text-[13px] font-medium text-white" style={{ background: '#0A84FF' }}>{t.restartNow}</button>
                        </div>
                      </div>
                    ) : bs && bs.vllmBootstrapError ? (
                      <div>
                        <div className="text-[12px] font-medium mb-1" style={{ color: '#E5484D' }}>{t.vllmSetupFailed}</div>
                        <div className="text-[12px] leading-relaxed mb-3 break-words" style={{ opacity: .75 }}>{bs.vllmBootstrapError}</div>
                        <div className="flex justify-end gap-2">
                          <button onClick={() => { setBootstrapHere(false); setOfferSetup(false); }}
                            className={`h-8 px-4 rounded-lg text-[13px] ${isDark ? 'bg-[#2B2C2F] text-[#E3E3E3]' : 'bg-[#F0F4F9] text-[#1F1F1F]'}`}>{t.cpCancel}</button>
                          <button onClick={() => bridge.vllm.bootstrapLocalVllm()}
                            className="h-8 px-4 rounded-lg text-[13px] font-medium text-white" style={{ background: '#0A84FF' }}>{t.vllmSetupRetry}</button>
                        </div>
                      </div>
                    ) : (
                      <VllmSetupProgress phase={bs && bs.vllmSetupPhase} attempt={(bs && bs.vllmSetupAttempt) || 0} isDark={isDark} t={t} />
                    )
                  ) : (
                    <div>
                      <div className="text-[13px] leading-relaxed mb-3">{t.vllmReentryOffer}</div>
                      <div className="flex justify-end gap-2">
                        <button onClick={() => setOfferSetup(false)}
                          className={`h-8 px-4 rounded-lg text-[13px] ${isDark ? 'bg-[#2B2C2F] text-[#E3E3E3]' : 'bg-[#F0F4F9] text-[#1F1F1F]'}`}>{t.cpCancel}</button>
                        <button onClick={() => { setBootstrapHere(true); bridge.vllm.bootstrapLocalVllm(); }}
                          className="h-8 px-4 rounded-lg text-[13px] font-medium text-white" style={{ background: '#0A84FF' }}>{t.vllmSetupEnable}</button>
                      </div>
                    </div>
                  )}
                </div>
              )}
            </div>
            <div className={`flex justify-end gap-2 px-5 py-4 border-t ${formDivider}`}>
              <button data-testid="model-form-cancel" onClick={onCancel} className={`h-10 px-4 rounded-full text-[15px] font-normal transition-colors ${isDark ? 'text-[#0A84FF] hover:bg-white/[0.06]' : 'text-[#007AFF] hover:bg-black/[0.04]'}`}>{t.cpCancel}</button>
              <button onClick={doSave} disabled={!canSave}
                className="h-10 px-5 rounded-full bg-[#007AFF] text-white text-[15px] font-semibold transition-colors disabled:opacity-35">{t.modelSaveBtn}</button>
            </div>
          </div>
        </div>
      );
    };

    const SettingsView = ({ activeTheme, setActiveTheme, language, setLanguage, superPerm, setSuperPerm, taskCompletedNotif, setTaskCompletedNotif, searchProvider, setSearchProvider, enabledSearchProviders = ['bing'], onAddSearchProvider, onDeleteSearchProvider, searchApiKey, setSearchApiKey, searchHasSavedKey, savedModels, activeModelId, onSaveModel, onDeleteModel, onSetActiveModel, onSaveSearchConfig, onConfirmSearchConfig, onMemoryEnabledChange, onPetEnabledChange, searchNeedsRestart, languageNeedsRestart, bs, t, onRestoreArchived, onDeleteArchived, updateFocusTick, onCloseSettings, initialSection = 'general' }) => {
      const isDark = activeTheme === 'dark';
      const platformCapabilities = (bs && bs.platformCapabilities) || {};
      const showSuperPermissionSettings = !!platformCapabilities.showSuperPermissionSettings;
      const usesBundledDependencyInstaller = !!platformCapabilities.usesBundledDependencyInstaller;
      const [activeSection, setActiveSection] = useState(initialSection || 'general');
      const [editingModel, setEditingModel] = useState(null);
      const [modelDeleteConfirm, setModelDeleteConfirm] = useState(null);
      const [editingSearch, setEditingSearch] = useState(null);
      const [pendingSearchProvider, setPendingSearchProvider] = useState(null);
      const [searchDeleteConfirm, setSearchDeleteConfirm] = useState(null);
      const [searchPickerOpen, setSearchPickerOpen] = useState(false);
      const [restartDialog, setRestartDialog] = useState(null);
      const modelEnvLocked = (bs && bs.effectiveModelConfig && bs.effectiveModelConfig.env_overrides) || [];
      const [feedbackOpen, setFeedbackOpen] = useState(false);
      const [feedbackDraft, setFeedbackDraft] = useState({ type: 'issue', title: '', description: '', attachments: [] });
      const [feedbackStatus, setFeedbackStatus] = useState({ state: 'idle', message: '', receipt: null });
      const [feedbackNotice, setFeedbackNotice] = useState('');
      const [archivedDeleteConfirm, setArchivedDeleteConfirm] = useState(null);
      const [llmApiModelBusy, setLlmApiModelBusy] = useState(false);
      const llmApiModels = (bs && bs.llmApiModels) || {};
      const builtinAvailableModels = llmApiModels.available_models || [];
      const builtinDefaultModel = llmApiModels.default_model || (((bs && bs.settings && bs.settings.advanced && bs.settings.advanced.builtin_llmapi_default_model) || '') || '');
      const versionUpdateRef = useRef(null);
      const hasUpdate = !!(bs && bs.updateInfo && bs.updateInfo.available);
      const archivedSessions = (bs && bs.archivedSessions) || [];
      const memorySettingsVisible = language === 'zh';
      const feedbackTypes = [
        { key: 'issue', label: t.feedbackIssue },
        { key: 'suggestion', label: t.feedbackSuggestion },
      ];
      const feedbackAllowedExt = new Set(['png', 'jpg', 'jpeg', 'gif', 'webp', 'mp4', 'mov', 'webm']);
      const feedbackVideoExt = new Set(['mp4', 'mov', 'webm']);
      const feedbackBaseName = p => String(p || '').replace(/\\/g, '/').split('/').pop() || String(p || '');
      const feedbackExt = p => {
        const name = feedbackBaseName(p);
        const idx = name.lastIndexOf('.');
        return idx >= 0 ? name.slice(idx + 1).toLowerCase() : '';
      };
      useEffect(() => {
        if (!updateFocusTick || !versionUpdateRef.current) return;
        requestAnimationFrame(() => {
          versionUpdateRef.current && versionUpdateRef.current.scrollIntoView({ behavior: 'smooth', block: 'center' });
        });
      }, [updateFocusTick]);
      useEffect(() => {
        if (initialSection) setActiveSection(initialSection);
      }, [initialSection]);
      useEffect(() => {
        if (!feedbackNotice) return;
        const timer = window.setTimeout(() => setFeedbackNotice(''), 2600);
        return () => window.clearTimeout(timer);
      }, [feedbackNotice]);
      const resetFeedback = () => {
        setFeedbackDraft({ type: 'issue', title: '', description: '', attachments: [] });
        setFeedbackStatus({ state: 'idle', message: '', receipt: null });
      };
      const closeFeedback = () => {
        const dirty = feedbackDraft.title.trim() || feedbackDraft.description.trim() || feedbackDraft.attachments.length > 0;
        if (dirty && feedbackStatus.state !== 'submitted' && !window.confirm(t.feedbackCloseConfirm)) return;
        setFeedbackOpen(false);
        if (feedbackStatus.state === 'submitted') resetFeedback();
      };
      const pickFeedbackAttachments = async () => {
        if (!bridge.available || !bridge.files.pickFeedbackFiles) {
          setFeedbackStatus({ state: 'failed_validation', message: t.feedbackPickUnavailable, receipt: null });
          return;
        }
        const paths = await bridge.files.pickFeedbackFiles();
        if (!paths || paths.length === 0) return;
        setFeedbackDraft(prev => {
          const next = prev.attachments.slice();
          for (const path of paths) {
            if (next.length >= 5) {
              setFeedbackStatus({ state: 'failed_validation', message: t.feedbackTooManyFiles, receipt: null });
              break;
            }
            const ext = feedbackExt(path);
            if (!feedbackAllowedExt.has(ext)) {
              setFeedbackStatus({ state: 'failed_validation', message: t.feedbackUnsupportedFile, receipt: null });
              continue;
            }
            const name = feedbackBaseName(path);
            next.push({
              path,
              name,
              media_type: feedbackVideoExt.has(ext) ? 'video' : 'image',
              mime: null,
              size_bytes: null,
            });
          }
          return { ...prev, attachments: next };
        });
      };
      const submitFeedbackDraft = async () => {
        if (!feedbackDraft.description.trim()) {
          setFeedbackStatus({ state: 'failed_validation', message: t.feedbackBodyRequired, receipt: null });
          return;
        }
        setFeedbackStatus({ state: 'submitting', message: '', receipt: null });
        try {
          const receipt = await bridge.feedback.submitFeedback({
            type: feedbackDraft.type,
            title: feedbackDraft.title.trim() || null,
            description: feedbackDraft.description,
            entry_point: 'settings',
            error_summary: null,
            attachments: feedbackDraft.attachments,
            privacy_notice_version: '2026-06-24',
          });
          if (receipt && receipt.status === 'submitted') {
            setFeedbackNotice((receipt && receipt.message) || t.feedbackSubmitted);
            resetFeedback();
            setFeedbackOpen(false);
            return;
          }
          setFeedbackStatus({
            state: 'failed_retryable',
            message: (receipt && receipt.message) || '',
            receipt,
          });
        } catch (e) {
          setFeedbackStatus({ state: 'failed_validation', message: String(e), receipt: null });
        }
      };
      const confirmArchivedDelete = () => {
        const id = archivedDeleteConfirm && archivedDeleteConfirm.id;
        setArchivedDeleteConfirm(null);
        if (id && onDeleteArchived) onDeleteArchived(id);
      };
      const refreshBuiltinModels = async () => {
        if (!bridge.available || !bridge.llmapi.getLlmApiModels) return;
        setLlmApiModelBusy(true);
        try { await bridge.llmapi.getLlmApiModels(); } finally { setLlmApiModelBusy(false); }
      };
      const setBuiltinDefaultModel = async model => {
        if (!bridge.available || !bridge.llmapi.setLlmApiDefaultModel || !model) return;
        setLlmApiModelBusy(true);
        try { await bridge.llmapi.setLlmApiDefaultModel(model); } finally { setLlmApiModelBusy(false); }
      };
      // 进设置页自动体检一次可选依赖装齐没; 之后用户可手动「重新检测」
      useEffect(() => {
        if (!bridge.available || (bs && (bs.deps || bs.depsChecking))) return;
        let cancelled = false;
        const run = () => { if (!cancelled) bridge.dependencies.checkDependencies(); };
        if (window.requestIdleCallback) {
          const idleId = window.requestIdleCallback(run, { timeout: 1200 });
          return () => {
            cancelled = true;
            if (window.cancelIdleCallback) window.cancelIdleCallback(idleId);
          };
        }
        const timerId = window.setTimeout(run, 300);
        return () => {
          cancelled = true;
          window.clearTimeout(timerId);
        };
      }, []);
      const IOSSection = ({ title, children, footer }) => (
        <section className="mb-6">
          {title && <div className={`px-3 mb-2 text-[12px] font-semibold ${isDark ? 'text-[#8E8E93]' : 'text-[#8A8A8E]'}`}>{title}</div>}
          <div className={`overflow-hidden rounded-[18px] ${isDark ? 'bg-[#2C2C2E]' : 'bg-white'}`}>{children}</div>
          {footer && <div className={`px-3 mt-2 text-[12px] leading-relaxed ${isDark ? 'text-[#8E8E93]' : 'text-[#8A8A8E]'}`}>{footer}</div>}
        </section>
      );
      const IOSRow = ({ label, desc, value, children, onClick, danger }) => {
        const RowTag = onClick ? 'button' : 'div';
        return (
        <RowTag
          type={onClick ? 'button' : undefined}
          onClick={onClick}
          className={`w-full min-h-[58px] flex flex-wrap items-center gap-3 px-4 py-2.5 text-left border-b last:border-b-0 ${
            isDark ? 'border-white/[0.10] text-[#F2F2F7]' : 'border-black/[0.12] text-[#1C1C1E]'
          } ${onClick ? (isDark ? 'hover:bg-white/[0.05]' : 'hover:bg-black/[0.035]') : ''}`}
        >
          <div className="flex-1 min-w-[120px]">
            <div className={`text-[15px] leading-5 font-normal whitespace-nowrap ${danger ? 'text-[#FF3B30]' : ''}`}>{label}</div>
            {desc && <div className={`mt-0.5 text-[13px] leading-5 ${isDark ? 'text-[#98989D]' : 'text-[#8A8A8E]'}`}>{desc}</div>}
          </div>
          {value && <div className={`text-[14px] shrink-0 ${isDark ? 'text-[#98989D]' : 'text-[#8A8A8E]'}`}>{value}</div>}
          {children}
        </RowTag>
        );
      };
      const IOSSwitch = ({ checked, onChange }) => (
        <button
          type="button"
          role="switch"
          aria-checked={checked}
          onClick={() => onChange(!checked)}
          className={`relative h-[26px] w-[46px] shrink-0 rounded-full transition-colors ${checked ? 'bg-[#34C759]' : (isDark ? 'bg-[#3A3A3C]' : 'bg-[#E5E5EA]')}`}
        >
          <span className={`absolute left-0 top-[2px] h-[22px] w-[22px] rounded-full bg-white shadow transition-transform ${checked ? 'translate-x-[22px]' : 'translate-x-[2px]'}`} />
        </button>
      );
      const SectionButton = ({ id, icon, label, dot }) => (
        <button
          type="button"
          onClick={() => setActiveSection(id)}
          className={`w-full h-10 px-3 rounded-[14px] flex items-center gap-2.5 text-[14px] transition-colors ${
            activeSection === id
              ? (isDark ? 'bg-[#173A5E] text-[#64B5F6]' : 'bg-[#D8EAFE] text-[#007AFF]')
              : (isDark ? 'text-[#F2F2F7] hover:bg-white/[0.06]' : 'text-[#1C1C1E] hover:bg-black/[0.04]')
          }`}
        >
          <span className={`w-7 h-7 rounded-[9px] flex items-center justify-center ${activeSection === id ? 'bg-[#007AFF]/10' : (isDark ? 'bg-white/[0.08]' : 'bg-black/[0.05]')}`}>{icon}</span>
          <span className="font-semibold truncate">{label}</span>
          {dot && <span className="ml-auto w-2.5 h-2.5 rounded-full bg-[#FF3B30]" />}
        </button>
      );
      const actionButton = (tone = 'blue') => {
        if (tone === 'green') return 'text-[#34C759] hover:bg-[#34C759]/10';
        if (tone === 'red') return 'text-[#FF3B30] hover:bg-[#FF3B30]/10';
        return 'text-[#007AFF] hover:bg-[#007AFF]/10';
      };
      const Group = ({ children }) => (
        <div className={`overflow-hidden rounded-[18px] border ${isDark ? 'bg-[#2C2C2E] border-white/[0.04]' : 'bg-white border-black/[0.03]'}`}>{children}</div>
      );
      const SectionTitle = ({ children }) => (
        <div className={`px-3 mb-2 text-[12px] leading-4 font-semibold ${isDark ? 'text-[#8E8E93]' : 'text-[#8A8A8E]'}`}>{children}</div>
      );
      const RadioDot = ({ active }) => (
        <span className={`block w-5 h-5 rounded-full border-[3px] ${active ? 'border-[#007AFF]' : (isDark ? 'border-[#636366]' : 'border-[#AEAEB2]')}`}>
          {active && <span className="block w-2 h-2 rounded-full bg-[#007AFF] mx-auto mt-[3px]" />}
        </span>
      );
      const Tag = ({ children, tone = 'green' }) => (
        <span className={`shrink-0 text-[12px] px-2 py-0.5 rounded-md ${
          tone === 'gray'
            ? (isDark ? 'bg-white/[0.08] text-[#C7C7CC]' : 'bg-[#E5E5EA] text-[#636366]')
            : 'bg-[#34C759]/15 text-[#248A3D]'
        }`}>{children}</span>
      );
      const userModels = visibleSortedModels(savedModels || [], bs);
      const isLocalModel = model => model && (model.preset === 'local_vllm' || /127\.0\.0\.1|localhost/i.test(model.base_url || ''));
      const searchOptions = [
        { key: 'bing', label: 'Bing', desc: '内置搜索' },
        { key: 'metaso', label: '秘塔', desc: '中文搜索服务' },
        { key: 'bocha', label: '博查', desc: '搜索服务' },
        { key: 'baidu', label: '百度', desc: '千帆 AI 搜索' },
        { key: 'tavily', label: 'Tavily', desc: '海外搜索服务' },
      ];
      const enabledSearchSet = new Set(['bing', ...(enabledSearchProviders || [])]);
      const enabledSearchList = searchOptions.filter(item => enabledSearchSet.has(item.key));
      const searchCredentialFor = provider => {
        const credentials = (bs && bs.settings && bs.settings.search && bs.settings.search.credentials) || {};
        return credentials[provider] || {};
      };
      const searchHasKey = provider => {
        if (provider === 'bing') return true;
        const credential = searchCredentialFor(provider);
        const state = credential.credential_state || (credential.has_secret ? 'configured' : 'missing');
        return !!credential.has_secret || state === 'configured' || state === 'env_override';
      };
      const newModelDraft = preset => {
        const defs = MODEL_PRESET_DEFS[preset] || MODEL_PRESET_DEFS.deepseek;
        return {
          __new: true,
          id: '',
          name: preset === 'local_vllm' ? '本地 Qwen3.6' : presetProviderLabel(preset, t),
          preset,
          context_window_tokens: preset === 'local_vllm' ? 262144 : null,
          max_output_tokens: preset === 'local_vllm' ? 24576 : null,
          model: defs.model,
          base_url: defs.baseUrl,
          api_key: '',
          __scope: preset === 'local_vllm' ? 'local' : 'cloud',
        };
      };
      const memoryEnabled = !!(bs && bs.settings && bs.settings.memory_enabled);
      const memory = (bs && bs.memory) || {};
      const identity = (memory.profile && memory.profile.identity) || {};
      const longTermItems = [
        ...(memory.preferences || []).map(item => ({ ...item, kind: 'preference', type: '长期偏好' })),
        ...(memory.work_context || []).map(item => ({ ...item, kind: 'work_context', type: '工作背景' })),
      ];
      const recentItems = [
        ...(memory.current_focus || []).filter(item => item.status !== 'archived').map(item => ({ ...item, kind: 'current_focus', type: '当前关注' })),
        ...(memory.recent_activity || []).filter(item => item.status !== 'archived').map(item => ({ ...item, kind: 'recent_activity', type: '近期动态' })),
      ];
      useEffect(() => {
        if (activeSection === 'memory' && memoryEnabled && bridge.available && bridge.memory.loadMemoryOverview) bridge.memory.loadMemoryOverview();
      }, [activeSection, memoryEnabled]);
      useEffect(() => {
        if (updateFocusTick) setActiveSection('update');
      }, [updateFocusTick]);
      const [memoryEditor, setMemoryEditor] = useState(null);
      const [memoryDeleteConfirm, setMemoryDeleteConfirm] = useState(null);
      const openMemoryItemViewer = item => {
        setMemoryEditor({
          mode: 'memory',
          kind: item.kind,
          id: item.id,
          title: '记忆详情',
          subtitle: '',
          label: '内容',
          value: item.text || item.content || '',
          originalValue: item.text || item.content || '',
          multiline: true,
          editing: false,
        });
      };
      const saveMemoryEditor = async () => {
        if (!memoryEditor || !bridge.available) return;
        const text = String(memoryEditor.value || '').trim();
        if (memoryEditor.mode === 'memory') {
          if (!text || !bridge.memory.updateMemoryItem) return;
          await bridge.memory.updateMemoryItem(memoryEditor.kind, memoryEditor.id, { text });
        } else if (memoryEditor.mode === 'profile') {
          if (!bridge.memory.saveMemoryProfilePatch) return;
          await bridge.memory.saveMemoryProfilePatch({ [memoryEditor.key]: text });
        }
        setMemoryEditor(null);
      };
      const deleteMemoryItem = async item => {
        if (!bridge.available || !bridge.memory.deleteMemoryItem) return;
        await bridge.memory.deleteMemoryItem(item.kind, item.id);
      };
      const editProfile = key => {
        const label = key === 'call_name' ? '用户称呼' : '助手昵称';
        setMemoryEditor({
          mode: 'profile',
          key,
          title: `编辑${label}`,
          subtitle: key === 'call_name' ? '助手称呼你的方式' : '你称呼助手的方式',
          label,
          value: identity[key] || '',
          multiline: false,
        });
      };
      const renderModelRows = models => models.length ? models.map(m => {
        const isActive = m.id === activeModelId;
        const isLocal = isLocalModel(m);
        const isReadonly = isReadonlyModel(m);
        const title = m.model || m.name;
        return (
          <div key={m.id} className={`min-h-[60px] grid grid-cols-[24px_32px_minmax(0,1fr)_auto] items-center gap-3 px-4 py-3 border-b last:border-b-0 ${isDark ? 'border-white/[0.10]' : 'border-black/[0.12]'}`}>
            <button onClick={() => !isActive && onSetActiveModel(m.id)} className="shrink-0" title={t.setActiveModel}>
              <RadioDot active={isActive} />
            </button>
            <ProviderIcon preset={m.preset || (isLocal ? 'local_vllm' : 'openai_compatible')} isDark={isDark} compact />
            <div className="min-w-0">
              <div className="flex items-center gap-2 min-w-0">
                <span className={`text-[15px] leading-5 font-normal truncate ${isDark ? 'text-[#F2F2F7]' : 'text-[#1C1C1E]'}`}>{title}</span>
                {isLocal && <Tag tone="gray">本地模型</Tag>}
                {isBuiltinLlmApiModel(m) && <Tag tone="gray">内置模型</Tag>}
                {isActive && <Tag>默认</Tag>}
              </div>
              {isBuiltinLlmApiModel(m) && (
                <div className="mt-2 flex items-center gap-2 flex-wrap">
                  <span className={`text-[12px] ${isDark ? 'text-[#98989D]' : 'text-[#8A8A8E]'}`}>{t.builtinModelSelect}</span>
                  <select
                    value={builtinDefaultModel || m.model || ''}
                    disabled={llmApiModelBusy || builtinAvailableModels.length === 0}
                    onChange={e => setBuiltinDefaultModel(e.target.value)}
                    className={`min-w-[220px] max-w-full rounded-lg border px-3 py-1.5 text-[12px] outline-none ${isDark ? 'bg-[#1E1F20] border-white/[0.10] text-[#F2F2F7]' : 'bg-white border-black/[0.12] text-[#1C1C1E]'}`}
                  >
                    {builtinAvailableModels.length === 0 ? (
                      <option value={m.model || ''}>{t.builtinModelEmpty}</option>
                    ) : builtinAvailableModels.map(model => <option key={model} value={model}>{model}</option>)}
                  </select>
                  <button onClick={refreshBuiltinModels} disabled={llmApiModelBusy}
                    className={`min-h-8 px-3 rounded-full text-[14px] font-medium disabled:opacity-50 ${actionButton('blue')}`}>
                    {llmApiModelBusy ? t.builtinModelLoading : t.builtinModelRefresh}
                  </button>
                </div>
              )}
            </div>
            <div className="shrink-0 flex items-center gap-2">
              {!isReadonly && <button onClick={() => setEditingModel({ ...m, __scope: isLocal ? 'local' : 'cloud' })} className={`min-h-8 px-3 rounded-full text-[14px] font-medium ${actionButton('blue')}`}>编辑</button>}
              {!isReadonly && models.length > 1 && !isLocal && <button onClick={() => setModelDeleteConfirm(m)} className={`min-h-8 px-3 rounded-full text-[14px] font-medium ${actionButton('red')}`}>删除</button>}
            </div>
          </div>
        );
      }) : <div className={`px-4 py-4 text-[14px] ${isDark ? 'text-[#98989D]' : 'text-[#8A8A8E]'}`}>暂无模型</div>;
      const petEnabled = !!(bs && bs.settings && bs.settings.pet && bs.settings.pet.enabled);
      const selectedPetId = (bs && typeof bs.selectedPet === 'string' && bs.selectedPet) || DEFAULT_PET_ID;
      const handlePetSelect = id => {
        if (!bridge.available || !bridge.settings.setSelectedPet) return Promise.resolve();
        return bridge.settings.setSelectedPet(id);
      };
      const renderGeneral = () => (
        <>
          <IOSSection title="外观">
            <IOSRow label="界面语言" desc="切换应用显示语言">
              <SSegmented isDark={isDark} value={language} onChange={v => { setLanguage(v); setRestartDialog('language'); }} options={[{ key: 'zh', label: '中文' }, { key: 'en', label: 'English' }, { key: 'ja', label: '日本語' }]} />
            </IOSRow>
            <IOSRow label="主题模式" desc="选择浅色或深色外观">
              <SSegmented isDark={isDark} value={activeTheme} onChange={setActiveTheme} options={[{ key: 'light', label: t.light }, { key: 'dark', label: t.dark }]} />
            </IOSRow>
          </IOSSection>
          <IOSSection title="通知">
            <IOSRow label="任务完成提醒" desc="任务完成后展示系统通知">
              <IOSSwitch checked={taskCompletedNotif} onChange={setTaskCompletedNotif} />
            </IOSRow>
          </IOSSection>
          <section className="mb-6">
            <div className={`px-3 mb-2 text-[12px] font-semibold ${isDark ? 'text-[#8E8E93]' : 'text-[#8A8A8E]'}`}>桌面助手</div>
            <div className={`overflow-hidden rounded-[18px] ${isDark ? 'bg-[#2C2C2E]' : 'bg-white'}`}>
              <div className={`w-full min-h-[58px] flex flex-wrap items-center gap-3 px-4 py-2.5 text-left border-b ${
                isDark ? 'border-white/[0.10] text-[#F2F2F7]' : 'border-black/[0.12] text-[#1C1C1E]'
              } ${petEnabled ? '' : 'last:border-b-0'}`}>
                <div className="flex-1 min-w-[120px]">
                  <div className="text-[15px] leading-5 font-normal whitespace-nowrap">桌伴公仔</div>
                  <div className={`mt-0.5 text-[13px] leading-5 ${isDark ? 'text-[#98989D]' : 'text-[#8A8A8E]'}`}>在桌面显示常驻小公仔</div>
                </div>
                <IOSSwitch checked={petEnabled} onChange={onPetEnabledChange} />
              </div>
              {petEnabled && (
                <div className={`px-4 pb-4 border-t ${isDark ? 'border-white/[0.10]' : 'border-black/[0.12]'}`}>
                  <PetSettingsSection
                    isDark={isDark}
                    enabled={petEnabled}
                    selectedPetId={selectedPetId}
                    onSelect={handlePetSelect}
                  />
                </div>
              )}
            </div>
          </section>
        </>
      );
      const renderModels = () => (
        <>
          <section className="mb-6">
            <SectionTitle>模型</SectionTitle>
            <Group>
              {renderModelRows(userModels)}
              <button data-testid="settings-model-add" onClick={() => setEditingModel(newModelDraft('deepseek'))}
                className={`w-full min-h-[52px] flex items-center justify-center gap-2 px-4 text-[16px] font-normal border-t ${isDark ? 'border-white/[0.10] text-[#0A84FF] hover:bg-white/[0.05]' : 'border-black/[0.12] text-[#007AFF] hover:bg-black/[0.035]'}`}>
                <Plus size={18} />
                <span>添加模型</span>
              </button>
            </Group>
            {modelEnvLocked.length > 0 && <div className={`px-3 mt-2 text-[12px] leading-relaxed ${isDark ? 'text-[#8E8E93]' : 'text-[#8A8A8E]'}`}>如果模型配置被环境变量管理，设置页会保留当前值，但修改可能需要到环境变量中完成。</div>}
          </section>
        </>
      );
      const renderSearch = () => (
        <>
          <section className="mb-6">
            <SectionTitle>搜索源列表</SectionTitle>
            <Group>
            {enabledSearchList.map(item => {
              return (
                <div key={item.key} className={`min-h-[60px] grid grid-cols-[24px_minmax(0,1fr)_auto] items-center gap-[14px] px-4 py-3 border-b last:border-b-0 ${isDark ? 'border-white/[0.10]' : 'border-black/[0.12]'}`}>
                  <button onClick={() => { setSearchProvider(item.key); setRestartDialog('search'); }} className="shrink-0" title="设为默认">
                    <RadioDot active={searchProvider === item.key} />
                  </button>
                  <div className="min-w-0">
                    <div className="flex items-center gap-2 min-w-0">
                      <span className={`text-[15px] leading-5 font-normal truncate ${isDark ? 'text-[#F2F2F7]' : 'text-[#1C1C1E]'}`}>{item.label}</span>
                      {item.key === searchProvider && <Tag>默认</Tag>}
                    </div>
                    <div className={`mt-0.5 text-[12px] leading-[17px] truncate ${isDark ? 'text-[#98989D]' : 'text-[#8A8A8E]'}`}>{item.desc}</div>
                  </div>
                  <div className="flex items-center gap-2">
                    {item.key !== 'bing' && <button onClick={() => { setPendingSearchProvider(null); setEditingSearch(item.key); }} className={`shrink-0 min-h-8 px-3 rounded-full text-[14px] font-medium ${actionButton('blue')}`}>编辑</button>}
                    {item.key !== 'bing' && <button onClick={() => setSearchDeleteConfirm(item)} className={`shrink-0 min-h-8 px-3 rounded-full text-[14px] font-medium ${actionButton('red')}`}>删除</button>}
                  </div>
                </div>
              );
            })}
            <button onClick={() => setSearchPickerOpen(true)}
              className={`w-full min-h-[52px] flex items-center justify-center gap-2 px-4 text-[16px] font-normal border-t ${isDark ? 'border-white/[0.10] text-[#0A84FF] hover:bg-white/[0.05]' : 'border-black/[0.12] text-[#007AFF] hover:bg-black/[0.035]'}`}>
              <Plus size={18} />
              <span>添加搜索源</span>
            </button>
            </Group>
          </section>
        </>
      );
      const renderMemoryList = (items, empty) => items.length ? items.map(item => {
        const text = item.text || item.content || '未命名记忆';
        return (
          <div key={`${item.kind}-${item.id}`} className={`min-h-[92px] flex items-start gap-4 px-4 py-3.5 border-b last:border-b-0 ${isDark ? 'border-white/[0.10]' : 'border-black/[0.12]'}`}>
            <div className="min-w-0 flex-1">
              <div className={`text-[15px] leading-6 whitespace-pre-wrap break-words line-clamp-3 ${isDark ? 'text-[#F2F2F7]' : 'text-[#1C1C1E]'}`}>{text}</div>
            </div>
            <button onClick={() => openMemoryItemViewer(item)} className={`shrink-0 mt-0.5 text-[14px] px-3 py-1.5 rounded-full ${actionButton('blue')}`}>查看</button>
          </div>
        );
      }) : <IOSRow label={empty} />;
      const renderMemory = () => (
        <>
          <IOSSection>
            <IOSRow label="启用记忆" desc="PINVOU 会记住称呼、偏好、工作背景和近期事项">
              <IOSSwitch checked={memoryEnabled} onChange={onMemoryEnabledChange} />
            </IOSRow>
          </IOSSection>
          {memoryEnabled && (
            <>
              <IOSSection title="个人资料">
                <IOSRow label="用户称呼" desc="助手称呼你的方式" value={identity.call_name || '未设置'} onClick={() => editProfile('call_name')}>
                  <ChevronDown size={22} className="-rotate-90 opacity-35" />
                </IOSRow>
                <IOSRow label="助手昵称" desc="你称呼助手的方式" value={identity.assistant_alias || 'PINVOU'} onClick={() => editProfile('assistant_alias')}>
                  <ChevronDown size={22} className="-rotate-90 opacity-35" />
                </IOSRow>
              </IOSSection>
              <IOSSection title="长期记忆">{renderMemoryList(longTermItems, '暂无长期记忆')}</IOSSection>
              <IOSSection title="短期记忆">{renderMemoryList(recentItems, '暂无短期记忆')}</IOSSection>
            </>
          )}
        </>
      );
      const renderData = () => (
        <IOSSection title="隐藏任务">
          {archivedSessions.length ? archivedSessions.map(s => (
            <IOSRow key={s.id} label={s.title || t.newChat} desc={`收纳于 ${formatSessionDate(s.archived_at || s.updated_at || s.created_at, language)}`}>
              <button onClick={() => onRestoreArchived && onRestoreArchived(s.id)} className={`shrink-0 text-[14px] px-3 py-1.5 rounded-full ${actionButton('blue')}`}>恢复</button>
              <button onClick={() => setArchivedDeleteConfirm(s)} className={`shrink-0 text-[14px] px-3 py-1.5 rounded-full ${actionButton('red')}`}>删除</button>
            </IOSRow>
          )) : <IOSRow label="暂无隐藏任务" desc="收纳后的任务会显示在这里" />}
        </IOSSection>
      );
      const renderUpdate = () => {
        const upd = bs && bs.updateInfo;
        const currentVersion = (bs && bs.appVersion) || (upd && upd.current_version) || '—';
        const notes = (upd && String(upd.notes || '').trim()) || '暂无更新说明';
        const updateChecking = !!(bs && bs.updateChecking);
        const updateDownloading = !!(bs && bs.updateDownloading);
        const updateCancelling = !!(bs && bs.updateCancelling);
        const updateReady = !!(bs && bs.updateReady);
        const updateProgress = (bs && bs.updateProgress) || 0;
        const isWindowsUpdate = upd && upd.platform === 'windows';
        const updateError = (bs && bs.updateError) || (bs && bs.updateCheckError && bs.updateCheckError !== 'latest' ? bs.updateCheckError : '');
        const updateStatusDesc = updateDownloading
          ? (updateProgress >= 100 ? '正在安装更新…' : `正在下载更新 ${updateProgress}%`)
          : updateReady
            ? (isWindowsUpdate ? '安装器已启动，应用将自动退出' : '升级完成，重启后生效')
            : (upd && upd.available ? `v${upd.latest_version}` : (bs && bs.updateCheckError === 'latest' ? '已是最新版本' : ''));
        const updateButtonLabel = updateChecking
          ? '检查中…'
          : updateDownloading
            ? (updateProgress >= 100 ? '安装中…' : (updateCancelling ? '取消中…' : '取消下载'))
            : updateReady
              ? (isWindowsUpdate ? '安装器已启动' : '立即重启')
              : (upd && upd.available ? (upd.platform === 'linux' ? '升级并重启' : '下载并安装') : '检查更新');
        const updateButtonDisabled = !bridge.available || updateChecking || updateCancelling || (updateDownloading && updateProgress >= 100) || (updateReady && isWindowsUpdate);
        const handleUpdateAction = () => {
          if (!bridge.available || updateChecking) return;
          if (updateDownloading) {
            if (updateProgress < 100 && !updateCancelling) bridge.updater.cancelUpdate();
            return;
          }
          if (updateReady) {
            if (!isWindowsUpdate) bridge.updater.restartApp();
            return;
          }
          if (upd && upd.available) bridge.updater.downloadAndInstallUpdate();
          else bridge.updater.checkForUpdate();
        };
        return (
          <div ref={versionUpdateRef} id="settings-version-update">
            <IOSSection title="版本">
              <IOSRow label="当前版本" desc="内测版" value={`v${currentVersion}`} />
              <IOSRow label={upd && upd.available ? '发现新版本' : '检查更新'} desc={updateStatusDesc}>
              <button data-settings-update-action="true" onClick={handleUpdateAction} disabled={updateButtonDisabled} className="h-9 px-4 rounded-full bg-[#007AFF] text-white text-[14px] font-semibold whitespace-nowrap disabled:opacity-50 disabled:cursor-not-allowed">{updateButtonLabel}</button>
            </IOSRow>
            </IOSSection>
            {updateError && (
              <div className="px-3 -mt-3 mb-4 text-[12px] leading-5 text-[#EA4335] break-words">{String(updateError)}</div>
            )}
            <section className="mb-6">
              <div className={`px-3 mb-2 text-[12px] font-semibold ${isDark ? 'text-[#8E8E93]' : 'text-[#8A8A8E]'}`}>更新内容</div>
              <div className={`rounded-[18px] px-4 py-3.5 text-[14px] leading-6 whitespace-pre-line ${isDark ? 'bg-[#2C2C2E] text-[#F2F2F7]' : 'bg-white text-[#1C1C1E]'}`}>{notes}</div>
            </section>
          </div>
        );
      };
      const renderPermissions = () => {
        const deps = (bs && bs.deps) || [];
        const checking = !!(bs && bs.depsChecking);
        const installing = !!(bs && bs.depsInstalling);
        const installError = bs && bs.depsInstallError;
        const missing = deps.filter(dep => !dep.installed);
        const checked = deps.length > 0;
        const busy = checking || installing;
        return (
          <>
            {showSuperPermissionSettings && (
              <IOSSection title="系统">
                <IOSRow label="高级执行权限" desc="允许助手执行环境配置等高级指令">
                  <IOSSwitch checked={!!superPerm} onChange={setSuperPerm} />
                </IOSRow>
              </IOSSection>
            )}
            <div id="settings-dependencies">
              <IOSSection
                title={t.depCheckTitle}
                footer={usesBundledDependencyInstaller ? t.depInstallNoteWindows : t.depInstallNote}
              >
                <IOSRow
                  label={checking ? t.depChecking : (!checked ? t.depCheckTitle : (missing.length ? `${missing.length}${t.depMissingSuffix}` : t.depAllOk))}
                  desc={installing ? t.depInstalling : (installError ? String(installError) : '')}
                >
                  <button
                    onClick={() => bridge.available && bridge.dependencies.checkDependencies()}
                    disabled={!bridge.available || busy}
                    className={`h-9 px-4 rounded-full text-[14px] font-semibold disabled:opacity-50 ${isDark ? 'bg-white/[0.08] text-[#0A84FF]' : 'bg-[#E5E5EA] text-[#007AFF]'}`}
                  >{checking ? t.depChecking : t.depRecheck}</button>
                </IOSRow>
                {missing.map(dep => (
                  <IOSRow key={dep.key} label={t[`dep_${dep.key}`] || dep.key} desc={dep.apt || ''}>
                    <Tag tone="gray">缺失</Tag>
                  </IOSRow>
                ))}
                {missing.length > 0 && (
                  <IOSRow label={usesBundledDependencyInstaller ? '安装缺失依赖' : t.depGoInstall}>
                    <button
                      onClick={() => bridge.available && bridge.dependencies.installDependencies()}
                      disabled={!bridge.available || busy}
                      className="h-9 px-4 rounded-full bg-[#007AFF] text-white text-[14px] font-semibold disabled:opacity-50"
                    >{installing ? t.depInstalling : t.depInstallBtn}</button>
                  </IOSRow>
                )}
              </IOSSection>
            </div>
          </>
        );
      };
      const renderHelp = () => (
        <IOSSection>
          <IOSRow label="提交问题或建议" desc="支持图片和视频附件，提交前会显示隐私提示">
            <button onClick={() => setFeedbackOpen(true)} className="h-9 px-4 rounded-full bg-[#007AFF] text-white text-[14px] font-semibold">提交反馈</button>
          </IOSRow>
        </IOSSection>
      );
      const renderContent = () => {
        if (activeSection === 'model') return renderModels();
        if (activeSection === 'search') return renderSearch();
        if (activeSection === 'memory') return renderMemory();
        if (activeSection === 'permissions') return renderPermissions();
        if (activeSection === 'data') return renderData();
        if (activeSection === 'update') return renderUpdate();
        if (activeSection === 'help') return renderHelp();
        return renderGeneral();
      };
      const sectionTitle = {
        general: '通用',
        model: '模型',
        search: '搜索',
        memory: '记忆',
        permissions: '权限与环境',
        data: '数据管理',
        update: '更新',
        help: '帮助反馈',
      }[activeSection] || '通用';
      const SearchSourceModal = ({ provider, isNew, onClose }) => {
        const option = searchOptions.find(x => x.key === provider);
        const [showSearchKey, setShowSearchKey] = useState(false);
        const [draftKey, setDraftKey] = useState('');
        const hasSavedKey = searchHasKey(provider);
        const canSaveSearch = (provider === 'bing' && isNew) || !!String(draftKey || '').trim();
        useEffect(() => {
          setDraftKey('');
          setShowSearchKey(false);
        }, [provider]);
        return (
          <div className="fixed inset-0 z-[100] flex items-center justify-center bg-black/50 animate-in fade-in duration-150" onClick={onClose}>
            <div onClick={e => e.stopPropagation()}
              className={`w-[430px] max-w-[90vw] max-h-[76vh] overflow-y-auto custom-scrollbar rounded-[22px] shadow-2xl ${isDark ? 'bg-[#1C1C1E] text-[#F2F2F7]' : 'bg-white text-[#1C1C1E]'}`}>
              <div className={`px-5 py-4 flex items-start justify-between gap-4 border-b ${isDark ? 'border-white/[0.10]' : 'border-black/[0.10]'}`}>
                <div>
                  <h2 className="text-[20px] leading-6 font-semibold">编辑搜索源</h2>
                  <p className={`mt-1 text-[13px] leading-[18px] ${isDark ? 'text-[#98989D]' : 'text-[#8A8A8E]'}`}>{option ? option.label : provider}</p>
                </div>
                <button onClick={onClose} className={`h-9 w-9 shrink-0 rounded-full flex items-center justify-center ${isDark ? 'bg-white/[0.08] text-[#C7C7CC]' : 'bg-[#E5E5EA] text-[#636366]'}`}><X size={18} /></button>
              </div>
              <div className="space-y-4 px-5 py-4">
                <section>
                  <div className={`overflow-hidden rounded-[16px] ${isDark ? 'bg-[#2C2C2E]' : 'bg-[#F2F2F7]'}`}>
                    <div className={`min-h-[54px] flex items-center gap-3 px-4 py-2.5 ${isDark ? 'text-[#F2F2F7]' : 'text-[#1C1C1E]'}`}>
                    <label className="shrink-0 text-[14px] leading-5">API Key</label>
                    <input type="text" value={draftKey} onChange={e => setDraftKey(e.target.value)}
                      autoFocus
                      placeholder={hasSavedKey ? '••••••••' : '输入 API Key'}
                      style={showSearchKey ? undefined : { WebkitTextSecurity: 'disc' }}
                      className={`min-w-0 flex-1 bg-transparent text-right text-[14px] leading-5 outline-none ${isDark ? 'placeholder:text-[#636366]' : 'placeholder:text-[#8A8A8E]'}`} />
                    <button type="button" onClick={() => setShowSearchKey(v => !v)} className="shrink-0 text-[14px] text-[#007AFF]">{showSearchKey ? '隐藏' : '显示'}</button>
                    </div>
                  </div>
                </section>
              </div>
              <div className={`flex justify-end gap-2 px-5 py-4 border-t ${isDark ? 'border-white/[0.10]' : 'border-black/[0.10]'}`}>
                <button onClick={onClose} className={`h-10 px-4 rounded-full text-[15px] font-normal transition-colors ${isDark ? 'text-[#0A84FF] hover:bg-white/[0.06]' : 'text-[#007AFF] hover:bg-black/[0.04]'}`}>取消</button>
                <button onClick={() => {
                  if (!canSaveSearch) return;
                  if (isNew) onAddSearchProvider && onAddSearchProvider(provider);
                  if (draftKey.trim()) setSearchApiKey(draftKey, provider);
                  onClose();
                  setRestartDialog('search');
                }} disabled={!canSaveSearch} className="h-10 px-5 rounded-full bg-[#007AFF] text-white text-[15px] font-semibold transition-colors disabled:opacity-35">保存</button>
              </div>
            </div>
          </div>
        );
      };
      const RestartDialog = ({ type }) => (
        <div className="fixed inset-0 z-[110] flex items-center justify-center bg-black/35 backdrop-blur-md px-4">
          <div className={`w-[340px] overflow-hidden rounded-[18px] shadow-2xl ${isDark ? 'bg-[#2C2C2E] text-[#F2F2F7]' : 'bg-white text-[#1C1C1E]'}`}>
            <div className="px-6 pt-6 pb-5 text-center">
              <h3 className="text-[18px] font-semibold">{type === 'search' ? '重启以应用搜索配置？' : '重启以应用语言设置？'}</h3>
              <p className={`mt-2 text-[14px] leading-5 ${isDark ? 'text-[#98989D]' : 'text-[#8A8A8E]'}`}>{type === 'search' ? '搜索源或凭据保存后，需要重启应用才能用于助手的联网搜索。' : '界面语言已切换，重启后助手回复语言也会同步生效。'}</p>
            </div>
            <div className={`grid grid-cols-2 border-t ${isDark ? 'border-white/[0.12]' : 'border-black/[0.12]'}`}>
              <button onClick={async () => {
                if (type === 'search' && onSaveSearchConfig) {
                  const saved = await onSaveSearchConfig();
                  if (saved === false) return;
                }
                setRestartDialog(null);
              }} className={`h-12 text-[17px] font-semibold border-r ${isDark ? 'border-white/[0.12] text-[#0A84FF]' : 'border-black/[0.12] text-[#007AFF]'}`}>稍后</button>
              <button onClick={() => { setRestartDialog(null); type === 'search' ? onConfirmSearchConfig() : (bridge.available && bridge.updater.restartApp()); }} className="h-12 text-[17px] font-semibold text-[#007AFF]">现在重启</button>
            </div>
          </div>
        </div>
      );
      const ModelDeleteDialog = ({ model }) => (
        <div className="fixed inset-0 z-[110] flex items-center justify-center bg-black/35 backdrop-blur-md px-4">
          <div className={`w-[270px] overflow-hidden rounded-[14px] shadow-2xl ${isDark ? 'bg-[#2C2C2E] text-[#F2F2F7]' : 'bg-white text-[#1C1C1E]'}`}>
            <div className="px-5 pt-5 pb-4 text-center">
              <h3 className="text-[17px] leading-6 font-semibold">删除模型？</h3>
              <p className={`mt-1 text-[13px] leading-[18px] ${isDark ? 'text-[#98989D]' : 'text-[#8A8A8E]'}`}>将移除该模型配置和已保存的凭据。</p>
            </div>
            <div className={`border-t ${isDark ? 'border-white/[0.12]' : 'border-black/[0.12]'}`}>
              <button onClick={() => { onDeleteModel(model); setModelDeleteConfirm(null); }} className={`w-full h-12 text-[17px] font-semibold text-[#FF3B30] border-b ${isDark ? 'border-white/[0.12]' : 'border-black/[0.12]'}`}>删除模型</button>
              <button onClick={() => setModelDeleteConfirm(null)} className="w-full h-12 text-[17px] font-semibold text-[#007AFF]">取消</button>
            </div>
          </div>
        </div>
      );
      const SearchDeleteDialog = ({ source }) => (
        <div className="fixed inset-0 z-[110] flex items-center justify-center bg-black/35 backdrop-blur-md px-4">
          <div className={`w-[270px] overflow-hidden rounded-[14px] shadow-2xl ${isDark ? 'bg-[#2C2C2E] text-[#F2F2F7]' : 'bg-white text-[#1C1C1E]'}`}>
            <div className="px-5 pt-5 pb-4 text-center">
              <h3 className="text-[17px] leading-6 font-semibold">删除搜索源？</h3>
              <p className={`mt-1 text-[13px] leading-[18px] ${isDark ? 'text-[#98989D]' : 'text-[#8A8A8E]'}`}>将移除 {source.label} 和已保存的凭据。</p>
            </div>
            <div className={`border-t ${isDark ? 'border-white/[0.12]' : 'border-black/[0.12]'}`}>
              <button onClick={() => { onDeleteSearchProvider && onDeleteSearchProvider(source.key); setSearchDeleteConfirm(null); setRestartDialog('search'); }} className={`w-full h-12 text-[17px] font-semibold text-[#FF3B30] border-b ${isDark ? 'border-white/[0.12]' : 'border-black/[0.12]'}`}>删除搜索源</button>
              <button onClick={() => setSearchDeleteConfirm(null)} className="w-full h-12 text-[17px] font-semibold text-[#007AFF]">取消</button>
            </div>
          </div>
        </div>
      );
      return (
        <div
          className="fixed inset-0 z-[80] flex items-center justify-center px-3 py-3 sm:px-5 sm:py-5 bg-black/45 backdrop-blur-[14px] animate-in fade-in duration-200"
          onClick={(event) => {
            if (event.target === event.currentTarget && onCloseSettings) {
              onCloseSettings();
            }
          }}
        >
          <div
            style={{ width: 'min(920px, calc(100vw - 24px))', height: 'min(620px, calc(100vh - 24px))' }}
            onClick={(event) => event.stopPropagation()}
            className={`relative flex overflow-hidden rounded-[24px] border shadow-[0_22px_58px_rgba(0,0,0,0.34)] ${isDark ? 'border-white/[0.14] bg-[#1C1C1E] text-[#F2F2F7]' : 'border-white/70 bg-[#F2F2F7] text-[#1C1C1E]'}`}
          >
            {onCloseSettings && (
              <button onClick={onCloseSettings} className={`absolute right-5 top-5 z-20 h-9 w-9 rounded-full flex items-center justify-center ${isDark ? 'bg-white/[0.08] text-[#C7C7CC]' : 'bg-[#E5E5EA] text-[#636366]'}`}>
                <X size={18} />
              </button>
            )}
            <aside
              style={{ width: 'clamp(150px, 24vw, 210px)' }}
              className={`shrink-0 overflow-y-auto custom-scrollbar border-r px-3 sm:px-4 py-5 sm:py-7 ${isDark ? 'border-white/[0.12]' : 'border-black/[0.12]'}`}
            >
              <div className={`mb-4 px-1 text-[12px] font-semibold ${isDark ? 'text-[#8E8E93]' : 'text-[#8A8A8E]'}`}>常用</div>
              <div className="space-y-2">
                <SectionButton id="general" icon={<Sparkles size={17} />} label="通用" />
                <SectionButton id="model" icon={<Cpu size={17} />} label="模型" />
                <SectionButton id="search" icon={<Search size={17} />} label="搜索" />
                {memorySettingsVisible && <SectionButton id="memory" icon={<Database size={17} />} label="记忆" />}
              </div>
              <div className={`mt-7 mb-4 px-1 text-[12px] font-semibold ${isDark ? 'text-[#8E8E93]' : 'text-[#8A8A8E]'}`}>系统</div>
              <div className="space-y-2">
                <SectionButton id="permissions" icon={<Wrench size={17} />} label="权限与环境" />
                <SectionButton id="data" icon={<Archive size={17} />} label="数据管理" />
                <SectionButton id="update" icon={<RefreshCw size={17} />} label="更新" dot={hasUpdate} />
                <SectionButton id="help" icon={<MessageSquare size={17} />} label="帮助反馈" />
              </div>
            </aside>
            <main className="flex-1 min-w-0 overflow-y-auto custom-scrollbar px-4 sm:px-6 md:px-8 py-5 sm:py-7">
              <div className="max-w-[680px]">
                <div className="mb-6 pr-12">
                  <h1 className="text-[24px] leading-tight font-semibold tracking-normal">{sectionTitle}</h1>
                </div>
                {renderContent()}
              </div>
            </main>
          </div>
          {editingModel && (
            <ModelFormModal isDark={isDark} t={t} initial={editingModel} bs={bs}
              onCancel={() => setEditingModel(null)}
              onSave={m => { onSaveModel(m); setEditingModel(null); }} />
          )}
          {modelDeleteConfirm && <ModelDeleteDialog model={modelDeleteConfirm} />}
          {searchDeleteConfirm && <SearchDeleteDialog source={searchDeleteConfirm} />}
          {searchPickerOpen && (
            <div className="fixed inset-0 z-[100] flex items-center justify-center bg-black/45 px-4 animate-in fade-in duration-150" onClick={() => setSearchPickerOpen(false)}>
              <div onClick={e => e.stopPropagation()}
                className={`w-[440px] max-w-[90vw] max-h-[76vh] overflow-y-auto custom-scrollbar rounded-[22px] shadow-2xl ${isDark ? 'bg-[#1C1C1E] text-[#F2F2F7]' : 'bg-white text-[#1C1C1E]'}`}>
                <div className={`px-5 py-4 flex items-start justify-between gap-4 border-b ${isDark ? 'border-white/[0.10]' : 'border-black/[0.10]'}`}>
                  <div>
                    <h2 className="text-[20px] leading-6 font-semibold">添加搜索源</h2>
                    <p className={`mt-1 text-[13px] leading-[18px] ${isDark ? 'text-[#98989D]' : 'text-[#8A8A8E]'}`}>选择搜索源后再填写必要凭据</p>
                  </div>
                  <button onClick={() => setSearchPickerOpen(false)} className={`h-9 w-9 shrink-0 rounded-full flex items-center justify-center ${isDark ? 'bg-white/[0.08] text-[#C7C7CC]' : 'bg-[#E5E5EA] text-[#636366]'}`}><X size={18} /></button>
                </div>
                <div className="px-5 py-4">
                  <div className={`overflow-hidden rounded-[16px] ${isDark ? 'bg-[#2C2C2E]' : 'bg-[#F2F2F7]'}`}>
                    {searchOptions.filter(item => !enabledSearchSet.has(item.key)).map(item => (
                      <button key={item.key} type="button" onClick={() => {
                          setSearchPickerOpen(false);
                          if (item.key !== 'bing') {
                            setPendingSearchProvider(item.key);
                            setEditingSearch(item.key);
                          } else {
                            onAddSearchProvider && onAddSearchProvider(item.key);
                            setRestartDialog('search');
                          }
                        }}
                        className={`w-full min-h-[56px] px-3.5 py-2.5 flex items-center gap-3 text-left border-b last:border-b-0 ${isDark ? 'border-white/[0.10] hover:bg-white/[0.06]' : 'border-black/[0.10] hover:bg-black/[0.035]'}`}>
                        <span className="min-w-0 flex-1">
                          <span className={`block text-[15px] leading-5 font-normal truncate ${isDark ? 'text-[#F2F2F7]' : 'text-[#1C1C1E]'}`}>{item.label}</span>
                          <span className={`block mt-0.5 text-[12px] leading-[17px] truncate ${isDark ? 'text-[#98989D]' : 'text-[#8A8A8E]'}`}>{item.desc}</span>
                        </span>
                        <ChevronDown size={16} className={`-rotate-90 shrink-0 ${isDark ? 'text-[#636366]' : 'text-[#C7C7CC]'}`} />
                      </button>
                    ))}
                  </div>
                </div>
              </div>
            </div>
          )}
          {editingSearch && <SearchSourceModal provider={editingSearch} isNew={pendingSearchProvider === editingSearch} onClose={() => { setEditingSearch(null); setPendingSearchProvider(null); }} />}
          {memoryEditor && (
            <div className="fixed inset-0 z-[100] flex items-center justify-center bg-black/45 px-4" onClick={() => setMemoryEditor(null)}>
              <div onClick={e => e.stopPropagation()} className={`w-full max-w-[500px] rounded-[24px] shadow-2xl ${isDark ? 'bg-[#1C1C1E] text-[#F2F2F7]' : 'bg-white text-[#1C1C1E]'}`}>
                <div className={`px-6 py-4 flex items-start justify-between border-b ${isDark ? 'border-white/[0.12]' : 'border-black/[0.12]'}`}>
                  <div>
                    <h2 className="text-[22px] leading-7 font-semibold">{memoryEditor.title}</h2>
                    <p className={`mt-1 text-[13px] leading-[18px] ${isDark ? 'text-[#98989D]' : 'text-[#8A8A8E]'}`}>{memoryEditor.subtitle}</p>
                  </div>
                  <button onClick={() => setMemoryEditor(null)} className={`h-10 w-10 rounded-full flex items-center justify-center ${isDark ? 'bg-white/[0.08]' : 'bg-[#E5E5EA]'}`}><X size={20} /></button>
                </div>
                <div className="px-6 py-5">
                  <label className="block">
                    <span className={`block px-1 mb-2 text-[13px] font-semibold ${isDark ? 'text-[#98989D]' : 'text-[#8A8A8E]'}`}>{memoryEditor.label}</span>
                    {memoryEditor.multiline ? (
                      <textarea
                        value={memoryEditor.value}
                        onChange={e => setMemoryEditor(prev => ({ ...prev, value: e.target.value }))}
                        rows={5}
                        className={`w-full rounded-[16px] px-4 py-3 text-[15px] leading-6 outline-none resize-none ${isDark ? 'bg-[#2C2C2E] text-[#F2F2F7] placeholder:text-[#636366]' : 'bg-[#F2F2F7] text-[#1C1C1E] placeholder:text-[#8A8A8E]'}`}
                      />
                    ) : (
                      <input
                        value={memoryEditor.value}
                        onChange={e => setMemoryEditor(prev => ({ ...prev, value: e.target.value }))}
                        className={`w-full rounded-[16px] px-4 py-3 text-[15px] outline-none ${isDark ? 'bg-[#2C2C2E] text-[#F2F2F7] placeholder:text-[#636366]' : 'bg-[#F2F2F7] text-[#1C1C1E] placeholder:text-[#8A8A8E]'}`}
                      />
                    )}
                  </label>
                  <div className="mt-6 flex justify-end gap-2.5">
                    <button onClick={() => setMemoryEditor(null)} className={`h-10 px-4 rounded-full text-[14px] font-semibold ${isDark ? 'bg-[#2C2C2E]' : 'bg-[#E5E5EA]'}`}>取消</button>
                    <button onClick={saveMemoryEditor} className="h-10 px-4 rounded-full bg-[#007AFF] text-white text-[14px] font-semibold">保存</button>
                  </div>
                </div>
              </div>
            </div>
          )}
          {restartDialog && <RestartDialog type={restartDialog} />}
          {archivedDeleteConfirm && createPortal(
            <ArchivedDeleteConfirmDialog
              theme={activeTheme}
              t={t}
              onCancel={() => setArchivedDeleteConfirm(null)}
              onConfirm={confirmArchivedDelete}
            />,
            document.body
          )}
          {feedbackOpen && (
            <div className="fixed inset-0 z-[100] flex items-center justify-center bg-black/45 px-4 animate-in fade-in duration-150" onClick={closeFeedback}>
              <div
                onClick={e => e.stopPropagation()}
                data-feedback-dialog="true"
                className={`w-[430px] max-w-[90vw] max-h-[76vh] overflow-y-auto rounded-[22px] shadow-2xl custom-scrollbar ${isDark ? 'bg-[#1C1C1E] text-[#F2F2F7]' : 'bg-white text-[#1C1C1E]'}`}
              >
                <div className={`px-5 py-4 flex items-start justify-between gap-4 border-b ${isDark ? 'border-white/[0.10]' : 'border-black/[0.10]'}`}>
                  <div className="min-w-0">
                    <h2 className="text-[20px] leading-6 font-semibold">{t.feedbackDialogTitle}</h2>
                    <p className={`mt-1 text-[13px] leading-[18px] ${isDark ? 'text-[#98989D]' : 'text-[#8A8A8E]'}`}>{t.feedbackDesc}</p>
                  </div>
                  <button onClick={closeFeedback} className={`h-9 w-9 shrink-0 rounded-full flex items-center justify-center ${isDark ? 'bg-white/[0.08] text-[#C7C7CC]' : 'bg-[#E5E5EA] text-[#636366]'}`}><X size={18} /></button>
                </div>
                <div className="space-y-4 px-5 py-4">
                  <section>
                    <div className={`overflow-hidden rounded-[16px] ${isDark ? 'bg-[#2C2C2E]' : 'bg-[#F2F2F7]'}`}>
                      <div className="min-h-[54px] flex items-center gap-3 px-4 py-2.5">
                        <label className={`shrink-0 text-[14px] leading-5 ${isDark ? 'text-[#F2F2F7]' : 'text-[#1C1C1E]'}`}>{t.feedbackType}</label>
                        <SSegmented isDark={isDark} value={feedbackDraft.type} onChange={type => setFeedbackDraft(prev => ({ ...prev, type }))} options={feedbackTypes} />
                      </div>
                    </div>
                  </section>
                  <section>
                    <div className={`overflow-hidden rounded-[16px] ${isDark ? 'bg-[#2C2C2E]' : 'bg-[#F2F2F7]'}`}>
                      <div className={`min-h-[54px] flex items-center gap-3 px-4 py-2.5 border-b ${isDark ? 'border-white/[0.10]' : 'border-black/[0.10]'}`}>
                        <label className="shrink-0 text-[14px] leading-5">{t.feedbackSubject}</label>
                        <input value={feedbackDraft.title} maxLength={120} onChange={e => setFeedbackDraft(prev => ({ ...prev, title: e.target.value }))}
                        placeholder={t.feedbackSubjectPh}
                        className={`min-w-0 flex-1 bg-transparent text-right text-[14px] leading-5 outline-none ${isDark ? 'placeholder:text-[#636366]' : 'placeholder:text-[#8A8A8E]'}`} />
                      </div>
                      <div className="px-4 py-3">
                        <div className="mb-2 text-[14px] leading-5">{t.feedbackBody}</div>
                        <textarea value={feedbackDraft.description} maxLength={5000} onChange={e => setFeedbackDraft(prev => ({ ...prev, description: e.target.value }))}
                        placeholder={t.feedbackBodyPh} rows={5}
                        className={`w-full resize-none bg-transparent text-[14px] leading-6 outline-none ${isDark ? 'placeholder:text-[#636366]' : 'placeholder:text-[#8A8A8E]'}`} />
                      </div>
                    </div>
                  </section>
                  <section>
                    <div className={`overflow-hidden rounded-[16px] ${isDark ? 'bg-[#2C2C2E]' : 'bg-[#F2F2F7]'}`}>
                      <div className={`min-h-[54px] flex items-center gap-3 px-4 py-2.5 border-b ${feedbackDraft.attachments.length > 0 ? (isDark ? 'border-white/[0.10]' : 'border-black/[0.10]') : 'border-transparent'}`}>
                        <div className="min-w-0 flex-1">
                          <div className="text-[14px] leading-5">{t.feedbackAttachments}</div>
                          <div className={`mt-0.5 text-[12px] leading-[17px] truncate ${isDark ? 'text-[#98989D]' : 'text-[#8A8A8E]'}`}>
                            {feedbackDraft.attachments.length > 0 ? `${feedbackDraft.attachments.length}/5` : t.feedbackNoAttachments}
                          </div>
                        </div>
                        <button onClick={pickFeedbackAttachments} className="shrink-0 text-[14px] text-[#007AFF]">{t.feedbackAddAttachment}</button>
                      </div>
                      {feedbackDraft.attachments.length > 0 && (
                        <div>
                        {feedbackDraft.attachments.map((a, idx) => (
                          <div key={`${a.path}-${idx}`} className={`min-h-[48px] flex items-center justify-between gap-3 px-4 py-2.5 border-b last:border-b-0 ${isDark ? 'border-white/[0.10]' : 'border-black/[0.10]'}`}>
                            <span className={`min-w-0 truncate text-[13px] ${isDark ? 'text-[#C7C7CC]' : 'text-[#636366]'}`}>{a.name}</span>
                            <button onClick={() => setFeedbackDraft(prev => ({ ...prev, attachments: prev.attachments.filter((_, i) => i !== idx) }))} className="shrink-0 text-[14px] text-[#FF3B30]">{t.cpDelete}</button>
                          </div>
                        ))}
                        </div>
                      )}
                    </div>
                    <div className={`px-1 mt-1.5 text-[12px] leading-4 ${isDark ? 'text-[#8E8E93]' : 'text-[#8A8A8E]'}`}>{t.feedbackAttachmentHint}</div>
                  </section>
                  <div className={`px-1 text-[12px] leading-5 ${isDark ? 'text-[#98989D]' : 'text-[#8A8A8E]'}`}>{t.feedbackPrivacy}</div>
                  {feedbackStatus.message && (
                    <div className={`rounded-[14px] px-4 py-3 text-[14px] ${feedbackStatus.state === 'submitted' ? 'bg-[#34C759]/15 text-[#248A3D]' : 'bg-[#FF3B30]/15 text-[#FF3B30]'}`}>
                      {feedbackStatus.message}
                    </div>
                  )}
                </div>
                <div className={`flex justify-end gap-2 px-5 py-4 border-t ${isDark ? 'border-white/[0.10]' : 'border-black/[0.10]'}`}>
                    <button onClick={closeFeedback} className={`h-10 px-4 rounded-full text-[15px] font-normal transition-colors ${isDark ? 'text-[#0A84FF] hover:bg-white/[0.06]' : 'text-[#007AFF] hover:bg-black/[0.04]'}`}>{t.cancel}</button>
                    {feedbackStatus.state === 'failed_retryable' && (
                      <button onClick={submitFeedbackDraft} className={`h-10 px-4 rounded-full text-[15px] font-normal transition-colors ${isDark ? 'text-[#0A84FF] hover:bg-white/[0.06]' : 'text-[#007AFF] hover:bg-black/[0.04]'}`}>{t.feedbackRetry}</button>
                    )}
                    <button onClick={submitFeedbackDraft} disabled={feedbackStatus.state === 'submitting'} className="h-10 px-5 rounded-full bg-[#007AFF] text-white text-[15px] font-semibold disabled:opacity-35">
                      {feedbackStatus.state === 'submitting' ? t.feedbackSubmitting : t.feedbackSubmit}
                    </button>
                </div>
              </div>
            </div>
          )}
          {feedbackNotice && (
            <div className="fixed left-1/2 bottom-8 z-[130] -translate-x-1/2 px-4 py-2.5 rounded-full bg-black/80 text-white text-[14px] shadow-xl backdrop-blur-md">
              {feedbackNotice}
            </div>
          )}
        </div>
      );
    };


    // ==========================================
    // Chat View (Gemini Centered Style + Messages)
    // ==========================================
    // 安装工具后新建会话弹出的介绍卡片（纯前端，不发 LLM query，点 chip 才发消息）

export { SCard, SRow, SField, SSegmented, SActionBar, MemorySettingsCard, MODEL_PRESET_DEFS, presetOptionsI18n, presetProviderLabel, ModelChip, ComposerModelSelector, RemoteControlModal, ScaledHtmlPreview, ComposerModeMenu, notifyComposerToolsChanged, ComposerToolMenu, ModelFormModal, SettingsView };
