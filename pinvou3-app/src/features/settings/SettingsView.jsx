import React, { useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { Archive, Briefcase, Check, ChevronDown, Cpu, Database, Edit2, FileText, Lightbulb, MessageSquare, MoreHorizontal, Paperclip, Plus, RefreshCw, Search, Smartphone, Sparkles, Store, Trash2, User, Video, Wrench, X, Zap } from '../../components/icons.jsx';
import { ArchivedDeleteConfirmDialog } from '../../components/layout/NavigationComponents.jsx';
import { VllmSetupProgress } from '../../components/VllmSetupProgress.jsx';
import { bridge } from '../../hooks/useBridge.js';
import { formatSessionDate } from '../../shared/date-utils.js';
import { visibleUserModels } from '../../shared/model-options.js';
import { buildComposerToolMenuState } from './composer-tool-menu-logic.js';
import { notifyComposerToolsChanged } from '../tools/tool-events.js';

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
      <div className={`p-1 rounded-full flex ${isDark ? 'bg-[#131314]' : 'bg-[#E1E5EA]'}`}>
        {options.map(o => (
          <button
            key={o.key}
            onClick={() => onChange(o.key)}
            className={`px-5 py-2 rounded-full text-[14px] font-medium transition-colors ${
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
        if (!bridge.available || !bridge.loadMemoryOverview) return;
        bridge.loadMemoryOverview();
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

      const reload = () => bridge.available && bridge.loadMemoryOverview && bridge.loadMemoryOverview();
      const saveProfile = async () => {
        if (!bridge.available || !bridge.saveMemoryProfilePatch) return;
        setSaving(true);
        try {
          await bridge.saveMemoryProfilePatch({
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
        if (!editing || !bridge.updateMemoryItem) return;
        setSaving(true);
        try {
          await bridge.updateMemoryItem(editing.kind, editing.id, {
            text: editing.text,
          });
          setEditing(null);
        } finally {
          setSaving(false);
        }
      };
      const deleteItem = async item => {
        setMenuFor(null);
        if (!item || !bridge.deleteMemoryItem) return;
        if (!window.confirm('删除后这条记忆不会再被使用，确定删除吗？')) return;
        await bridge.deleteMemoryItem(item.kind, item.id);
      };
      const archiveItem = async item => {
        setMenuFor(null);
        if (!item || !bridge.archiveRecentWorkMemory) return;
        await bridge.archiveRecentWorkMemory(item.id);
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
      kimi:        { baseUrl: 'https://api.moonshot.cn/v1',              model: 'kimi-k2.6' },
      openai_compatible: { baseUrl: 'https://api.openai.com/v1',        model: 'gpt-4o' },
      qwen:        { baseUrl: 'https://dashscope.aliyuncs.com/compatible-mode/v1', model: 'qwen-max' },
      doubao:      { baseUrl: 'https://ark.cn-beijing.volces.com/api/v3', model: 'doubao-pro-256k' },
      minimax:     { baseUrl: 'https://api.minimax.chat/v1',            model: 'abab6.5s-chat' },
      glm:         { baseUrl: 'https://open.bigmodel.cn/api/paas/v4',   model: 'glm-4-plus' },
      mimo:        { baseUrl: 'https://api.xiaomimimo.com/v1',          model: 'mimo-v2-flash' },
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
        if (bridge.available) bridge.switchModel(activeSessionId, id);
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
      function pick(id) { setOpen(false); if (id !== effectiveId && bridge.available) bridge.switchModel(activeSessionId, id); }
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
          bridge.startRemoteControl(null).catch(() => {});
        }
      }, []);

      async function handleRefreshRemoteControl() {
        if (!bridge.available) return;
        setActionBusy(true);
        try {
          await bridge.refreshRemoteControlQr(null);
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
          await bridge.stopRemoteControl();
          onClose();
        } finally {
          setActionBusy(false);
        }
      }

      async function handleRetryRemoteControl() {
        if (!bridge.available) return;
        setActionBusy(true);
        try { await bridge.startRemoteControl(null); }
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
          const list = await window.__TAURI__.core.invoke('list_marketplace_tools');
          if (isAlive()) setMarketplaceTools(Array.isArray(list) ? list : []);
        } catch (e) { /* ignore */ }
        try {
          const skills = await window.__TAURI__.core.invoke('list_marketplace_skills');
          if (isAlive()) setMarketplaceSkills(Array.isArray(skills) ? skills : []);
        } catch (e) { /* ignore */ }
        try {
          const dis = await window.__TAURI__.core.invoke('get_disabled_connectors');
          if (isAlive()) setDisabled(new Set(dis || []));
        } catch (e) { /* ignore */ }
        try {
          const fs = await window.__TAURI__.core.invoke('feishu_skills_state');
          if (isAlive()) { setFeishuOn(!!(fs && fs.connected)); setFeishuEnabled(!fs || fs.enabled !== false); }
        } catch (e) { /* ignore */ }
        try {
          const ws = await window.__TAURI__.core.invoke('wecom_skills_state');
          if (isAlive()) { setWecomOn(!!(ws && ws.connected)); setWecomEnabled(!ws || ws.enabled !== false); }
        } catch (e) { /* ignore */ }
        try {
          const ds = await window.__TAURI__.core.invoke('dingtalk_skills_state');
          if (isAlive()) { setDingtalkOn(!!(ds && ds.connected)); setDingtalkEnabled(!ds || ds.enabled !== false); }
        } catch (e) { /* ignore */ }
        try {
          const es = await window.__TAURI__.core.invoke('eip_status');
          if (isAlive()) setEipOn(!!(es && es.connected));
        } catch (e) { /* ignore */ }
        try {
          const zs = await window.__TAURI__.core.invoke('zhidao_status');
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
          window.__TAURI__.core.invoke('set_disabled_connectors',
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
      const [name, setName] = useState(initial.name || '');
      const [preset, setPreset] = useState(initial.preset || 'local_vllm');
      const [model, setModel] = useState(initial.model || '');
      const [baseUrl, setBaseUrl] = useState(initial.base_url || '');
      const [contextWindow, setContextWindow] = useState(initial.context_window_tokens ? String(initial.context_window_tokens) : '');
      const [maxOutput, setMaxOutput] = useState(initial.max_output_tokens ? String(initial.max_output_tokens) : '');
      const [apiKey, setApiKey] = useState('');
      const [keyAction, setKeyAction] = useState(initial.__new ? 'replace' : 'keep_existing');
      const [showKey, setShowKey] = useState(false);
      const [testing, setTesting] = useState(false);
      const [testResult, setTestResult] = useState(null);
      const [detecting, setDetecting] = useState(false);
      const [detectResult, setDetectResult] = useState(null); // { candidates } | { error } | null
      // 本机预装大模型「再入口」:检测无运行实例但有预装时,提示启用;走同一 bootstrap。
      const [offerSetup, setOfferSetup] = useState(false);   // 检测到预装,显示启用提示
      const [bootstrapHere, setBootstrapHere] = useState(false); // 从本页发起了 bootstrap(隔离全局态,避免开机引导的成功态串到这里)
      function applyPreset(p) {
        setPreset(p);
        const defs = MODEL_PRESET_DEFS[p] || MODEL_PRESET_DEFS.local_vllm;
        setBaseUrl(defs.baseUrl); setModel(defs.model);
        setContextWindow(p === 'local_vllm' ? '262144' : '');
        setMaxOutput(p === 'local_vllm' ? '24576' : '');
        if (p !== 'local_vllm') { setApiKey(''); setKeyAction(initial.__new ? 'replace' : 'keep_existing'); }
      }
      async function handleTest() {
        if (!bridge.available) return;
        setTesting(true); setTestResult(null);
        try { const msg = await bridge.testModelConnection(baseUrl.trim(), keyAction === 'replace' ? apiKey.trim() : '', initial.__new ? null : initial.id); setTestResult({ ok: true, msg: String(msg) }); }
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
          const result = await bridge.discoverLocalVllm({
            currentBaseUrl: baseUrl.trim() || null,
            savedBaseUrl: initial.base_url || null,
          });
          const online = ((result && result.candidates) || []).filter(c => c.status !== 'offline');
          setDetectResult({ candidates: online });
          if (online.length === 1) applyCandidate(online[0]); // 唯一可用实例直接填充
          else if (online.length === 0) {
            // 没探到运行中的实例:看本机是否有预装大模型,有则提示一键启用(走同一 bootstrap)。
            const setup = await bridge.detectLocalVllmSetup();
            if (setup && setup.has_packages && !setup.vllm_online) setOfferSetup(true);
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
      const canSave = !!(name.trim() && model.trim() && baseUrl.trim());
      function doSave() {
        if (!canSave) return;
        const id = initial.__new ? ('m_' + Date.now().toString(36) + Math.random().toString(36).slice(2, 7)) : initial.id;
        const contextTokens = Number.parseInt(contextWindow, 10);
        const outputTokens = Number.parseInt(maxOutput, 10);
        onSave({
          id: id, name: name.trim(), preset: preset,
          context_window_tokens: Number.isFinite(contextTokens) && contextTokens > 0 ? contextTokens : null,
          max_output_tokens: Number.isFinite(outputTokens) && outputTokens > 0 ? outputTokens : null,
          model: model.trim(), base_url: baseUrl.trim(),
          api_key: keyAction === 'replace' ? apiKey.trim() : '', credential_action: keyAction,
        });
      }
      const credentialState = initial.credential_state || (initial.has_secret ? 'configured' : 'missing');
      const hasSavedKey = !!initial.has_secret || credentialState === 'configured' || credentialState === 'env_override';
      const keyStatusText = credentialState === 'env_override' ? t.credEnvOverride
        : credentialState === 'unavailable' ? t.credUnavailable
        : hasSavedKey ? t.credConfigured
        : t.credNotConfigured;
      return (
        <div className="fixed inset-0 z-[100] flex items-center justify-center bg-black/50 animate-in fade-in duration-150" onClick={onCancel}>
          <div onClick={e => e.stopPropagation()}
            className={`w-[460px] max-w-[92vw] max-h-[88vh] overflow-y-auto rounded-[24px] p-6 shadow-2xl ${isDark ? 'bg-[#1E1F20] text-[#E3E3E3]' : 'bg-white text-[#1F1F1F]'}`}>
            <h2 className="text-[18px] font-medium mb-5">{initial.__new ? t.modelFormAddTitle : t.modelFormEditTitle}</h2>
            <div className="space-y-4">
              <SField isDark={isDark} label={t.modelDisplayName} type="text" value={name} onChange={e => setName(e.target.value)} />
              <div>
                <span className={`text-[14px] block mb-2 ${isDark ? 'text-[#C4C7C5]' : 'text-[#444746]'}`}>{t.modelPreset}</span>
                <div className="relative">
                  <select value={preset} onChange={e => applyPreset(e.target.value)}
                    className={`w-full appearance-none px-4 py-2 pr-10 rounded-lg text-[14px] outline-none ${isDark ? 'bg-[#131314] text-[#E3E3E3] border border-[#444746]' : 'bg-white text-[#1F1F1F] border border-[#C4C7C5]'}`}>
                    {presetOptionsI18n(t).map(o => <option key={o.key} value={o.key}>{o.label}</option>)}
                  </select>
                  <div className="absolute right-3 top-1/2 -translate-y-1/2 pointer-events-none"><ChevronDown size={16} className={isDark ? 'text-[#C4C7C5]' : 'text-[#444746]'} /></div>
                </div>
              </div>
              <SField isDark={isDark} label={t.customModelName} type="text" value={model} onChange={e => setModel(e.target.value)} />
              <SField isDark={isDark} label={t.customBaseUrl} type="text" value={baseUrl} onChange={e => setBaseUrl(e.target.value)} />
              <div className="grid grid-cols-2 gap-3">
                <SField isDark={isDark} label={t.modelContextWindow} type="number" min="1" step="1" value={contextWindow} onChange={e => setContextWindow(e.target.value)} />
                <SField isDark={isDark} label={t.modelMaxOutput} type="number" min="1" step="1" value={maxOutput} onChange={e => setMaxOutput(e.target.value)} />
              </div>
              <div>
                <span className={`text-[14px] block mb-2 ${isDark ? 'text-[#C4C7C5]' : 'text-[#444746]'}`}>{t.customApiKey}</span>
                <div className={`mb-2 text-[12px] ${isDark ? 'text-[#9AA0A6]' : 'text-[#5F6368]'}`}>{keyStatusText}</div>
                <div className="relative">
                  <input type={showKey ? 'text' : 'password'} value={apiKey} onChange={e => { setApiKey(e.target.value); if (e.target.value.trim()) setKeyAction('replace'); }}
                    placeholder={hasSavedKey ? t.credEnterNewKey : (preset === 'local_vllm' ? 'local-no-auth' : 'sk-...')}
                    className={`w-full px-4 py-2 pr-10 rounded-lg text-[14px] outline-none ${isDark ? 'bg-[#131314] text-[#E3E3E3] border border-[#444746] focus:border-[#A8C7FA]' : 'bg-white text-[#1F1F1F] border border-[#C4C7C5] focus:border-[#0B57D0]'}`} />
                  <button onClick={() => setShowKey(s => !s)} className="absolute right-2 top-1/2 -translate-y-1/2 text-[14px] opacity-70 px-1">{showKey ? '🙈' : '👁'}</button>
                </div>
              </div>
                {hasSavedKey && (
                  <div className="mt-2 flex items-center gap-2 flex-wrap">
                    <button onClick={() => { setKeyAction('keep_existing'); setApiKey(''); }}
                      className={`text-[12px] px-3 py-1.5 rounded-full ${keyAction === 'keep_existing' ? (isDark ? 'bg-[#174EA6] text-white' : 'bg-[#D2E3FC] text-[#174EA6]') : (isDark ? 'bg-[#2B2C2F] text-[#C4C7C5]' : 'bg-[#F0F4F9] text-[#444746]')}`}>{t.credKeep}</button>
                    <button onClick={() => setKeyAction('replace')}
                      className={`text-[12px] px-3 py-1.5 rounded-full ${keyAction === 'replace' ? (isDark ? 'bg-[#174EA6] text-white' : 'bg-[#D2E3FC] text-[#174EA6]') : (isDark ? 'bg-[#2B2C2F] text-[#C4C7C5]' : 'bg-[#F0F4F9] text-[#444746]')}`}>{t.credReplace}</button>
                    <button onClick={() => { setKeyAction('delete'); setApiKey(''); }}
                      className={`text-[12px] px-3 py-1.5 rounded-full ${keyAction === 'delete' ? (isDark ? 'bg-[#5F2120] text-[#F28B82]' : 'bg-[#FCE8E6] text-[#C5221F]') : (isDark ? 'bg-[#2B2C2F] text-[#F28B82]' : 'bg-[#F0F4F9] text-[#C5221F]')}`}>{t.credDeleteKey}</button>
                  </div>
                )}
              <div className="flex items-center gap-3 flex-wrap">
                <button onClick={handleTest} disabled={testing || !baseUrl.trim()}
                  className={`text-[13px] font-medium px-4 py-2 rounded-full transition-colors disabled:opacity-50 ${isDark ? 'bg-[#2B2C2F] text-[#E3E3E3] hover:bg-[#333537]' : 'bg-[#F0F4F9] text-[#1F1F1F] hover:bg-[#E8EAED]'}`}>
                  {testing ? t.testingConn : t.testConnection}
                </button>
                {preset === 'local_vllm' && (
                  <button onClick={handleDetect} disabled={detecting}
                    className={`text-[13px] font-medium px-4 py-2 rounded-full transition-colors disabled:opacity-50 ${isDark ? 'bg-[#2B2C2F] text-[#E3E3E3] hover:bg-[#333537]' : 'bg-[#F0F4F9] text-[#1F1F1F] hover:bg-[#E8EAED]'}`}>
                    {detecting ? t.detectingLocalVllm : t.detectLocalVllm}
                  </button>
                )}
                {testResult && (
                  <span className={`text-[12px] max-w-[280px] truncate ${testResult.ok ? (isDark ? 'text-[#93D5A6]' : 'text-[#137333]') : (isDark ? 'text-[#F28B82]' : 'text-[#C5221F]')}`}>{testResult.msg}</span>
                )}
              </div>
              {preset === 'local_vllm' && detectResult && (
                <div className={`rounded-xl border p-3 space-y-2 ${isDark ? 'border-[#333537] bg-[#131314]' : 'border-[#E0E3E7] bg-[#F8F9FB]'}`}>
                  {detectResult.error ? (
                    <span className={`text-[12px] ${isDark ? 'text-[#F28B82]' : 'text-[#C5221F]'}`}>{t.vllmDetectError(detectResult.error)}</span>
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
                          <button onClick={() => bridge.available && bridge.restartApp()}
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
                          <button onClick={() => bridge.bootstrapLocalVllm()}
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
                        <button onClick={() => { setBootstrapHere(true); bridge.bootstrapLocalVllm(); }}
                          className="h-8 px-4 rounded-lg text-[13px] font-medium text-white" style={{ background: '#0A84FF' }}>{t.vllmSetupEnable}</button>
                      </div>
                    </div>
                  )}
                </div>
              )}
            </div>
            <div className="flex justify-end gap-2 mt-6">
              <button onClick={onCancel} className={`text-[13px] font-medium px-4 py-2 rounded-full transition-colors ${isDark ? 'text-[#C4C7C5] hover:bg-[#333537]' : 'text-[#444746] hover:bg-[#F0F4F9]'}`}>{t.cpCancel}</button>
              <button onClick={doSave} disabled={!canSave}
                className={`text-[13px] font-medium px-5 py-2 rounded-full transition-colors disabled:opacity-50 ${isDark ? 'bg-[#A8C7FA] text-[#041E49] hover:bg-[#C2D7FB]' : 'bg-[#0B57D0] text-white hover:bg-[#1967D2]'}`}>{t.modelSaveBtn}</button>
            </div>
          </div>
        </div>
      );
    };

    const SettingsView = ({ activeTheme, setActiveTheme, language, setLanguage, superPerm, setSuperPerm, taskCompletedNotif, setTaskCompletedNotif, searchProvider, setSearchProvider, searchApiKey, setSearchApiKey, searchCredential, searchKeyAction, searchHasSavedKey, onKeepSearchApiKey, onReplaceSearchApiKey, onDeleteSearchApiKey, savedModels, activeModelId, onSaveModel, onDeleteModel, onSetActiveModel, onConfirmSearchConfig, onMemoryEnabledChange, searchNeedsRestart, languageNeedsRestart, bs, t, onRestoreArchived, onDeleteArchived, updateFocusTick }) => {
      const isDark = activeTheme === 'dark';
      const [editingModel, setEditingModel] = useState(null);
      const modelEnvLocked = (bs && bs.effectiveModelConfig && bs.effectiveModelConfig.env_overrides) || [];
      const [feedbackOpen, setFeedbackOpen] = useState(false);
      const [feedbackDraft, setFeedbackDraft] = useState({ type: 'issue', title: '', description: '', attachments: [] });
      const [feedbackStatus, setFeedbackStatus] = useState({ state: 'idle', message: '', receipt: null });
      const [archivedDeleteConfirm, setArchivedDeleteConfirm] = useState(null);
      const versionUpdateRef = useRef(null);
      const searchCredentialState = (searchCredential && searchCredential.credential_state) || (searchHasSavedKey ? 'configured' : 'missing');
      const searchKeyStatusText = searchCredentialState === 'env_override' ? t.credEnvOverride
        : searchCredentialState === 'unavailable' ? t.credUnavailable
        : searchHasSavedKey ? t.credConfigured
        : t.credNotConfigured;
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
        if (!bridge.available || !bridge.pickFeedbackFiles) {
          setFeedbackStatus({ state: 'failed_validation', message: t.feedbackPickUnavailable, receipt: null });
          return;
        }
        const paths = await bridge.pickFeedbackFiles();
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
          const receipt = await bridge.submitFeedback({
            type: feedbackDraft.type,
            title: feedbackDraft.title.trim() || null,
            description: feedbackDraft.description,
            entry_point: 'settings',
            error_summary: null,
            attachments: feedbackDraft.attachments,
            privacy_notice_version: '2026-06-24',
          });
          if (receipt && receipt.status === 'submitted') {
            window.alert((receipt && receipt.message) || t.feedbackSubmitted);
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
      // 进设置页自动体检一次可选依赖装齐没; 之后用户可手动「重新检测」
      useEffect(() => {
        if (!bridge.available || (bs && (bs.deps || bs.depsChecking))) return;
        let cancelled = false;
        const run = () => { if (!cancelled) bridge.checkDependencies(); };
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
      const presetOptions = [
        { key: 'local_vllm',  label: t.modelPresetLocalVllm },
        { key: 'deepseek',    label: t.modelPresetDeepseek },
        { key: 'kimi',        label: t.modelPresetKimi },
        { key: 'openai_compatible', label: t.modelPresetOpenaiCompatible },
        { key: 'qwen',        label: t.modelPresetQwen },
        { key: 'doubao',      label: t.modelPresetDoubao },
        { key: 'minimax',     label: t.modelPresetMinimax },
        { key: 'glm',         label: t.modelPresetGlm },
        { key: 'mimo',        label: t.modelPresetMimo },
      ];
      return (
        <div className="flex-1 flex flex-col w-full h-full relative z-10 animate-in fade-in duration-300">

          {/* 标题随内容一起滚动：固定标题 + 硬裁切的滚动边界在视觉上打架 */}
          <div className="flex-1 overflow-y-auto px-16 pb-20 custom-scrollbar">
            <div className="max-w-[800px]">
              <h1 className="text-[32px] font-normal tracking-tight pt-12 pb-8">{t.settings}</h1>
              <div className="space-y-8">

              <SCard isDark={isDark} title={t.archivedTasks}>
                <div className="space-y-3">
                  {t.archivedTasksDesc && <p className={`text-[14px] leading-relaxed ${isDark ? 'text-[#C4C7C5]' : 'text-[#444746]'}`}>{t.archivedTasksDesc}</p>}
                  {archivedSessions.length === 0 ? (
                    <div className={`rounded-xl px-4 py-3 text-[13px] ${isDark ? 'bg-[#131314] text-[#9AA0A6]' : 'bg-white text-[#5F6368]'}`}>{t.archivedEmpty}</div>
                  ) : (
                    <div className="max-h-[360px] overflow-y-auto custom-scrollbar pr-1 -mr-1 space-y-2">
                      {archivedSessions.map(s => (
                        <div key={s.id} className={`flex items-center gap-3 rounded-xl border px-4 py-3 ${isDark ? 'border-[#333537] bg-[#131314]' : 'border-[#E0E3E7] bg-white'}`}>
                          <Archive size={16} className={`shrink-0 ${isDark ? 'text-[#9AA0A6]' : 'text-[#5F6368]'}`} />
                          <div className="flex-1 min-w-0">
                            <div className={`text-[14px] font-medium truncate ${isDark ? 'text-[#E3E3E3]' : 'text-[#1F1F1F]'}`}>{s.title || t.newChat}</div>
                            <div className={`text-[12px] truncate ${isDark ? 'text-[#9AA0A6]' : 'text-[#5F6368]'}`}>收纳于 {formatSessionDate(s.archived_at || s.updated_at || s.created_at, language)}</div>
                          </div>
                          <button onClick={() => onRestoreArchived && onRestoreArchived(s.id)}
                            className={`shrink-0 text-[12px] px-3 py-1.5 rounded-full transition-colors ${isDark ? 'text-[#A8C7FA] hover:bg-[#333537]' : 'text-[#0B57D0] hover:bg-[#F0F4F9]'}`}>{t.restoreArchived}</button>
                          <button onClick={() => setArchivedDeleteConfirm(s)}
                            className={`shrink-0 text-[12px] px-3 py-1.5 rounded-full transition-colors ${isDark ? 'text-[#F28B82] hover:bg-[#333537]' : 'text-[#C5221F] hover:bg-[#FCE8E6]'}`}>{t.cpDelete}</button>
                        </div>
                      ))}
                    </div>
                  )}
                </div>
              </SCard>

              {archivedDeleteConfirm && createPortal(
                <ArchivedDeleteConfirmDialog
                  theme={activeTheme}
                  t={t}
                  onCancel={() => setArchivedDeleteConfirm(null)}
                  onConfirm={confirmArchivedDelete}
                />,
                document.body
              )}

              <SCard isDark={isDark} title={t.feedbackTitle}>
                <div className="flex items-start justify-between gap-4">
                  <div className="min-w-0">
                    <p className={`text-[14px] leading-relaxed ${isDark ? 'text-[#C4C7C5]' : 'text-[#444746]'}`}>{t.feedbackDesc}</p>
                  </div>
                  <button
                    onClick={() => { setFeedbackOpen(true); if (feedbackStatus.state === 'submitted') resetFeedback(); }}
                    className={`shrink-0 inline-flex items-center gap-2 text-[13px] font-medium px-4 py-2 rounded-full transition-colors ${isDark ? 'bg-[#A8C7FA] text-[#041E49] hover:bg-[#C2D7FB]' : 'bg-[#0B57D0] text-white hover:bg-[#1967D2]'}`}
                  >
                    <MessageSquare size={16} /> {t.feedbackOpen}
                  </button>
                </div>
              </SCard>

              {memorySettingsVisible && (
                <MemorySettingsCard
                  isDark={isDark}
                  bs={bs}
                  memoryEnabled={!!(bs && bs.settings && bs.settings.memory_enabled)}
                  onMemoryEnabledChange={onMemoryEnabledChange}
                />
              )}

              <SCard isDark={isDark} title={t.modelBackend}>
                <div className="space-y-4">
                  <p className={`text-[13px] ${isDark ? 'text-[#9AA0A6]' : 'text-[#5F6368]'}`}>{t.modelBackendDesc}</p>
                  {modelEnvLocked.length > 0 && (
                    <div className={`text-[12px] rounded-lg px-3 py-2 ${isDark ? 'bg-[#3A2E1A] text-[#F9D67A]' : 'bg-[#FEF7E0] text-[#B06000]'}`}>
                      {t.modelEnvLocked(modelEnvLocked.join(', '))}
                    </div>
                  )}
                  <div className="space-y-2">
                    {savedModels.map(m => {
                      const isActive = m.id === activeModelId;
                      return (
                        <div key={m.id} className={`flex items-center gap-3 rounded-xl border px-4 py-3 ${isDark ? 'border-[#333537] bg-[#131314]' : 'border-[#E0E3E7] bg-white'}`}>
                          <button onClick={() => { if (!isActive) onSetActiveModel(m.id); }} title={t.setActiveModel} className="shrink-0">
                            <span className={`block w-4 h-4 rounded-full border-2 ${isActive ? 'border-[#0B57D0] bg-[#0B57D0]' : (isDark ? 'border-[#5F6368]' : 'border-[#9AA0A6]')}`}>
                              {isActive && <span className="block w-1.5 h-1.5 mx-auto mt-[3px] rounded-full bg-white"></span>}
                            </span>
                          </button>
                          <div className="flex-1 min-w-0">
                            <div className="flex items-center gap-2">
                              <span className={`text-[14px] font-medium truncate ${isDark ? 'text-[#E3E3E3]' : 'text-[#1F1F1F]'}`}>{m.name}</span>
                              <span className={`shrink-0 text-[10px] px-1.5 py-0.5 rounded ${isDark ? 'bg-[#2A2B2D] text-[#9AA0A6]' : 'bg-[#F0F4F9] text-[#5F6368]'}`}>{presetProviderLabel(m.preset, t)}</span>
                              {isActive && <span className={`shrink-0 text-[10px] px-1.5 py-0.5 rounded ${isDark ? 'bg-[#1A3A1F] text-[#93D5A6]' : 'bg-[#E6F4EA] text-[#137333]'}`}>{t.modelActiveTag}</span>}
                            </div>
                            <div className={`text-[12px] truncate ${isDark ? 'text-[#9AA0A6]' : 'text-[#5F6368]'}`}>{m.model} · {m.base_url}</div>
                          </div>
                          <button onClick={() => setEditingModel(m)} title={t.editModel} className={`shrink-0 text-[12px] px-3 py-1.5 rounded-full transition-colors ${isDark ? 'text-[#A8C7FA] hover:bg-[#333537]' : 'text-[#0B57D0] hover:bg-[#F0F4F9]'}`}>{t.editModel}</button>
                          {savedModels.length > 1 && (
                            <button onClick={() => onDeleteModel(m)} title={t.deleteModelBtn} className={`shrink-0 text-[12px] px-3 py-1.5 rounded-full transition-colors ${isDark ? 'text-[#F28B82] hover:bg-[#333537]' : 'text-[#C5221F] hover:bg-[#FCE8E6]'}`}>{t.deleteModelBtn}</button>
                          )}
                        </div>
                      );
                    })}
                  </div>
                  <button onClick={() => setEditingModel({ __new: true, id: '', name: '', preset: 'local_vllm', context_window_tokens: 262144, max_output_tokens: 24576, model: MODEL_PRESET_DEFS.local_vllm.model, base_url: MODEL_PRESET_DEFS.local_vllm.baseUrl, api_key: '' })}
                    className={`text-[13px] font-medium px-4 py-2 rounded-full transition-colors ${isDark ? 'bg-[#2B2C2F] text-[#E3E3E3] hover:bg-[#333537]' : 'bg-[#F0F4F9] text-[#1F1F1F] hover:bg-[#E8EAED]'}`}>{t.addModel}</button>
                </div>
              </SCard>
              {editingModel && (
                <ModelFormModal isDark={isDark} t={t} initial={editingModel} bs={bs}
                  onCancel={() => setEditingModel(null)}
                  onSave={m => { onSaveModel(m); setEditingModel(null); }} />
              )}
              <SCard isDark={isDark} title={t.searchBackend}>
                <div className="space-y-6">
                  <SRow isDark={isDark} label={t.searchSource} desc={t.searchBackendDesc}>
                    <SSegmented isDark={isDark} value={searchProvider} onChange={setSearchProvider}
                      options={[{ key: 'bing', label: 'Bing' }, { key: 'metaso', label: 'Metaso' }, { key: 'bocha', label: 'Bocha' }, { key: 'baidu', label: 'Baidu' }, { key: 'tavily', label: 'Tavily' }]} />
                  </SRow>

                  {searchProvider === 'bing' ? (
                    <span className={`text-[13px] block ${isDark ? 'text-[#9AA0A6]' : 'text-[#5F6368]'}`}>
                      {t.searchKeyHintBing}
                    </span>
                  ) : (
                    <div>
                      <div className={`mb-2 text-[12px] ${isDark ? 'text-[#9AA0A6]' : 'text-[#5F6368]'}`}>{searchKeyStatusText}</div>
                      <SField isDark={isDark} label={t.searchKey} type="password"
                        value={searchApiKey} onChange={e => setSearchApiKey(e.target.value)}
                        placeholder={searchHasSavedKey ? t.credEnterNewKey : t.searchKeyPlaceholder} />
                      {searchHasSavedKey && (
                        <div className="mt-2 flex items-center gap-2 flex-wrap">
                          <button onClick={onKeepSearchApiKey}
                            className={`text-[12px] px-3 py-1.5 rounded-full ${searchKeyAction === 'keep_existing' ? (isDark ? 'bg-[#174EA6] text-white' : 'bg-[#D2E3FC] text-[#174EA6]') : (isDark ? 'bg-[#2B2C2F] text-[#C4C7C5]' : 'bg-[#F0F4F9] text-[#444746]')}`}>{t.credKeep}</button>
                          <button onClick={onReplaceSearchApiKey}
                            className={`text-[12px] px-3 py-1.5 rounded-full ${searchKeyAction === 'replace' ? (isDark ? 'bg-[#174EA6] text-white' : 'bg-[#D2E3FC] text-[#174EA6]') : (isDark ? 'bg-[#2B2C2F] text-[#C4C7C5]' : 'bg-[#F0F4F9] text-[#444746]')}`}>{t.credReplace}</button>
                          <button onClick={onDeleteSearchApiKey}
                            className={`text-[12px] px-3 py-1.5 rounded-full ${searchKeyAction === 'delete' ? (isDark ? 'bg-[#5F2120] text-[#F28B82]' : 'bg-[#FCE8E6] text-[#C5221F]') : (isDark ? 'bg-[#2B2C2F] text-[#F28B82]' : 'bg-[#F0F4F9] text-[#C5221F]')}`}>{t.credDeleteKey}</button>
                        </div>
                      )}
                      <span className={`text-[12px] mt-2 block ${isDark ? 'text-[#9AA0A6]' : 'text-[#5F6368]'}`}>
                        {searchProvider === 'metaso' ? t.searchKeyHintMetaso : searchProvider === 'baidu' ? t.searchKeyHintBaidu : searchProvider === 'tavily' ? t.searchKeyHintTavily : t.searchKeyHintBocha}
                      </span>

                      <div className={`mt-3 px-3 py-2 rounded-lg flex items-center justify-between gap-3 ${isDark ? 'bg-[#131314]' : 'bg-white'}`}>
                        <span className={`text-[12px] ${isDark ? 'text-[#9AA0A6]' : 'text-[#5F6368]'}`}>
                          {searchProvider === 'metaso' ? t.searchGetKeyMetasoSteps : searchProvider === 'baidu' ? t.searchGetKeyBaiduSteps : searchProvider === 'tavily' ? t.searchGetKeyTavilySteps : t.searchGetKeyBochaSteps}
                        </span>
                        <button
                          onClick={() => {
                            const url = searchProvider === 'metaso'
                              ? 'https://metaso.cn/search-api/api-keys'
                              : searchProvider === 'baidu'
                              ? 'https://console.bce.baidu.com/iam/#/iam/apikey/list'
                              : searchProvider === 'tavily'
                              ? 'https://app.tavily.com/'
                              : 'https://open.bochaai.com/';
                            if (bridge.available) bridge.openExternalUrl(url);
                          }}
                          className={`text-[12px] font-medium px-3 py-1 rounded-full whitespace-nowrap transition-colors ${
                            isDark ? 'bg-[#A8C7FA] text-[#041E49] hover:bg-[#C2D7FB]'
                                   : 'bg-[#0B57D0] text-white hover:bg-[#1967D2]'
                          }`}
                        >{t.searchGetKey} ↗</button>
                      </div>
                    </div>
                  )}

                  {searchNeedsRestart && (
                    <SActionBar isDark={isDark} message={t.searchRestartHint}
                      actionLabel={t.confirmAndRestart} onAction={onConfirmSearchConfig} />
                  )}
                </div>
              </SCard>

              <SCard isDark={isDark} title={t.sysPerm}>
                <SRow isDark={isDark} label={t.sudo} desc={t.sudoDesc}>
                  <button
                    onClick={setSuperPerm}
                    className={`w-[52px] h-[32px] rounded-full p-[2px] transition-colors duration-300 relative ${superPerm ? (isDark ? 'bg-[#A8C7FA]' : 'bg-[#0B57D0]') : (isDark ? 'bg-[#444746]' : 'bg-[#747775]')}`}
                  >
                    {/* off 态 thumb 暗色下用浅灰——原深蓝在深灰轨道上几乎不可见 */}
                    <div className={`w-[28px] h-[28px] rounded-full shadow-sm transform transition-transform duration-300 ${superPerm ? (isDark ? 'bg-[#041E49]' : 'bg-white') : (isDark ? 'bg-[#C4C7C5]' : 'bg-white')} ${superPerm ? 'translate-x-[20px]' : 'translate-x-0'}`} />
                  </button>
                </SRow>
              </SCard>

              <SCard isDark={isDark} title={t.notifications}>
                <SRow isDark={isDark} label={t.taskCompletedNotif} desc={t.taskCompletedNotifDesc}>
                  <button
                    onClick={() => setTaskCompletedNotif(!taskCompletedNotif)}
                    className={`w-[52px] h-[32px] rounded-full p-[2px] transition-colors duration-300 relative ${taskCompletedNotif ? (isDark ? 'bg-[#A8C7FA]' : 'bg-[#0B57D0]') : (isDark ? 'bg-[#444746]' : 'bg-[#747775]')}`}
                    aria-pressed={taskCompletedNotif}
                  >
                    <div className={`w-[28px] h-[28px] rounded-full shadow-sm transform transition-transform duration-300 ${taskCompletedNotif ? (isDark ? 'bg-[#041E49]' : 'bg-white') : (isDark ? 'bg-[#C4C7C5]' : 'bg-white')} ${taskCompletedNotif ? 'translate-x-[20px]' : 'translate-x-0'}`} />
                  </button>
                </SRow>
              </SCard>

              <SCard isDark={isDark} title={t.appearance}>
                <div className="space-y-8">
                  <SRow isDark={isDark} label={t.lang} desc={t.langDesc}>
                    <SSegmented isDark={isDark} value={language} onChange={setLanguage}
                      options={[{ key: 'zh', label: '中文' }, { key: 'en', label: 'English' }, { key: 'ja', label: '日本語' }]} />
                  </SRow>
                  {languageNeedsRestart && (
                    <SActionBar isDark={isDark} message={t.langRestartHint}
                      actionLabel={t.restartNow} onAction={() => bridge.available && bridge.restartApp()} />
                  )}
                  <SRow isDark={isDark} label={t.theme} desc={t.themeDesc}>
                    <SSegmented isDark={isDark} value={activeTheme} onChange={setActiveTheme}
                      options={[{ key: 'light', label: t.light }, { key: 'dark', label: t.dark }]} />
                  </SRow>
                </div>
              </SCard>

              {/* 版本与更新: Linux 检查→下载 deb→安装→重启;Windows 检查→下载 zip→启动 MSI→退出 */}
              <SCard
                ref={versionUpdateRef}
                id="settings-version-update"
                style={{ scrollMarginTop: '24px', scrollMarginBottom: '24px' }}
                isDark={isDark}
                title={t.versionUpdate}
                titleAdornment={hasUpdate ? <span className="w-2 h-2 rounded-full bg-[#EA4335]" /> : null}
              >
                {(() => {
                  const upd = bs && bs.updateInfo;
                  const checking = !!(bs && bs.updateChecking);
                  const downloading = !!(bs && bs.updateDownloading);
                  const ready = !!(bs && bs.updateReady);
                  const progress = (bs && bs.updateProgress) || 0;
                  const subText = isDark ? 'text-[#C4C7C5]' : 'text-[#444746]';
                  const isWindowsUpdate = !!(upd && upd.platform === 'windows');
                  const currentVersion = (bs && bs.appVersion) || (upd && upd.current_version) || '—';
                  return (
                    <div>
                      <div className="flex items-center justify-between">
                        <div className="pr-8">
                          <span className="text-[16px] block mb-1">
                            {t.curVer} v{currentVersion} (内测版)
                          </span>
                          <span className={`text-[14px] leading-relaxed block ${subText}`}>
                            {checking ? t.checking
                              : (bs && bs.updateCheckError === 'latest') ? t.upToDate
                              : (bs && bs.updateCheckError) ? (t.updateCheckFailed + ': ' + bs.updateCheckError)
                              : (upd && upd.available) ? (t.newVersionFound + ': v' + upd.latest_version)
                              : ''}
                          </span>
                        </div>
                        <button
                          onClick={() => bridge.available && !checking && bridge.checkForUpdate()}
                          disabled={checking}
                          className={`text-[12px] font-medium px-4 py-2 rounded-full whitespace-nowrap transition-colors ${
                            isDark ? 'bg-[#333537] text-[#E3E3E3] hover:bg-[#444746]'
                                   : 'bg-[#E1E5EA] text-[#1F1F1F] hover:bg-[#D3D9E0]'
                          } ${checking ? 'opacity-50' : ''}`}
                        >{t.checkUpdate}</button>
                      </div>

                      {upd && upd.available && (
                        <div className={`mt-5 pt-5 border-t ${isDark ? 'border-[#333537]' : 'border-[#DDE3EA]'}`}>
                          {upd.notes && (
                            <div className="mb-4">
                              <span className={`text-[12px] block mb-1 ${subText}`}>{t.updateNotes}</span>
                              <span className="text-[14px] leading-relaxed whitespace-pre-wrap block">{upd.notes}</span>
                            </div>
                          )}

                          {ready ? (
                            <div className="flex items-center justify-between">
                              <span className="text-[14px]">✅ {isWindowsUpdate ? t.updateInstallerStarted : t.updateComplete}</span>
                              {!isWindowsUpdate && (
                                <button
                                  onClick={() => bridge.available && bridge.restartApp()}
                                  className={`text-[12px] font-medium px-4 py-2 rounded-full whitespace-nowrap transition-colors ${
                                    isDark ? 'bg-[#A8C7FA] text-[#041E49] hover:bg-[#C2D7FB]'
                                           : 'bg-[#0B57D0] text-white hover:bg-[#1967D2]'
                                  }`}
                                >{t.restartNow}</button>
                              )}
                            </div>
                          ) : downloading ? (
                            <div>
                              <div className="flex items-center justify-between mb-2">
                                <span className={`text-[12px] ${subText}`}>{progress >= 100 ? t.installing : t.downloading}</span>
                                <span className={`text-[12px] ${subText}`}>{progress}%</span>
                              </div>
                              <div className={`h-[6px] rounded-full overflow-hidden ${isDark ? 'bg-[#333537]' : 'bg-[#DDE3EA]'}`}>
                                <div className="h-full rounded-full transition-all duration-200" style={{ width: progress + '%', backgroundColor: '#0B57D0' }} />
                              </div>
                              {/* 取消仅下载阶段可用; 进 install(pkexec/apt)后系统接管不可中断 */}
                              {progress < 100 && (
                                <div className="flex justify-end mt-3">
                                  <button
                                    onClick={() => bridge.available && bridge.cancelUpdate()}
                                    disabled={!!(bs && bs.updateCancelling)}
                                    className={`text-[12px] font-medium px-4 py-2 rounded-full whitespace-nowrap transition-colors ${
                                      isDark ? 'bg-[#333537] text-[#E3E3E3] hover:bg-[#444746]'
                                             : 'bg-[#E1E5EA] text-[#1F1F1F] hover:bg-[#D3D9E0]'
                                    } ${(bs && bs.updateCancelling) ? 'opacity-50' : ''}`}
                                  >{(bs && bs.updateCancelling) ? t.cancelling : t.cancel}</button>
                                </div>
                              )}
                            </div>
                          ) : (
                            <button
                              onClick={() => bridge.available && bridge.downloadAndInstallUpdate()}
                              className={`text-[12px] font-medium px-4 py-2 rounded-full whitespace-nowrap transition-colors ${
                                isDark ? 'bg-[#A8C7FA] text-[#041E49] hover:bg-[#C2D7FB]'
                                       : 'bg-[#0B57D0] text-white hover:bg-[#1967D2]'
                              }`}
                            >{(isWindowsUpdate ? t.downloadInstall : t.downloadInstallRestart)} (v{upd.latest_version})</button>
                          )}

                          {bs && bs.updateError && (
                            <span className="text-[13px] mt-3 block text-[#EA4335]">⚠️ {bs.updateError}</span>
                          )}
                        </div>
                      )}
                    </div>
                  );
                })()}
              </SCard>

              {/* 依赖体检: 进页自动检测,只报缺失项 + 一键(pkexec apt)安装,成功/失败提示 */}
              <SCard isDark={isDark} title={t.depCheckTitle}>
                {(() => {
                  const deps = (bs && bs.deps) || [];
                  const checking = !!(bs && bs.depsChecking);
                  const installing = !!(bs && bs.depsInstalling);
                  const installErr = bs && bs.depsInstallError;
                  const subText = isDark ? 'text-[#C4C7C5]' : 'text-[#444746]';
                  const checked = deps.length > 0;
                  const missing = deps.filter(d => !d.installed);
                  const busy = checking || installing;
                  const btnGhost = isDark ? 'bg-[#333537] text-[#E3E3E3] hover:bg-[#444746]' : 'bg-[#E1E5EA] text-[#1F1F1F] hover:bg-[#D3D9E0]';
                  const btnPrimary = isDark ? 'bg-[#A8C7FA] text-[#041E49] hover:bg-[#C2D7FB]' : 'bg-[#0B57D0] text-white hover:bg-[#1967D2]';
                  return (
                    <div>
                      <div className="flex items-center justify-between">
                        <span className={`text-[14px] leading-relaxed pr-4 ${subText}`}>
                          {checking ? t.depChecking
                            : installing ? t.depInstalling
                            : !checked ? t.depChecking
                            : missing.length === 0 ? ('✅ ' + t.depAllOk)
                            : ('⚠️ ' + missing.length + t.depMissingSuffix)}
                        </span>
                        <button
                          onClick={() => bridge.available && !busy && bridge.checkDependencies()}
                          disabled={busy}
                          className={`text-[12px] font-medium px-4 py-2 rounded-full whitespace-nowrap transition-colors ${btnGhost} ${busy ? 'opacity-50' : ''}`}
                        >{t.depRecheck}</button>
                      </div>

                      {checked && missing.length > 0 && (
                        <div className={`mt-4 pt-4 border-t ${isDark ? 'border-[#333537]' : 'border-[#DDE3EA]'}`}>
                          <div className="space-y-2 mb-4">
                            {missing.map(d => (
                              <div key={d.key} className="flex items-center justify-between text-[14px]">
                                <span>⚠️ {(t['dep_' + d.key] || d.key)}</span>
                                <span className={`text-[12px] ${subText}`}>{d.apt}</span>
                              </div>
                            ))}
                          </div>
                          <div className="flex items-center justify-between gap-3">
                            <span className={`text-[12px] ${subText}`}>{t.depInstallNote}</span>
                            <button
                              onClick={() => bridge.available && !busy && bridge.installDependencies()}
                              disabled={busy}
                              className={`text-[12px] font-medium px-4 py-2 rounded-full whitespace-nowrap transition-colors ${btnPrimary} ${busy ? 'opacity-50' : ''}`}
                            >{installing ? t.depInstalling : t.depInstallBtn}</button>
                          </div>
                          {installErr && <span className="text-[13px] mt-3 block text-[#EA4335]">❌ {installErr}</span>}
                        </div>
                      )}
                    </div>
                  );
                })()}
              </SCard>

              </div>
            </div>
          </div>
          {feedbackOpen && (
            <div className="fixed inset-0 z-[80] flex items-center justify-center p-4 bg-black/45">
              <div className={`w-full max-w-[680px] max-h-[88vh] overflow-y-auto rounded-[20px] shadow-2xl ${isDark ? 'bg-[#1E1F20] text-[#E3E3E3]' : 'bg-white text-[#1F1F1F]'}`}>
                <div className={`sticky top-0 z-10 flex items-center justify-between gap-3 px-6 py-4 border-b ${isDark ? 'bg-[#1E1F20] border-[#333537]' : 'bg-white border-[#E0E3E7]'}`}>
                  <h2 className="text-[18px] font-medium">{t.feedbackDialogTitle}</h2>
                  <button onClick={closeFeedback} className={`w-9 h-9 rounded-full flex items-center justify-center ${isDark ? 'hover:bg-[#333537]' : 'hover:bg-[#F0F4F9]'}`}><X size={18} /></button>
                </div>
                <div className="p-6 space-y-5">
                  <div>
                    <label className="block text-[13px] font-medium mb-2">{t.feedbackType}</label>
                    <SSegmented isDark={isDark} value={feedbackDraft.type} onChange={type => setFeedbackDraft(prev => ({ ...prev, type }))} options={feedbackTypes} />
                  </div>
                  <div>
                    <label className="block text-[13px] font-medium mb-2">{t.feedbackSubject}</label>
                    <input value={feedbackDraft.title} maxLength={120} onChange={e => setFeedbackDraft(prev => ({ ...prev, title: e.target.value }))}
                      placeholder={t.feedbackSubjectPh}
                      className={`w-full rounded-xl px-4 py-3 text-[14px] outline-none border ${isDark ? 'bg-[#131314] border-[#333537] text-[#E3E3E3]' : 'bg-[#F8F9FB] border-[#DDE3EA] text-[#1F1F1F]'}`} />
                  </div>
                  <div>
                    <label className="block text-[13px] font-medium mb-2">{t.feedbackBody}</label>
                    <textarea value={feedbackDraft.description} maxLength={5000} onChange={e => setFeedbackDraft(prev => ({ ...prev, description: e.target.value }))}
                      placeholder={t.feedbackBodyPh} rows={6}
                      className={`w-full resize-y rounded-xl px-4 py-3 text-[14px] leading-relaxed outline-none border ${isDark ? 'bg-[#131314] border-[#333537] text-[#E3E3E3]' : 'bg-[#F8F9FB] border-[#DDE3EA] text-[#1F1F1F]'}`} />
                  </div>
                  <div>
                    <div className="flex items-center justify-between gap-3 mb-2">
                      <label className="text-[13px] font-medium">{t.feedbackAttachments}</label>
                      <button onClick={pickFeedbackAttachments} className={`inline-flex items-center gap-2 text-[12px] font-medium px-3 py-2 rounded-full ${isDark ? 'bg-[#333537] hover:bg-[#444746]' : 'bg-[#E8EEF7] hover:bg-[#DCE6F5]'}`}>
                        <Paperclip size={15} /> {t.feedbackAddAttachment}
                      </button>
                    </div>
                    <p className={`text-[12px] mb-3 ${isDark ? 'text-[#9AA0A6]' : 'text-[#5F6368]'}`}>{t.feedbackAttachmentHint}</p>
                    {feedbackDraft.attachments.length === 0 ? (
                      <div className={`text-[13px] rounded-xl border px-4 py-3 ${isDark ? 'border-[#333537] text-[#9AA0A6]' : 'border-[#DDE3EA] text-[#5F6368]'}`}>{t.feedbackNoAttachments}</div>
                    ) : (
                      <div className="space-y-2">
                        {feedbackDraft.attachments.map((a, idx) => (
                          <div key={`${a.path}-${idx}`} className={`flex items-center justify-between gap-3 rounded-xl border px-3 py-2 ${isDark ? 'border-[#333537] bg-[#131314]' : 'border-[#DDE3EA] bg-[#F8F9FB]'}`}>
                            <div className="min-w-0 flex items-center gap-2">
                              {a.media_type === 'video' ? <Video size={16} /> : <FileText size={16} />}
                              <span className="truncate text-[13px]">{a.name}</span>
                            </div>
                            <button onClick={() => setFeedbackDraft(prev => ({ ...prev, attachments: prev.attachments.filter((_, i) => i !== idx) }))}
                              className={`w-8 h-8 rounded-full flex items-center justify-center ${isDark ? 'hover:bg-[#333537]' : 'hover:bg-[#E8EEF7]'}`}><Trash2 size={15} /></button>
                          </div>
                        ))}
                      </div>
                    )}
                  </div>
                  <div className={`rounded-xl px-4 py-3 text-[12px] leading-relaxed ${isDark ? 'bg-[#131314] text-[#C4C7C5]' : 'bg-[#F8F9FB] text-[#444746]'}`}>{t.feedbackPrivacy}</div>
                  {feedbackStatus.message && (
                    <div className={`rounded-xl px-4 py-3 text-[13px] ${feedbackStatus.state === 'submitted' ? (isDark ? 'bg-[#17351F] text-[#93D5A6]' : 'bg-[#E6F4EA] text-[#137333]') : (isDark ? 'bg-[#3A1E1E] text-[#F28B82]' : 'bg-[#FCE8E6] text-[#C5221F]')}`}>
                      {feedbackStatus.message}
                    </div>
                  )}
                  <div className="flex items-center justify-end gap-2">
                    <button onClick={closeFeedback} className={`text-[13px] font-medium px-4 py-2 rounded-full ${isDark ? 'hover:bg-[#333537]' : 'hover:bg-[#F0F4F9]'}`}>{t.cancel}</button>
                    {feedbackStatus.state === 'failed_retryable' && (
                      <button onClick={submitFeedbackDraft} className={`text-[13px] font-medium px-4 py-2 rounded-full ${isDark ? 'bg-[#333537] hover:bg-[#444746]' : 'bg-[#E8EEF7] hover:bg-[#DCE6F5]'}`}>{t.feedbackRetry}</button>
                    )}
                    <button onClick={submitFeedbackDraft} disabled={feedbackStatus.state === 'submitting'}
                      className={`text-[13px] font-medium px-5 py-2 rounded-full disabled:opacity-50 ${isDark ? 'bg-[#A8C7FA] text-[#041E49] hover:bg-[#C2D7FB]' : 'bg-[#0B57D0] text-white hover:bg-[#1967D2]'}`}>
                      {feedbackStatus.state === 'submitting' ? t.feedbackSubmitting : t.feedbackSubmit}
                    </button>
                  </div>
                </div>
              </div>
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
