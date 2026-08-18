import React, { useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { Archive, Briefcase, Check, ChevronDown, Code, Cpu, Database, Edit2, FileText, Globe, Lightbulb, MessageSquare, MoreHorizontal, Paperclip, Plus, RefreshCw, Search, Sparkles, Store, Trash2, User, Users, Video, Wrench, X, Zap } from '../../components/icons.jsx';
import { ComposerPopover } from '../../components/ComposerPopover.jsx';
import { VllmSetupProgress } from '../../components/VllmSetupProgress.jsx';
import PetSettingsSection from '../pet/PetSettingsSection.jsx';
import { DEFAULT_PET_ID } from '../pet/pet-registry.js';
import { bridge, isLocalModel, useBridgeState } from '../../hooks/useBridge.js';
import { visibleUserModels } from '../../shared/model-options.js';
import { can, isWeb } from '../../shared/platform.js';
import { buildComposerToolMenuState } from './composer-tool-menu-logic.js';
import { notifyComposerToolsChanged } from '../tools/tool-events.js';
import qwenIcon from '../../brand-icons/qwen.svg';
import {
  MODEL_PRESET_DEFS, PROVIDER_KIND_CODING_PLAN, PROVIDER_KIND_OFFICIAL_API, PROVIDER_KIND_CUSTOM,
  MODEL_CATALOG_SECTIONS, MODEL_CATALOG, CLOUD_MODEL_PROVIDERS,
  BRAND_ICON_BY_PRESET, BRAND_ICON_BY_VENDOR,
  presetOptionsI18n, presetProviderLabel,
  normalizedProviderBaseUrl, findCloudProviderForModel, providerLabelForModel, isCodingPlanModel, catalogItemMatchesModel,
  groupModelsForSelector, selectorMainLabel, selectorSubLabel,
  reasoningEffortTiersForModel, reasoningEffortForModelSwitch, normalizeStoredReasoningEffort,
} from './model-catalog.js';
import { invokeTauri } from '../../platform/tauri/client.js';
import {
  artifactPreviewExternalUrlFromMessage,
  buildArtifactPreviewDocument,
} from '../artifacts/artifact-preview-navigation.js';
import { ProvidersSection } from './ProvidersSection.jsx';

function isReadonlyModel(model) {
  return !!(model && (model.readonly || model.system));
}

function visibleSortedModels(models) {
  return (models || [])
    .filter(model => model && model.id)
    .slice();
}

const SCard = React.forwardRef(({ title, titleAdornment, children, id, style }, ref) => (
      <section ref={ref} id={id} style={style} className={`rounded-[24px] p-6 bg-[#F0F4F9] dark:bg-[#1E1F20]`}>
        <h2 className="text-[18px] font-medium mb-6 flex items-center gap-2">
          <span>{title}</span>
          {titleAdornment}
        </h2>
        {children}
      </section>
    ));

    const SRow = ({ label, desc, children }) => (
      <div className="flex items-center justify-between gap-8">
        <div className="min-w-0">
          <span className="text-[16px] block mb-1">{label}</span>
          {desc && <span className={`text-[13px] block text-[#444746] dark:text-[#C4C7C5]`}>{desc}</span>}
        </div>
        <div className="shrink-0">{children}</div>
      </div>
    );

    const SField = ({ label, ...inputProps }) => (
      <div>
        <span className={`text-[14px] block mb-2 text-[#444746] dark:text-[#C4C7C5]`}>{label}</span>
        <input
          {...inputProps}
          className={`w-full px-4 py-2 rounded-lg text-[14px] outline-none transition-colors bg-white text-[#1F1F1F] border border-[#C4C7C5] focus:border-[#0B57D0] dark:bg-[#131314] dark:text-[#E3E3E3] dark:border-[#444746] dark:focus:border-[#A8C7FA]`}
        />
      </div>
    );

    const SSegmented = ({ options, value, onChange }) => (
      <div data-testid="settings-segmented" className={`p-1 rounded-full flex flex-wrap justify-end gap-1 max-w-full max-sm:w-full max-sm:flex-nowrap bg-[#E1E5EA] dark:bg-[#131314]`}>
        {options.map(o => (
          <button
            key={o.key}
            onClick={() => onChange(o.key)}
            className={`min-w-[72px] px-4 py-2 rounded-full text-[14px] font-medium transition-colors max-sm:min-w-0 max-sm:flex-1 max-sm:px-2 ${
              value === o.key ? ('bg-white text-[#0B57D0] shadow-sm dark:bg-[#A8C7FA] dark:text-[#041E49]') : ''
            }`}
          >{o.label}</button>
        ))}
      </div>
    );

    // 「需重启」统一表达：改动后才出现，一句说明 + 一个动作，替代常驻大按钮和斜体小字
    const SActionBar = ({ message, actionLabel, onAction }) => (
      <div className={`flex items-center justify-between gap-4 px-4 py-3 rounded-xl bg-white dark:bg-[#131314]`}>
        <span className={`text-[13px] text-[#444746] dark:text-[#C4C7C5]`}>{message}</span>
        <button
          onClick={onAction}
          className={`text-[13px] font-medium px-4 py-2 rounded-full whitespace-nowrap transition-colors bg-[#0B57D0] text-white hover:bg-[#1967D2] dark:bg-[#A8C7FA] dark:text-[#041E49] dark:hover:bg-[#C2D7FB]`}
        >{actionLabel}</button>
      </div>
    );

    const MemorySettingsCard = ({ bs, memoryEnabled, onMemoryEnabledChange, t }) => {
      const copy = t.uiSettingsView;
      const detailCopy = t.uiSettingsDetail;
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
      const subText = 'text-[#444746] dark:text-[#C4C7C5]';
      const faintText = 'text-[#6B7280] dark:text-[#8F969E]';
      const border = 'border-[#DDE3EA] dark:border-[#333537]';
      const itemBg = 'bg-white dark:bg-[#131314]';
      const cardBg = 'bg-white border-[#DDE3EA] dark:bg-[#17191D] dark:border-white/[0.08]';
      const panelBg = 'bg-[#F8FAFD] text-[#1F1F1F] dark:bg-[#1F2023] dark:text-[#E8EAED]';
      const inputBg = 'bg-white border-[#DDE3EA] text-[#1F1F1F] placeholder:text-[#8A9099] dark:bg-[#131314] dark:border-[#3C4043] dark:text-[#E8EAED] dark:placeholder:text-[#777D86]';
      const ghostBtn = 'bg-[#E1E5EA] text-[#1F1F1F] hover:bg-[#D3D9E0] dark:bg-white/[0.07] dark:text-[#E3E3E3] dark:hover:bg-white/[0.11]';
      const dangerBtn = 'text-[#C5221F] hover:bg-[#FCE8E6] dark:text-[#F28B82] dark:hover:bg-[#3A2425]';
      const primaryBtn = 'bg-[#0B57D0] text-white hover:bg-[#1967D2] dark:bg-[#A8C7FA] dark:text-[#041E49] dark:hover:bg-[#C2D7FB]';
      const selectedTab = 'bg-[#E8F0FE] border-[#B8D1FF] text-[#0B57D0] dark:bg-[rgba(43,119,255,0.16)] dark:border-[rgba(70,145,255,0.35)] dark:text-[#D8E8FF]';
      const profileCount = (identity.call_name ? 1 : 0) + (identity.assistant_alias ? 1 : 0);
      const profileSummary = [
        identity.call_name ? copy.profileCallName(identity.call_name) : '',
        identity.assistant_alias ? copy.profileAssistantAlias(identity.assistant_alias) : '',
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
        { key: 'long_term', label: detailCopy.longMemory, count: longTermCount, icon: Database },
        { key: 'recent', label: copy.memoryTabRecent, count: recentCount, icon: RefreshCw },
      ];
      const tabMeta = tabs.find(x => x.key === tab) || tabs[0];
      const memoryTypeLabel = kind => kind === 'current_focus' ? detailCopy.memoryTypes.current_focus
        : kind === 'recent_activity' ? detailCopy.memoryTypes.recent_activity
        : kind === 'work_context' ? detailCopy.memoryTypes.work_context
        : detailCopy.memoryTypes.preference;
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
        if (!window.confirm(copy.memoryDeleteConfirm)) return;
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
        if (!raw) return copy.memoryTimeSaved;
        const date = new Date(raw);
        if (Number.isNaN(date.getTime())) return copy.memoryTimeSaved;
        const diff = Date.now() - date.getTime();
        const day = 24 * 60 * 60 * 1000;
        if (diff >= 0 && diff < day) return copy.memoryTimeToday;
        if (diff >= day && diff < 7 * day) return copy.memoryTimeDaysAgo(Math.floor(diff / day));
        return copy.memoryTimeDate(date.getMonth() + 1, date.getDate());
      };
      const confidenceText = item => {
        const n = Number(item.confidence);
        if (!Number.isFinite(n)) return copy.memoryConfidenceAuto;
        if (n >= 0.85) return copy.memoryConfidenceHigh;
        if (n >= 0.65) return copy.memoryConfidenceMid;
        return copy.memoryConfidenceLow;
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
                  {copy.memorySource} · {confidenceText(item)}
                </div>
              </div>
              <button
                title={copy.memoryMoreActions}
                onClick={(e) => {
                  e.stopPropagation();
                  setMenuFor(menuFor === rowKey ? null : rowKey);
                }}
                className={`shrink-0 w-8 h-8 rounded-full flex items-center justify-center transition-colors text-[#5F6368] hover:bg-black/[0.06] dark:text-[#AEB4BC] dark:hover:bg-white/[0.08] dark:hover:text-[#F2F3F5]`}
              >
                <MoreHorizontal size={17} />
              </button>
            </div>
            {menuFor === rowKey && (
              <div onClick={(e) => e.stopPropagation()} className={`absolute right-4 top-12 z-10 min-w-[118px] rounded-xl border ${border} bg-white text-[#1F1F1F] dark:bg-[#24262B] dark:text-[#E8EAED] shadow-2xl overflow-hidden`}>
                <button onClick={() => startEdit(item)} className={`w-full flex items-center gap-2 px-3 py-2 text-left text-[13px] hover:bg-black/[0.04] dark:hover:bg-white/[0.07]`}><Edit2 size={14} />{detailCopy.edit}</button>
                {(item.kind === 'current_focus' || item.kind === 'recent_activity') && (
                  <button onClick={() => archiveItem(item)} className={`w-full flex items-center gap-2 px-3 py-2 text-left text-[13px] hover:bg-black/[0.04] dark:hover:bg-white/[0.07]`}><Archive size={14} />{copy.memoryArchive}</button>
                )}
                <button onClick={() => deleteItem(item)} className={`w-full flex items-center gap-2 px-3 py-2 text-left text-[13px] ${dangerBtn}`}><Trash2 size={14} />{detailCopy.delete}</button>
              </div>
            )}
          </div>
        );
      };

      return (
        <>
          <SCard title={copy.memoryCardTitle}>
            <div className="flex items-center justify-between gap-4">
              <div className="min-w-0">
                <div className={`text-[14px] font-medium text-[#1F1F1F] dark:text-[#E8EAED]`}>
                  {memoryEnabled ? copy.memoryEnabled : copy.memoryDisabled}
                </div>
                <div className={`mt-1 text-[13px] leading-relaxed ${subText}`}>
                  {memoryEnabled
                    ? (memory.loading ? copy.memoryLoading : (profileSummary ? copy.memorySummaryWithProfile(profileSummary, total) : copy.memorySummary(total)))
                    : copy.memoryOffDesc}
                </div>
                {memory.error && <div className="mt-2 text-[13px] text-[#EA4335]">{memory.error}</div>}
              </div>
              <div className="shrink-0 flex items-center gap-2">
                <button
                  onClick={() => onMemoryEnabledChange && onMemoryEnabledChange(!memoryEnabled)}
                  role="switch"
                  aria-checked={!!memoryEnabled}
                  title={memoryEnabled ? copy.memoryTurnOff : copy.memoryTurnOn}
                  className={`w-12 h-7 rounded-full p-1 flex items-center transition-colors ${memoryEnabled ? 'justify-end bg-[#0B57D0]' : `justify-start bg-[#DADCE0] dark:bg-[#3C4043]`}`}
                >
                  <span className="block w-5 h-5 rounded-full bg-white shadow" />
                </button>
                {memoryEnabled && (
                  <button onClick={() => { setOpen(true); reload(); }} className={`text-[13px] font-medium px-4 py-2 rounded-full transition-colors ${primaryBtn}`}>
                    {copy.memoryViewManage}
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
                    <div className="text-[19px] font-semibold">{copy.memoryCenterTitle}</div>
                    <div className={`text-[12px] mt-1 ${subText}`}>{copy.memoryCenterDesc}</div>
                  </div>
                  <div className="flex items-center gap-2">
                    <button onClick={reload} disabled={!!memory.loading} className={`inline-flex items-center gap-1.5 text-[12px] px-3 py-1.5 rounded-full ${ghostBtn}`}><RefreshCw size={13} className={memory.loading ? 'animate-spin' : ''} />{memory.loading ? copy.memorySyncing : copy.memorySync}</button>
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
                          className={`w-full flex items-center gap-2 text-left px-3 py-2 rounded-full border text-[13px] transition-colors ${tab === key ? selectedTab : `border-transparent hover:bg-black/[0.04] dark:hover:bg-white/[0.06]`}`}
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
                      <div className={`mb-4 rounded-2xl border px-4 py-3 bg-white border-[#DDE3EA] dark:bg-white/[0.04] dark:border-white/[0.08]`}>
                        <div className={`text-[13px] leading-relaxed ${subText}`}>{copy.memoryOffNotice}</div>
                      </div>
                    )}
                    <div className="flex flex-col md:flex-row md:items-center justify-between gap-3 mb-5">
                      <div>
                        <div className="text-[16px] font-semibold">{tabMeta.label}</div>
                        <div className={`text-[12px] mt-1 ${faintText}`}>{tab === 'long_term' ? copy.memoryLongTermTabDesc : copy.memoryRecentTabDesc} · {copy.memoryItemCount(tabMeta.count)}</div>
                      </div>
                      <div className={`h-10 min-w-0 md:w-[260px] flex items-center gap-2 rounded-full border px-3 ${inputBg}`}>
                        <Search size={15} className={faintText} />
                        <input value={query} onChange={e => setQuery(e.target.value)} onClick={e => e.stopPropagation()} placeholder={copy.memorySearchPlaceholder} className="w-full bg-transparent outline-none text-[13px]" />
                      </div>
                    </div>

                    {tab === 'long_term' ? (
                      <div className="space-y-4">
                        <div className="grid grid-cols-1 md:grid-cols-2 gap-3">
                          <SField label={copy.memoryCallNameLabel} value={draft.call_name} onChange={e => setDraft({ ...draft, call_name: e.target.value })} placeholder={copy.memoryCallNamePlaceholder} />
                          <SField label={copy.memoryAssistantAliasLabel} value={draft.assistant_alias} onChange={e => setDraft({ ...draft, assistant_alias: e.target.value })} placeholder={copy.memoryAssistantAliasPlaceholder} />
                        </div>
                        <div className="flex justify-end">
                          <button onClick={saveProfile} disabled={saving} className={`text-[12px] font-medium px-4 py-2 rounded-full ${primaryBtn} ${saving ? 'opacity-50' : ''}`}>{saving ? detailCopy.saving : detailCopy.save}</button>
                        </div>
                        {filteredList.length === 0 ? (
                          <div className={`text-[13px] ${subText}`}>{query.trim() ? copy.memoryNoMatchLongTerm : copy.memoryEmptyLongTerm}</div>
                        ) : (
                          <div className="space-y-3">{filteredList.map(item => <MemoryRow key={`${item.kind}:${item.id}`} item={item} />)}</div>
                        )}
                        <div className={`rounded-2xl border px-4 py-3 bg-white/70 border-[#DDE3EA] dark:bg-white/[0.03] dark:border-white/[0.06]`}>
                          <div className={`text-[12px] leading-relaxed ${faintText}`}>{copy.memoryLongTermHint}</div>
                        </div>
                      </div>
                    ) : filteredList.length === 0 ? (
                      <div className={`text-[13px] ${subText}`}>{query.trim() ? copy.memoryNoMatchRecent : copy.memoryEmptyRecent}</div>
                    ) : (
                      <div className="space-y-3">
                        {filteredList.map(item => <MemoryRow key={`${item.kind}:${item.id}`} item={item} />)}
                        <div className={`rounded-2xl border px-4 py-3 bg-white/70 border-[#DDE3EA] dark:bg-white/[0.03] dark:border-white/[0.06]`}>
                          <div className={`text-[12px] leading-relaxed ${faintText}`}>{copy.memoryRecentHint}</div>
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
                    <div className="text-[16px] font-semibold">{detailCopy.editTitle(memoryTypeLabel(editing.kind))}</div>
                    <div className={`text-[12px] mt-1 ${subText}`}>{copy.memoryEditDesc}</div>
                  </div>
                  <button onClick={() => setEditing(null)} className={`w-8 h-8 rounded-full flex items-center justify-center ${ghostBtn}`}><X size={15} /></button>
                </div>
                <div className="space-y-3">
                  <label className="block">
                    <span className={`block text-[12px] mb-1.5 ${subText}`}>{detailCopy.content}</span>
                    <textarea value={editing.text} onChange={e => setEditing({ ...editing, text: e.target.value })} rows={5} className={`w-full rounded-xl border px-3 py-2 text-[14px] outline-none resize-none ${inputBg}`} />
                  </label>
                </div>
                <div className="mt-5 flex justify-end gap-2">
                  <button onClick={() => setEditing(null)} className={`text-[13px] px-4 py-2 rounded-full ${ghostBtn}`}>{detailCopy.cancel}</button>
                  <button onClick={saveEdit} disabled={saving || !editing.text.trim()} className={`text-[13px] font-medium px-4 py-2 rounded-full ${primaryBtn} ${(saving || !editing.text.trim()) ? 'opacity-50' : ''}`}>{saving ? detailCopy.saving : detailCopy.save}</button>
                </div>
              </div>
            </div>
          )}

        </>
      );
    };

    const ProviderIcon = ({ preset, vendor, providerKind, model, compact = false }) => {
      const modelId = String(model || '').toLowerCase();
      if (preset === 'local_vllm' && modelId.includes('qwen')) {
        return (
          <span className={`${compact ? 'h-8 w-8 rounded-[9px]' : 'h-9 w-9 rounded-[10px]'} shrink-0 flex items-center justify-center overflow-hidden bg-white border border-black/[0.08] dark:border-transparent`}>
            <img src={qwenIcon} alt="" className={`${compact ? 'h-6 w-6' : 'h-7 w-7'} object-contain`} />
          </span>
        );
      }
      if (providerKind === PROVIDER_KIND_CODING_PLAN) {
        const src = BRAND_ICON_BY_VENDOR[vendor];
        if (src) {
          const darkBacked = vendor === 'kimi';
          return (
            <span className={`${compact ? 'h-8 w-8 rounded-[9px]' : 'h-9 w-9 rounded-[10px]'} shrink-0 flex items-center justify-center overflow-hidden ${darkBacked ? 'bg-[#111827]' : ('bg-white border border-black/[0.08] dark:border-transparent')}`}>
              <img src={src} alt="" className={`${compact ? 'h-6 w-6' : 'h-7 w-7'} object-contain`} />
            </span>
          );
        }
        return (
          <span className={`${compact ? 'h-8 w-8 rounded-[9px]' : 'h-9 w-9 rounded-[10px]'} shrink-0 flex items-center justify-center overflow-hidden bg-[#007AFF]/10 text-[#007AFF] dark:bg-[#0A84FF]/18 dark:text-[#64B5F6]`}>
            <Code size={compact ? 17 : 19} strokeWidth={2.2} />
          </span>
        );
      }
      if (preset === 'local_vllm') {
        return (
          <span className={`${compact ? 'h-8 w-8 rounded-[9px]' : 'h-9 w-9 rounded-[10px]'} shrink-0 flex items-center justify-center overflow-hidden bg-[#007AFF]/10 text-[#007AFF] dark:bg-[#0A84FF]/18 dark:text-[#64B5F6]`}>
            <Cpu size={compact ? 18 : 20} strokeWidth={2.2} />
          </span>
        );
      }
      const src = BRAND_ICON_BY_PRESET[preset] || (vendor && BRAND_ICON_BY_VENDOR[vendor]);
      if (!src) return null;
      const darkBacked = preset === 'kimi';
      return (
        <span className={`${compact ? 'h-8 w-8 rounded-[9px]' : 'h-9 w-9 rounded-[10px]'} shrink-0 flex items-center justify-center overflow-hidden ${darkBacked ? 'bg-[#111827]' : ('bg-white border border-black/[0.08] dark:border-transparent')}`}>
          <img src={src} alt="" className={`${compact ? 'h-6 w-6' : 'h-7 w-7'} object-contain`} />
        </span>
      );
    };

    // 输入框底栏:模型选择器(iOS 化;darkMode:'class' 故用 dark: 变体)。
    // 可选“显式会话态驱动”props（代码模块原生车道用）：sessionId/sessionModelId/
    // busy/onSwitchModel 传入时绕开 bridge 聊天 active 绑定；不传走原 bs/bridge 路径。
    const ComposerModelSelector = ({
      t,
      bs,
      onGotoSettings,
      compact,
      sessionId: sessionIdProp,
      sessionModelId: sessionModelIdProp,
      busy: busyProp,
      onSwitchModel,
      multiAgentEnabled: multiAgentEnabledProp,
      multiAgentAvailable: multiAgentAvailableProp,
      onToggleMultiAgent,
    }) => {
      const [open, setOpen] = useState(false);
      const triggerRef = useRef(null);
      const canManageModels = can('modelManagement');
      const canSwitchModels = can('sessionModelSwitch');
      // 多智能体模式 = 模型列表下方的会话级开关（ADR-0006）。状态权威在
      // 后端 mode_state，这里只读 bs 镜像；翻转后 bridge 回写权威状态。
      const canMultiAgent = can('multiAgent') && multiAgentAvailableProp !== false;
      const multiAgentOn = multiAgentEnabledProp !== undefined
        ? Boolean(multiAgentEnabledProp)
        : !!(bs && bs.modeState && bs.modeState.multiAgent);
      const multiAgentCopy = (t && t.uiMultiAgent) || {};
      // 防重入（复核点名）：后端事务完成前再点会带着旧状态重复提交，
      // 其中一次名册推送失败的回滚还会覆盖另一次已开启的状态。切换期间
      // 禁用按钮；bridge 侧另有 in-flight 丢弃兜底（双入口防线）。
      const [multiAgentBusy, setMultiAgentBusy] = useState(false);
      // 揭幕动效只在用户点击开启这一刻播放：会话切换/重启同步出现的
      // 开启态、弹层关了再开，都不重播揭幕（真机点名），但光晕持续漂移
      // （真机点名"动画不能停"，挂在常态类上）。触发源是点击处理器而
      // 不是状态上升沿；点击关闭或弹层关闭时清掉标记，避免误续播。
      const [multiAgentRevealing, setMultiAgentRevealing] = useState(false);
      useEffect(() => {
        if (!open) setMultiAgentRevealing(false);
      }, [open]);
      async function toggleMultiAgent() {
        if (multiAgentBusy || busy) return;
        if (!onToggleMultiAgent
          && !(bridge.available && bridge.interaction && bridge.interaction.setMultiAgentMode)) return;
        setMultiAgentRevealing(!multiAgentOn);
        setMultiAgentBusy(true);
        try {
          if (onToggleMultiAgent) await onToggleMultiAgent(!multiAgentOn);
          else await bridge.interaction.setMultiAgentMode(!multiAgentOn);
        } finally {
          setMultiAgentBusy(false);
        }
      }
      const savedModels = visibleUserModels((bs && bs.savedModels) || []);
      const activeSessionId = sessionIdProp !== undefined ? sessionIdProp : (bs ? bs.activeSessionId : null);
      const activeModelId = bs && bs.activeModelId;
      const currentSessionModelId = sessionModelIdProp !== undefined ? sessionModelIdProp : (bs && bs.currentSessionModelId);
      const busy = busyProp !== undefined ? busyProp : (bs ? bs.busy : false);
      const effectiveId = currentSessionModelId || activeModelId;
      const current = savedModels.find(m => m.id === effectiveId);
      const reasoningEffortTiers = current ? (reasoningEffortTiersForModel(current) || []) : [];
      // 存量档位（可能保存过底座归一前的旧值，如 deepseek 的 medium）先归一到
      // 档位表内等价档位再高亮，避免「档位表不含该值 → 下拉无高亮」。
      const reasoningEffortValue = current ? normalizeStoredReasoningEffort(current, current.reasoning_effort) : null;
      const [effortSaveError, setEffortSaveError] = useState('');
      function setReasoningEffortForCurrent(tier) {
        if (!current) return;
        if (tier === reasoningEffortValue) return;
        setEffortSaveError('');
        const next = { ...current, reasoning_effort: tier };
        if (!bridge.available) { setOpen(false); return; }
        // 保存成功才收弹层；失败保留弹层以便展示 effortSaveError（否则错误渲染
        // 在已关闭的 popover 内不可达）。
        bridge.models.saveModel(next)
          .then(() => { setOpen(false); setEffortSaveError(''); })
          .catch((error) => {
            const message = (error && error.message)
              ? error.message
              : ((t && t.saveModelFailed) || '保存失败');
            setEffortSaveError(message);
          });
      }
      if (!savedModels.length) return null;
      function pick(id) {
        setOpen(false);
        setEffortSaveError('');
        if (id === effectiveId) return;
        if (onSwitchModel) { onSwitchModel(activeSessionId, id); return; }
        if (bridge.available) bridge.models.switchModel(activeSessionId, id);
      }
      return (
        <div className="relative min-w-0">
          <button ref={triggerRef} onClick={() => { if (!busy && canSwitchModels) setOpen(o => !o); }} disabled={busy || !canSwitchModels}
            title={(current ? selectorMainLabel(current, t) : t.modelNonePick) + (busy ? ' · ' + t.modelSwitchBusy : '')}
            className={`relative shrink-0 flex items-center justify-center ${multiAgentOn ? 'text-[#6d28d9] dark:text-[#c4b5fd]' : 'text-gray-700 dark:text-gray-200'} transition-colors border disabled:opacity-50 ${compact ? 'w-9 h-9 rounded-full bg-transparent hover:bg-black/5 dark:hover:bg-white/10 border-transparent' : 'h-8 gap-1.5 rounded-[12px] px-2.5 text-[12px] font-semibold min-w-0 max-w-full bg-black/[0.045] dark:bg-white/[0.055] hover:bg-black/[0.07] dark:hover:bg-white/[0.09] border-black/[0.045] dark:border-white/[0.06]'}`}>
            {compact ? (
              <>
                <Cpu size={18} className="opacity-80" />
                <span className="absolute top-1 right-1 w-1.5 h-1.5 rounded-full bg-[#34C759] ring-2 ring-white dark:ring-[#161618]"></span>
              </>
            ) : (
              <>
                <span className="w-1.5 h-1.5 shrink-0 rounded-full bg-[#34C759]"></span>
                <span className="max-w-[116px] truncate">{t.composerModelLabel(current ? selectorMainLabel(current, t) : t.modelNonePick)}</span>
                <ChevronDown size={13} className="opacity-50 shrink-0" />
              </>
            )}
          </button>
          <ComposerPopover open={open && canSwitchModels} onClose={() => setOpen(false)} triggerRef={triggerRef} compact={compact}
            desktopClassName="absolute bottom-full left-0 mb-2 z-50 w-64 max-h-[340px] overflow-y-auto bg-white dark:bg-[#1E1E20] border border-black/5 dark:border-white/10 rounded-2xl shadow-xl p-1.5">
                {(() => {
                  const { preset, custom } = groupModelsForSelector(savedModels);
                  const renderGroup = (label, items, withDivider) => items.length > 0 && (
                    <>
                      {withDivider && <div className="h-px bg-black/5 dark:bg-white/10 my-1.5 mx-2" />}
                      <div className="px-3 pt-1.5 pb-1 text-[11px] font-semibold text-gray-400 dark:text-gray-500">{label}</div>
                      {items.map(m => (
                        <button key={m.id} onClick={() => pick(m.id)}
                          className="w-full flex items-center justify-between gap-2 px-3 py-2 text-left rounded-xl transition-colors group hover:bg-[#007AFF] hover:text-white">
                          <span className="flex items-center gap-2.5 min-w-0">
                            <Cpu size={15} className="shrink-0 text-gray-400 group-hover:text-white/90" />
                            <span className="min-w-0">
                              <span className="block text-[13px] truncate text-gray-700 dark:text-gray-200 group-hover:text-white">{selectorMainLabel(m, t)}</span>
                              <span className="block text-[11px] truncate text-gray-400 dark:text-gray-500 group-hover:text-white/80">{selectorSubLabel(m, t)}</span>
                            </span>
                          </span>
                          {m.id === effectiveId && <Check size={15} className="shrink-0 text-[#007AFF] group-hover:text-white" />}
                        </button>
                      ))}
                    </>
                  );
                  return (
                    <>
                      {renderGroup(t.modelGroupPreset, preset, false)}
                      {renderGroup(t.modelGroupCustom, custom, preset.length > 0)}
                    </>
                  );
                })()}
                {current && reasoningEffortTiers.length > 0 && (
                  <>
                    <div className="h-px bg-black/5 dark:bg-white/10 my-1.5 mx-2" />
                    <div className="px-3 pt-1 pb-1">
                      <div className="text-[11px] font-semibold text-gray-400 dark:text-gray-500 mb-1.5">{t.thinkingDepth}</div>
                      <div className="flex flex-wrap gap-1">
                        {reasoningEffortTiers.map(tier => (
                          <button key={tier} onClick={() => setReasoningEffortForCurrent(tier)}
                            className={`h-7 min-w-[48px] px-2.5 rounded-full text-[12px] font-medium transition-colors ${
                              reasoningEffortValue === tier
                                ? 'bg-[#007AFF] text-white'
                                : 'bg-black/[0.05] dark:bg-white/[0.08] text-gray-600 dark:text-gray-300 hover:bg-black/[0.09] dark:hover:bg-white/[0.13]'
                            }`}>
                            {t.thinkingDepthTiers[tier] || tier}
                          </button>
                        ))}
                      </div>
                      {effortSaveError && (
                        <div className="mt-1.5 text-[11px] leading-4 text-[#FF3B30] dark:text-[#FF6B6B]">{effortSaveError}</div>
                      )}
                    </div>
                  </>
                )}
                {canMultiAgent && (
                  <>
                    <div className="h-px bg-black/5 dark:bg-white/10 my-1.5 mx-2" />
                    <button type="button" data-testid="multiagent-toggle" onClick={toggleMultiAgent}
                      disabled={multiAgentBusy || busy}
                      title={multiAgentCopy.toggleHint || ''}
                      onAnimationEnd={(event) => {
                        if (event.animationName === 'pinvou-ultra-reveal') setMultiAgentRevealing(false);
                      }}
                      className={`w-full flex items-center justify-between px-3 py-2.5 text-[13px] rounded-xl ${
                        multiAgentOn
                          ? 'pinvou-ultra-row'
                          : 'text-gray-700 dark:text-gray-200 hover:bg-black/[0.045] dark:hover:bg-white/[0.07]'
                      } ${multiAgentRevealing ? 'pinvou-ultra-row-reveal' : ''}`}>
                      <span className="flex items-center gap-2.5 min-w-0">
                        <Users size={15} className={`shrink-0 ${multiAgentOn ? 'text-current' : 'text-gray-400'}`} />
                        <span className={`truncate ${multiAgentOn ? 'font-medium' : ''}`}>{multiAgentCopy.toggleLabel || ''}</span>
                      </span>
                      <span aria-hidden="true" className={`relative shrink-0 w-8 h-[18px] rounded-full transition-colors ${multiAgentOn ? 'bg-white/30' : 'bg-black/20 dark:bg-white/25'}`}>
                        <span className={`absolute top-[2px] left-0 w-[14px] h-[14px] rounded-full bg-white shadow transition-transform ${multiAgentOn ? 'translate-x-[16px]' : 'translate-x-[2px]'}`} />
                      </span>
                    </button>
                  </>
                )}
                {canManageModels && (
                  <>
                    <div className="h-px bg-black/5 dark:bg-white/10 my-1.5 mx-2" />
                    <button onClick={() => { setOpen(false); if (onGotoSettings) onGotoSettings(); }}
                      className="w-full flex items-center gap-2.5 px-3 py-2.5 text-[13px] text-gray-700 dark:text-gray-200 hover:bg-[#007AFF] hover:text-white rounded-xl transition-colors group">
                      <Plus size={15} className="text-gray-400 group-hover:text-white/90" />
                      {t.manageModels}
                    </button>
                  </>
                )}
          </ComposerPopover>
        </div>
      );
    };

    const WebAccessModal = ({ theme, bs, t, onClose }) => {
      const canManageWebAccess = can('webAccessAdmin');
      const [refreshConfirmOpen, setRefreshConfirmOpen] = useState(false);
      const [actionBusy, setActionBusy] = useState(false);
      const webAccess = (bs && bs.webAccess) || {};
      const webAccessActive = !!webAccess.active;
      const statusKey = webAccess.starting ? 'starting' : (webAccess.status || 'idle');
      const remoteCopy = t.uiRemote;
      const statusColors = { idle:'#8A9097', starting:'#F9AB00', connecting_relay:'#F9AB00', waiting_web_client:'#F9AB00', web_client_connected:'#34A853', web_client_disconnected:'#F9AB00', revoked:'#EA4335', stopped:'#8A9097', error:'#EA4335' };
      const statusCopy = remoteCopy.status[statusKey];
      const statusMeta = statusCopy
        ? { label: statusCopy[0], detail: statusKey === 'error' ? (webAccess.last_error || statusCopy[1]) : statusCopy[1], color: statusColors[statusKey] }
        : { label: String(statusKey), detail: remoteCopy.updated, color: '#8A9097' };

      useEffect(() => {
        if (!webAccessActive && bridge.available) {
          bridge.remoteControl.startRemoteControl(null).catch(() => {});
        }
      }, [canManageWebAccess]);

      async function handleRotateWebAccess() {
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

      async function handleDisableWebAccess() {
        if (!bridge.available) return;
        setActionBusy(true);
        try {
          await bridge.remoteControl.stopRemoteControl();
          onClose();
        } finally {
          setActionBusy(false);
        }
      }

      async function handleRetryWebAccess() {
        if (!bridge.available) return;
        setActionBusy(true);
        try { await bridge.remoteControl.startRemoteControl(null); }
        catch (_) {}
        finally { setActionBusy(false); }
      }

      if (!canManageWebAccess) return null;

      return (
        <div className="fixed inset-0 z-[90] flex items-center justify-center p-4 bg-black/45" onClick={onClose}>
          <div onClick={e => e.stopPropagation()} className={`relative w-full max-w-[420px] rounded-[22px] shadow-2xl p-5 bg-white text-[#1F1F1F] dark:bg-[#1E1F20] dark:text-[#E3E3E3]`}>
            <div className="flex items-start justify-between gap-3 mb-4">
              <div>
                <div className="text-[17px] font-semibold">{remoteCopy.title}</div>
                <div className={`text-[12px] mt-1 text-[#5F6368] dark:text-[#AEB4BC]`}>{remoteCopy.desc}</div>
              </div>
              <button onClick={onClose} className={`w-8 h-8 rounded-full flex items-center justify-center hover:bg-black/5 dark:hover:bg-white/10`}><X size={17} /></button>
            </div>
            <div className={`rounded-[16px] border p-3 mb-4 border-black/10 bg-[#F8F9FA] dark:border-white/10 dark:bg-white/[0.035]`}>
              <div className="flex items-start justify-between gap-3">
                <div className="min-w-0 flex items-start gap-3">
                  <div className={`mt-0.5 w-9 h-9 rounded-xl flex items-center justify-center shrink-0 bg-white text-[#5F6368] dark:bg-white/5 dark:text-[#C4C7C5]`}><Globe size={17} /></div>
                  <div className="min-w-0">
                    <div className="text-[14px] font-medium">{remoteCopy.browser}</div>
                    <div className={`text-[12px] mt-1 leading-relaxed text-[#6F7378] dark:text-[#9AA0A6]`}>{statusMeta.detail}</div>
                  </div>
                </div>
                <div className="flex items-center gap-2 shrink-0">
                  <span className={`inline-flex items-center gap-1.5 px-2 py-1 rounded-full text-[11px] bg-white text-[#5F6368] dark:bg-white/5 dark:text-[#C4C7C5]`}>
                    <span className="w-1.5 h-1.5 rounded-full" style={{ background: statusMeta.color }}></span>{statusMeta.label}
                  </span>
                  {webAccessActive && <button disabled={actionBusy} onClick={handleDisableWebAccess}
                    className={`px-3 py-1.5 rounded-lg text-[12px] disabled:opacity-50 border border-black/10 hover:bg-black/5 dark:border dark:border-white/10 dark:hover:bg-white/10`}>{remoteCopy.stop}</button>}
                </div>
              </div>
            </div>
            {webAccess.url ? (
              <div className={`w-full rounded-[14px] border px-4 py-4 border-black/10 bg-[#F8F9FA] dark:border-white/10 dark:bg-white/5`}>
                {webAccess.qr_data_url && (
                  <div className="flex flex-col items-center mb-4">
                    <div className="p-3 rounded-[16px] bg-white shadow-sm">
                      <img src={webAccess.qr_data_url} alt={remoteCopy.qrAlt} className="block w-[220px] h-[220px]" />
                    </div>
                    <div className={`mt-2 text-[12px] text-[#5F6368] dark:text-[#AEB4BC]`}>{remoteCopy.qrHint}</div>
                  </div>
                )}
                <div className={`mb-1 text-[11px] font-medium text-[#6F7378] dark:text-[#9AA0A6]`}>{remoteCopy.link}</div>
                <div className={`select-all break-all text-[12px] leading-relaxed text-[#174EA6] dark:text-[#D2E3FC]`}>{webAccess.url}</div>
                <div className={`mt-2 text-[11px] text-[#777C83] dark:text-[#8F959D]`}>{remoteCopy.linkHint}</div>
              </div>
            ) : (
              <div className={`text-[13px] px-3 py-4 rounded-xl bg-[#F1F3F4] text-[#3C4043] dark:bg-white/5 dark:text-[#C4C7C5]`}>
                {webAccess.starting ? remoteCopy.generating : (webAccess.last_error || remoteCopy.notStarted)}
              </div>
            )}
            {webAccess.last_error && <div className="mt-3 text-[12px] text-[#EA4335] break-all">{webAccess.last_error}</div>}
            <div className="mt-4 flex items-center justify-end gap-2">
              <button onClick={() => navigator.clipboard && navigator.clipboard.writeText(webAccess.url || '')}
                disabled={!webAccess.url}
                className={`px-3.5 py-2 rounded-full text-[13px] bg-black/5 hover:bg-black/10 disabled:opacity-40 dark:bg-white/10 dark:hover:bg-white/15 dark:disabled:opacity-40`}>{remoteCopy.copy}</button>
              {webAccessActive ? <button disabled={actionBusy} onClick={() => setRefreshConfirmOpen(true)}
                className={`px-3.5 py-2 rounded-full text-[13px] disabled:opacity-50 bg-black/5 hover:bg-black/10 dark:bg-white/10 dark:hover:bg-white/15`}>{remoteCopy.refresh}</button>
                : <button disabled={actionBusy} onClick={handleRetryWebAccess}
                  className="px-3.5 py-2 rounded-full text-[13px] bg-[#0B57D0] text-white hover:bg-[#0842A0] disabled:opacity-50">{remoteCopy.enable}</button>}
            </div>
            {refreshConfirmOpen && (
              <div className="absolute inset-0 z-10 flex items-center justify-center p-4 rounded-[22px] bg-black/55" onClick={() => !actionBusy && setRefreshConfirmOpen(false)}>
                <div onClick={e => e.stopPropagation()} className={`w-full max-w-[330px] rounded-[18px] p-5 shadow-2xl bg-white dark:bg-[#2A2B2D]`}>
                  <div className="text-[16px] font-semibold">{remoteCopy.refreshTitle}</div>
                  <div className={`text-[13px] leading-relaxed mt-2 text-[#5F6368] dark:text-[#B7BBC0]`}>{remoteCopy.refreshDesc}</div>
                  <div className="mt-5 flex justify-end gap-2">
                    <button disabled={actionBusy} onClick={() => setRefreshConfirmOpen(false)} className={`px-4 py-2 rounded-lg text-[13px] bg-black/5 hover:bg-black/10 dark:bg-white/5 dark:hover:bg-white/10`}>{t.cancel}</button>
                    <button disabled={actionBusy} onClick={handleRotateWebAccess} className="px-4 py-2 rounded-lg text-[13px] font-medium bg-white text-[#202124] hover:bg-[#F1F3F4] disabled:opacity-60">{actionBusy ? remoteCopy.refreshing : remoteCopy.refresh}</button>
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
    const clampPreviewScale = value => Math.max(0.1, Math.min(3, Number(value) || 1));
    const ScaledHtmlPreview = ({ html, onFrameLoad, onOpenExternal, zoomMode = 'auto-width', customScale = 1, onScaleChange, onCustomScaleChange }) => {
      const wrapRef = useRef(null);
      const frameRef = useRef(null);
      const naturalRef = useRef(null); // 当前 html 的内容自然尺寸缓存(在面板参考宽度下测得)
      const [box, setBox] = useState(null); // { w, h, scale }
      const [ready, setReady] = useState(false);
      const managedZoom = zoomMode !== 'auto-width';
      const canvasW = managedZoom ? 1440 : null;
      const measure = () => {
        try {
          const fr = frameRef.current, wrap = wrapRef.current;
          if (!fr || !wrap || !fr.contentWindow) return;
          const doc = fr.contentWindow.document;
          const de = doc.documentElement, bd = doc.body;
          const panelW = wrap.clientWidth;
          const panelH = wrap.clientHeight;
          // 内容自然尺寸只在面板参考宽度下测量一次并缓存；后续仅依据面板尺寸重算缩放。
          // 若把「已按自然宽度撑开的 iframe 视口」每次再喂回测量，弹层里的 vw/vh 与
          // 溢出内容（如 right:-120px 的绝对定位元素）会让 scrollWidth/scrollHeight 随 iframe
          // 被撑大而继续放大，触发 ResizeObserver 无限反馈 → 预览无限放大。
          let nat = naturalRef.current;
          if (!nat) {
            const cw = Math.max(de ? de.scrollWidth : 0, bd ? bd.scrollWidth : 0);
            const ch = Math.max(de ? de.scrollHeight : 0, bd ? bd.scrollHeight : 0);
            nat = { w: cw, h: ch };
            // iframe 内容可能尚未真正加载(scrollWidth=0 或 scrollHeight=0)：这种空测量不写入缓存，
            // 等 onLoad 后测到真实尺寸再缓存，避免把首轮空值固化导致正常页面缩放出错。
            if (cw > 0 && ch > 0) naturalRef.current = nat;
          }
          const w = managedZoom ? Math.max(canvasW, nat.w || 0) : nat.w;
          const h = nat.h;
          let scale = 1;
          if (zoomMode === 'fit') {
            const widthScale = w > 0 && panelW > 0 ? panelW / w : 1;
            const heightScale = h > 0 && panelH > 0 ? panelH / h : 1;
            scale = Math.min(widthScale, heightScale);
          } else if (zoomMode === 'custom') {
            scale = clampPreviewScale(customScale);
          } else if (zoomMode === 'fit-width' || zoomMode === 'auto-width') {
            scale = (w > panelW && w > 0) ? panelW / w : 1;
          }
          scale = clampPreviewScale(scale);
          var nextBox = { w, h, scale, panelW, panelH };
          setBox(prev => (
            prev &&
            Math.abs(prev.w - nextBox.w) < 0.5 &&
            Math.abs(prev.h - nextBox.h) < 0.5 &&
            Math.abs(prev.scale - nextBox.scale) < 0.001 &&
            Math.abs(prev.panelW - nextBox.panelW) < 0.5 &&
            Math.abs(prev.panelH - nextBox.panelH) < 0.5
              ? prev
              : nextBox
          ));
          if (onScaleChange) onScaleChange(scale);
        } catch (e) { /* 未就绪/跨域，忽略 */ }
      };
      useEffect(() => { setReady(false); setBox(null); naturalRef.current = null; }, [html]);
      useEffect(() => { measure(); }, [zoomMode, customScale]);
      useEffect(() => {
        if (!wrapRef.current || typeof ResizeObserver === 'undefined') return;
        const ro = new ResizeObserver(() => measure());
        ro.observe(wrapRef.current);
        return () => ro.disconnect();
      }, [zoomMode, customScale]);
      const applyWheelZoom = deltaY => {
        const base = box ? box.scale : customScale;
        const next = clampPreviewScale(base + (deltaY < 0 ? 0.1 : -0.1));
        if (onCustomScaleChange) onCustomScaleChange(next);
      };
      const handleWheel = event => {
        if (!managedZoom || !event.ctrlKey) return;
        event.preventDefault();
        event.stopPropagation();
        applyWheelZoom(event.deltaY);
      };
      useEffect(() => {
        const fr = frameRef.current;
        if (!managedZoom || !fr || !fr.contentWindow) return undefined;
        let doc = null;
        try {
          doc = fr.contentWindow.document;
        } catch (e) {
          return undefined;
        }
        if (!doc) return undefined;
        const handleFrameWheel = event => {
          if (!event.ctrlKey) return;
          event.preventDefault();
          event.stopPropagation();
          applyWheelZoom(event.deltaY);
        };
        doc.addEventListener('wheel', handleFrameWheel, { passive: false, capture: true });
        return () => doc.removeEventListener('wheel', handleFrameWheel, { capture: true });
      }, [managedZoom, ready, box && box.scale, customScale, onCustomScaleChange]);
      useEffect(() => {
        const handlePreviewMessage = event => {
          const frameWindow = frameRef.current && frameRef.current.contentWindow;
          if (!frameWindow || event.source !== frameWindow) return;
          const url = artifactPreviewExternalUrlFromMessage(event.data);
          if (url && onOpenExternal) onOpenExternal(url);
        };
        window.addEventListener('message', handlePreviewMessage);
        return () => window.removeEventListener('message', handlePreviewMessage);
      }, [onOpenExternal]);
      const scaled = box && box.scale !== 1;
      const scaledW = box ? Math.max(1, Math.ceil(box.w * box.scale)) : 0;
      const scaledH = box ? Math.max(1, Math.ceil(box.h * box.scale)) : 0;
      const stageStyle = box
        ? {
          minWidth: Math.max(box.panelW || 0, scaledW || box.w) + 'px',
          minHeight: Math.max(box.panelH || 0, scaledH || box.h) + 'px',
          display: 'flex',
          justifyContent: (zoomMode === 'fit' || zoomMode === 'custom') && scaledW <= (box.panelW || 0) ? 'center' : 'flex-start',
          alignItems: (zoomMode === 'fit' || zoomMode === 'custom') && scaledH <= (box.panelH || 0) ? 'center' : 'flex-start',
        }
        : { minWidth: '100%', minHeight: '100%' };
      const frameStyle = () => {
        if (box && scaled) {
          return { position: 'absolute', left: 0, top: 0, width: box.w + 'px', height: box.h + 'px', transform: 'scale(' + box.scale + ')', transformOrigin: 'top left', colorScheme: 'dark' };
        }
        if (managedZoom && box) return { width: box.w + 'px', height: box.h + 'px', minHeight: '480px', colorScheme: 'dark' };
        return { width: '100%', height: '100%', minHeight: '480px', colorScheme: 'dark' };
      };
      const wrapStyle = managedZoom
        ? { minHeight: 0, height: '100%', overflow: zoomMode === 'fit' ? 'hidden' : 'auto' }
        : (scaled ? { height: scaledH } : { minHeight: 480, height: '100%' });
      return (
        <div ref={wrapRef} data-testid="artifact-html-preview-scroll" onWheel={handleWheel} className="relative w-full bg-[#15171a]" style={wrapStyle}>
          {!ready && <div className="h-[480px] bg-[#15171a]"></div>}
          <div data-testid="artifact-html-preview-stage" style={managedZoom ? stageStyle : (box && scaled ? { width: scaledW + 'px', height: scaledH + 'px', position: 'relative' } : { width: '100%', height: '100%' })}>
            <div style={box && scaled ? { width: scaledW + 'px', height: scaledH + 'px', position: 'relative', flex: '0 0 auto' } : (managedZoom && box ? { width: box.w + 'px', height: box.h + 'px', flex: '0 0 auto' } : { width: '100%', height: '100%' })}>
              <iframe ref={frameRef} sandbox="allow-same-origin allow-scripts" data-testid="artifact-html-preview-frame" onLoad={() => { measure(); if (onFrameLoad) onFrameLoad(frameRef.current); setTimeout(() => setReady(true), 80); }}
                className={`border-0 block bg-[#15171a] transition-opacity duration-300 ${ready ? 'opacity-100' : 'opacity-0 absolute pointer-events-none'}`}
                data-zoom-mode={zoomMode}
                data-zoom-scale={box ? String(box.scale) : ''}
                style={frameStyle()}
                srcDoc={buildArtifactPreviewDocument(html)} />
            </div>
          </div>
        </div>
      );
    };

    // 输入框「技能」入口：⚡ 药丸 + popover。视觉设计=内置自动技能（只读，模型 load_skill 时显"使用中"
    // 并高亮药丸）。activeSkill 由 bridge 检测 load_skill 设，纯只读指示。
    const ComposerModeMenu = ({ t, bs, compact }) => {
      const [open, setOpen] = useState(false);
      const SKILLS = [
        { id: 'visual-design', name: t.uiSettingsView.visualDesignSkillName, desc: t.uiSettingsView.visualDesignSkillDesc, kind: 'auto' },
      ];
      const activeId = bs && bs.activeSkill;
      const cur = SKILLS.find(s => s.id === activeId && s.kind === 'auto');
      return (
        <div className="relative">
          <button onClick={() => setOpen(o => !o)} title={cur ? cur.name : t.composerMode}
            className={`flex items-center shrink-0 font-semibold transition-colors border ${compact ? 'justify-center w-9 h-9 rounded-full' : 'h-8 gap-1.5 rounded-[12px] px-2.5 text-[12px] whitespace-nowrap'} ${cur
              ? 'bg-[#007AFF]/[0.1] dark:bg-[#0A84FF]/20 text-[#007AFF] dark:text-[#5AC8FA] border-[#007AFF]/20 dark:border-[#0A84FF]/30'
              : 'bg-black/[0.045] dark:bg-white/[0.055] hover:bg-black/[0.07] dark:hover:bg-white/[0.09] text-gray-700 dark:text-gray-200 border-black/[0.045] dark:border-white/[0.06]'}`}>
            <Zap size={compact ? 14 : 13} className={cur ? '' : 'opacity-70'} />
            {!compact && (cur ? cur.name : t.composerMode)}
            {!compact && <ChevronDown size={13} className="opacity-50 shrink-0" />}
          </button>
          {open && (
            <>
              <div className="fixed inset-0 z-40" onClick={() => setOpen(false)}></div>
              <div className="absolute bottom-full left-0 mb-2 z-50 w-64 bg-white dark:bg-[#1E1E20] border border-black/5 dark:border-white/10 rounded-2xl shadow-xl p-1.5">
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

    // 可选触发器变体：triggerVariant='pill' 时触发器渲染为代码页配置组同款 pill
    //（triggerLabel 为可选 10px 前缀文案；triggerTestId 覆盖默认 testid），
    // 下拉内容不变；不传变体时聊天页外观逐字节不变。
    const ComposerToolMenu = ({ t, onGotoTools, compact, activeSkill, triggerVariant, triggerLabel, triggerTestId, scope, activeSessionId: activeSessionIdProp }) => {
      const [open, setOpen] = useState(false);
      const triggerRef = useRef(null);
      const canMutateToolStore = can('toolStoreMutations');
      // 只增不减：有活动会话时只阻隔「关闭」——已进入上下文的工具撤不回，
      // 关闭只能在会话开始前做；「打开」仍允许（后端热重载保留给「增」即时生效）。
      const sessionsState = useBridgeState(['sessions']);
      // scope='code'（原生代码车道）时由调用方传入该车道的活动会话 id——显式会话态
      // 驱动，绕开 bridge 聊天 active 绑定（二轮评审：code 门控不得读聊天域
      // activeSessionId）；plain 缺省沿用聊天侧。
      const hasActiveSession = scope === 'code'
        ? !!activeSessionIdProp
        : !!(sessionsState && sessionsState.activeSessionId);
      // 无权限才全局禁用；会话中的「关闭」阻隔是逐行判断（只有已开 = enabled 才禁）。
      const toolSwitchDisabled = !canMutateToolStore;
      // scope: 'code' = 原生代码会话(独立开关,默认全关),缺省 = 普通会话(plain)。
      const toolScope = scope === 'code' ? 'code' : 'plain';
      const [marketplaceTools, setMarketplaceTools] = useState([]);
      const [marketplaceSkills, setMarketplaceSkills] = useState([]);
      const [disabled, setDisabled] = useState(() => new Set()); // 被关掉的包 id(开关 off，按 scope 持久)
      const [hidden, setHidden] = useState(() => new Set()); // 被不可见的包 id(可见性预过滤，按 scope 持久)
      const [projectSkillsEnabled, setProjectSkillsEnabled] = useState(false); // 项目级 skills(仅 code scope 生效)
      const [projectSkillsHelp, setProjectSkillsHelp] = useState(false); // 项目技能帮助弹窗(功能说明+扫描目录)
      const [feishuOn, setFeishuOn] = useState(false); // 飞书是否已连接(CLI 路线)
      const [feishuEnabled, setFeishuEnabled] = useState(true); // 飞书技能是否启用(未手动停用)
      const [wecomOn, setWecomOn] = useState(false); // 企微是否已连接(CLI 路线)
      const [wecomEnabled, setWecomEnabled] = useState(true); // 企微技能是否启用(未手动停用)
      const [dingtalkOn, setDingtalkOn] = useState(false); // 钉钉是否已连接(CLI 路线)
      const [dingtalkEnabled, setDingtalkEnabled] = useState(true); // 钉钉技能是否启用(未手动停用)
      const [tmeetOn, setTmeetOn] = useState(false); // 腾讯会议是否已连接(CLI 路线)
      const [tmeetEnabled, setTmeetEnabled] = useState(true); // 腾讯会议技能是否启用(未手动停用)
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
          const dis = await invokeTauri('get_disabled_connectors', { scope: toolScope });
          if (isAlive()) setDisabled(new Set(dis || []));
        } catch (e) { /* ignore */ }
        try {
          const hid = await invokeTauri('get_bundle_visibility', { scope: toolScope });
          if (isAlive()) setHidden(new Set(hid || []));
        } catch (e) { /* ignore */ }
        try {
          const proj = await invokeTauri('get_project_skills_enabled');
          if (isAlive()) setProjectSkillsEnabled(!!proj);
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
          const ts = await invokeTauri('tmeet_skills_state');
          if (isAlive()) { setTmeetOn(!!(ts && ts.connected)); setTmeetEnabled(!ts || ts.enabled !== false); }
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
      // 项目技能帮助弹窗 Esc 关闭（与项目其他 modal 惯例一致，仅弹窗打开时挂监听）
      useEffect(() => {
        if (!projectSkillsHelp) return undefined;
        const onKey = event => { if (event.key === 'Escape') setProjectSkillsHelp(false); };
        window.addEventListener('keydown', onKey);
        return () => window.removeEventListener('keydown', onKey);
      }, [projectSkillsHelp]);
      function toggleTool(id, enabled) {
        // 只增不减：会话中允许「打开」（enabled=false），只阻隔「关闭」（enabled=true）。
        if (toolSwitchDisabled || (hasActiveSession && enabled)) return;
        // scope 收敛后：工具/技能/CLI 开关统一为包 id 单一禁用集（后端
        // disabled_bundles.json），技能行 id 即包 id，不再带 `skill:` 前缀。
        const next = new Set(disabled);
        next.has(id) ? next.delete(id) : next.add(id);
        setDisabled(next);
        // 按 scope 持久:落盘 + 广播给所有在跑引擎,关一次该 scope 所有新对话/新窗口都继承。
        if (bridge.available) {
          invokeTauri('set_disabled_connectors',
            { connectorIds: Array.from(next), scope: toolScope }).catch(() => {});
        }
      }
      function toggleProjectSkills() {
        if (toolSwitchDisabled || (hasActiveSession && projectSkillsEnabled)) return;
        const next = !projectSkillsEnabled;
        setProjectSkillsEnabled(next);
        if (bridge.available) {
          invokeTauri('set_project_skills_enabled', { enabled: next }).catch(() => {});
        }
      }
      const menuState = buildComposerToolMenuState({
        marketplaceTools,
        marketplaceSkills,
        disabledIds: Array.from(disabled),
        hiddenIds: Array.from(hidden),
        activeSkill,
        scope: toolScope,
        serviceStates: [
          { id: 'feishu', title: t.uiSettingsView.serviceFeishu, connected: feishuOn, enabled: feishuEnabled },
          { id: 'wecom', title: t.uiSettingsView.serviceWecom, connected: wecomOn, enabled: wecomEnabled },
          { id: 'dingtalk', title: t.uiSettingsView.serviceDingtalk, connected: dingtalkOn, enabled: dingtalkEnabled },
          { id: 'tmeet', title: t.uiSettingsView.serviceTmeet, connected: tmeetOn, enabled: tmeetEnabled },
        ],
      });
      const { connectedServices, toolRows, skillRows, enabledCount, allSkillsDisabled } = menuState;
      // 内置技能名称/描述由 composer-tool-menu-logic.js 数据提供，在 UI 边界按当前语言覆盖
      const localizedSkillRows = skillRows.map(row => (row.kind === 'builtin-skill' && row.skillId === 'visual-design')
        ? { ...row, title: t.uiSettingsView.visualDesignSkillName, description: t.uiSettingsView.visualDesignSkillDesc }
        : row);
      const statusBadge = (label, tone = 'green') => {
        const cls = tone === 'blue'
          ? 'text-[#007AFF] dark:text-[#5AC8FA] bg-[#007AFF]/10 dark:bg-[#0A84FF]/15'
          : 'text-[#34C759] bg-[#34C759]/10';
        return <span className={`shrink-0 inline-flex items-center gap-1 text-[10px] font-semibold ${cls} px-2 py-0.5 rounded-full leading-none`}><span className={`w-1.5 h-1.5 rounded-full ${tone === 'blue' ? 'bg-[#007AFF] dark:bg-[#5AC8FA]' : 'bg-[#34C759]'}`} />{label}</span>;
      };
      const switchRow = (row) => {
        const rowDisabled = toolSwitchDisabled || (hasActiveSession && row.enabled);
        return (
        <div key={row.id} className="flex items-center justify-between gap-2 px-3 py-2.5 rounded-xl font-medium">
          <span className="min-w-0 flex items-center gap-1.5">
            <span className="block text-[13px] text-gray-700 dark:text-gray-200 truncate">{row.title}</span>
            {row.kind === 'service' && statusBadge(t.composerConnected, 'green')}
          </span>
          <button onClick={() => toggleTool(row.id, row.enabled)} aria-label={row.id} disabled={rowDisabled}
            className={`relative inline-flex h-5 w-[34px] shrink-0 items-center rounded-full transition-colors disabled:cursor-default ${rowDisabled ? 'opacity-70' : ''} ${row.enabled ? 'bg-[#34C759]' : 'bg-[#E5E5EA] dark:bg-[#39393D]'}`}>
            <span className={`inline-block h-4 w-4 rounded-full bg-white shadow transition-transform ${row.enabled ? 'translate-x-[16px]' : 'translate-x-[2px]'}`} />
          </button>
        </div>
        );
      };
      const readonlyRow = (row, label, tone = 'green') => (
        <div key={row.id} className="flex items-center justify-between gap-2 px-3 py-2.5 rounded-xl font-medium">
          <span className="min-w-0">
            <span className="block text-[13px] text-gray-700 dark:text-gray-200 truncate">{row.title}</span>
          </span>
          {statusBadge(label, tone)}
        </div>
      );
      // 权限只读开关：显示开关状态（受静态表控制），但不可手动切换；保留「内置」标识。
      const readonlySwitchRow = (row) => (
        <div key={row.id} className="flex items-center justify-between gap-2 px-3 py-2.5 rounded-xl font-medium">
          <span className="min-w-0 flex items-center gap-1.5">
            <span className="block text-[13px] text-gray-700 dark:text-gray-200 truncate">{row.title}</span>
            {statusBadge(row.active ? t.composerSkillInUse : t.composerBuiltinAuto, row.active ? 'green' : 'blue')}
          </span>
          <button disabled aria-label={row.id} title={t.composerReadonlySwitch}
            className={`relative inline-flex h-5 w-[34px] shrink-0 items-center rounded-full transition-colors cursor-not-allowed bg-[#34C759]/60`}>
            <span className="inline-block h-4 w-4 rounded-full bg-white shadow translate-x-[16px]" />
          </button>
        </div>
      );
      return (
        <div className="relative shrink-0">
          {triggerVariant === 'pill' ? (
            <button
              ref={triggerRef}
              type="button"
              data-testid={triggerTestId || 'composer-tool-menu-trigger'}
              onClick={() => setOpen(o => !o)}
              title={t.composerTools}
              aria-expanded={open}
              className="inline-flex h-8 min-w-0 max-w-[220px] items-center gap-1.5 overflow-hidden rounded-xl border px-2.5 transition-all cursor-pointer hover:-translate-y-px hover:shadow-sm focus-within:border-[#007AFF]/45 focus-within:ring-2 focus-within:ring-[#007AFF]/10 border-black/[0.07] bg-black/[0.025] text-[#1F1F1F] dark:border-white/[0.09] dark:bg-white/[0.055] dark:text-[#E8EAED]"
            >
              {triggerLabel && (
                <span className="pointer-events-none shrink-0 text-[10px] font-medium text-gray-400 dark:text-gray-500">
                  {triggerLabel}
                </span>
              )}
              <span className="pointer-events-none min-w-0 truncate text-[11px] font-semibold">
                {t.composerTools}
              </span>
              {enabledCount > 0 && (
                <span className="min-w-4 h-4 rounded-full bg-[#007AFF] px-1 text-center text-[10px] font-bold leading-4 text-white shrink-0">{enabledCount}</span>
              )}
              <ChevronDown
                size={12}
                aria-hidden="true"
                className={`pointer-events-none ml-auto shrink-0 text-gray-400 transition-transform ${open ? 'rotate-180' : ''}`}
              />
            </button>
          ) : (
          <button ref={triggerRef} data-testid={triggerTestId || 'composer-tool-menu-trigger'} onClick={() => setOpen(o => !o)} title={t.composerTools}
            className={`relative shrink-0 flex items-center justify-center text-gray-700 dark:text-gray-200 transition-colors border ${compact ? 'w-9 h-9 rounded-full bg-transparent hover:bg-black/5 dark:hover:bg-white/10 border-transparent' : 'h-8 gap-1.5 rounded-[12px] px-2.5 text-[12px] font-semibold whitespace-nowrap bg-black/[0.045] dark:bg-white/[0.055] hover:bg-black/[0.07] dark:hover:bg-white/[0.09] border-black/[0.045] dark:border-white/[0.06]'}`}>
            <Wrench size={compact ? 18 : 13} className="opacity-80" />
            {!compact && t.composerTools}
            {enabledCount > 0 && (compact
              ? <span className="absolute -top-1 -right-1 min-w-[16px] h-4 px-1 text-[10px] leading-4 text-center font-bold bg-[#007AFF] text-white rounded-full">{enabledCount}</span>
              : <span className="min-w-4 h-4 rounded-full bg-[#007AFF] px-1 text-center text-[10px] font-bold leading-4 text-white shrink-0">{enabledCount}</span>)}
            {!compact && <ChevronDown size={13} className="opacity-50 shrink-0" />}
          </button>
          )}
          <ComposerPopover open={open} onClose={() => setOpen(false)} triggerRef={triggerRef} compact={compact}
            menuProps={{ 'data-testid': 'composer-tool-menu' }}
            desktopClassName="absolute bottom-full left-0 mb-2 w-72 max-h-[420px] z-50 overflow-y-auto custom-scrollbar bg-white dark:bg-[#1E1E20] border border-black/5 dark:border-white/10 rounded-2xl shadow-xl p-1.5">
                {connectedServices.map(switchRow)}
                {toolRows.map(switchRow)}
                {localizedSkillRows.length === 0 ? (
                  (connectedServices.length === 0 && toolRows.length === 0) ? (
                    <div className="px-3 py-2 text-[13px] text-gray-400 dark:text-gray-500">{t.composerModeNone}</div>
                  ) : null
                ) : (
                  <>
                    {localizedSkillRows.map(row => row.switchable
                      ? switchRow(row)
                      : row.readonly
                        ? readonlySwitchRow(row)
                        : readonlyRow(row, row.active ? t.composerSkillInUse : t.composerBuiltinAuto, row.active ? 'green' : 'blue'))}
                    {/* 该 scope 全部技能被关：空态提示（组合目录为空 → 模型看不到任何技能） */}
                    {allSkillsDisabled && (
                      <div className="px-3 pt-1 pb-1 text-[11px] text-gray-400 dark:text-gray-500">{t.composerSkillAllDisabled}</div>
                    )}
                  </>
                )}
                {toolScope === 'code' && (
                  <>
                    <div className="h-px bg-black/5 dark:bg-white/10 my-1.5 mx-2" />
                    <div className="px-3 py-2">
                      <div className="flex items-center justify-between gap-2">
                        <span className="min-w-0">
                          <span className="block text-[13px] text-gray-700 dark:text-gray-200 truncate">
                            {t.composerProjectSkills}
                            <button onClick={() => setProjectSkillsHelp(true)} aria-label={t.composerProjectSkillsHelpTitle}
                              className="inline-flex items-center justify-center w-[15px] h-[15px] ml-1 rounded-full text-[10px] font-semibold leading-none text-gray-400 dark:text-gray-500 hover:text-gray-600 dark:hover:text-gray-300 hover:bg-black/5 dark:hover:bg-white/10 align-middle">?</button>
                          </span>
                          <span className="block text-[10px] text-gray-400 dark:text-gray-500">{t.composerProjectSkillsDesc}</span>
                        </span>
                        <button onClick={toggleProjectSkills} aria-label="project-skills" disabled={toolSwitchDisabled || (hasActiveSession && projectSkillsEnabled)}
                          className={`relative inline-flex h-5 w-[34px] shrink-0 items-center rounded-full transition-colors disabled:cursor-default ${(toolSwitchDisabled || (hasActiveSession && projectSkillsEnabled)) ? 'opacity-70' : ''} ${projectSkillsEnabled ? 'bg-[#34C759]' : 'bg-[#E5E5EA] dark:bg-[#39393D]'}`}>
                          <span className={`inline-block h-4 w-4 rounded-full bg-white shadow transition-transform ${projectSkillsEnabled ? 'translate-x-[16px]' : 'translate-x-[2px]'}`} />
                        </button>
                      </div>
                      {projectSkillsEnabled && (
                        <div className="mt-1.5 text-[11px] leading-snug text-amber-600 dark:text-amber-400">{t.composerProjectSkillsWarning}</div>
                      )}
                    </div>
                  </>
                )}
                <div className="h-px bg-black/5 dark:bg-white/10 my-1.5 mx-2" />
                <button onClick={() => { setOpen(false); if (onGotoTools) onGotoTools(); }}
                  className="w-full flex items-center gap-2.5 px-3 py-2.5 text-[13px] text-gray-700 dark:text-gray-200 hover:bg-[#007AFF] hover:text-white rounded-xl transition-colors group">
                  <Store size={15} className="text-gray-400 group-hover:text-white/90" />
                  {t.composerManageTools}
                </button>
          </ComposerPopover>
          {projectSkillsHelp && createPortal(
            <div className="fixed inset-0 z-[90] flex items-center justify-center p-4 bg-black/45" onClick={() => setProjectSkillsHelp(false)}>
              <div onClick={e => e.stopPropagation()} className="relative w-full max-w-[380px] rounded-[22px] shadow-2xl p-5 bg-white text-[#1F1F1F] dark:bg-[#1E1F20] dark:text-[#E3E3E3]">
                <div className="flex items-start justify-between gap-3 mb-4">
                  <div className="text-[16px] font-semibold">{t.composerProjectSkillsHelpTitle}</div>
                  <button onClick={() => setProjectSkillsHelp(false)} className="w-8 h-8 rounded-full flex items-center justify-center hover:bg-black/5 dark:hover:bg-white/10"><X size={17} /></button>
                </div>
                <div className="text-[12px] leading-relaxed text-[#5F6368] dark:text-[#AEB4BC]">{t.composerProjectSkillsHelpBody}</div>
                <div className="mt-3 text-[12px] font-medium">{t.composerProjectSkillsHelpDirsLabel}</div>
                <div className="mt-1.5 rounded-[14px] border p-3 border-black/10 bg-[#F8F9FA] dark:border-white/10 dark:bg-white/[0.035]">
                  {String(t.composerProjectSkillsHelpDirs).split('\n').map((dir, i) => (
                    <div key={dir} className="flex items-center gap-2 text-[11px] font-mono text-gray-600 dark:text-gray-300 py-0.5">
                      <span className="text-[10px] text-gray-400 dark:text-gray-500">{i + 1}</span>{dir}
                    </div>
                  ))}
                </div>
                <div className="mt-3 text-[11px] leading-snug text-amber-600 dark:text-amber-400">{t.composerProjectSkillsWarning}</div>
              </div>
            </div>,
            document.body
          )}
        </div>
      );
    };

    // 添加/编辑模型模态弹窗。
    const ModelFormModal = ({ isDark, t, initial, onCancel, onSave, bs, models = [] }) => {
      const settingsCopy = t.uiSettingsDetail;
      const localVllmSupported = !!(bs.platformCapabilities && bs.platformCapabilities.localVllmSupported);
      const modelScope = initial.__scope || (initial.preset === 'local_vllm' ? 'local' : 'cloud');
      const initialProvider = modelScope === 'cloud' ? findCloudProviderForModel(initial) : null;
      const initialCatalogGroups = MODEL_CATALOG[modelScope] || MODEL_CATALOG.cloud;
      const initialCatalogMatch = initialCatalogGroups.some(group =>
        group.preset === initial.preset
        && (!initialProvider || group.key === initialProvider.key)
        && group.items.some(item => !item.custom && catalogItemMatchesModel(item, initial.model))
      );
      const canSetUpLocalModel = can('localModelSetup');
      const [name, setName] = useState(initial.name || '');
      const [nameTouched, setNameTouched] = useState(!initial.__new && !!initial.name);
      const [preset, setPreset] = useState(initial.preset || (localVllmSupported ? 'local_vllm' : 'deepseek'));
      const [providerKey, setProviderKey] = useState(initialProvider ? initialProvider.key : '');
      const [providerKind, setProviderKind] = useState(initial.provider_kind || (initialProvider && initialProvider.providerKind) || (modelScope === 'cloud' ? PROVIDER_KIND_OFFICIAL_API : ''));
      const [vendor, setVendor] = useState(initial.vendor || (initialProvider && initialProvider.vendor) || '');
      const [endpointMode, setEndpointMode] = useState((initialProvider && initialProvider.endpointMode) || '');
      const [model, setModel] = useState(initial.model || '');
      const [baseUrl, setBaseUrl] = useState(initial.base_url || '');
      const [contextWindow, setContextWindow] = useState(initial.context_window_tokens ? String(initial.context_window_tokens) : '');
      const [maxOutput, setMaxOutput] = useState(initial.max_output_tokens ? String(initial.max_output_tokens) : '');
      // 思考深度档位：初始取已保存值（先归一——存量可能是底座归一前的旧值，
      // 如 deepseek 的 medium），无则按模型默认（vllm→off，其余→high；
      // 底座不支持的模型无默认，保持 null = 未显式设置，避免保存时污染 SavedModel）。
      const [reasoningEffort, setReasoningEffort] = useState(
        normalizeStoredReasoningEffort(initial, initial.reasoning_effort)
      );
      const [apiKey, setApiKey] = useState('');
      const [keyAction, setKeyAction] = useState(initial.__new ? 'replace' : 'keep_existing');
      const [showKey, setShowKey] = useState(false);
      const [localKeyEnabled, setLocalKeyEnabled] = useState(!initial.__new && initial.preset === 'local_vllm' && !!initial.has_secret);
      const [pickerOpen, setPickerOpen] = useState(!!initial.__new && initial.preset !== 'local_vllm');
      const [pickerTab, setPickerTab] = useState(initial.__scope === 'local' ? 'local' : 'cloud');
      const [providerModelPickerOpen, setProviderModelPickerOpen] = useState(false);
      const [customModel, setCustomModel] = useState(!!initial.__custom || (!initial.__new && initial.preset !== 'local_vllm' && !initialCatalogMatch));
      const [keyRevealError, setKeyRevealError] = useState('');
      const [testing, setTesting] = useState(false);
      const [testResult, setTestResult] = useState(null);
      const [detecting, setDetecting] = useState(false);
      const [detectResult, setDetectResult] = useState(null); // { candidates } | { error } | null
      const [localDetecting, setLocalDetecting] = useState(false);
      const [localDetectResult, setLocalDetectResult] = useState(null);
      // 图片输入能力三档(auto/enabled/disabled)与兜底视觉模型引用(阶段 G 设置页控件)。
      const [imageCapability, setImageCapability] = useState(initial.image_capability_override || 'auto');
      const [visionModelId, setVisionModelId] = useState(initial.vision_model_id || '');
      const [imageCapabilityPickerOpen, setImageCapabilityPickerOpen] = useState(false);
      // 「自动探测」保存时按钮转「正在探测图片能力…」,探测随保存完成、弹窗直接关闭。
      const [savingModel, setSavingModel] = useState(false);
      // 「保存时检测」检测到明确不支持时的待决策信号(ImageProbeOutcome);
      // 非空时弹窗保持,给出再次检测/去配置视觉模型/直接保存(自动处理)三选一。
      const [probeDecision, setProbeDecision] = useState(null);
      // 保存失败(连接/写盘错误)行内提示:非空时弹窗保持,交用户修正后重试,
      // 不静默关闭丢弃表单输入。
      const [saveError, setSaveError] = useState('');
      // 视觉模型选择探测:选中模型必须先通过图片识别探测——探测中列表保持
      // 展开、该行右侧显示忙转圈;通过后收起列表选中,未通过则拒绝并提示排查。
      const [visionProbingKey, setVisionProbingKey] = useState(null);
      const [visionProbeError, setVisionProbeError] = useState(null);
      const [visionModelPickerOpen, setVisionModelPickerOpen] = useState(false);
      // 测试图片能力(设计 §7.3):仅主动点击触发;表单关键值变化后上一次结果不再可信,清除。
      const [imageTesting, setImageTesting] = useState(false);
      const [imageTestResult, setImageTestResult] = useState(null); // { status, verified, summary } | null
      useEffect(() => { setImageTestResult(null); }, [model, baseUrl, apiKey, preset]);
      // 本机预装大模型「再入口」:检测无运行实例但有预装时,提示启用;走同一 bootstrap。
      const [offerSetup, setOfferSetup] = useState(false);   // 检测到预装,显示启用提示
      const [bootstrapHere, setBootstrapHere] = useState(false); // 从本页发起了 bootstrap(隔离全局态,避免开机引导的成功态串到这里)
      const localizeProvider = group => group
        ? { ...group, ...(settingsCopy.providerCatalog[group.key] || {}) }
        : null;
      const baseCatalogGroups = (MODEL_CATALOG[modelScope] || MODEL_CATALOG.cloud).map(localizeProvider);
      const catalogGroups = !initial.__new && modelScope === 'cloud'
        ? baseCatalogGroups.filter(group => initialProvider ? group.key === initialProvider.key : group.preset === initial.preset)
        : baseCatalogGroups;
      const activeProvider = modelScope === 'cloud'
        ? localizeProvider(CLOUD_MODEL_PROVIDERS.find(group => group.key === providerKey) || findCloudProviderForModel({ preset, model, base_url: baseUrl, provider_kind: providerKind, vendor }) || null)
        : null;
      const isCodingPlan = providerKind === PROVIDER_KIND_CODING_PLAN || (activeProvider && activeProvider.providerKind === PROVIDER_KIND_CODING_PLAN);
      // 当前表单模型可切换的思考深度档位（底座不支持的模型为空 = 不提供切换）。
      const reasoningEffortTiers = reasoningEffortTiersForModel({ preset, model, vendor, base_url: baseUrl, provider_kind: providerKind }) || [];
      function normalizeConnectionTestResult(value, isCodingPlanProvider) {
        if (value && typeof value === 'object' && !Array.isArray(value)) {
          const code = String(value.code || (value.ok ? 'ok' : 'unknown'));
          let message = settingsCopy.connectionMessages[code]
            || (value.ok ? settingsCopy.connectionMessages.ok : settingsCopy.connectionMessages.unknown);
          if (isCodingPlanProvider && (code === 'endpoint_not_found' || code === 'method_not_allowed')) {
            message = settingsCopy.codingPlanTestUnavailable;
          }
          return {
            ok: !!value.ok,
            code,
            message,
            detail: value.detail ? String(value.detail) : '',
          };
        }
        const raw = String(value || '');
        const httpMatch = raw.match(/HTTP\s+(\d{3})/i);
        if (httpMatch) {
          const status = Number(httpMatch[1]);
          const legacy = {
            ok: status >= 200 && status < 300,
            code: status === 401 ? 'auth_invalid' : status === 403 ? 'auth_forbidden' : status === 429 ? 'rate_limited' : 'http_error',
            message: status === 401 ? settingsCopy.connectionMessages.auth_invalid
              : status === 403 ? settingsCopy.connectionMessages.auth_forbidden
                : status === 429 ? settingsCopy.connectionMessages.rate_limited
                  : (status >= 200 && status < 300 ? settingsCopy.connectionMessages.ok : settingsCopy.connectionMessages.http_error),
            detail: `HTTP ${status}`,
          };
          if (isCodingPlanProvider && (status === 404 || status === 405)) {
            legacy.code = status === 404 ? 'endpoint_not_found' : 'method_not_allowed';
            legacy.message = settingsCopy.codingPlanTestUnavailable;
          }
          return legacy;
        }
        if (raw === 'ok') return { ok: true, code: 'ok', message: settingsCopy.connectionMessages.ok, detail: '' };
        return { ok: false, code: 'unknown', message: settingsCopy.connectionMessages.unknown, detail: '' };
      }
      function applyCatalogItem(group, item) {
        const p = group.preset;
        setPreset(p);
        const defs = MODEL_PRESET_DEFS[p] || MODEL_PRESET_DEFS[localVllmSupported ? 'local_vllm' : 'deepseek'];
        const nextModel = item.custom ? '' : (item.model || defs.model);
        const nextBaseUrl = normalizedProviderBaseUrl(group) || defs.baseUrl;
        setProviderKey(group.key || '');
        setProviderKind(group.providerKind || (p === 'openai_compatible' ? PROVIDER_KIND_CUSTOM : PROVIDER_KIND_OFFICIAL_API));
        setVendor(group.vendor || '');
        setEndpointMode(group.endpointMode || '');
        setBaseUrl(nextBaseUrl);
        setModel(nextModel);
        if (!nameTouched) setName(p === 'local_vllm' ? settingsCopy.localModelName(nextModel) : (item.custom ? group.title : item.title));
        setContextWindow(p === 'local_vllm' ? '262144' : '');
        setMaxOutput(p === 'local_vllm' ? '24576' : '');
        // 换目录项时重置思考深度到该模型的默认档位（vllm→off，其余→high；
        // 无档位模型置 null = 未显式设置）。带上 nextBaseUrl 以按新 route 判定档位。
        setReasoningEffort(reasoningEffortForModelSwitch({ preset: p, model: nextModel, vendor: group.vendor || vendor, base_url: nextBaseUrl }));
        if (p !== 'local_vllm') {
          setApiKey('');
          setKeyAction(initial.__new ? 'replace' : 'keep_existing');
        } else {
          setApiKey('');
          setKeyAction(initial.__new ? 'replace' : 'keep_existing');
        }
        setCustomModel(!!item.custom);
        setProviderModelPickerOpen(false);
        setPickerOpen(false);
      }
      // 手输编辑会改变 reasoning-effort route 的字段（model ID / base_url）后，把思考
      // 深度归一到新 route：仍在档位表内的值保留（不覆盖用户有效选择，如 vLLM 的 high、
      // 非 K3 moonshot 的 off），不在档位表内的按底座真实等价值回落（如 K3 的 off→low、
      // medium→high，或 openai_compatible 改到官方 deepseek 端点后档位从无到有 → 默认
      // high）。与目录选择（chooseModel / applyCatalogItem）的「重置到默认」区分：目录
      // 选择是显式换模型，手输是改字段，保留有效值更符合直觉且不会误清用户的旧选择。
      function renormalizeReasoningEffort(modelDescriptor) {
        setReasoningEffort(normalizeStoredReasoningEffort(modelDescriptor, reasoningEffort));
      }
      function handleModelIdChange(value) {
        setModel(value);
        renormalizeReasoningEffort({ preset, model: value, vendor, base_url: baseUrl });
      }
      function handleBaseUrlChange(value) {
        setBaseUrl(value);
        renormalizeReasoningEffort({ preset, model, vendor, base_url: value });
      }
      async function handleTest() {
        if (!bridge.available) return;
        setTesting(true); setTestResult(null);
        const testKey = keyAction === 'replace' || (isLocalPreset && localKeyEnabled) ? apiKey.trim() : '';
        try {
          const result = await bridge.models.testModelConnection(baseUrl.trim(), testKey, initial.__new ? null : initial.id);
          setTestResult(normalizeConnectionTestResult(result, isCodingPlan));
        } catch (e) {
          setTestResult(normalizeConnectionTestResult(e, isCodingPlan));
        }
        finally { setTesting(false); }
      }
      // 测试图片能力(设计 §7.3):与测试连接同一模式——表单未保存也按当前表单值发测,
      // 凭据优先用新填的 key,否则由后端按 model_id 读已保存凭据。
      function normalizeImageCapabilityTestResult(value) {
        if (value && typeof value === 'object' && !Array.isArray(value)) {
          const status = ['supported', 'unsupported', 'unverified', 'error'].includes(value.status) ? value.status : 'error';
          return { status, verified: !!value.verified, summary: value.summary ? String(value.summary) : '' };
        }
        return { status: 'error', verified: false, summary: String(value || '') };
      }
      async function handleImageCapabilityTest() {
        if (!bridge.available || !bridge.models.testImageInputCapability) return;
        setImageTesting(true); setImageTestResult(null);
        const testKey = keyAction === 'replace' || (isLocalPreset && localKeyEnabled) ? apiKey.trim() : '';
        try {
          const result = await bridge.models.testImageInputCapability(model.trim(), baseUrl.trim(), testKey, initial.__new ? null : initial.id);
          setImageTestResult(normalizeImageCapabilityTestResult(result));
        } catch (e) {
          setImageTestResult({ status: 'error', verified: false, summary: String(e && e.message ? e.message : e) });
        }
        finally { setImageTesting(false); }
      }
      // 探测本机 vLLM：只扫 127.0.0.1/localhost 的 8000-8002，探到唯一可用实例直接自动填充。
      function applyCandidate(c) {
        if (!c) return;
        // 优先填充已加载的模型：Ollama/LM Studio 的列表含全部已下载模型，
        // 选未加载的模型 = 首次推理时由框架 JIT 静默载入内存（可能几十 GB）。
        const entries = Array.isArray(c.models) && c.models.length
          ? c.models.map(m => (typeof m === 'string' ? { id: m, loaded: null } : m))
          : [];
        const preferred = entries.find(e => e && e.id && e.loaded === true)
          || entries.find(e => e && e.id && e.loaded == null);
        const modelId = preferred ? preferred.id : (c.model || '');
        if (c.base_url) setBaseUrl(c.base_url);
        if (modelId) { setModel(modelId); if (!name.trim()) setName(modelId); }
        setApiKey('');
        setKeyAction(initial.__new ? 'replace' : 'keep_existing');
      }
      async function handleDetect() {
        if (!canSetUpLocalModel || !bridge.available || detecting) return;
        // macOS/Windows 后端无 discover_local_vllm / detect_local_vllm_setup 命令(已 cfg linux),
        // 此处非 Linux 直接返回,避免 invoke 不存在的命令 reject 报错。
        if (!bridge.available || detecting) return;
        if (!localVllmSupported) return;
        setDetecting(true); setDetectResult(null); setTestResult(null); setOfferSetup(false); setBootstrapHere(false);
        try {
          const result = await bridge.vllm.discoverLocalVllm({
            currentBaseUrl: baseUrl.trim() || null,
            savedBaseUrl: initial.base_url || null,
          });
          const online = ((result && result.candidates) || []).filter(c => c.status !== 'offline');
          setDetectResult({ candidates: online });
          // 唯一可用实例直接填充——但只自动填充"已加载"的模型。Ollama/LM Studio
          // 的列表接口返回全部已下载模型，JIT 机制下选未加载模型 = 首次推理时
          // 静默载入内存（可能是几十 GB），必须交给用户显式选择。
          if (online.length === 1) {
            const c = online[0];
            const entries = Array.isArray(c.models) && c.models.length
              ? c.models.map(m => (typeof m === 'string' ? { id: m, loaded: null } : m))
              : (c.model ? [{ id: c.model, loaded: null }] : []);
            const loadedEntry = entries.find(e => e && e.id && e.loaded === true);
            if (loadedEntry) applyCandidate({ base_url: c.base_url, model: loadedEntry.id });
          }
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
      const showCodingPlanModelField = !isLocalPreset && isCodingPlan;
      const showProviderModelField = !isLocalPreset && !!activeProvider && Array.isArray(activeProvider.items) && activeProvider.items.length > 0;
      const showModelIdField = isLocalPreset || customModel || showProviderModelField;
      const showBaseUrlField = isLocalPreset || (customModel && preset === 'openai_compatible' && !isCodingPlan);
      const showCustomCloudKeyField = !isLocalPreset && customModel;
      const showLocalKeyField = isLocalPreset && localKeyEnabled;
      const showDisplayNameField = isLocalPreset && !initial.__new;
      const showConfigFields = showDisplayNameField || showModelIdField || showBaseUrlField || showCustomCloudKeyField;
      const selectedProvider = isLocalPreset ? presetProviderLabel(preset, t) : (activeProvider ? (activeProvider.configTitle || activeProvider.title) : presetProviderLabel(preset, t));
      const selectedModelLabel = model || settingsCopy.customModel;
      const modalTitle = initial.__new
        ? (isCodingPlan ? settingsCopy.addProvider(selectedProvider) : t.modelFormAddTitle)
        : (isCodingPlan ? settingsCopy.editProvider(selectedProvider) : t.modelFormEditTitle);
      const saveName = showDisplayNameField
        ? (name.trim() || settingsCopy.localModelName(model.trim()))
        : (isLocalPreset ? (name.trim() || settingsCopy.localModelName(model.trim())) : (model.trim() || selectedProvider));
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
            setKeyRevealError(String(error || settingsCopy.apiKeyReadFailed));
          }
        }
        setShowKey(nextVisible);
      }
      // 「保存时检测」流程:检测支持 → 回填落盘后直接关闭;检测明确不支持 →
      // 后端**不落盘**,弹窗保持并给出三选一(再次检测/去配置视觉模型/
      // 直接保存落自动处理);error/连接不通 → 后端已落「自动处理」,直接关闭。
      async function doSave(skipProbe) {
        if (!canSave || savingModel) return;
        const id = initial.__new ? ('m_' + Date.now().toString(36) + Math.random().toString(36).slice(2, 7)) : initial.id;
        const contextTokens = Number.parseInt(contextWindow, 10);
        const outputTokens = Number.parseInt(maxOutput, 10);
        const nextKeyAction = isLocalPreset
          ? (localKeyEnabled && apiKey.trim() ? 'replace' : 'keep_existing')
          : (apiKey.trim() ? 'replace' : (initial.__new || !hasSavedKey ? 'replace' : 'keep_existing'));
        const nextApiKey = isLocalPreset
          ? (localKeyEnabled && apiKey.trim() ? apiKey.trim() : '')
          : (!isLocalPreset && apiKey.trim() ? apiKey.trim() : '');
        setSavingModel(true);
        setSaveError('');
        try {
          const outcome = await onSave({
            id: id, name: saveName, preset: preset,
            context_window_tokens: Number.isFinite(contextTokens) && contextTokens > 0 ? contextTokens : null,
            max_output_tokens: Number.isFinite(outputTokens) && outputTokens > 0 ? outputTokens : null,
            // 仅当前表单模型支持档位时保存；手输 model 变为无档位模型时置 null(#209)。
            reasoning_effort: reasoningEffortTiers.length > 0 ? (reasoningEffort || null) : null,
            model: model.trim(), base_url: baseUrl.trim(),
            api_key: nextApiKey, credential_action: nextKeyAction,
            provider_kind: providerKind || null,
            vendor: vendor || null,
            endpoint_mode: endpointMode || null,
            // 图片能力/视觉模型(阶段 G):选了自身等同未配置。
            // 用户「直接保存(自动处理)」时带 pinvou 档且不再检测。
            image_capability_override: skipProbe ? 'pinvou' : (imageCapability || 'auto'),
            vision_model_id: visionModelId && visionModelId !== id ? visionModelId : null,
            // 「保存时检测」档:后端在保存时检测连接+识图,按结果回填。
            probe_image_capability: !skipProbe && imageCapability === 'auto',
          });
          const probe = outcome && outcome.image_probe;
          if (probe && (probe.status === 'unsupported' || probe.status === 'unverified')) {
            // 明确不支持:后端未写盘,弹窗保持,交用户决策。
            setProbeDecision(probe);
          } else {
            // supported 已回填落盘 / error/unknown 已落自动处理 / 无检测:直接关闭。
            onCancel();
          }
        } catch (e) {
          // 保存失败(连接/写盘错误):保持弹窗并给行内提示,不丢弃表单输入。
          setSaveError(String(e && e.message ? e.message : e));
        } finally {
          setSavingModel(false);
        }
      }
      // 视觉模型选择:一律识图探测(无表内加速)——识别出测试图(supported)
      // 才收起列表并选中;未通过则列表保持展开、该模型被拒绝并提示排查,
      // 用户可继续选择其他模型。
      async function handleVisionModelChoose(key) {
        if (visionProbingKey) return; // 探测中忽略其他点击
        setVisionProbeError(null);
        if (!key) {
          setVisionModelId('');
          setVisionModelPickerOpen(false);
          return;
        }
        const candidate = visionCandidates.find(item => item.id === key);
        if (!candidate || !bridge.available || !bridge.models.testImageInputCapability) {
          setVisionModelId(key);
          setVisionModelPickerOpen(false);
          return;
        }
        setVisionProbingKey(key); // 列表保持展开,该行右侧显示忙转圈
        try {
          const result = await bridge.models.testImageInputCapability(
            candidate.model || candidate.name || '',
            candidate.base_url || '',
            '',
            candidate.id,
          );
          if (result && result.status === 'supported') {
            setVisionModelId(key);
            setVisionModelPickerOpen(false); // 成功按最终结果收起列表
          } else {
            setVisionProbeError(settingsCopy.visionModelProbeError(
              result && result.summary ? result.summary : ''));
          }
        } catch (e) {
          setVisionProbeError(settingsCopy.visionModelProbeError(
            String(e && e.message ? e.message : e)));
        } finally {
          setVisionProbingKey(null);
        }
      }
      function makeModelId() {
        return 'm_' + Date.now().toString(36) + Math.random().toString(36).slice(2, 7);
      }
      function localCandidateRows(result) {
        const candidates = (result && Array.isArray(result.candidates)) ? result.candidates : [];
        return candidates.flatMap(candidate => {
          // 新后端 models 为 [{id, loaded}]；兼容旧后端的字符串数组。
          const entries = Array.isArray(candidate.models) && candidate.models.length
            ? candidate.models.map(m => (typeof m === 'string' ? { id: m, loaded: null } : m))
            : (candidate.model ? [{ id: candidate.model, loaded: null }] : []);
          return entries.map((entry, index) => ({
            key: `${candidate.base_url || 'local'}:${entry.id}`,
            model: entry.id,
            loaded: entry.loaded === undefined ? null : entry.loaded,
            base_url: candidate.base_url || '',
            provider: candidate.provider || 'local',
            label: candidate.label || settingsCopy.localModel,
            max_model_len: index === 0 ? candidate.max_model_len : null,
          })).filter(row => row.model && row.base_url);
        }).sort((a, b) => (a.loaded === false ? 1 : 0) - (b.loaded === false ? 1 : 0)); // 已加载/未知的排前，未加载的沉底
      }
      function buildLocalModelPayload(row) {
        return {
          id: makeModelId(),
          name: settingsCopy.localModelName(row.model),
          preset: 'local_vllm',
          context_window_tokens: row.max_model_len || null,
          max_output_tokens: null,
          model: row.model,
          base_url: row.base_url,
          api_key: '',
          credential_action: 'keep_existing',
        };
      }
      async function handleLocalDetect() {
        if (!bridge.available || !bridge.vllm.discoverLocalVllm || localDetecting) return;
        setLocalDetecting(true);
        setLocalDetectResult(null);
        try {
          const result = await bridge.vllm.discoverLocalVllm({
            currentBaseUrl: null,
            savedBaseUrl: null,
          });
          setLocalDetectResult({ candidates: (result && result.candidates) || [] });
        } catch (error) {
          setLocalDetectResult({ error: String(error || t.uiSettingsView.detectFailed) });
        } finally {
          setLocalDetecting(false);
        }
      }
      function startManualLocalModel() {
        const defs = MODEL_PRESET_DEFS.local_vllm;
        setPreset('local_vllm');
        setModel('');
        setBaseUrl(defs.baseUrl);
        setName(settingsCopy.localModelName(''));
        setContextWindow('');
        setMaxOutput('');
        setApiKey('');
        setKeyAction('keep_existing');
        setLocalKeyEnabled(false);
        setCustomModel(true);
        setPickerOpen(false);
        // 本地模型 → 手动添加是显式切换 route：丢弃草稿残留的思考深度，回落到 vLLM
        // 默认 off（防 SSE timeout）。否则新建 DeepSeek 草稿初始化的 high 会被当成
        // 合法 vLLM 档位保留，保存时显式写入 reasoning_effort=high，绕过桥接层
        // 「vllm→off」的默认约束。与 applyCatalogItem / chooseModel 的切换语义一致。
        setReasoningEffort(reasoningEffortForModelSwitch({ preset: 'local_vllm', model: '', vendor, base_url: defs.baseUrl }));
      }
      const catalogSectionTitleClass = `px-1 mb-2 text-[12px] leading-4 font-semibold text-[#8A8A8E] dark:text-[#8E8E93]`;
      const catalogGroupClass = `overflow-hidden rounded-[16px] bg-[#F2F2F7] dark:bg-[#2C2C2E]`;
      const formSectionTitle = `px-1 mb-1.5 text-[12px] leading-4 font-semibold text-[#8A8A8E] dark:text-[#8E8E93]`;
      const formGroup = `overflow-hidden rounded-[16px] bg-[#F2F2F7] dark:bg-[#2C2C2E]`;
      const formDivider = 'border-black/[0.10] dark:border-white/[0.10]';
      const renderProviderModelField = () => {
        const items = activeProvider ? activeProvider.items : [];
        const known = items.some(item => !item.custom && catalogItemMatchesModel(item, model));
        const selectedItem = known ? items.find(item => !item.custom && catalogItemMatchesModel(item, model)) : null;
        const selectedLabel = customModel || !known ? `${settingsCopy.customModel} ID` : ((selectedItem && selectedItem.title) || model);
        const chooseModel = (item) => {
          const nextModel = (!item || item.custom) ? '' : item.model;
          if (!item || item.custom) {
            setCustomModel(true);
            setModel('');
            if (!nameTouched) setName(activeProvider ? (activeProvider.configTitle || activeProvider.title) : selectedProvider);
          } else {
            setCustomModel(false);
            setModel(item.model);
            if (!nameTouched) setName(item.title || item.model);
          }
          // 同一 provider 内换模型时重置思考深度到新模型的默认档位：K2.6 选 off 后切 K3
          // 会残留不在 K3 档位表内的 off，界面无高亮且保存仍写旧值；与 applyCatalogItem 一致。
          setReasoningEffort(reasoningEffortForModelSwitch({ preset, model: nextModel, vendor, base_url: baseUrl }));
          setProviderModelPickerOpen(false);
        };
        return (
          <>
            <button
              type="button"
              onClick={() => setProviderModelPickerOpen(open => !open)}
              className={`w-full min-h-[54px] flex items-center gap-3 px-4 py-2.5 text-left border-b last:border-b-0 ${formDivider}`}
            >
              <span className={`shrink-0 text-[14px] leading-5 text-[#1C1C1E] dark:text-[#F2F2F7]`}>{t.uiSettingsView.modelLabel}</span>
              <span className={`min-w-0 flex-1 text-right text-[14px] leading-5 truncate text-[#1C1C1E] dark:text-[#F2F2F7]`}>{selectedLabel}</span>
              <ChevronDown
                size={16}
                className={`shrink-0 transition-transform ${providerModelPickerOpen ? 'rotate-180' : ''} text-[#8A8A8E] dark:text-[#8E8E93]`}
              />
            </button>
            {providerModelPickerOpen && (
              <div className={`border-b last:border-b-0 ${formDivider}`}>
                {items.map(item => {
                  const active = item.custom ? (customModel || !known) : (!customModel && catalogItemMatchesModel(item, model));
                  return (
                    <button
                      type="button"
                      key={item.custom ? '__custom__' : item.model}
                      onClick={() => chooseModel(item)}
                      className={`w-full min-h-[50px] flex items-center gap-3 pl-7 pr-4 py-2.5 text-left border-b last:border-b-0 border-black/[0.08] hover:bg-black/[0.035] dark:border-white/[0.08] dark:hover:bg-white/[0.06]`}
                    >
                      <span className="min-w-0 flex-1">
                        <span className={`block text-[14px] leading-5 truncate ${active ? ('text-[#007AFF] dark:text-[#64B5F6]') : ('text-[#1C1C1E] dark:text-[#F2F2F7]')}`}>{item.custom ? ((activeProvider && settingsCopy.customModelTitles[activeProvider.key]) || settingsCopy.customModelTitle(selectedProvider)) : (item.title || item.model || `${settingsCopy.customModel} ID`)}</span>
                        {item.desc && <span className={`block mt-0.5 text-[12px] leading-[16px] truncate text-[#8A8A8E] dark:text-[#8E8E93]`}>{item.custom
                          ? (activeProvider && activeProvider.providerKind === PROVIDER_KIND_CODING_PLAN ? settingsCopy.customCodingPlanDesc : (activeProvider.preset === 'local_vllm' ? settingsCopy.customLocalDesc : (activeProvider.preset === 'openai_compatible' ? settingsCopy.customCompatibleDesc : settingsCopy.customModelDesc)))
                          : (settingsCopy.modelDescriptions[item.desc] || item.desc)}</span>}
                      </span>
                      {active && <Check size={17} strokeWidth={2.4} className={'text-[#007AFF] dark:text-[#64B5F6]'} />}
                    </button>
                  );
                })}
              </div>
            )}
            {(customModel || !known) && renderInlineField({
              label: settingsCopy.modelId,
              value: model,
              onChange: e => handleModelIdChange(e.target.value),
              placeholder: isCodingPlan ? t.uiSettingsView.codingPlanModelIdPlaceholder : settingsCopy.modelIdPlaceholder,
            })}
          </>
        );
      };
      const renderInlineField = ({ label, value, onChange, placeholder, type = 'text', trailing, readOnly = false }) => (
        <div className={`min-h-[54px] flex items-center gap-3 px-4 py-2.5 border-b last:border-b-0 ${formDivider}`}>
          <label className={`shrink-0 text-[14px] leading-5 text-[#1C1C1E] dark:text-[#F2F2F7]`}>{label}</label>
          <input
            type={type}
            value={value}
            onChange={onChange}
            readOnly={readOnly}
            placeholder={placeholder}
            className={`min-w-0 flex-1 bg-transparent text-right text-[14px] leading-5 outline-none text-[#1C1C1E] placeholder:text-[#8A8A8E] dark:text-[#F2F2F7] dark:placeholder:text-[#636366] ${readOnly ? 'cursor-default' : ''}`}
          />
          {trailing}
        </div>
      );
      const renderCloudProviderPicker = () => {
        const bySection = ['coding_plan', 'official_api', 'custom'].map(section => ({
          section,
          title: settingsCopy.catalogSections[section] || MODEL_CATALOG_SECTIONS[section],
          groups: catalogGroups.filter(group => (group.section || 'official_api') === section),
        })).filter(item => item.groups.length > 0);
        return (
          <div className="space-y-4">
            {bySection.map(section => (
              <section key={section.section}>
                <div className={catalogSectionTitleClass}>{section.title}</div>
                <div className={catalogGroupClass}>
                  {section.groups.map(group => {
                    const first = group.items.find(item => !item.custom) || group.items[0] || {};
                    return (
                      <button
                        type="button"
                        key={group.key}
                        onClick={() => applyCatalogItem(group, first)}
                        className={`w-full min-h-[58px] px-3.5 py-2.5 flex items-center gap-3 text-left border-b last:border-b-0 border-black/[0.10] hover:bg-black/[0.035] dark:border-white/[0.10] dark:hover:bg-white/[0.06]`}
                      >
                        <ProviderIcon preset={group.preset} vendor={group.vendor} providerKind={group.providerKind} compact />
                        <span className="min-w-0 flex-1">
                          <span className={`block text-[15px] leading-5 font-normal truncate text-[#1C1C1E] dark:text-[#F2F2F7]`}>{group.title}</span>
                          <span className={`block mt-0.5 text-[12px] leading-[17px] truncate text-[#8A8A8E] dark:text-[#98989D]`}>{group.desc || first.desc || ''}</span>
                        </span>
                        <ChevronDown size={16} className={`-rotate-90 shrink-0 text-[#C7C7CC] dark:text-[#636366]`} />
                      </button>
                    );
                  })}
                </div>
              </section>
            ))}
          </div>
        );
      };
      const renderCatalogPicker = () => (
        <div className="space-y-4">
          {catalogGroups.map(group => (
            <section key={group.key}>
              <div className={catalogSectionTitleClass}>{group.providerKind === PROVIDER_KIND_CODING_PLAN ? group.title : presetProviderLabel(group.preset, t)}</div>
              <div className={catalogGroupClass}>
                {group.items.map(item => {
                  const active = preset === group.preset && !item.custom && catalogItemMatchesModel(item, model);
                  const itemTitle = item.custom ? (settingsCopy.customModelTitles[group.key] || settingsCopy.customModelTitle(presetProviderLabel(group.preset, t))) : item.title;
                  const itemDescription = item.custom
                    ? (group.providerKind === PROVIDER_KIND_CODING_PLAN ? settingsCopy.customCodingPlanDesc : (group.preset === 'local_vllm' ? settingsCopy.customLocalDesc : (group.preset === 'openai_compatible' ? settingsCopy.customCompatibleDesc : settingsCopy.customModelDesc)))
                    : (settingsCopy.modelDescriptions[item.desc] || item.desc);
                  return (
                    <button
                      type="button"
                      key={`${group.key}-${itemTitle}`}
                      onClick={() => applyCatalogItem(group, item)}
                      className={`w-full min-h-[56px] px-3.5 py-2.5 flex items-center gap-3 text-left border-b last:border-b-0 ${active ? 'bg-[#007AFF]/10' : ''} border-black/[0.10] hover:bg-black/[0.035] dark:border-white/[0.10] dark:hover:bg-white/[0.06]`}
                    >
                      <ProviderIcon preset={group.preset} vendor={group.vendor} providerKind={group.providerKind} compact />
                      <span className="min-w-0 flex-1">
                        <span className={`block text-[15px] leading-5 font-normal truncate text-[#1C1C1E] dark:text-[#F2F2F7]`}>{itemTitle}</span>
                        <span className={`block mt-0.5 text-[12px] leading-[17px] truncate text-[#8A8A8E] dark:text-[#98989D]`}>{itemDescription}</span>
                      </span>
                      {active ? <Check size={16} className="shrink-0 text-[#007AFF]" /> : <ChevronDown size={16} className={`-rotate-90 shrink-0 text-[#C7C7CC] dark:text-[#636366]`} />}
                    </button>
                  );
                })}
              </div>
            </section>
          ))}
        </div>
      );
      const renderLocalPicker = () => {
        const rows = localCandidateRows(localDetectResult);
        const mutedText = 'text-[#8A8A8E] dark:text-[#98989D]';
        const actionClass = `shrink-0 min-h-8 px-3 rounded-full text-[14px] font-medium bg-[#007AFF]/10 text-[#007AFF] hover:bg-[#007AFF]/16 dark:bg-[#0A84FF]/20 dark:text-[#0A84FF] dark:hover:bg-[#0A84FF]/28`;
        return (
          <div className="space-y-4">
            <section>
              <div className={catalogGroupClass}>
                <div className={`min-h-[56px] px-3.5 py-2.5 flex items-center gap-3 text-left border-b last:border-b-0 border-black/[0.10] dark:border-white/[0.10]`}>
                  <ProviderIcon preset="local_vllm" compact />
                  <span className="min-w-0 flex-1">
                    <span className={`block text-[15px] leading-5 font-normal truncate text-[#1C1C1E] dark:text-[#F2F2F7]`}>{settingsCopy.autoDetectLocalModel}</span>
                    <span className={`block mt-0.5 text-[12px] leading-[17px] truncate ${mutedText}`}>{settingsCopy.localDetectionTargets}</span>
                  </span>
                  <button type="button" disabled={localDetecting} onClick={handleLocalDetect}
                    className={`${actionClass} disabled:opacity-45`}>{localDetecting ? t.detectingLocalVllm : (localDetectResult ? settingsCopy.redetect : settingsCopy.detect)}</button>
                </div>
                {localDetectResult && localDetectResult.error && (
                  <div className={`px-3.5 py-3 text-[12px] leading-5 border-b last:border-b-0 border-black/[0.10] text-[#C5221F] dark:border-white/[0.10] dark:text-[#F28B82]`}>{localDetectResult.error}</div>
                )}
                {localDetectResult && !localDetectResult.error && rows.length === 0 && (
                  <div className={`px-3.5 py-3 text-[13px] leading-5 border-b last:border-b-0 border-black/[0.10] text-[#8A8A8E] dark:border-white/[0.10] dark:text-[#98989D]`}>{settingsCopy.noRunningLocalModel}</div>
                )}
                {rows.map(row => (
                  <div key={row.key} className={`min-h-[58px] px-3.5 py-2.5 flex items-center gap-3 text-left border-b last:border-b-0 border-black/[0.10] dark:border-white/[0.10]`}>
                    <ProviderIcon preset="local_vllm" compact />
                    <span className="min-w-0 flex-1">
                      <span className={`flex items-center gap-1.5 text-[15px] leading-5 font-normal text-[#1C1C1E] dark:text-[#F2F2F7]`}>
                        <span className="truncate">{row.model}</span>
                        {row.loaded === false && (
                          <span className={`shrink-0 text-[12px] px-2 py-0.5 rounded-md bg-[#E5E5EA] text-[#636366] dark:bg-white/[0.08] dark:text-[#C7C7CC]`}>{settingsCopy.modelNotLoadedTag}</span>
                        )}
                      </span>
                      <span className={`block mt-0.5 text-[12px] leading-[17px] truncate ${mutedText}`}>
                        {row.loaded === false ? `${row.label} · ${row.base_url} · ${settingsCopy.modelNotLoadedHint}` : `${row.label} · ${row.base_url}`}
                      </span>
                    </span>
                    <button type="button" onClick={() => onSave(buildLocalModelPayload(row))}
                      className={actionClass}>{settingsCopy.add}</button>
                  </div>
                ))}
              </div>
            </section>
            <section>
              <div className={catalogGroupClass}>
                <button type="button" onClick={startManualLocalModel}
                  className={`w-full min-h-[56px] px-3.5 py-2.5 flex items-center gap-3 text-left hover:bg-black/[0.035] dark:hover:bg-white/[0.06]`}>
                  <ProviderIcon preset="local_vllm" compact />
                  <span className="min-w-0 flex-1">
                    <span className={`block text-[15px] leading-5 font-normal truncate text-[#1C1C1E] dark:text-[#F2F2F7]`}>{settingsCopy.manualLocalModel}</span>
                    <span className={`block mt-0.5 text-[12px] leading-[17px] truncate ${mutedText}`}>{settingsCopy.manualLocalModelDesc}</span>
                  </span>
                  <ChevronDown size={16} className={`-rotate-90 shrink-0 text-[#C7C7CC] dark:text-[#636366]`} />
                </button>
              </div>
            </section>
          </div>
        );
      };
      // 图片输入能力 + 兜底视觉模型(阶段 G):与发送时后端复核同一组 SavedModel 字段。
      const imageCapabilityOptions = [
        // 自动探测(auto):保存时探测连接+识图并回填;能(enabled)/不能(disabled):
        // 人工钉死;pinvou 决策(pinvou):内置已验证表判断,不探测。
        { key: 'auto', label: settingsCopy.imageCapabilityAuto },
        { key: 'enabled', label: settingsCopy.imageCapabilityEnabled },
        { key: 'disabled', label: settingsCopy.imageCapabilityDisabled },
        { key: 'pinvou', label: settingsCopy.imageCapabilityPinvou },
      ];
      // 视觉兜底候选:显示除当前模型外的全部模型,不做能力过滤——选择时
      // 一律识图探测,supported 才允许选中(探测是唯一闸门;disabled 可能是
      // 历史探测误判残留,不应隐藏,如 kimi-for-coding)。
      const visionCandidates = (models || []).filter(item => item && item.id && item.id !== initial.id);
      const visionOptions = [{ key: '', label: settingsCopy.visionModelNone }]
        .concat(visionCandidates.map(item => ({ key: item.id, label: item.name || item.model })));
      const renderPickerRow = ({ testId, label, value, options, currentKey, open, onToggle, onChoose, probingKey, probeError }) => (
        <>
          <button
            type="button"
            data-testid={`${testId}-toggle`}
            onClick={onToggle}
            className={`w-full min-h-[54px] flex items-center gap-3 px-4 py-2.5 text-left border-b last:border-b-0 ${formDivider}`}
          >
            <span className={`shrink-0 text-[14px] leading-5 ${isDark ? 'text-[#F2F2F7]' : 'text-[#1C1C1E]'}`}>{label}</span>
            <span className={`min-w-0 flex-1 text-right text-[14px] leading-5 truncate ${isDark ? 'text-[#F2F2F7]' : 'text-[#1C1C1E]'}`}>{value}</span>
            <ChevronDown
              size={16}
              className={`shrink-0 transition-transform ${open ? 'rotate-180' : ''} ${isDark ? 'text-[#8E8E93]' : 'text-[#8A8A8E]'}`}
            />
          </button>
          {open && (
            <div className={`border-b last:border-b-0 ${formDivider}`}>
              {options.map(option => {
                const active = option.key === currentKey;
                const probing = probingKey === option.key;
                return (
                  <button
                    type="button"
                    data-testid={`${testId}-option-${option.key || 'none'}`}
                    key={option.key || '__none__'}
                    onClick={() => onChoose(option.key)}
                    disabled={probingKey ? !probing : false}
                    className={`w-full min-h-[50px] flex items-center gap-3 pl-7 pr-4 py-2.5 text-left border-b last:border-b-0 ${isDark ? 'border-white/[0.08] hover:bg-white/[0.06]' : 'border-black/[0.08] hover:bg-black/[0.035]'} ${probing ? 'opacity-70' : ''}`}
                  >
                    <span className={`min-w-0 flex-1 text-[14px] leading-5 truncate ${active ? (isDark ? 'text-[#64B5F6]' : 'text-[#007AFF]') : (isDark ? 'text-[#F2F2F7]' : 'text-[#1C1C1E]')}`}>{option.label}</span>
                    {probing ? (
                      <span data-testid={`${testId}-probing`} className="shrink-0 flex items-center gap-1.5 text-[12px] leading-4 text-[#0A84FF]">
                        <RefreshCw size={13} className="animate-spin" />
                        {settingsCopy.visionModelProbing}
                      </span>
                    ) : active ? <Check size={17} strokeWidth={2.4} className={isDark ? 'text-[#64B5F6]' : 'text-[#007AFF]'} /> : null}
                  </button>
                );
              })}
              {probeError && (
                <div data-testid={`${testId}-probe-error`} className={`px-7 py-2.5 text-[12px] leading-4 border-b last:border-b-0 ${isDark ? 'text-[#FFD60A] border-white/[0.08]' : 'text-[#B25E00] border-black/[0.08]'}`}>{probeError}</div>
              )}
            </div>
          )}
        </>
      );
      const renderImageInputSection = () => {
        const capabilityLabel = (imageCapabilityOptions.find(option => option.key === imageCapability) || imageCapabilityOptions[0]).label;
        const visionLabel = (visionOptions.find(option => option.key === visionModelId) || visionOptions[0]).label;
        // 结果文案:supported 附模型回复摘要;仅当结果为 supported 且档位为 auto 时提示可设「支持图片」。
        // unverified(未识别出测试色 / 400 非图片拒绝)统一「原因未知」,不得宣称支持或不支持。
        const imageTestText = !imageTestResult
          ? settingsCopy.imageCapabilityTestHint
          : imageTestResult.status === 'supported'
            ? settingsCopy.imageCapabilityTestSupported
              + (imageTestResult.summary ? ` · ${settingsCopy.imageCapabilityTestReply(imageTestResult.summary)}` : '')
              + (imageCapability === 'auto' ? ` · ${settingsCopy.imageCapabilityTestEnableHint}` : '')
            : imageTestResult.status === 'unsupported'
              ? settingsCopy.imageCapabilityTestUnsupported + (imageTestResult.summary ? ` · ${imageTestResult.summary}` : '')
              : imageTestResult.status === 'unverified'
                // 后端 summary 已自带「未能正确识别图像，原因未知」完整句,直接展示避免重复。
                ? (imageTestResult.summary || settingsCopy.imageCapabilityTestUnverified)
                : settingsCopy.imageCapabilityTestError + (imageTestResult.summary ? ` · ${imageTestResult.summary}` : '');
        const imageTestColor = !imageTestResult
          ? (isDark ? 'text-[#98989D]' : 'text-[#8A8A8E]')
          : imageTestResult.status === 'supported'
            ? (isDark ? 'text-[#93D5A6]' : 'text-[#137333]')
            : imageTestResult.status === 'unsupported'
              ? (isDark ? 'text-[#FFD60A]' : 'text-[#FF9500]')
              : imageTestResult.status === 'unverified'
                ? (isDark ? 'text-[#FFD60A]' : 'text-[#B25E00]')
                : 'text-[#FF3B30]';
        return (
          <section>
            <div className={formGroup}>
              {renderPickerRow({
                testId: 'image-capability',
                label: settingsCopy.imageCapability,
                value: capabilityLabel,
                options: imageCapabilityOptions,
                currentKey: imageCapability,
                open: imageCapabilityPickerOpen,
                onToggle: () => { setImageCapabilityPickerOpen(open => !open); setVisionModelPickerOpen(false); },
                onChoose: key => { setImageCapability(key); setImageCapabilityPickerOpen(false); },
              })}
              {renderPickerRow({
                testId: 'vision-model',
                label: settingsCopy.visionModel,
                value: visionLabel,
                options: visionOptions,
                currentKey: visionModelId,
                open: visionModelPickerOpen,
                onToggle: () => { setVisionModelPickerOpen(open => !open); setImageCapabilityPickerOpen(false); },
                onChoose: handleVisionModelChoose,
                // 选择探测:探测中该行右侧显示忙转圈,未通过在该行下方提示排查。
                probingKey: visionProbingKey,
                probeError: visionProbeError,
              })}
            </div>
            <div className={`px-1 mt-1.5 text-[12px] leading-4 ${isDark ? 'text-[#8E8E93]' : 'text-[#8A8A8E]'}`}>{settingsCopy.visionModelDesc}</div>
            {/* §11.8/§11.9 静态隐私说明:云端模型图片随消息外发,本地模型图片不离开本机。 */}
            <div data-testid="image-privacy-desc" className={`px-1 mt-1 text-[12px] leading-4 ${isDark ? 'text-[#8E8E93]' : 'text-[#8A8A8E]'}`}>{settingsCopy.imagePrivacyDesc}</div>
            <div className={`mt-3 ${formGroup}`}>
              <div className={`min-h-[54px] flex items-center gap-3 px-4 py-2.5 border-b last:border-b-0 ${formDivider}`}>
                <span data-testid="image-capability-test-result" className={`min-w-0 flex-1 text-[13px] leading-5 ${imageTestColor}`}>
                  {imageTestText}
                </span>
                <button type="button" data-testid="image-capability-test" onClick={handleImageCapabilityTest}
                  disabled={imageTesting || !model.trim() || !baseUrl.trim()}
                  className={`shrink-0 min-h-8 px-3 rounded-full text-[14px] font-medium disabled:opacity-45 ${isDark ? 'bg-[#0A84FF]/20 text-[#0A84FF] hover:bg-[#0A84FF]/28' : 'bg-[#007AFF]/10 text-[#007AFF] hover:bg-[#007AFF]/16'}`}>
                  {imageTesting ? t.testingConn : settingsCopy.imageCapabilityTest}
                </button>
              </div>
            </div>
          </section>
        );
      };
      if (initial.__new && pickerOpen) {
        return (
          <div data-testid="model-form-backdrop" className="fixed inset-0 z-[100] flex items-center justify-center bg-black/45 px-4 animate-in fade-in duration-150">
            <div data-testid="model-form-dialog" role="dialog" aria-modal="true"
              onClick={e => e.stopPropagation()}
              className={`w-[440px] max-w-[90vw] max-h-[76vh] overflow-y-auto custom-scrollbar rounded-[22px] shadow-2xl bg-white text-[#1C1C1E] dark:bg-[#1C1C1E] dark:text-[#F2F2F7]`}>
              <div className={`px-5 py-4 flex items-start justify-between gap-4 border-b border-black/[0.10] dark:border-white/[0.10]`}>
                <div>
                  <h2 className="text-[20px] leading-6 font-semibold">{t.modelFormAddTitle}</h2>
                  <p className={`mt-1 text-[13px] leading-[18px] text-[#8A8A8E] dark:text-[#98989D]`}>{settingsCopy.chooseModelDesc}</p>
                </div>
                <button data-testid="model-form-cancel" onClick={onCancel} className={`h-9 w-9 shrink-0 rounded-full flex items-center justify-center bg-[#E5E5EA] text-[#636366] dark:bg-white/[0.08] dark:text-[#C7C7CC]`}><X size={18} /></button>
              </div>
              <div className="px-5 pt-4">
                <div className={`p-1 rounded-full grid grid-cols-2 gap-1 bg-[#F2F2F7] dark:bg-[#2C2C2E]`}>
                  {[
                    { key: 'cloud', label: settingsCopy.cloudModels },
                    { key: 'local', label: settingsCopy.localModels },
                  ].map(tab => (
                    <button key={tab.key} type="button" onClick={() => setPickerTab(tab.key)}
                      className={`h-9 rounded-full text-[14px] font-medium transition-colors ${pickerTab === tab.key ? ('bg-white text-[#007AFF] shadow-sm dark:bg-[#3A3A3C] dark:text-[#F2F2F7]') : ('text-[#636366] dark:text-[#C7C7CC]')}`}>
                      {tab.label}
                    </button>
                  ))}
                </div>
              </div>
              <div className="px-5 py-4">{pickerTab === 'local' ? renderLocalPicker() : renderCloudProviderPicker()}</div>
            </div>
          </div>
        );
      }
      return (
        <div data-testid="model-form-backdrop" className="fixed inset-0 z-[100] flex items-center justify-center bg-black/50 animate-in fade-in duration-150">
          <div data-testid="model-form-dialog" role="dialog" aria-modal="true" onClick={e => e.stopPropagation()}
            className={`w-[430px] max-w-[90vw] max-h-[76vh] overflow-y-auto custom-scrollbar rounded-[22px] shadow-2xl bg-white text-[#1C1C1E] dark:bg-[#1C1C1E] dark:text-[#F2F2F7]`}>
            <div className={`px-5 py-4 flex items-start justify-between gap-4 border-b ${formDivider}`}>
              <div>
                <h2 className="text-[20px] leading-6 font-semibold">{modalTitle}</h2>
                <p className={`mt-1 text-[13px] leading-[18px] text-[#8A8A8E] dark:text-[#98989D]`}>{isLocalPreset ? selectedModelLabel : `${isCodingPlan ? `Coding Plan · ${settingsCopy.toolCalling}` : selectedProvider + ' · ' + selectedModelLabel}`}</p>
              </div>
              <button data-testid="model-form-cancel" onClick={onCancel} className={`h-9 w-9 shrink-0 rounded-full flex items-center justify-center bg-[#E5E5EA] text-[#636366] dark:bg-white/[0.08] dark:text-[#C7C7CC]`}><X size={18} /></button>
            </div>
            <div className="space-y-4 px-5 py-4">
              <div className={`overflow-hidden rounded-[18px] border border-black/[0.08] bg-white dark:border-white/[0.10] dark:bg-[#2C2C2E]`}>
                {isLocalPreset ? (
                  <div className="w-full min-h-[62px] px-4 py-3 flex items-center gap-3 text-left">
                    <ProviderIcon preset={preset} vendor={vendor} providerKind={providerKind} compact />
                    <span className="min-w-0 flex-1">
                      <span className="block text-[15px] leading-5 font-normal truncate">{selectedProvider}</span>
                      <span className={`block mt-0.5 text-[12px] leading-[17px] truncate text-[#8A8A8E] dark:text-[#98989D]`}>{selectedModelLabel}</span>
                    </span>
                  </div>
                ) : (
                  <button
                    type="button"
                    onClick={() => setPickerOpen(v => !v)}
                    className={`w-full min-h-[62px] px-4 py-3 flex items-center gap-3 text-left hover:bg-black/[0.035] dark:hover:bg-white/[0.05]`}
                  >
                    <ProviderIcon preset={preset} vendor={vendor} providerKind={providerKind} compact />
                    <span className="min-w-0 flex-1">
                      <span className="block text-[15px] leading-5 font-normal truncate">{selectedProvider}</span>
                      <span className={`block mt-0.5 text-[12px] leading-[17px] truncate text-[#8A8A8E] dark:text-[#98989D]`}>{selectedModelLabel}</span>
                    </span>
                    <span className="shrink-0 text-[14px] text-[#007AFF]">{pickerOpen ? settingsCopy.collapse : settingsCopy.change}</span>
                  </button>
                )}
                {pickerOpen && !isLocalPreset && (
                  <div className={`border-t px-4 py-4 border-black/[0.12] dark:border-white/[0.10]`}>
                    {renderCatalogPicker()}
                  </div>
                )}
              </div>
              {!isLocalPreset && !customModel && (
                <section>
                  <div className={formGroup}>
                    <div className="min-h-[54px] flex items-center gap-3 px-4 py-2.5">
                      <label className={`shrink-0 text-[14px] leading-5 text-[#1C1C1E] dark:text-[#F2F2F7]`}>API Key</label>
                      <input type={showKey ? 'text' : 'password'} autoComplete="off" value={apiKey} onChange={e => { setApiKey(e.target.value); if (e.target.value.trim()) setKeyAction('replace'); }}
                        placeholder={hasSavedKey ? '••••••••' : settingsCopy.apiKeyPlaceholder}
                        className={`min-w-0 flex-1 bg-transparent text-right text-[14px] leading-5 outline-none text-[#1C1C1E] placeholder:text-[#8A8A8E] dark:text-[#F2F2F7] dark:placeholder:text-[#636366]`} />
                      <button type="button" onClick={toggleApiKeyVisibility} className="shrink-0 text-[14px] text-[#007AFF]">{showKey ? settingsCopy.hide : settingsCopy.show}</button>
                    </div>
                  </div>
                  {keyRevealError && <div className="px-1 mt-1.5 text-[12px] leading-4 text-[#FF3B30]">{keyRevealError}</div>}
                </section>
              )}
              {showConfigFields && (
                <section>
                  <div className={formGroup}>
                    {showDisplayNameField && renderInlineField({
                      label: t.modelDisplayName,
                      value: name,
                      onChange: e => { setNameTouched(true); setName(e.target.value); },
                      placeholder: settingsCopy.localModel,
                    })}
                    {showProviderModelField && renderProviderModelField()}
                    {showModelIdField && !showProviderModelField && renderInlineField({ label: isLocalPreset ? settingsCopy.localModelId : settingsCopy.modelId, value: model, onChange: e => handleModelIdChange(e.target.value), placeholder: isLocalPreset ? '' : settingsCopy.modelIdPlaceholder })}
                    {showCustomCloudKeyField && (
                      <div className={`min-h-[54px] flex items-center gap-3 px-4 py-2.5 border-b last:border-b-0 ${formDivider}`}>
                        <label className={`shrink-0 text-[14px] leading-5 text-[#1C1C1E] dark:text-[#F2F2F7]`}>API Key</label>
                        <input type={showKey ? 'text' : 'password'} autoComplete="off" value={apiKey} onChange={e => { setApiKey(e.target.value); if (e.target.value.trim()) setKeyAction('replace'); }}
                          placeholder={hasSavedKey ? '••••••••' : settingsCopy.apiKeyPlaceholder}
                          className={`min-w-0 flex-1 bg-transparent text-right text-[14px] leading-5 outline-none text-[#1C1C1E] placeholder:text-[#8A8A8E] dark:text-[#F2F2F7] dark:placeholder:text-[#636366]`} />
                        <button type="button" onClick={toggleApiKeyVisibility} className="shrink-0 text-[14px] text-[#007AFF]">{showKey ? settingsCopy.hide : settingsCopy.show}</button>
                      </div>
                    )}
                    {showBaseUrlField && renderInlineField({ label: t.customBaseUrl, value: baseUrl, onChange: e => handleBaseUrlChange(e.target.value) })}
                    {isLocalPreset && (
                      <div className={`min-h-[54px] flex items-center gap-3 px-4 py-2.5 border-b last:border-b-0 ${formDivider}`}>
                        <label className={`shrink-0 text-[14px] leading-5 text-[#1C1C1E] dark:text-[#F2F2F7]`}>{settingsCopy.apiKeyRequired}</label>
                        <button type="button" onClick={() => setLocalKeyEnabled(v => !v)}
                          className={`ml-auto h-8 min-w-[52px] rounded-full px-1 flex items-center transition-colors ${localKeyEnabled ? 'bg-[#007AFF]' : ('bg-[#D1D1D6] dark:bg-[#3A3A3C]')}`}
                          aria-pressed={localKeyEnabled}>
                          <span className={`block h-6 w-6 rounded-full bg-white shadow-sm transition-transform ${localKeyEnabled ? 'translate-x-5' : 'translate-x-0'}`} />
                        </button>
                      </div>
                    )}
                    {showLocalKeyField && (
                      <div className={`min-h-[54px] flex items-center gap-3 px-4 py-2.5 border-b last:border-b-0 ${formDivider}`}>
                        <label className={`shrink-0 text-[14px] leading-5 text-[#1C1C1E] dark:text-[#F2F2F7]`}>API Key</label>
                        <input type={showKey ? 'text' : 'password'} autoComplete="off" value={apiKey} onChange={e => { setApiKey(e.target.value); if (e.target.value.trim()) setKeyAction('replace'); }}
                          placeholder={hasSavedKey ? '••••••••' : settingsCopy.apiKeyPlaceholder}
                          className={`min-w-0 flex-1 bg-transparent text-right text-[14px] leading-5 outline-none text-[#1C1C1E] placeholder:text-[#8A8A8E] dark:text-[#F2F2F7] dark:placeholder:text-[#636366]`} />
                        <button type="button" onClick={toggleApiKeyVisibility} className="shrink-0 text-[14px] text-[#007AFF]">{showKey ? settingsCopy.hide : settingsCopy.show}</button>
                      </div>
                    )}
                  </div>
                  {keyRevealError && <div className="px-1 mt-1.5 text-[12px] leading-4 text-[#FF3B30]">{keyRevealError}</div>}
                </section>
              )}
              {renderImageInputSection()}
              {showConfigFields && reasoningEffortTiers.length > 0 && (
                <section>
                  <div className={formGroup}>
                    <div className="min-h-[54px] flex items-center gap-3 px-4 py-2.5">
                      <span className={`shrink-0 text-[14px] leading-5 text-[#1C1C1E] dark:text-[#F2F2F7]`}>{settingsCopy.reasoningEffort}</span>
                      <div className="ml-auto flex flex-wrap justify-end gap-1">
                        {reasoningEffortTiers.map(tier => (
                          <button
                            key={tier}
                            type="button"
                            onClick={() => setReasoningEffort(tier)}
                            className={`h-7 min-w-[52px] px-3 rounded-full text-[13px] font-medium transition-colors ${
                              reasoningEffort === tier
                                ? 'bg-[#007AFF] text-white dark:bg-[#0A84FF]'
                                : 'bg-[#E5E5EA] text-[#636366] hover:bg-[#D9D9DE] dark:bg-white/[0.07] dark:text-[#C7C7CC] dark:hover:bg-white/[0.12]'
                            }`}
                          >{settingsCopy.reasoningEffortTiers[tier] || tier}</button>
                        ))}
                      </div>
                    </div>
                  </div>
                </section>
              )}
              {showConfigFields && (
                <section>
                  <div className={formGroup}>
                    <div className="min-h-[54px] flex items-center gap-3 px-4 py-2.5">
                      <span className={`min-w-0 flex-1 text-[13px] leading-5 ${testResult ? (testResult.ok ? ('text-[#137333] dark:text-[#93D5A6]') : 'text-[#FF3B30]') : ('text-[#8A8A8E] dark:text-[#98989D]')}`}>
                        {testResult ? testResult.message : settingsCopy.testBeforeSave}
                      </span>
                      <button type="button" onClick={handleTest} disabled={testing || !baseUrl.trim()}
                        className={`shrink-0 min-h-8 px-3 rounded-full text-[14px] font-medium disabled:opacity-45 bg-[#007AFF]/10 text-[#007AFF] hover:bg-[#007AFF]/16 dark:bg-[#0A84FF]/20 dark:text-[#0A84FF] dark:hover:bg-[#0A84FF]/28`}>
                        {testing ? t.testingConn : t.testConnection}
                      </button>
                    </div>
                  </div>
                </section>
              )}
              {preset === 'local_vllm' && detectResult && (
                <div className={`rounded-xl border p-3 space-y-2 border-[#E0E3E7] bg-[#F8F9FB] dark:border-[#333537] dark:bg-[#131314]`}>
                  {detectResult.error ? (
                    <span className={`text-[12px] text-[#C5221F] dark:text-[#F28B82]`}>{t.vllmDetectError(detectResult.error)}</span>
                  ) : detectResult.engineState === 'starting' ? (
                    <span className={`text-[12px] text-[#0B57D0] dark:text-[#A8C7FA]`}>{t.vllmDetectStarting}</span>
                  ) : detectResult.candidates.length === 0 ? (
                    <span className={`text-[12px] text-[#5F6368] dark:text-[#9AA0A6]`}>{t.vllmDetectNone}</span>
                  ) : (
                    <>
                      <span className={`text-[12px] text-[#137333] dark:text-[#93D5A6]`}>{t.vllmDetectFound(detectResult.candidates.length)}</span>
                      {detectResult.candidates.map(c => (
                        <button key={c.base_url} onClick={() => applyCandidate(c)}
                          className={`w-full text-left rounded-lg border px-3 py-2 transition-colors border-[#E0E3E7] hover:bg-[#F0F4F9] dark:border-[#333537] dark:hover:bg-[#2A2B2D]`}>
                          <div className={`text-[13px] truncate text-[#1F1F1F] dark:text-[#E3E3E3]`}>{c.base_url}</div>
                          <div className={`text-[11px] truncate text-[#5F6368] dark:text-[#9AA0A6]`}>
                            {vllmStatusLabel(c.status)}
                            {c.model ? ` · ${t.vllmDetectedModel}: ${c.model}` : ''}
                            {c.max_model_len ? ` · ${t.vllmDetectedContext}: ${c.max_model_len}` : ''}
                          </div>
                        </button>
                      ))}
                    </>
                  )}
                  <span className={`text-[11px] block text-[#9AA0A6] dark:text-[#5F6368]`}>{t.vllmDetectHint}</span>
                </div>
              )}
              {preset === 'local_vllm' && canSetUpLocalModel && (offerSetup || bootstrapHere) && (
                <div className={`rounded-xl border p-3 border-[#E0E3E7] bg-[#F8F9FB] dark:border-[#333537] dark:bg-[#131314]`}>
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
                            className={`h-8 px-4 rounded-lg text-[13px] bg-[#F0F4F9] text-[#1F1F1F] dark:bg-[#2B2C2F] dark:text-[#E3E3E3]`}>{t.cpCancel}</button>
                          <button onClick={() => bridge.vllm.bootstrapLocalVllm()}
                            className="h-8 px-4 rounded-lg text-[13px] font-medium text-white" style={{ background: '#0A84FF' }}>{t.vllmSetupRetry}</button>
                        </div>
                      </div>
                    ) : (
                      <VllmSetupProgress phase={bs && bs.vllmSetupPhase} attempt={(bs && bs.vllmSetupAttempt) || 0} t={t} />
                    )
                  ) : (
                    <div>
                      <div className="text-[13px] leading-relaxed mb-3">{t.vllmReentryOffer}</div>
                      <div className="flex justify-end gap-2">
                        <button onClick={() => setOfferSetup(false)}
                          className={`h-8 px-4 rounded-lg text-[13px] bg-[#F0F4F9] text-[#1F1F1F] dark:bg-[#2B2C2F] dark:text-[#E3E3E3]`}>{t.cpCancel}</button>
                        <button onClick={() => { setBootstrapHere(true); bridge.vllm.bootstrapLocalVllm(); }}
                          className="h-8 px-4 rounded-lg text-[13px] font-medium text-white" style={{ background: '#0A84FF' }}>{t.vllmSetupEnable}</button>
                      </div>
                    </div>
                  )}
                </div>
              )}
            </div>
            {/* 保存失败行内提示:弹窗保持,交用户修正后重试。 */}
            {saveError && (
              <div data-testid="model-form-save-error" className={`px-5 py-3 border-t ${formDivider}`}>
                <div className={`text-[13px] leading-5 ${isDark ? 'text-[#FF453A]' : 'text-[#D70015]'}`}>
                  {settingsCopy.imageCapabilitySaveFailed(saveError)}
                </div>
              </div>
            )}
            {/* 「保存时检测」明确不支持:后端未写盘,弹窗保持,交用户决策。
                summary 为 provider/模型回复原文,由 i18n 函数 key 决定是否展示。 */}
            {probeDecision && (
              <div data-testid="image-probe-decision" className={`px-5 py-3 border-t ${formDivider}`}>
                <div className={`text-[13px] leading-5 ${isDark ? 'text-[#FFD60A]' : 'text-[#B25E00]'}`}>
                  {settingsCopy.imageCapabilityDecisionTitle}
                  {settingsCopy.imageCapabilityDecisionDetail(probeDecision.summary)}
                </div>
                <div className="mt-2 flex flex-wrap gap-2">
                  <button data-testid="image-probe-retest" disabled={savingModel} onClick={() => { setProbeDecision(null); doSave(); }}
                    className={`h-8 px-3 rounded-full text-[13px] font-medium disabled:opacity-35 ${isDark ? 'bg-[#0A84FF]/20 text-[#0A84FF]' : 'bg-[#007AFF]/10 text-[#007AFF]'}`}>
                    {settingsCopy.imageCapabilityDecisionRetest}
                  </button>
                  <button data-testid="image-probe-configure-vision" disabled={savingModel} onClick={() => setProbeDecision(null)}
                    className={`h-8 px-3 rounded-full text-[13px] font-medium disabled:opacity-35 ${isDark ? 'bg-white/[0.08] text-[#C7C7CC]' : 'bg-[#E5E5EA] text-[#636366]'}`}>
                    {settingsCopy.imageCapabilityDecisionConfigureVision}
                  </button>
                  <button data-testid="image-probe-save-auto" disabled={savingModel} onClick={() => { setProbeDecision(null); doSave(true); }}
                    className={`h-8 px-3 rounded-full text-[13px] font-medium disabled:opacity-35 ${isDark ? 'bg-white/[0.08] text-[#C7C7CC]' : 'bg-[#E5E5EA] text-[#636366]'}`}>
                    {settingsCopy.imageCapabilityDecisionSaveAuto}
                  </button>
                </div>
              </div>
            )}
            <div className={`flex justify-end gap-2 px-5 py-4 border-t ${formDivider}`}>
              <button data-testid="model-form-cancel" onClick={onCancel} className={`h-10 px-4 rounded-full text-[15px] font-normal transition-colors text-[#007AFF] hover:bg-black/[0.04] dark:text-[#0A84FF] dark:hover:bg-white/[0.06]`}>{t.cpCancel}</button>
              <button data-testid="model-form-save" onClick={() => doSave()} disabled={!canSave || savingModel}
                className="h-10 px-5 rounded-full bg-[#007AFF] text-white text-[15px] font-semibold transition-colors disabled:opacity-35">
                {savingModel ? settingsCopy.imageCapabilitySaving : t.modelSaveBtn}
              </button>
            </div>
          </div>
        </div>
      );
    };

    const SettingsView = ({ activeTheme, setActiveTheme, language, setLanguage, superPerm, setSuperPerm, taskCompletedNotif, setTaskCompletedNotif, searchProvider, setSearchProvider, enabledSearchProviders = ['bing'], onAddSearchProvider, onDeleteSearchProvider, searchApiKey, setSearchApiKey, searchHasSavedKey, savedModels, activeModelId, onSaveModel, onDeleteModel, onSetActiveModel, onSaveSearchConfig, onConfirmSearchConfig, onMemoryEnabledChange, onPetEnabledChange, searchNeedsRestart, languageNeedsRestart, bs, t, sidebarDateGrouping = true, onSidebarDateGroupingChange, updateFocusTick, onCloseSettings, initialSection = 'general' }) => {
      const settingsCopy = t.uiSettingsDetail;
      const platformCapabilities = (bs && bs.platformCapabilities) || {};
      const showSuperPermissionSettings = !!platformCapabilities.showSuperPermissionSettings;
      const usesBundledDependencyInstaller = !!platformCapabilities.usesBundledDependencyInstaller;
      const usesHomebrewDependencyInstaller = !!platformCapabilities.usesHomebrewDependencyInstaller;
      // 「ACP 管理」（原 Provider 管理）并入模型设置页：activeSection 用 'model'，
      // modelTab 区分「模型 / ACP 管理」两个子页；深链 initialSection='providers'
      // （代码页错误横幅等入口）映射为模型页 + ACP 子页。
      const [activeSection, setActiveSection] = useState(initialSection === 'providers' ? 'model' : (initialSection || 'general'));
      const [modelTab, setModelTab] = useState(initialSection === 'providers' ? 'acp' : 'models');
      const canUsePet = can('pet');
      const canUseSuperPermission = can('superPermission');
      const canUpdateApp = can('appUpdate');
      const canInstallDependencies = can('dependencyInstall');
      const canConfigureDesktopNotifications = can('desktopNotifications');
      const canManageModels = can('modelManagement');
      const acpProvidersTabVisible = !!platformCapabilities.codexAcpSupported;
      const canPickHostFiles = can('hostFilePicker');
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
      const versionUpdateRef = useRef(null);
      const hasUpdate = !!(bs && bs.updateInfo && bs.updateInfo.available);
      const memorySettingsVisible = !!(bs && bs.settings && bs.settings.language === 'zh-Hans');
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
        if (!canUpdateApp || !updateFocusTick || !versionUpdateRef.current) return;
        requestAnimationFrame(() => {
          versionUpdateRef.current && versionUpdateRef.current.scrollIntoView({ behavior: 'smooth', block: 'center' });
        });
      }, [canUpdateApp, updateFocusTick]);
      useEffect(() => {
        if (initialSection === 'providers') {
          setActiveSection('model');
          setModelTab('acp');
        } else if (initialSection) {
          setActiveSection(initialSection);
        }
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
      // 进设置页自动体检一次可选依赖装齐没; 之后用户可手动「重新检测」
      useEffect(() => {
        if (!canInstallDependencies || !bridge.available || (bs && (bs.deps || bs.depsChecking))) return;
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
          {title && <div className={`px-3 mb-2 text-[12px] font-semibold text-[#8A8A8E] dark:text-[#8E8E93]`}>{title}</div>}
          <div className={`overflow-hidden rounded-[18px] bg-white dark:bg-[#2C2C2E]`}>{children}</div>
          {footer && <div className={`px-3 mt-2 text-[12px] leading-relaxed text-[#8A8A8E] dark:text-[#8E8E93]`}>{footer}</div>}
        </section>
      );
      const IOSRow = ({ label, desc, value, children, onClick, danger }) => {
        const RowTag = onClick ? 'button' : 'div';
        return (
        <RowTag
          type={onClick ? 'button' : undefined}
          onClick={onClick}
          className={`w-full min-h-[58px] flex flex-wrap items-center gap-3 px-4 py-2.5 text-left border-b last:border-b-0 max-sm:flex-col max-sm:items-stretch ${
            'border-black/[0.12] text-[#1C1C1E] dark:border-white/[0.10] dark:text-[#F2F2F7]'
          } ${onClick ? ('hover:bg-black/[0.035] dark:hover:bg-white/[0.05]') : ''}`}
        >
          <div className="flex-1 min-w-[120px] max-sm:min-w-0">
            <div className={`text-[15px] leading-5 font-normal whitespace-nowrap ${danger ? 'text-[#FF3B30]' : ''}`}>{label}</div>
            {desc && <div className={`mt-0.5 text-[13px] leading-5 text-[#8A8A8E] dark:text-[#98989D]`}>{desc}</div>}
          </div>
          {value && <div className={`text-[14px] shrink-0 text-[#8A8A8E] dark:text-[#98989D]`}>{value}</div>}
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
          className={`relative h-[26px] w-[46px] shrink-0 rounded-full transition-colors ${checked ? 'bg-[#34C759]' : ('bg-[#E5E5EA] dark:bg-[#3A3A3C]')}`}
        >
          <span className={`absolute left-0 top-[2px] h-[22px] w-[22px] rounded-full bg-white shadow transition-transform ${checked ? 'translate-x-[22px]' : 'translate-x-[2px]'}`} />
        </button>
      );
      const SectionButton = ({ id, icon, label, dot }) => (
        <button
          type="button"
          data-testid={`settings-section-${id}`}
          onClick={() => setActiveSection(id)}
          className={`w-full h-10 px-3 rounded-[14px] flex items-center gap-2.5 text-[14px] transition-colors max-sm:w-auto max-sm:shrink-0 ${
            activeSection === id
              ? ('bg-[#D8EAFE] text-[#007AFF] dark:bg-[#173A5E] dark:text-[#64B5F6]')
              : ('text-[#1C1C1E] hover:bg-black/[0.04] dark:text-[#F2F2F7] dark:hover:bg-white/[0.06]')
          }`}
        >
          <span className={`w-7 h-7 rounded-[9px] flex items-center justify-center ${activeSection === id ? 'bg-[#007AFF]/10' : ('bg-black/[0.05] dark:bg-white/[0.08]')}`}>{icon}</span>
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
        <div className={`overflow-hidden rounded-[18px] border bg-white border-black/[0.03] dark:bg-[#2C2C2E] dark:border-white/[0.04]`}>{children}</div>
      );
      const SectionTitle = ({ children }) => (
        <div className={`px-3 mb-2 text-[12px] leading-4 font-semibold text-[#8A8A8E] dark:text-[#8E8E93]`}>{children}</div>
      );
      const RadioDot = ({ active }) => (
        <span className={`block w-5 h-5 rounded-full border-[3px] ${active ? 'border-[#007AFF]' : ('border-[#AEAEB2] dark:border-[#636366]')}`}>
          {active && <span className="block w-2 h-2 rounded-full bg-[#007AFF] mx-auto mt-[3px]" />}
        </span>
      );
      const Tag = ({ children, tone = 'green' }) => (
        <span className={`shrink-0 text-[12px] px-2 py-0.5 rounded-md ${
          tone === 'gray'
            ? ('bg-[#E5E5EA] text-[#636366] dark:bg-white/[0.08] dark:text-[#C7C7CC]')
            : 'bg-[#34C759]/15 text-[#248A3D]'
        }`}>{children}</span>
      );
      const userModels = visibleSortedModels(savedModels || []);
      const searchOptions = [
        { key: 'bing', label: 'Bing', desc: settingsCopy.searchDescriptions.bing },
        { key: 'metaso', label: t.uiSettingsView.searchProviderMetaso, desc: settingsCopy.searchDescriptions.metaso },
        { key: 'bocha', label: t.uiSettingsView.searchProviderBocha, desc: settingsCopy.searchDescriptions.bocha },
        { key: 'baidu', label: t.uiSettingsView.searchProviderBaidu, desc: settingsCopy.searchDescriptions.baidu },
        { key: 'tavily', label: 'Tavily', desc: settingsCopy.searchDescriptions.tavily },
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
          name: preset === 'local_vllm' ? settingsCopy.localDefaultName : presetProviderLabel(preset, t),
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
      const memoryWarning = Array.isArray(memory.warnings) ? memory.warnings[0] : null;
      const memoryWarningCode = memoryWarning && typeof memoryWarning === 'object' ? memoryWarning.code : '';
      const memoryError = memory.error || memoryWarning;
      const memoryErrorMessage = memory.error
        ? settingsCopy.memoryLoadFailed
        : memoryWarningCode === 'runtime_refresh_failed'
          ? settingsCopy.memoryRuntimeRefreshFailed
          : memoryWarningCode === 'memory_topic_cleanup_required'
            ? settingsCopy.memoryTopicCleanupRequired
          : memoryWarningCode === 'snapshot_refresh_failed'
            ? settingsCopy.memorySnapshotRefreshFailed
            : settingsCopy.memorySourceUnavailable;
      const identity = (memory.profile && memory.profile.identity) || {};
      const longTermItems = [
        ...(memory.preferences || []).map(item => ({ ...item, kind: 'preference', type: settingsCopy.memoryTypes.preference })),
        ...(memory.work_context || []).map(item => ({ ...item, kind: 'work_context', type: settingsCopy.memoryTypes.work_context })),
      ];
      const recentItems = [
        ...(memory.current_focus || []).filter(item => item.status !== 'archived').map(item => ({ ...item, kind: 'current_focus', type: settingsCopy.memoryTypes.current_focus })),
        ...(memory.recent_activity || []).filter(item => item.status !== 'archived').map(item => ({ ...item, kind: 'recent_activity', type: settingsCopy.memoryTypes.recent_activity })),
      ];
      useEffect(() => {
        if (activeSection === 'memory' && memoryEnabled && bridge.available && bridge.memory.loadMemoryOverview) bridge.memory.loadMemoryOverview();
      }, [activeSection, memoryEnabled]);
      useEffect(() => {
        if (updateFocusTick) setActiveSection('update');
      }, [updateFocusTick]);
      const [memoryEditor, setMemoryEditor] = useState(null);
      const [memorySaving, setMemorySaving] = useState(false);
      const [memoryEditorError, setMemoryEditorError] = useState('');
      const [profileSaveError, setProfileSaveError] = useState('');
      const [memoryDeleteConfirm, setMemoryDeleteConfirm] = useState(null);
      const openMemoryItemViewer = item => {
        setMemoryEditor({
          mode: 'memory',
          kind: item.kind,
          id: item.id,
          title: settingsCopy.memoryDetail,
          subtitle: '',
          label: settingsCopy.content,
          value: item.text || item.content || '',
          originalValue: item.text || item.content || '',
          multiline: true,
          editing: false,
        });
      };
      const saveMemoryEditor = async () => {
        if (!memoryEditor || !bridge.available || memorySaving) return;
        const text = String(memoryEditor.value || '').trim();
        setMemorySaving(true);
        setMemoryEditorError('');
        setProfileSaveError('');
        try {
          if (memoryEditor.mode === 'memory') {
            if (!text || !bridge.memory.updateMemoryItem) return;
            await bridge.memory.updateMemoryItem(memoryEditor.kind, memoryEditor.id, { text });
          } else if (memoryEditor.mode === 'profile') {
            if (!bridge.memory.saveMemoryProfilePatch) return;
            await bridge.memory.saveMemoryProfilePatch({ [memoryEditor.key]: text });
          }
          setMemoryEditor(null);
        } catch (error) {
          setMemoryEditorError(String(error));
          setProfileSaveError(String(error));
        } finally {
          setMemorySaving(false);
        }
      };
      const deleteMemoryItem = async item => {
        if (!bridge.available || !bridge.memory.deleteMemoryItem) return;
        await bridge.memory.deleteMemoryItem(item.kind, item.id);
      };
      const editProfile = key => {
        const label = key === 'call_name' ? settingsCopy.userCallName : settingsCopy.assistantNickname;
        setMemoryEditorError('');
        setMemoryEditor({
          mode: 'profile',
          key,
          title: settingsCopy.editTitle(label),
          subtitle: key === 'call_name' ? settingsCopy.callNameDesc : settingsCopy.assistantNameDesc,
          label,
          value: identity[key] || '',
          multiline: false,
        });
      };
      const renderModelRows = (models, totalCount) => models.length ? models.map(m => {
        const total = totalCount != null ? totalCount : models.length;
        const isActive = m.id === activeModelId;
        const isLocal = isLocalModel(m);
        const isReadonly = isReadonlyModel(m);
        const codingPlan = isCodingPlanModel(m);
        const providerLabel = providerLabelForModel(m, t);
        const title = m.model || m.name;
        return (
          <div key={m.id} className={`min-h-[60px] grid grid-cols-[24px_32px_minmax(0,1fr)_auto] items-center gap-3 px-4 py-3 border-b last:border-b-0 border-black/[0.12] dark:border-white/[0.10]`}>
            <button onClick={() => !isActive && onSetActiveModel(m.id)} className="shrink-0" title={t.setActiveModel}>
              <RadioDot active={isActive} />
            </button>
            <ProviderIcon preset={m.preset || (isLocal ? 'local_vllm' : 'openai_compatible')} vendor={m.vendor} providerKind={m.provider_kind} model={m.model} compact />
            <div className="min-w-0">
              <div className="flex items-center gap-2 min-w-0">
                <span className={`text-[15px] leading-5 font-normal truncate text-[#1C1C1E] dark:text-[#F2F2F7]`}>{title}</span>
                {isLocal && <Tag tone="gray">{settingsCopy.localModel}</Tag>}
                {codingPlan && <Tag tone="gray">Coding Plan</Tag>}
                {isActive && <Tag>{settingsCopy.defaultTag}</Tag>}
              </div>
              <div className={`mt-0.5 text-[12px] leading-[17px] truncate text-[#8A8A8E] dark:text-[#98989D]`}>{providerLabel} · {m.model}</div>
            </div>
            <div className="shrink-0 flex items-center gap-2">
              {!isReadonly && <button onClick={() => setEditingModel({ ...m, __scope: isLocal ? 'local' : 'cloud' })} className={`min-h-8 px-3 rounded-full text-[14px] font-medium ${actionButton('blue')}`}>{settingsCopy.edit}</button>}
              {!isReadonly && total > 1 && <button onClick={() => setModelDeleteConfirm(m)} className={`min-h-8 px-3 rounded-full text-[14px] font-medium ${actionButton('red')}`}>{settingsCopy.delete}</button>}
            </div>
          </div>
        );
      }) : <div className={`px-4 py-4 text-[14px] text-[#8A8A8E] dark:text-[#98989D]`}>{settingsCopy.noModels}</div>;
      const petEnabled = !!(bs && bs.settings && bs.settings.pet && bs.settings.pet.enabled);
      const selectedPetId = (bs && typeof bs.selectedPet === 'string' && bs.selectedPet) || DEFAULT_PET_ID;
      const handlePetSelect = id => {
        if (!bridge.available || !bridge.settings.setSelectedPet) return Promise.resolve();
        return bridge.settings.setSelectedPet(id);
      };
      const renderGeneral = () => (
        <>
          <IOSSection title={t.uiSettings.appearance}>
            <IOSRow label={t.uiSettings.language} desc={t.uiSettings.languageDesc}>
              <SSegmented value={language} onChange={v => { setLanguage(v); setRestartDialog('language'); }} options={[{ key: 'zh', label: '中文' }, { key: 'en', label: 'English' }, { key: 'ja', label: '日本語' }]} />
            </IOSRow>
            <IOSRow label={t.uiSettings.theme} desc={t.uiSettings.themeDesc}>
              <SSegmented value={activeTheme} onChange={setActiveTheme} options={[{ key: 'light', label: t.light }, { key: 'dark', label: t.dark }]} />
            </IOSRow>
          </IOSSection>
          <IOSSection title={t.sidebarSection}>
            <IOSRow label={t.sidebarDateGrouping} desc={t.sidebarDateGroupingDesc}>
              <IOSSwitch checked={sidebarDateGrouping} onChange={onSidebarDateGroupingChange} />
            </IOSRow>
          </IOSSection>
          {canConfigureDesktopNotifications && (
          <IOSSection title={t.uiSettings.notifications}>
            <IOSRow label={t.uiSettings.taskNotice} desc={t.uiSettings.taskNoticeDesc}>
              <IOSSwitch checked={taskCompletedNotif} onChange={setTaskCompletedNotif} />
            </IOSRow>
          </IOSSection>
          )}
          {canUsePet && (
          <section className="mb-6">
            <div className={`px-3 mb-2 text-[12px] font-semibold text-[#8A8A8E] dark:text-[#8E8E93]`}>{t.uiSettings.desktopAssistant}</div>
            <div className={`overflow-hidden rounded-[18px] bg-white dark:bg-[#2C2C2E]`}>
              <div className={`w-full min-h-[58px] flex flex-wrap items-center gap-3 px-4 py-2.5 text-left border-b ${
                'border-black/[0.12] text-[#1C1C1E] dark:border-white/[0.10] dark:text-[#F2F2F7]'
              } ${petEnabled ? '' : 'last:border-b-0'}`}>
                <div className="flex-1 min-w-[120px]">
                  <div className="text-[15px] leading-5 font-normal whitespace-nowrap">{t.uiSettings.pet}</div>
                  <div className={`mt-0.5 text-[13px] leading-5 text-[#8A8A8E] dark:text-[#98989D]`}>{t.uiSettings.petDesc}</div>
                </div>
                <IOSSwitch checked={petEnabled} onChange={onPetEnabledChange} />
              </div>
              {petEnabled && (
                <div className={`px-4 pb-4 border-t border-black/[0.12] dark:border-white/[0.10]`}>
                  <PetSettingsSection
                    enabled={petEnabled}
                    selectedPetId={selectedPetId}
                    t={t}
                    onSelect={handlePetSelect}
                  />
                </div>
              )}
            </div>
          </section>
          )}
        </>
      );
      const renderModels = () => (
        <>
          {acpProvidersTabVisible && (
            // 左上角小胶囊切换：模型 / ACP 管理（替代原列表上方「模型」小字标题；
            // 原侧栏「Provider 管理」分节并入为子页）
            <div data-testid="settings-model-tabs" className="mb-3 inline-flex items-center gap-0.5 p-0.5 rounded-full bg-black/[0.05] dark:bg-white/[0.07]">
              {[
                { key: 'models', label: t.uiSettings.model },
                { key: 'acp', label: t.uiSettings.providers },
              ].map(tab => (
                <button key={tab.key} type="button" data-testid={`settings-model-tab-${tab.key}`} onClick={() => setModelTab(tab.key)}
                  className={`h-7 px-3 rounded-full text-[12px] font-semibold transition-colors ${modelTab === tab.key ? ('bg-white text-[#007AFF] shadow-sm dark:bg-[#3A3A3C] dark:text-[#F2F2F7]') : ('text-[#8A8A8E] hover:text-[#636366] dark:text-[#8E8E93] dark:hover:text-[#C7C7CC]')}`}>
                  {tab.label}
                </button>
              ))}
            </div>
          )}
          {modelTab === 'acp' && acpProvidersTabVisible ? (
            <ProvidersSection t={t} />
          ) : (
          <section className="mb-6">
            {/* 有 ACP 子页时顶端胶囊切换已承担「模型」标题语义，不再重复小字标题 */}
            {!acpProvidersTabVisible && <SectionTitle>{settingsCopy.modelSection}</SectionTitle>}
            <Group>
              {(() => {
                const { preset, custom } = groupModelsForSelector(userModels);
                const any = preset.length > 0 || custom.length > 0;
                return (
                  <>
                    {!any && renderModelRows([], userModels.length)}
                    {preset.length > 0 && (
                      <>
                        <div className="px-4 pt-2 pb-1 text-[12px] font-semibold text-[#8A8A8E] dark:text-[#8E8E93]">{t.modelGroupPreset}</div>
                        {renderModelRows(preset, userModels.length)}
                      </>
                    )}
                    {custom.length > 0 && (
                      <>
                        <div className={`px-4 pt-2 pb-1 text-[12px] font-semibold text-[#8A8A8E] dark:text-[#8E8E93]${preset.length > 0 ? ' border-t border-black/[0.12] dark:border-white/[0.10]' : ''}`}>{t.modelGroupCustom}</div>
                        {renderModelRows(custom, userModels.length)}
                      </>
                    )}
                  </>
                );
              })()}
              <button data-testid="settings-model-add" onClick={() => setEditingModel(newModelDraft('deepseek'))}
                className={`w-full min-h-[52px] flex items-center justify-center gap-2 px-4 text-[16px] font-normal border-t border-black/[0.12] text-[#007AFF] hover:bg-black/[0.035] dark:border-white/[0.10] dark:text-[#0A84FF] dark:hover:bg-white/[0.05]`}>
                <Plus size={18} />
                <span>{settingsCopy.addModel}</span>
              </button>
            </Group>
            {modelEnvLocked.length > 0 && <div className={`px-3 mt-2 text-[12px] leading-relaxed text-[#8A8A8E] dark:text-[#8E8E93]`}>{settingsCopy.envManaged}</div>}
          </section>
          )}
        </>
      );
      const renderSearch = () => (
        <>
          <section className="mb-6">
            <SectionTitle>{settingsCopy.searchList}</SectionTitle>
            <Group>
            {enabledSearchList.map(item => {
              return (
                <div key={item.key} className={`min-h-[60px] grid grid-cols-[24px_minmax(0,1fr)_auto] items-center gap-[14px] px-4 py-3 border-b last:border-b-0 border-black/[0.12] dark:border-white/[0.10]`}>
                  <button onClick={() => { setSearchProvider(item.key); setRestartDialog('search'); }} className="shrink-0" title={settingsCopy.setDefault}>
                    <RadioDot active={searchProvider === item.key} />
                  </button>
                  <div className="min-w-0">
                    <div className="flex items-center gap-2 min-w-0">
                      <span className={`text-[15px] leading-5 font-normal truncate text-[#1C1C1E] dark:text-[#F2F2F7]`}>{item.label}</span>
                      {item.key === searchProvider && <Tag>{settingsCopy.defaultTag}</Tag>}
                    </div>
                    <div className={`mt-0.5 text-[12px] leading-[17px] truncate text-[#8A8A8E] dark:text-[#98989D]`}>{item.desc}</div>
                  </div>
                  <div className="flex items-center gap-2">
                    {item.key !== 'bing' && <button onClick={() => { setPendingSearchProvider(null); setEditingSearch(item.key); }} className={`shrink-0 min-h-8 px-3 rounded-full text-[14px] font-medium ${actionButton('blue')}`}>{settingsCopy.edit}</button>}
                    {item.key !== 'bing' && <button onClick={() => setSearchDeleteConfirm(item)} className={`shrink-0 min-h-8 px-3 rounded-full text-[14px] font-medium ${actionButton('red')}`}>{settingsCopy.delete}</button>}
                  </div>
                </div>
              );
            })}
            <button onClick={() => setSearchPickerOpen(true)}
              className={`w-full min-h-[52px] flex items-center justify-center gap-2 px-4 text-[16px] font-normal border-t border-black/[0.12] text-[#007AFF] hover:bg-black/[0.035] dark:border-white/[0.10] dark:text-[#0A84FF] dark:hover:bg-white/[0.05]`}>
              <Plus size={18} />
              <span>{settingsCopy.addSearch}</span>
            </button>
            </Group>
          </section>
        </>
      );
      const renderMemoryList = (items, empty) => items.length ? items.map(item => {
        const text = item.text || item.content || settingsCopy.unnamedMemory;
        return (
          <div key={`${item.kind}-${item.id}`} className={`min-h-[92px] flex items-start gap-4 px-4 py-3.5 border-b last:border-b-0 border-black/[0.12] dark:border-white/[0.10]`}>
            <div className="min-w-0 flex-1">
              <div className={`text-[15px] leading-6 whitespace-pre-wrap break-words line-clamp-3 text-[#1C1C1E] dark:text-[#F2F2F7]`}>{text}</div>
            </div>
            <button onClick={() => openMemoryItemViewer(item)} className={`shrink-0 mt-0.5 text-[14px] px-3 py-1.5 rounded-full ${actionButton('blue')}`}>{settingsCopy.view}</button>
          </div>
        );
      }) : <IOSRow label={empty} />;
      const renderMemory = () => (
        <>
          <IOSSection>
            <IOSRow label={settingsCopy.enableMemory} desc={settingsCopy.enableMemoryDesc}>
              <IOSSwitch checked={memoryEnabled} onChange={onMemoryEnabledChange} />
            </IOSRow>
          </IOSSection>
          {memoryEnabled && (
            <>
              {profileSaveError ? (
                <div data-testid="memory-settings-error" role="alert" aria-live="polite" className="mb-4 rounded-[14px] bg-[#FF3B30]/10 px-4 py-3 text-[13px] leading-5 text-[#FF3B30]">
                  {settingsCopy.memorySaveFailed}
                </div>
              ) : memoryError && (
                <div data-testid="memory-settings-error" role="alert" aria-live="polite" className="mb-4 rounded-[14px] bg-[#FF3B30]/10 px-4 py-3 text-[13px] leading-5 text-[#FF3B30]">
                  {memoryErrorMessage}
                </div>
              )}
              <IOSSection title={settingsCopy.profile}>
                <div data-testid="memory-profile-call-name">
                  <IOSRow label={settingsCopy.userCallName} desc={settingsCopy.callNameDesc} value={identity.call_name || settingsCopy.notSet} onClick={() => editProfile('call_name')}>
                    <ChevronDown size={22} className="-rotate-90 opacity-35" />
                  </IOSRow>
                </div>
                <IOSRow label={settingsCopy.assistantNickname} desc={settingsCopy.assistantNameDesc} value={identity.assistant_alias || 'PINVOU'} onClick={() => editProfile('assistant_alias')}>
                  <ChevronDown size={22} className="-rotate-90 opacity-35" />
                </IOSRow>
              </IOSSection>
              <IOSSection title={settingsCopy.longMemory}>{renderMemoryList(longTermItems, settingsCopy.noLongMemory)}</IOSSection>
              <IOSSection title={settingsCopy.shortMemory}>{renderMemoryList(recentItems, settingsCopy.noShortMemory)}</IOSSection>
            </>
          )}
        </>
      );
      const renderUpdate = () => {
        const upd = bs && bs.updateInfo;
        const currentVersion = (bs && bs.appVersion) || (upd && upd.current_version) || '—';
        const notes = (upd && String(upd.notes || '').trim()) || t.uiSettings.noReleaseNotes;
        const updateChecking = !!(bs && bs.updateChecking);
        const updateDownloading = !!(bs && bs.updateDownloading);
        const updateCancelling = !!(bs && bs.updateCancelling);
        const updateReady = !!(bs && bs.updateReady);
        const updateProgress = (bs && bs.updateProgress) || 0;
        const isWindowsUpdate = upd && upd.platform === 'windows';
        const updateError = (bs && bs.updateError) || (bs && bs.updateCheckError && bs.updateCheckError !== 'latest' ? bs.updateCheckError : '');
        const updateStatusDesc = updateDownloading
          ? (updateProgress >= 100 ? t.uiSettings.installingUpdate : t.uiSettings.downloading(updateProgress))
          : updateReady
            ? (isWindowsUpdate ? t.updateInstallerStarted : t.updateComplete)
            : (upd && upd.available ? `v${upd.latest_version}` : (bs && bs.updateCheckError === 'latest' ? t.upToDate : ''));
        const updateButtonLabel = updateChecking
          ? t.checking
          : updateDownloading
            ? (updateProgress >= 100 ? t.installing : (updateCancelling ? t.cancelling : t.uiSettings.cancelDownload))
            : updateReady
              ? (isWindowsUpdate ? t.uiSettings.installerStarted : t.restartNow)
              : (upd && upd.available ? (upd.platform === 'linux' ? t.downloadInstallRestart : t.downloadInstall) : t.checkUpdate);
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
            <IOSSection title={t.uiSettings.version}>
              <IOSRow label={t.uiSettings.currentVersion} desc={t.uiSettings.beta} value={`v${currentVersion}`} />
              <IOSRow label={upd && upd.available ? t.newVersionFound : t.checkUpdate} desc={updateStatusDesc}>
              <button data-settings-update-action="true" onClick={handleUpdateAction} disabled={updateButtonDisabled} className="h-9 px-4 rounded-full bg-[#007AFF] text-white text-[14px] font-semibold whitespace-nowrap disabled:opacity-50 disabled:cursor-not-allowed">{updateButtonLabel}</button>
            </IOSRow>
            </IOSSection>
            {updateError && (
              <div className="px-3 -mt-3 mb-4 text-[12px] leading-5 text-[#EA4335] break-words">{String(updateError)}</div>
            )}
            <section className="mb-6">
              <div className={`px-3 mb-2 text-[12px] font-semibold text-[#8A8A8E] dark:text-[#8E8E93]`}>{t.uiSettings.releaseNotes}</div>
              <div className={`rounded-[18px] px-4 py-3.5 text-[14px] leading-6 whitespace-pre-line bg-white text-[#1C1C1E] dark:bg-[#2C2C2E] dark:text-[#F2F2F7]`}>{notes}</div>
            </section>
          </div>
        );
      };
      const renderPermissions = () => {
        const deps = (bs && bs.deps) || [];
        const checking = !!(bs && bs.depsChecking);
        const installing = !!(bs && bs.depsInstalling);
        const installError = bs && bs.depsInstallError;
        const installProgress = bs && bs.depsInstallProgress;
        const missing = deps.filter(dep => !dep.installed);
        const hasInstallableMissing = missing.some(dep => String(dep.install_action || dep.apt || '').trim());
        const checked = deps.length > 0;
        const busy = checking || installing;
        // 安装中实时进度文案:优先用后端 deps:install_progress 事件(逐包 + brew 输出行),
        // 解决一键安装全程只有静态「安装中…」像卡死的问题(尤其 libreoffice cask 长尾)。
        const progressText = (installing && installProgress)
          ? `${installProgress.package}（${installProgress.current}/${installProgress.total}）${installProgress.detail ? ' ' + installProgress.detail : ''}`
          : null;
        return (
          <>
            {showSuperPermissionSettings && (
              <IOSSection title={settingsCopy.system}>
                <IOSRow label={settingsCopy.advancedPermission} desc={settingsCopy.advancedPermissionDesc}>
                  <IOSSwitch checked={!!superPerm} onChange={setSuperPerm} />
                </IOSRow>
              </IOSSection>
            )}
            <div id="settings-dependencies">
              <IOSSection
                title={t.depCheckTitle}
                footer={usesHomebrewDependencyInstaller ? t.depInstallNoteMac : (usesBundledDependencyInstaller ? t.depInstallNoteWindows : t.depInstallNote)}
              >
                <IOSRow
                  label={checking ? t.depChecking : (!checked ? t.depCheckTitle : (missing.length ? `${missing.length}${t.depMissingSuffix}` : t.depAllOk))}
                  desc={progressText || (installing ? t.depInstalling : (installError ? String(installError) : ''))}
                >
                  <button
                    onClick={() => bridge.available && bridge.dependencies.checkDependencies()}
                    disabled={!bridge.available || busy}
                    className={`h-9 px-4 rounded-full text-[14px] font-semibold disabled:opacity-50 bg-[#E5E5EA] text-[#007AFF] dark:bg-white/[0.08] dark:text-[#0A84FF]`}
                  >{checking ? t.depChecking : t.depRecheck}</button>
                </IOSRow>
                {missing.map(dep => (
                  <IOSRow key={dep.key} label={t[`dep_${dep.key}`] || dep.key} desc={((dep.hint && (t[`depHint_${dep.hint}`] || dep.hint)) || dep.apt || '').trim()}>
                    <Tag tone="gray">{settingsCopy.missing}</Tag>
                  </IOSRow>
                ))}
                {hasInstallableMissing && (
                  <IOSRow label={usesBundledDependencyInstaller ? settingsCopy.installMissing : t.depGoInstall}>
                    <button
                      onClick={() => bridge.available && bridge.dependencies.installDependencies()}
                      disabled={!bridge.available || busy}
                      className="h-9 px-4 rounded-full bg-[#007AFF] text-white text-[14px] font-semibold disabled:opacity-50"
                    >{installing ? (progressText || t.depInstalling) : t.depInstallBtn}</button>
                  </IOSRow>
                )}
              </IOSSection>
            </div>
          </>
        );
      };
      const renderHelp = () => (
        <IOSSection>
          <IOSRow label={settingsCopy.feedbackTitle} desc={settingsCopy.feedbackDesc}>
            <button onClick={() => setFeedbackOpen(true)} className="h-9 px-4 rounded-full bg-[#007AFF] text-white text-[14px] font-semibold">{settingsCopy.submitFeedback}</button>
          </IOSRow>
        </IOSSection>
      );
      const renderContent = () => {
        if (activeSection === 'model') return renderModels();
        if (activeSection === 'search') return renderSearch();
        if (activeSection === 'memory') return renderMemory();
        if (activeSection === 'permissions') return renderPermissions();
        if (activeSection === 'update') return renderUpdate();
        if (activeSection === 'help') return renderHelp();
        return renderGeneral();
      };
      const sectionTitle = (activeSection === 'model' && modelTab === 'acp' && acpProvidersTabVisible)
        ? t.uiSettings.providers
        : ({
            general: t.uiSettings.general,
            model: t.uiSettings.model,
            search: t.uiSettings.search,
            memory: t.uiSettings.memory,
            permissions: t.uiSettings.permissions,
            update: t.uiSettings.update,
            help: t.uiSettings.help,
          }[activeSection] || t.uiSettings.general);
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
              className={`w-[430px] max-w-[90vw] max-h-[76vh] overflow-y-auto custom-scrollbar rounded-[22px] shadow-2xl bg-white text-[#1C1C1E] dark:bg-[#1C1C1E] dark:text-[#F2F2F7]`}>
              <div className={`px-5 py-4 flex items-start justify-between gap-4 border-b border-black/[0.10] dark:border-white/[0.10]`}>
                <div>
                  <h2 className="text-[20px] leading-6 font-semibold">{settingsCopy.editSearch}</h2>
                  <p className={`mt-1 text-[13px] leading-[18px] text-[#8A8A8E] dark:text-[#98989D]`}>{option ? option.label : provider}</p>
                </div>
                <button onClick={onClose} className={`h-9 w-9 shrink-0 rounded-full flex items-center justify-center bg-[#E5E5EA] text-[#636366] dark:bg-white/[0.08] dark:text-[#C7C7CC]`}><X size={18} /></button>
              </div>
              <div className="space-y-4 px-5 py-4">
                <section>
                  <div className={`overflow-hidden rounded-[16px] bg-[#F2F2F7] dark:bg-[#2C2C2E]`}>
                    <div className={`min-h-[54px] flex items-center gap-3 px-4 py-2.5 text-[#1C1C1E] dark:text-[#F2F2F7]`}>
                    <label className="shrink-0 text-[14px] leading-5">API Key</label>
                    <input type="text" value={draftKey} onChange={e => setDraftKey(e.target.value)}
                      autoFocus
                      placeholder={hasSavedKey ? '••••••••' : settingsCopy.apiKeyPlaceholder}
                      style={showSearchKey ? undefined : { WebkitTextSecurity: 'disc' }}
                      className={`min-w-0 flex-1 bg-transparent text-right text-[14px] leading-5 outline-none placeholder:text-[#8A8A8E] dark:placeholder:text-[#636366]`} />
                    <button type="button" onClick={() => setShowSearchKey(v => !v)} className="shrink-0 text-[14px] text-[#007AFF]">{showSearchKey ? settingsCopy.hide : settingsCopy.show}</button>
                    </div>
                  </div>
                </section>
              </div>
              <div className={`flex justify-end gap-2 px-5 py-4 border-t border-black/[0.10] dark:border-white/[0.10]`}>
                <button onClick={onClose} className={`h-10 px-4 rounded-full text-[15px] font-normal transition-colors text-[#007AFF] hover:bg-black/[0.04] dark:text-[#0A84FF] dark:hover:bg-white/[0.06]`}>{settingsCopy.cancel}</button>
                <button onClick={() => {
                  if (!canSaveSearch) return;
                  if (isNew) onAddSearchProvider && onAddSearchProvider(provider);
                  if (draftKey.trim()) setSearchApiKey(draftKey, provider);
                  onClose();
                  setRestartDialog('search');
                }} disabled={!canSaveSearch} className="h-10 px-5 rounded-full bg-[#007AFF] text-white text-[15px] font-semibold transition-colors disabled:opacity-35">{settingsCopy.save}</button>
              </div>
            </div>
          </div>
        );
      };
      const RestartDialog = ({ type }) => (
        <div className="fixed inset-0 z-[110] flex items-center justify-center bg-black/35 backdrop-blur-md px-4">
          <div className={`w-[340px] overflow-hidden rounded-[18px] shadow-2xl bg-white text-[#1C1C1E] dark:bg-[#2C2C2E] dark:text-[#F2F2F7]`}>
            <div className="px-6 pt-6 pb-5 text-center">
              <h3 className="text-[18px] font-semibold">{type === 'search' ? settingsCopy.restartSearchTitle : settingsCopy.restartLanguageTitle}</h3>
              <p className={`mt-2 text-[14px] leading-5 text-[#8A8A8E] dark:text-[#98989D]`}>{type === 'search' ? settingsCopy.restartSearchDesc : settingsCopy.restartLanguageDesc}</p>
            </div>
            <div className={`grid grid-cols-2 border-t border-black/[0.12] dark:border-white/[0.12]`}>
              <button onClick={async () => {
                if (type === 'search' && onSaveSearchConfig) {
                  const saved = await onSaveSearchConfig();
                  if (saved === false) return;
                }
                setRestartDialog(null);
              }} className={`h-12 text-[17px] font-semibold border-r border-black/[0.12] text-[#007AFF] dark:border-white/[0.12] dark:text-[#0A84FF]`}>{settingsCopy.later}</button>
              <button onClick={() => { setRestartDialog(null); type === 'search' ? onConfirmSearchConfig() : (bridge.available && bridge.updater.restartApp()); }} className="h-12 text-[17px] font-semibold text-[#007AFF]">{settingsCopy.restartNow}</button>
            </div>
          </div>
        </div>
      );
      const ModelDeleteDialog = ({ model }) => (
        <div className="fixed inset-0 z-[110] flex items-center justify-center bg-black/35 backdrop-blur-md px-4">
          <div className={`w-[270px] overflow-hidden rounded-[14px] shadow-2xl bg-white text-[#1C1C1E] dark:bg-[#2C2C2E] dark:text-[#F2F2F7]`}>
            <div className="px-5 pt-5 pb-4 text-center">
              <h3 className="text-[17px] leading-6 font-semibold">{settingsCopy.deleteModelTitle}</h3>
              <p className={`mt-1 text-[13px] leading-[18px] text-[#8A8A8E] dark:text-[#98989D]`}>{settingsCopy.deleteModelDesc}</p>
            </div>
            <div className={`border-t border-black/[0.12] dark:border-white/[0.12]`}>
              <button onClick={() => { onDeleteModel(model); setModelDeleteConfirm(null); }} className={`w-full h-12 text-[17px] font-semibold text-[#FF3B30] border-b border-black/[0.12] dark:border-white/[0.12]`}>{settingsCopy.deleteModel}</button>
              <button onClick={() => setModelDeleteConfirm(null)} className="w-full h-12 text-[17px] font-semibold text-[#007AFF]">{settingsCopy.cancel}</button>
            </div>
          </div>
        </div>
      );
      const SearchDeleteDialog = ({ source }) => (
        <div className="fixed inset-0 z-[110] flex items-center justify-center bg-black/35 backdrop-blur-md px-4">
          <div className={`w-[270px] overflow-hidden rounded-[14px] shadow-2xl bg-white text-[#1C1C1E] dark:bg-[#2C2C2E] dark:text-[#F2F2F7]`}>
            <div className="px-5 pt-5 pb-4 text-center">
              <h3 className="text-[17px] leading-6 font-semibold">{settingsCopy.deleteSearchTitle}</h3>
              <p className={`mt-1 text-[13px] leading-[18px] text-[#8A8A8E] dark:text-[#98989D]`}>{settingsCopy.deleteSearchDesc(source.label)}</p>
            </div>
            <div className={`border-t border-black/[0.12] dark:border-white/[0.12]`}>
              <button onClick={() => { onDeleteSearchProvider && onDeleteSearchProvider(source.key); setSearchDeleteConfirm(null); setRestartDialog('search'); }} className={`w-full h-12 text-[17px] font-semibold text-[#FF3B30] border-b border-black/[0.12] dark:border-white/[0.12]`}>{settingsCopy.deleteSearch}</button>
              <button onClick={() => setSearchDeleteConfirm(null)} className="w-full h-12 text-[17px] font-semibold text-[#007AFF]">{settingsCopy.cancel}</button>
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
            data-testid="settings-dialog"
            style={{ width: 'min(920px, calc(100vw - 24px))', height: 'min(620px, calc(100vh - 24px))' }}
            onClick={(event) => event.stopPropagation()}
            className={`relative flex flex-col sm:flex-row overflow-hidden rounded-[24px] border shadow-[0_22px_58px_rgba(0,0,0,0.34)] border-white/70 bg-[#F2F2F7] text-[#1C1C1E] dark:border-white/[0.14] dark:bg-[#1C1C1E] dark:text-[#F2F2F7]`}
          >
            {/* 窄屏:Tab 条与关闭键同排,X 在滚动区外侧,Tab 滚动不会穿到它底下;
                桌面:包裹层 display:contents 不参与布局,维持左栏 + 悬浮 X 不变 */}
            <div className={`sm:contents max-sm:flex max-sm:items-center max-sm:shrink-0 max-sm:border-b border-black/[0.12] dark:border-white/[0.12]`}>
            <aside
              data-testid="settings-nav"
              className={`w-full sm:w-[clamp(150px,24vw,210px)] shrink-0 max-sm:flex-1 max-sm:min-w-0 overflow-x-auto sm:overflow-x-hidden sm:overflow-y-auto custom-scrollbar max-sm-hide-scrollbar sm:border-r px-3 sm:px-4 py-3 sm:py-7 max-sm:flex max-sm:items-center max-sm:gap-2 border-black/[0.12] dark:border-white/[0.12]`}
            >
              <div className={`mb-4 px-1 text-[12px] font-semibold max-sm:hidden text-[#8A8A8E] dark:text-[#8E8E93]`}>{t.uiSettings.common}</div>
              <div className="space-y-2 max-sm:flex max-sm:space-y-0 max-sm:gap-2">
                <SectionButton id="general" icon={<Sparkles size={17} />} label={t.uiSettings.general} />
                <SectionButton id="model" icon={<Cpu size={17} />} label={t.uiSettings.model} />
                <SectionButton id="search" icon={<Search size={17} />} label={t.uiSettings.search} />
                {memorySettingsVisible && <SectionButton id="memory" icon={<Database size={17} />} label={t.uiSettings.memory} />}
              </div>
              <div className={`mt-7 mb-4 px-1 text-[12px] font-semibold max-sm:hidden text-[#8A8A8E] dark:text-[#8E8E93]`}>{t.uiSettings.system}</div>
              <div className="space-y-2 max-sm:flex max-sm:space-y-0 max-sm:gap-2">
                {canUseSuperPermission && <SectionButton id="permissions" icon={<Wrench size={17} />} label={t.uiSettings.permissions} />}
                {canUpdateApp && <SectionButton id="update" icon={<RefreshCw size={17} />} label={t.uiSettings.update} dot={hasUpdate} />}
                <SectionButton id="help" icon={<MessageSquare size={17} />} label={t.uiSettings.help} />
              </div>
            </aside>
            {onCloseSettings && (
              <button data-testid="settings-close" onClick={onCloseSettings} aria-label={settingsCopy.closeSettings} className={`sm:absolute sm:right-5 sm:top-5 z-20 h-9 w-9 shrink-0 max-sm:mr-3 rounded-full flex items-center justify-center bg-[#E5E5EA] text-[#636366] dark:bg-white/[0.08] dark:text-[#C7C7CC]`}>
                <X size={18} />
              </button>
            )}
            </div>
            <main data-testid="settings-content" className="w-full flex-1 min-w-0 min-h-0 overflow-y-auto custom-scrollbar px-4 sm:px-6 md:px-8 py-4 sm:py-7">
              <div className="max-w-[680px]">
                <div className="mb-5 sm:mb-6">
                  <h1 className="text-[22px] sm:text-[24px] leading-tight font-semibold tracking-normal">{sectionTitle}</h1>
                </div>
                {renderContent()}
              </div>
            </main>
          </div>
          {canManageModels && editingModel && (
            <ModelFormModal isDark={activeTheme === 'dark'} t={t} initial={editingModel} bs={bs} models={userModels}
              onCancel={() => setEditingModel(null)}
              // 保存/探测结果由弹窗内部控制关闭(自动探测有结论时保持打开展示)。
              onSave={async m => onSaveModel(m)} />
          )}
          {modelDeleteConfirm && <ModelDeleteDialog model={modelDeleteConfirm} />}
          {searchDeleteConfirm && <SearchDeleteDialog source={searchDeleteConfirm} />}
          {searchPickerOpen && (
            <div className="fixed inset-0 z-[100] flex items-center justify-center bg-black/45 px-4 animate-in fade-in duration-150" onClick={() => setSearchPickerOpen(false)}>
              <div onClick={e => e.stopPropagation()}
                className={`w-[440px] max-w-[90vw] max-h-[76vh] overflow-y-auto custom-scrollbar rounded-[22px] shadow-2xl bg-white text-[#1C1C1E] dark:bg-[#1C1C1E] dark:text-[#F2F2F7]`}>
                <div className={`px-5 py-4 flex items-start justify-between gap-4 border-b border-black/[0.10] dark:border-white/[0.10]`}>
                  <div>
                    <h2 className="text-[20px] leading-6 font-semibold">{settingsCopy.addSearch}</h2>
                    <p className={`mt-1 text-[13px] leading-[18px] text-[#8A8A8E] dark:text-[#98989D]`}>{settingsCopy.addSearchDesc}</p>
                  </div>
                  <button onClick={() => setSearchPickerOpen(false)} className={`h-9 w-9 shrink-0 rounded-full flex items-center justify-center bg-[#E5E5EA] text-[#636366] dark:bg-white/[0.08] dark:text-[#C7C7CC]`}><X size={18} /></button>
                </div>
                <div className="px-5 py-4">
                  <div className={`overflow-hidden rounded-[16px] bg-[#F2F2F7] dark:bg-[#2C2C2E]`}>
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
                        className={`w-full min-h-[56px] px-3.5 py-2.5 flex items-center gap-3 text-left border-b last:border-b-0 border-black/[0.10] hover:bg-black/[0.035] dark:border-white/[0.10] dark:hover:bg-white/[0.06]`}>
                        <span className="min-w-0 flex-1">
                          <span className={`block text-[15px] leading-5 font-normal truncate text-[#1C1C1E] dark:text-[#F2F2F7]`}>{item.label}</span>
                          <span className={`block mt-0.5 text-[12px] leading-[17px] truncate text-[#8A8A8E] dark:text-[#98989D]`}>{item.desc}</span>
                        </span>
                        <ChevronDown size={16} className={`-rotate-90 shrink-0 text-[#C7C7CC] dark:text-[#636366]`} />
                      </button>
                    ))}
                  </div>
                </div>
              </div>
            </div>
          )}
          {editingSearch && <SearchSourceModal provider={editingSearch} isNew={pendingSearchProvider === editingSearch} onClose={() => { setEditingSearch(null); setPendingSearchProvider(null); }} />}
          {memoryEditor && (
            <div className="fixed inset-0 z-[100] flex items-center justify-center bg-black/45 px-4" onClick={() => { if (!memorySaving) setMemoryEditor(null); }}>
              <div onClick={e => e.stopPropagation()} className={`w-full max-w-[500px] rounded-[24px] shadow-2xl bg-white text-[#1C1C1E] dark:bg-[#1C1C1E] dark:text-[#F2F2F7]`}>
                <div className={`px-6 py-4 flex items-start justify-between border-b border-black/[0.12] dark:border-white/[0.12]`}>
                  <div>
                    <h2 className="text-[22px] leading-7 font-semibold">{memoryEditor.title}</h2>
                    <p className={`mt-1 text-[13px] leading-[18px] text-[#8A8A8E] dark:text-[#98989D]`}>{memoryEditor.subtitle}</p>
                  </div>
                  <button onClick={() => setMemoryEditor(null)} disabled={memorySaving} className={`h-10 w-10 rounded-full flex items-center justify-center bg-[#E5E5EA] dark:bg-white/[0.08] disabled:opacity-40`}><X size={20} /></button>
                </div>
                <div className="px-6 py-5">
                  <label className="block">
                    <span className={`block px-1 mb-2 text-[13px] font-semibold text-[#8A8A8E] dark:text-[#98989D]`}>{memoryEditor.label}</span>
                    {memoryEditor.multiline ? (
                      <textarea
                        value={memoryEditor.value}
                        onChange={e => setMemoryEditor(prev => ({ ...prev, value: e.target.value }))}
                        rows={5}
                        className={`w-full rounded-[16px] px-4 py-3 text-[15px] leading-6 outline-none resize-none bg-[#F2F2F7] text-[#1C1C1E] placeholder:text-[#8A8A8E] dark:bg-[#2C2C2E] dark:text-[#F2F2F7] dark:placeholder:text-[#636366]`}
                      />
                    ) : (
                      <input
                        data-testid="memory-editor-input"
                        value={memoryEditor.value}
                        onChange={e => setMemoryEditor(prev => ({ ...prev, value: e.target.value }))}
                        className={`w-full rounded-[16px] px-4 py-3 text-[15px] outline-none bg-[#F2F2F7] text-[#1C1C1E] placeholder:text-[#8A8A8E] dark:bg-[#2C2C2E] dark:text-[#F2F2F7] dark:placeholder:text-[#636366]`}
                      />
                    )}
                  </label>
                  {memoryEditorError && <div data-testid="memory-editor-error" role="alert" aria-live="assertive" className="mt-3 text-[13px] leading-5 text-[#FF3B30]">{settingsCopy.memorySaveFailed}</div>}
                  <div className="mt-6 flex justify-end gap-2.5">
                    <button onClick={() => setMemoryEditor(null)} disabled={memorySaving} className={`h-10 px-4 rounded-full text-[14px] font-semibold bg-[#E5E5EA] dark:bg-[#2C2C2E] disabled:opacity-40`}>{settingsCopy.cancel}</button>
                    <button data-testid="memory-editor-save" onClick={saveMemoryEditor} disabled={memorySaving} className="h-10 px-4 rounded-full bg-[#007AFF] text-white text-[14px] font-semibold disabled:opacity-40">{memorySaving ? settingsCopy.saving : settingsCopy.save}</button>
                  </div>
                </div>
              </div>
            </div>
          )}
          {restartDialog && <RestartDialog type={restartDialog} />}
          {feedbackOpen && (
            <div className="fixed inset-0 z-[100] flex items-center justify-center bg-black/45 px-4 animate-in fade-in duration-150" onClick={closeFeedback}>
              <div
                onClick={e => e.stopPropagation()}
                data-feedback-dialog="true"
                className={`w-[430px] max-w-[90vw] max-h-[76vh] overflow-y-auto rounded-[22px] shadow-2xl custom-scrollbar bg-white text-[#1C1C1E] dark:bg-[#1C1C1E] dark:text-[#F2F2F7]`}
              >
                <div className={`px-5 py-4 flex items-start justify-between gap-4 border-b border-black/[0.10] dark:border-white/[0.10]`}>
                  <div className="min-w-0">
                    <h2 className="text-[20px] leading-6 font-semibold">{t.feedbackDialogTitle}</h2>
                    <p className={`mt-1 text-[13px] leading-[18px] text-[#8A8A8E] dark:text-[#98989D]`}>{t.feedbackDesc}</p>
                  </div>
                  <button onClick={closeFeedback} className={`h-9 w-9 shrink-0 rounded-full flex items-center justify-center bg-[#E5E5EA] text-[#636366] dark:bg-white/[0.08] dark:text-[#C7C7CC]`}><X size={18} /></button>
                </div>
                <div className="space-y-4 px-5 py-4">
                  <section>
                    <div className={`overflow-hidden rounded-[16px] bg-[#F2F2F7] dark:bg-[#2C2C2E]`}>
                      <div className="min-h-[54px] flex items-center gap-3 px-4 py-2.5">
                        <label className={`shrink-0 text-[14px] leading-5 text-[#1C1C1E] dark:text-[#F2F2F7]`}>{t.feedbackType}</label>
                        <SSegmented value={feedbackDraft.type} onChange={type => setFeedbackDraft(prev => ({ ...prev, type }))} options={feedbackTypes} />
                      </div>
                    </div>
                  </section>
                  <section>
                    <div className={`overflow-hidden rounded-[16px] bg-[#F2F2F7] dark:bg-[#2C2C2E]`}>
                      <div className={`min-h-[54px] flex items-center gap-3 px-4 py-2.5 border-b border-black/[0.10] dark:border-white/[0.10]`}>
                        <label className="shrink-0 text-[14px] leading-5">{t.feedbackSubject}</label>
                        <input value={feedbackDraft.title} maxLength={120} onChange={e => setFeedbackDraft(prev => ({ ...prev, title: e.target.value }))}
                        placeholder={t.feedbackSubjectPh}
                        className={`min-w-0 flex-1 bg-transparent text-right text-[14px] leading-5 outline-none placeholder:text-[#8A8A8E] dark:placeholder:text-[#636366]`} />
                      </div>
                      <div className="px-4 py-3">
                        <div className="mb-2 text-[14px] leading-5">{t.feedbackBody}</div>
                        <textarea value={feedbackDraft.description} maxLength={5000} onChange={e => setFeedbackDraft(prev => ({ ...prev, description: e.target.value }))}
                        placeholder={t.feedbackBodyPh} rows={5}
                        className={`w-full resize-none bg-transparent text-[14px] leading-6 outline-none placeholder:text-[#8A8A8E] dark:placeholder:text-[#636366]`} />
                      </div>
                    </div>
                  </section>
                  <section>
                    <div className={`overflow-hidden rounded-[16px] bg-[#F2F2F7] dark:bg-[#2C2C2E]`}>
                      <div className={`min-h-[54px] flex items-center gap-3 px-4 py-2.5 border-b ${feedbackDraft.attachments.length > 0 ? ('border-black/[0.10] dark:border-white/[0.10]') : 'border-transparent'}`}>
                        <div className="min-w-0 flex-1">
                          <div className="text-[14px] leading-5">{t.feedbackAttachments}</div>
                          <div className={`mt-0.5 text-[12px] leading-[17px] truncate text-[#8A8A8E] dark:text-[#98989D]`}>
                            {feedbackDraft.attachments.length > 0 ? `${feedbackDraft.attachments.length}/5` : t.feedbackNoAttachments}
                          </div>
                        </div>
                        {canPickHostFiles && <button onClick={pickFeedbackAttachments} className="shrink-0 text-[14px] text-[#007AFF]">{t.feedbackAddAttachment}</button>}
                      </div>
                      {feedbackDraft.attachments.length > 0 && (
                        <div>
                        {feedbackDraft.attachments.map((a, idx) => (
                          <div key={`${a.path}-${idx}`} className={`min-h-[48px] flex items-center justify-between gap-3 px-4 py-2.5 border-b last:border-b-0 border-black/[0.10] dark:border-white/[0.10]`}>
                            <span className={`min-w-0 truncate text-[13px] text-[#636366] dark:text-[#C7C7CC]`}>{a.name}</span>
                            <button onClick={() => setFeedbackDraft(prev => ({ ...prev, attachments: prev.attachments.filter((_, i) => i !== idx) }))} className="shrink-0 text-[14px] text-[#FF3B30]">{t.cpDelete}</button>
                          </div>
                        ))}
                        </div>
                      )}
                    </div>
                    <div className={`px-1 mt-1.5 text-[12px] leading-4 text-[#8A8A8E] dark:text-[#8E8E93]`}>{t.feedbackAttachmentHint}</div>
                  </section>
                  <div className={`px-1 text-[12px] leading-5 text-[#8A8A8E] dark:text-[#98989D]`}>{t.feedbackPrivacy}</div>
                  {feedbackStatus.message && (
                    <div className={`rounded-[14px] px-4 py-3 text-[14px] ${feedbackStatus.state === 'submitted' ? 'bg-[#34C759]/15 text-[#248A3D]' : 'bg-[#FF3B30]/15 text-[#FF3B30]'}`}>
                      {feedbackStatus.message}
                    </div>
                  )}
                </div>
                <div className={`flex justify-end gap-2 px-5 py-4 border-t border-black/[0.10] dark:border-white/[0.10]`}>
                    <button onClick={closeFeedback} className={`h-10 px-4 rounded-full text-[15px] font-normal transition-colors text-[#007AFF] hover:bg-black/[0.04] dark:text-[#0A84FF] dark:hover:bg-white/[0.06]`}>{t.cancel}</button>
                    {feedbackStatus.state === 'failed_retryable' && (
                      <button onClick={submitFeedbackDraft} className={`h-10 px-4 rounded-full text-[15px] font-normal transition-colors text-[#007AFF] hover:bg-black/[0.04] dark:text-[#0A84FF] dark:hover:bg-white/[0.06]`}>{t.feedbackRetry}</button>
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

export { SCard, SRow, SField, SSegmented, SActionBar, MemorySettingsCard, MODEL_PRESET_DEFS, presetOptionsI18n, presetProviderLabel, ComposerModelSelector, WebAccessModal, ScaledHtmlPreview, ComposerModeMenu, notifyComposerToolsChanged, ComposerToolMenu, ModelFormModal, SettingsView };
