import { useMemo, useState } from 'react';
import { RefreshCw, Search, X } from '../../components/icons.jsx';
import {
  FILTER_VISIBLE_ROW_LIMIT,
  factSourceLabelKey,
  filterModelRows,
  formatTokensCount,
  modelDuplicationKey,
  normalizeDiscoveredRows,
} from './endpoint-model-picker-logic.js';

// Shared row chrome, verbatim from the local-detection picker rows in
// SettingsView so both candidate lists read as the same surface.
const ROW_DIVIDER = 'border-b last:border-b-0 border-black/[0.10] dark:border-white/[0.10]';
const MUTED_TEXT = 'text-[#8A8A8E] dark:text-[#98989D]';
const TAG_CLASS = 'shrink-0 text-[12px] px-2 py-0.5 rounded-md bg-[#E5E5EA] text-[#636366] dark:bg-white/[0.08] dark:text-[#C7C7CC]';
const ACTION_CLASS = 'shrink-0 min-h-8 px-3 rounded-full text-[14px] font-medium bg-[#007AFF]/10 text-[#007AFF] hover:bg-[#007AFF]/[0.16] dark:bg-[#0A84FF]/20 dark:text-[#0A84FF] dark:hover:bg-[#0A84FF]/[0.28] disabled:opacity-45';

// Rows above this count stop rendering (Rust already caps a listing at 2000):
// one DOM row per model is the picker's main render cost, so a capped list +
// "N more" footer keeps keystrokes on huge endpoints flat (round-1 review M7).
// Refining the filter re-slices the same pipeline.
const PICKER_RENDER_CAP = 200;

/**
 * Endpoint model picker (frozen spec §2.6): a pure, props-driven candidate
 * list shared by the chat model line (ModelFormModal) and the ACP provider
 * line (ProviderFormModal).
 *
 * Hard invariants (pinned by tests/settings_ui_smoke.js source guards):
 * - the component NEVER fetches: `listing`/`loading` arrive as props and the
 *   parent is the only caller of the probe bridge method;
 * - the component NEVER adds a model on its own: every mutation is an explicit
 *   user click routed through `onPick` (single row) or `onAddAll` (batch),
 *   and the parent decides what those callbacks do (save / confirm / fill).
 *
 * Props:
 * - `open` render gate; everything returns null when closed.
 * - `onClose` header X button.
 * - `listing` raw EndpointModelsListing (serde snake_case) or null.
 * - `loading` shows the busy row instead of the list.
 * - `onPick(model)` row add button.
 * - `filterable` show the filter input above the row limit (default true).
 * - `onAddAll` optional: when provided (chat line only) the header gains the
 *   全部添加 button; the ACP line omits the prop and gets no batch button.
 * - `baseUrl` base URL the listing was probed with; keys the saved-model
 *   dedup (id + base_url).
 * - `savedKeys` Set of saved-model keys (savedModelKeys()); matching rows are
 *   tagged 已保存 and their add button is disabled.
 * - `pickBusy` optional: a row-add save is in flight (round-3 review M3) —
 *   every row's add button disables so a rapid double-click cannot fire a
 *   second save with a fresh record id; the batch Add All button disables
 *   too (round-4 review M4): a batch snapshot taken while a single-row save
 *   is still in flight would count that row as unsaved and save it twice.
 * - `addNotice` optional `{ ok, text } | null`: a row/batch add result passed
 *   through while the picker stays open on failure (round-2 review M4) — the
 *   form's own status line sits beneath this full-screen overlay, so a failed
 *   save renders as a banner between the filter and the rows until the parent
 *   clears it (close / new fetch).
 * - `t` the i18n dictionary prop passed down the SettingsView tree (same
 *   pattern as ReasoningTierPicker; no direct i18n import here).
 */
const EndpointModelPicker = ({ open, onClose, listing, loading, onPick, filterable = true, onAddAll, baseUrl, savedKeys, addNotice, pickBusy = false, t }) => {
  const settingsCopy = t.uiSettingsDetail;
  const [filterDraft, setFilterDraft] = useState('');
  // Rows pipeline memoized on [listing, filterDraft] (round-1 review M7):
  // normalize + filter previously re-ran on every keystroke-render.
  const rows = useMemo(() => normalizeDiscoveredRows(listing), [listing]);
  const savedSet = savedKeys instanceof Set ? savedKeys : null;
  const showFilter = filterable && rows.length > FILTER_VISIBLE_ROW_LIMIT;
  const visibleRows = useMemo(
    () => (showFilter ? filterModelRows(rows, filterDraft) : rows),
    [rows, showFilter, filterDraft],
  );
  // Render cap: only the first PICKER_RENDER_CAP filtered rows hit the DOM;
  // the rest collapse into the "N more" footer below the list.
  const renderedRows = visibleRows.slice(0, PICKER_RENDER_CAP);
  const hiddenRowCount = visibleRows.length - renderedRows.length;
  // Addable count memoized on [rows, savedSet, baseUrl] (round-3 review
  // minor: it previously recomputed — one dup-key build per row — on every
  // render including every filter keystroke it does not depend on).
  // Counted over UNIQUE duplication keys (round-4 review M4 follow-up): the
  // confirmed batch collapses in-listing duplicate ids (splitBatchAdd), so a
  // listing carrying the same id twice must not advertise two addable rows.
  const addableCount = useMemo(() => {
    const uniqueKeys = new Set();
    for (const row of rows) uniqueKeys.add(modelDuplicationKey(row.id, baseUrl));
    if (!savedSet) return uniqueKeys.size;
    let addable = 0;
    for (const key of uniqueKeys) if (!savedSet.has(key)) addable += 1;
    return addable;
  }, [rows, savedSet, baseUrl]);
  if (!open) return null;
  const dupKeyOf = row => modelDuplicationKey(row.id, baseUrl);
  // Status copy: map the serde ListingStatus onto the tri-lingual hints;
  // transport errors reuse the existing connectionMessages voices so a
  // timeout/DNS failure reads like the connection test next to the button.
  const connectionCopy = settingsCopy.connectionMessages;
  let statusText = null;
  if (!loading && listing) {
    if (listing.status === 'auth_required') statusText = settingsCopy.authRequiredHint;
    else if (listing.status === 'unsupported') statusText = settingsCopy.unsupportedHint;
    else if (listing.status === 'error') {
      // 复审 minor-8:错误嗓音全部复用连接测试的 connectionMessages 三语文案
      // (零新增键,入口 chunk 有启动预算红线):
      // - 3xx 跳转 → redirect(「服务地址发生跳转」,与连接测试同源);
      // - tls / network(核心层的传输类别,http_status 为 null)各自取对应
      //   提示——network 复用 unknown 的「连接失败,请稍后重试」,不再一律
      //   落到 http_error 的「检查配置」(网络失败与配置无关)。
      const httpStatus = typeof listing.http_status === 'number' ? listing.http_status : null;
      statusText = httpStatus !== null && httpStatus >= 300 && httpStatus < 400
        ? connectionCopy.redirect
        : {
          timeout: connectionCopy.timeout,
          dns: connectionCopy.dns_failed,
          connect: connectionCopy.connection_refused,
          network: connectionCopy.unknown,
          tls: connectionCopy.tls_error,
        }[listing.transport] || connectionCopy.http_error;
    } else if (listing.status === 'listed' && rows.length === 0) statusText = settingsCopy.noModelsFound;
  }
  // Rust caps a listing (pagination / row / metadata budgets) and reports it
  // via the serde `truncated` flag — surface it instead of silently dropping
  // rows the user never sees (round-1 review picker minor).
  const listingTruncated = !loading && !!listing && listing.truncated === true;
  // role 前缀(ctx/out)进 React key(复审 minor-15):context/output 两枚徽章
  // 可能同源同值(如都来自端点且都是 33K),不带 role 会撞出重复的 key。
  const renderFactBadge = (tokens, source, role) => {
    const text = formatTokensCount(tokens);
    if (!text) return null;
    const sourceKey = factSourceLabelKey(source);
    return (
      <span key={`${role}-${source}-${text}`} className={`shrink-0 inline-flex items-center gap-1 text-[12px] px-2 py-0.5 rounded-md bg-[#007AFF]/10 text-[#007AFF] dark:bg-[#0A84FF]/20 dark:text-[#0A84FF]`}>
        <span className="font-medium">{text}</span>
        {sourceKey && <span className={MUTED_TEXT}>{settingsCopy[sourceKey]}</span>}
      </span>
    );
  };
  return (
    <div data-testid="endpoint-model-picker" className="fixed inset-0 z-[100] flex items-center justify-center bg-black/45 px-4 animate-in fade-in duration-150">
      {/* biome-ignore lint/a11y/useKeyWithClickEvents: click-bubbling stop layer; keyboard events need no bubbling; close button in the modal header */}
      {/* biome-ignore lint/a11y/noStaticElementInteractions: click-bubbling stop layer */}
      <div data-testid="endpoint-model-picker-dialog" role="dialog" aria-modal="true" aria-labelledby="endpoint-model-picker-title" onClick={e => e.stopPropagation()}
        className={`w-[440px] max-w-[90vw] flex flex-col overflow-hidden rounded-[22px] shadow-2xl max-h-[76vh] bg-white text-[#1C1C1E] dark:bg-[#1C1C1E] dark:text-[#F2F2F7]`}>
        {/* Scroll收敛在内层 min-h-0 容器(与 model-form-scroll 同款,WebKit 兼容见 SettingsView 注释)。 */}
        <div data-testid="endpoint-model-picker-scroll" className="min-h-0 flex-1 overflow-y-auto overscroll-contain custom-scrollbar">
          <div className={`px-5 py-4 flex items-start justify-between gap-4 border-b ${ROW_DIVIDER}`}>
            <div className="min-w-0">
              <h2 id="endpoint-model-picker-title" className="text-[20px] leading-6 font-semibold">{settingsCopy.modelListTitle}</h2>
              <p className={`mt-1 text-[13px] leading-[18px] truncate ${MUTED_TEXT}`}>{baseUrl || ''}</p>
            </div>
            <div className="shrink-0 flex items-center gap-2">
              {onAddAll && (
                <button type="button" data-testid="endpoint-model-add-all" onClick={onAddAll}
                  disabled={loading || pickBusy || addableCount === 0}
                  className={ACTION_CLASS}>{settingsCopy.addAllModels}</button>
              )}
              <button type="button" data-testid="endpoint-model-picker-close" onClick={onClose}
                aria-label={settingsCopy.modelListClose}
                className="h-9 w-9 shrink-0 rounded-full flex items-center justify-center bg-[#E5E5EA] text-[#636366] dark:bg-white/[0.08] dark:text-[#C7C7CC]"><X size={18} /></button>
            </div>
          </div>
          <div className="px-5 py-4">
            {loading && (
              <div data-testid="endpoint-model-picker-loading" className="min-h-[56px] flex items-center gap-2 text-[14px] text-[#0A84FF]">
                <RefreshCw size={15} className="animate-spin" />
                {settingsCopy.fetchingModelList}
              </div>
            )}
            {!loading && statusText && (
              <div data-testid="endpoint-model-picker-status" className={`px-1 py-3 text-[13px] leading-5 ${MUTED_TEXT}`}>{statusText}</div>
            )}
            {listingTruncated && (
              <div data-testid="endpoint-model-picker-truncated" className={`px-1 pt-3 text-[13px] leading-5 ${MUTED_TEXT}`}>{settingsCopy.modelListTruncated}</div>
            )}
            {!loading && !statusText && rows.length > 0 && (
              <div className="overflow-hidden rounded-[16px] bg-[#F2F2F7] dark:bg-[#2C2C2E]">
                {showFilter && (
                  <div className={`flex items-center gap-2 px-3.5 py-2.5 border-b ${ROW_DIVIDER}`}>
                    <Search size={15} className={`shrink-0 ${MUTED_TEXT}`} />
                    <input
                      value={filterDraft}
                      onChange={e => setFilterDraft(e.target.value)}
                      placeholder={settingsCopy.modelListFilterPlaceholder}
                      aria-label={settingsCopy.modelListFilterPlaceholder}
                      data-testid="endpoint-model-filter"
                      spellCheck={false}
                      className="min-w-0 flex-1 bg-transparent text-right text-[14px] leading-5 outline-none text-[#1C1C1E] placeholder:text-[#8A8A8E] dark:text-[#F2F2F7] dark:placeholder:text-[#636366]"
                    />
                  </div>
                )}
                {addNotice && (
                  // 复审 M4:添加结果就地透传——表单状态行在本组件的全屏遮罩
                  // 下方不可见,picker 保持打开重试时失败原因(或成功回执)必须
                  // 在行列表上方可见,父层在关闭/新一轮探测时清除。
                  // role="status" + aria-live="polite"(round-4 FE polish):
                  // 结果异步到达,读屏要播报而不抢焦点。
                  <div data-testid="endpoint-model-add-notice" role="status" aria-live="polite" className={`px-3.5 py-2.5 text-[13px] leading-5 ${ROW_DIVIDER} ${addNotice.ok ? 'text-[#137333] dark:text-[#93D5A6]' : 'text-[#FF3B30]'}`}>
                    {addNotice.text}
                  </div>
                )}
                {renderedRows.map(row => {
                  const alreadySaved = !!savedSet && savedSet.has(dupKeyOf(row));
                  return (
                    <div key={row.key} data-testid="endpoint-model-row" className={`min-h-[58px] px-3.5 py-2.5 flex items-center gap-3 text-left ${ROW_DIVIDER}`}>
                      <span className="min-w-0 flex-1">
                        <span className={`flex items-center gap-1.5 text-[15px] leading-5 font-normal text-[#1C1C1E] dark:text-[#F2F2F7]`}>
                          <span className="truncate">{row.id}</span>
                          {row.loaded === false && (
                            <span className={TAG_CLASS}>{settingsCopy.modelNotLoadedTag}</span>
                          )}
                          {alreadySaved && (
                            <span data-testid="endpoint-model-saved-tag" className={TAG_CLASS}>{settingsCopy.modelAlreadySaved}</span>
                          )}
                        </span>
                        {row.title && (
                          <span className={`block mt-0.5 text-[12px] leading-[17px] truncate ${MUTED_TEXT}`}>{row.title}</span>
                        )}
                        {(row.contextWindowTokens || row.maxOutputTokens) && (
                          <span className="mt-1 flex items-center gap-1.5 flex-wrap">
                            {renderFactBadge(row.contextWindowTokens, row.contextFrom, 'ctx')}
                            {renderFactBadge(row.maxOutputTokens, row.outputFrom, 'out')}
                          </span>
                        )}
                      </span>
                      <button type="button" data-testid="endpoint-model-row-add" disabled={alreadySaved || pickBusy}
                        onClick={() => onPick(row)} className={ACTION_CLASS}>{settingsCopy.addModelRow}</button>
                    </div>
                  );
                })}
                {showFilter && visibleRows.length === 0 && (
                  // Filtered-empty is NOT the same as "the endpoint returned
                  // nothing": use the dedicated copy (round-1 review minor).
                  <div data-testid="endpoint-model-filter-empty" className={`px-3.5 py-3 text-[13px] leading-5 ${MUTED_TEXT}`}>{settingsCopy.modelListFilterEmpty}</div>
                )}
                {hiddenRowCount > 0 && (
                  // round-3 review minor: the "refine the filter" advice only
                  // makes sense once a filter is actually in use; with an
                  // untouched filter the footer carries the plain count.
                  <div data-testid="endpoint-model-picker-more" className={`px-3.5 py-3 text-[13px] leading-5 ${MUTED_TEXT}`}>
                    {filterDraft.trim()
                      ? settingsCopy.modelListMoreCount(hiddenRowCount)
                      : settingsCopy.modelListMoreCountPlain(hiddenRowCount)}
                  </div>
                )}
              </div>
            )}
          </div>
        </div>
      </div>
    </div>
  );
};

export default EndpointModelPicker;
