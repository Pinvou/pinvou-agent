import { useEffect, useState } from 'react';
import { ChevronDown, Wrench } from '../../components/icons.jsx';
import { StatusDot } from '../../components/StatusDot.jsx';
import { bridge, useBridgeState } from '../../hooks/useBridge.js';
import { can } from '../../shared/platform.js';
import { isAgentWaitCall, isExpertDelegationCall } from '../conversation/conversation-model.js';
import { spawnGroupOf } from '../multiagent/spawn-aggregation.mjs';
import { dispatchOpenSubagent } from '../multiagent/subagent-panel-event.mjs';
import { QuestionChoiceCard } from '../conversation/QuestionChoiceCard.jsx';
import {
  buildUserInputAnswers,
  normalizeUserInputQuestions,
} from '../conversation/user-input-shared.js';
import { useShellTaskCancel } from '../chat/shell-task-cancel.js';
import { extractComputerUseScreenshotPath } from '../computer-use/computer-use-logic.js';
import { AcShieldCheck, AcSparkles, DiffView, GrepView, ListDirView, OutputError, OutputPre, ReceiptBlock, ShellTextView, ShellView, StockQuoteCard, TODO_TOOLS, TodoView, WeatherCard, isQuietTool, isReceipt, isStockQuoteTool, isWeatherTool, looksDiff, toolSummary, tryParseJson, tryTailJson, unwrapMcpTextEnvelope } from './tool-common.jsx';

const isShellExecutionTool = name => [
  'bash',
  'exec_shell',
  'exec_shell_wait',
  'exec_wait',
  'task_shell_start',
  'task_shell_wait',
  'shell',
  'Bash',
].includes(name);

// P1-C：专家卡是桌面能力。Web 构建没有 multiAgent bridge（capability 关闭），
// 强行渲染专家卡会吞掉原生 agent 工具的输出、点开只得空面板——capability
// 关闭时走回通用工具卡。模块级常量：对一次构建恒定，不破坏 Hook 数量稳定。
const EXPERT_CARD_ENABLED = can('multiAgent');

// Web 端的多智能体会话是只读的（桌面专属，ADR-0006）：计划裁决/受阻兜底
// 这类会触发新一轮模型执行的卡片操作，与输入框一同置灰。权威拦截在后端
// remote_control 漏斗（复核 P1），前端只是如实反馈。桌面端恒 false。
// 两个读取器共享同一次 bridge modeState 读取，只差桌面/Web 门卫：
// multiAgentWebReadOnly 先看 multiAgent 能力（桌面恒 false），swarmModeOn
// 先看 bridge 可用性（装饰性边框用，bridge 缺席时视作未开启）。
const chatModeState = () => {
  if (!bridge.state || typeof bridge.state.get !== 'function') return null;
  const chat = bridge.state.get('chat') || {};
  return (chat.modeState && chat.modeState.multiAgent) ? chat.modeState : null;
};

const multiAgentWebReadOnly = () => {
  if (can('multiAgent')) return false;
  return !!chatModeState();
};

// Read-only mirror of the swarm mode switch (same source as composer-shared:
// modeState.multiAgent, read through the shared chatModeState helper above).
// Decorative border color only; the authoritative state
// and the switch interaction live on the composer / bridge side.
function swarmModeOn() {
  if (!bridge.available) return false;
  return !!chatModeState();
}

/**
 * Swarm spawn count row: consecutive spawn calls within one message aggregate
 * into a single small row ("Pinvou created x agents"); a new spawn only
 * increments x in place. The count comes from spawn-aggregation's result over
 * the message item sequence and updates incrementally with the session flow.
 * The row records dispatch events, so its dot breathes only while some spawn
 * call of the group is still dispatching (pending/running); once every call
 * has settled the row reads as history — the live agent status lives in the
 * running overlay. Clicking dispatches `pinvou:open-subagent` (agentId=null →
 * panel list state).
 */
const AgentSpawnCountRow = ({ count, failed = 0, running = 0, sessionId, t, interactive = true }) => {
  const copy = t.uiMultiAgent;
  const on = swarmModeOn();
  // The null-agentId click opens the subagent panel's list state, which only
  // the main chat host implements; hosts without that view render the row
  // honestly non-interactive instead of dispatching into a no-op handler.
  const clickable = interactive && !!sessionId;
  const accent = on
    ? 'bg-[#7C3AED] dark:bg-[#A78BFA]'
    : 'bg-[#0B57D0] dark:bg-[#A8C7FA]';
  return (
    <button
      type="button"
      data-testid="agent-spawn-count-row"
      disabled={!clickable}
      onClick={clickable ? () => dispatchOpenSubagent(null, sessionId) : undefined}
      title={clickable ? copy.spawnedAgentsRowHint : undefined}
      className={`my-1 flex max-w-[520px] items-center gap-2 rounded-full px-2 py-1 text-[11.5px] text-[#8E8E93] ${
        clickable ? 'cursor-pointer hover:bg-black/[0.04] dark:hover:bg-white/[0.06]' : 'cursor-default'
      }`}
    >
      <span
        className={`inline-block h-1.5 w-1.5 shrink-0 rounded-full ${accent} ${running > 0 ? 'animate-pulse' : ''}`}
      />
      <span className="truncate">{copy.spawnedAgentsRow(count)}</span>
      {failed > 0 && (
        <span className="shrink-0 text-[#C5221F] dark:text-[#F28B82]">
          {copy.agentCard.spawnFailed} × {failed}
        </span>
      )}
    </button>
  );
};

/**
 * Quiet coordination row for non-spawn `agent` calls (status/wait/cancel,
 * ADR-0006 swarm rework): one muted line so coordination operations do not
 * pose as new delegations. Spawn-type calls never reach this component —
 * ToolCard routes them to the aggregated AgentSpawnCountRow ahead of it.
 */
const ExpertAgentCard = ({ item, t }) => {
  const copy = t.uiMultiAgent;
  const args = item.args || {};
  const action = isAgentWaitCall(item.name, args) ? 'wait' : String(args.action || 'start');
  return (
    <div
        data-testid="agent-coordination-row"
        className="my-1 flex items-center gap-2 px-1 text-[11.5px] text-[#8E8E93]"
      >
        <span className="inline-block h-1.5 w-1.5 rounded-full bg-current opacity-50" />
        <span className="truncate">{copy.coordinationRow(action)}{args.agent_id ? ` · ${args.agent_id}` : ''}</span>
      </div>
  );
};

// ── computer_use screenshot card ───────────────────────────────────────────
// The tool stores screenshots in the session workspace at attachments/computer_use/*.png;
// the text output carries the absolute path. Whenever the feature switch is off, always
// fall back to the default tool card (the feature stays invisible); fall back likewise when
// the output carries no screenshot path.
// Subscribed-slice read (useBridgeState, same pattern as SettingsView's
// computer-use section) instead of an imperative bridge.state.get() during
// render: the imperative read only re-evaluated on unrelated parent
// re-renders, so a toggle flip left stale screenshot cards behind. React
// function components only — do not call outside render.
const useComputerUseToolCardEnabled = () => {
  const slice = useBridgeState(['computerUse']);
  return !!(slice && slice.computerUse && slice.computerUse.enabled);
};

function computerUseScreenshotForItem(item, featureEnabled) {
  if (!item || item.name !== 'computer_use' || item.state !== 'done') return null;
  if (!featureEnabled) return null;
  return extractComputerUseScreenshotPath(item.output);
}

const ComputerUseScreenshotCard = ({ item, path, t }) => {
  const copy = t.uiComputerUse;
  const [imageUrl, setImageUrl] = useState(null);
  const [loadFailed, setLoadFailed] = useState(false);
  useEffect(() => {
    let cancelled = false;
    setImageUrl(null); // eslint-disable-line react-hooks/set-state-in-effect -- a new screenshot path starts a fresh load; synchronously clearing the previous image avoids a stale frame flash
    setLoadFailed(false);
    if (!bridge.available || !bridge.artifacts || !bridge.artifacts.readArtifactImageB64) {
      setLoadFailed(true);
      return;
    }
    bridge.artifacts.readArtifactImageB64(path)
      .then((url) => { if (!cancelled) { setImageUrl(url || null); if (!url) setLoadFailed(true); } })
      .catch(() => { if (!cancelled) setLoadFailed(true); });
    return () => { cancelled = true; };
  }, [path]);
  if (loadFailed) return <OutputPre text={item.output} />;
  const openFull = () => {
    if (bridge.available && bridge.artifacts && bridge.artifacts.openArtifactExternal) {
      bridge.artifacts.openArtifactExternal(path, item.sessionId);
    }
  };
  return (
    <div data-testid="computer-use-screenshot-card" className="my-1">
      <div className="text-[11px] mb-1 text-[#757575] dark:text-[#8E8E8E]">{copy.screenshotCaption}</div>
      {imageUrl ? (
        <button
          type="button"
          onClick={openFull}
          title={path}
          className="block max-w-[360px] rounded-[12px] overflow-hidden border border-black/10 dark:border-white/10 hover:opacity-90 transition-opacity"
        >
          <img src={imageUrl} alt={copy.screenshotCaption} className="block w-full h-auto" />
        </button>
      ) : (
        <div className="text-[12px] text-[#757575] dark:text-[#8E8E8E]">{copy.screenshotLoading}</div>
      )}
    </div>
  );
};

// eslint-disable-next-line sonarjs/cognitive-complexity -- per-tool output view routing; splitting by tool has low payoff;legacy view; tracked separately
const ToolOutput = ({ item, t }) => {
      const computerUseEnabled = useComputerUseToolCardEnabled();
      const out = item.output;
      if (item.success === false) return <OutputError text={out} />;
      // computer_use: render the screenshot card when the output references
      // attachments/computer_use/*.png; with no screenshot or the feature off, fall back to
      // the default <OutputPre>.
      if (item.name === 'computer_use') {
        const screenshotPath = computerUseScreenshotForItem(item, computerUseEnabled);
        if (screenshotPath) return <ComputerUseScreenshotCard item={item} path={screenshotPath} t={t} />;
        return <OutputPre text={out} />;
      }
      if (isWeatherTool(item.name)) {
        const raw = unwrapMcpTextEnvelope(out);
        const w = tryParseJson(raw);
        if (w && w.type === 'weather' && !w.error) return <WeatherCard data={w} t={t} />;
      }
      // 股票报价卡片：iwencai 返回表格数据 → 映射为卡片
      if (isStockQuoteTool(item.name)) {
        const raw = unwrapMcpTextEnvelope(out);
        const w = tryParseJson(raw);
        if (w && Array.isArray(w.datas) && w.datas.length > 0) {
          const d = w.datas[0];
          const findVal = (obj, keyword) => {
            for (const k of Object.keys(obj)) {
              // eslint-disable-next-line unicorn/prefer-number-coercion -- market fields may carry unit suffixes; keep the permissive parseFloat like the price field below
              if (k.includes(keyword)) return Number.parseFloat(obj[k]);
            }
            // A miss must stay undefined: StockQuoteCard's fmt/isNaN fallback and
            // >= 0 gain/loss test both rely on undefined semantics — null would
            // crash null.toFixed and null >= 0 would read as a gain.
            return undefined; // eslint-disable-line unicorn/no-useless-undefined -- the consumer depends on undefined fallback semantics
          };
          const mapped = {
            name: d['股票简称'] || '--',
            code: (d['股票代码'] || '').replace(/\.\w+$/, ''),
            price: Number.parseFloat(d['最新价']), // eslint-disable-line unicorn/prefer-number-coercion -- quote fields may carry unit characters; keep parseFloat's lenient parsing
            changePercent: findVal(d, '涨跌幅'),
            open: findVal(d, '开盘价'),
            high: findVal(d, '最高价'),
            low: findVal(d, '最低价'),
          };
          return <StockQuoteCard data={mapped} t={t} />;
        }
        if (w && w.type === 'stock_quote' && !w.error) return <StockQuoteCard data={w} t={t} />;
      }
      if (isReceipt(out)) return <ReceiptBlock text={out} t={t} />;
      if (item.name === 'list_dir' || (item.name === 'File' && item.args?.action === 'list')) { const v = tryParseJson(out); if (Array.isArray(v)) return <ListDirView items={v} t={t} />; }
      else if (item.name === 'grep_files' || (item.name === 'File' && item.args?.action === 'search_content')) { const v = tryParseJson(out); if (v && Array.isArray(v.matches)) return <GrepView data={v} t={t} />; }
      else if (isShellExecutionTool(item.name)) {
        const v = tryParseJson(out);
        if (v && (v.stdout != null || v.exit_code != null || v.status)) return <ShellView data={v} t={t} />;
        return <ShellTextView cmd={item.args && item.args.command} text={out} />;
      }
      // File.write / File.edit 走 unified diff；File.patch 返回结构化 PatchResult。
      else if (item.name === 'File' && item.args?.action === 'patch') {
        const result = tryParseJson(out);
        if (result && typeof result === 'object') {
          const files = Array.isArray(result.touched_files) ? result.touched_files : [];
          return <div className="space-y-1 text-xs">
            {result.message ? <div>{String(result.message)}</div> : null}
            {files.map(path => <div key={path} className="font-mono break-all">{path}</div>)}
            {(result.files_applied != null || result.hunks_applied != null) ? <div className="text-[#757575] dark:text-[#8E8E8E]">
              files {result.files_applied ?? files.length} · hunks {result.hunks_applied ?? 0}
            </div> : null}
          </div>;
        }
      }
      else if ((item.name === 'File' && ['write', 'edit'].includes(item.args?.action)) || ['edit', 'write', 'edit_file', 'write_file'].includes(item.name)) { if (looksDiff(out)) return <DiffView text={out} t={t} />; }
      else if (TODO_TOOLS.includes(item.name)) { const v = tryTailJson(out); if (v && Array.isArray(v.items)) return <TodoView snap={v} t={t} />; }
      return <OutputPre text={out} />;
    };

    // Swarm rework (ADR-0006): spawn-type `agent` calls no longer render the
    // space-hogging expert card banner; they become one aggregated count row.
    // Coordination operations (status/wait/cancel) still go through
    // ExpertAgentCard's quiet coordination row. The early return happens
    // before any Hook of this component, and item.name never changes for a
    // given instance, so each instance's Hook count stays constant.
    // eslint-disable-next-line sonarjs/cognitive-complexity -- tool card rendering contains many inline branches;legacy view; tracked separately
    const ToolCard = ({ item, t, variant = 'legacy', sessionId, spawnRowInteractive = true }) => {
      if (EXPERT_CARD_ENABLED && (item.name === 'agent' || isAgentWaitCall(item.name, item.args))) {
        const delegation = isExpertDelegationCall(item.name, item.args);
        if (delegation) {
          // Spawn items arrive pre-annotated: each lane's projection input is
          // annotated exactly once (ChatView for the main chat, projectNativeLane
          // for the codex native lane), so the count row reads the item's own
          // fields directly. The codex native host has no subagent list view,
          // so its rows render non-interactive instead of dispatching into a
          // no-op `pinvou:open-subagent` handler.
          const group = item.spawnGroup;
          const hidden = item.spawnGroupHidden;
          // Non-first spawns of an aggregated sequence do not repeat the text row.
          if (hidden) return null;
          const resolved = group || spawnGroupOf(item);
          return (
            <AgentSpawnCountRow
              count={resolved.count}
              failed={resolved.failed || 0}
              running={resolved.running || 0}
              sessionId={sessionId}
              interactive={spawnRowInteractive}
              t={t}
            />
          );
        }
        return <ExpertAgentCard item={item} t={t} />;
      }
      const isTimeline = variant === 'timeline';
      const isRunning = item.state === 'running';
      // eslint-disable-next-line react-hooks/rules-of-hooks -- the early-return branch is constant for an instance's lifetime (see the comment above); the per-instance Hook count is stable
      const { cancelling, cancelError: shellCancelError, cancel: cancelShellTask } = useShellTaskCancel(t);
      // eslint-disable-next-line react-hooks/rules-of-hooks -- same as above
      const computerUseEnabled = useComputerUseToolCardEnabled();
      // Tools with a visual card (weather/stocks/computer_use screenshot) expand directly
      // when done, no collapsing
      const hasCard = ((isWeatherTool(item.name) || isStockQuoteTool(item.name)) && item.state === 'done')
        || !!computerUseScreenshotForItem(item, computerUseEnabled);
      const hasLiveShellOutput = isShellExecutionTool(item.name)
        && isRunning
        && (item.liveOutput || item.output != null);
      // eslint-disable-next-line react-hooks/rules-of-hooks -- same as above
      const [expanded, setExpanded] = useState(!isTimeline && hasCard);
      // eslint-disable-next-line react-hooks/rules-of-hooks -- same as above
      useEffect(() => {
        if (!isTimeline && hasCard) {
          setExpanded(true); // eslint-disable-line react-hooks/set-state-in-effect -- expand once when the weather/stock card completes; idempotent
        }
      }, [hasCard, isTimeline]);
      const displayExpanded = hasLiveShellOutput || expanded;
      const isDone = item.state === 'done';
      const isFailed = item.state === 'failed';
      const quiet = isQuietTool(item);
      const summary = toolSummary(item.name, item.args, t);

      // 状态色:按 isRunning/isDone/isFailed 三态,各自给出 light base + dark: token。
      const statusColor = isRunning
        ? 'text-[#0B57D0] dark:text-[#A8C7FA]'
        : isDone
          ? 'text-[#137333] dark:text-[#93D5A6]'
          : 'text-[#C5221F] dark:text-[#F28B82]';

      const statusText = isRunning ? t.toolRunning
        : (item.exitCode == null ? (isDone ? t.toolDone : t.toolFailed) : `${isDone ? t.toolDone : t.toolFailed} · exit ${item.exitCode}`);
      const timelineStatusText = isRunning
        ? t.uiToolRender.running
        : item.exitCode == null
          ? isDone
            ? t.uiToolRender.done
            : t.uiToolRender.failed
          : `${isDone ? t.uiToolRender.done : t.uiToolRender.failed} · exit ${item.exitCode}`;
      const mutedColor = 'text-[#757575] dark:text-[#8E8E8E]';
      const cancelBackground = (event) => {
        event.stopPropagation();
        cancelShellTask(item.sessionId, item.taskId);
      };
      const cancelButton = item.taskId && isRunning ? (
        <button
          type="button"
          data-testid="cancel-shell-task"
          data-shell-task-id={item.taskId}
          disabled={cancelling}
          onClick={cancelBackground}
          className={`text-[11px] px-2 py-1 rounded-full disabled:opacity-50 bg-black/5 text-[#C5221F] hover:bg-black/10 dark:bg-white/10 dark:text-[#F28B82] dark:hover:bg-white/15`}
        >
          {cancelling ? t.cancelling : t.cancel}
        </button>
      ) : null;

      const detail = displayExpanded ? (
        <div className={`${isTimeline ? 'px-3 pb-3' : 'px-4 pb-3'} border-t border-black/5 dark:border-white/5`}>
          {item.output == null
            ? null
            : <div className="mt-2"><ToolOutput item={item} t={t} /></div>}
        </div>
      ) : null;

      if (isTimeline) {
        const tone = isFailed
          ? 'text-red-500 bg-red-500/10'
          : isRunning
            ? 'text-blue-500 bg-blue-500/10'
            : 'text-gray-500 bg-black/[0.04] dark:bg-white/[0.06]';
        const summaryPrefix = summary ? `${summary} · ` : '';
        const meta = `${summaryPrefix}${timelineStatusText}`;
        const toggleExpanded = () => setExpanded(value => !value);
        return (
          <div
            data-tool-card-variant="timeline"
            data-tool-name={item.name}
            className={`rounded-xl border ${
              isFailed ? 'border-red-500/20' : 'border-black/[0.05] dark:border-white/[0.07]'
            } bg-white/45 dark:bg-white/[0.015]`}
          >
            {/* biome-ignore lint/a11y/useSemanticElements: the tool card collapse header hosts a multi-child layout; a button would break existing styles */}
            <div
              role="button"
              tabIndex={0}
              onClick={toggleExpanded}
              onKeyDown={(event) => {
                if (event.key === 'Enter' || event.key === ' ') {
                  event.preventDefault();
                  toggleExpanded();
                }
              }}
              className="w-full min-h-10 px-2.5 py-2 flex items-center gap-2.5 text-left rounded-xl cursor-pointer hover:bg-black/[0.025] dark:hover:bg-white/[0.035]"
            >
              <span className={`w-6 h-6 shrink-0 rounded-lg flex items-center justify-center ${tone}`}>
                <Wrench size={13} />
              </span>
              <span className="min-w-0 flex-1">
                <span className="block truncate text-[12px] font-medium">{item.name}</span>
                <span className="block mt-0.5 truncate text-[10px] text-gray-400">{meta}</span>
              </span>
              {isRunning && <StatusDot tone="run" />}
              {cancelButton}
              <ChevronDown size={13} className={`shrink-0 text-gray-400 transition-transform ${displayExpanded ? 'rotate-180' : ''}`} />
            </div>
            {shellCancelError && (
              <div className="px-3 pb-2 text-[11px] text-red-500">{shellCancelError}</div>
            )}
            {detail}
          </div>
        );
      }

      // 弱化类：单行灰条。完成态低调（图标灰），运行/失败态保留状态色以便察觉。
      if (quiet) {
        const iconColor = isDone ? mutedColor : statusColor;
        return (
          <div className={expanded ? `rounded-[12px] overflow-hidden border border-black/5 dark:border-white/5` : ''}>
            {/* biome-ignore lint/a11y/useSemanticElements: the tool card collapse header hosts a multi-child layout; a button would break existing styles */}
            <div
              role="button"
              tabIndex={0}
              className={`flex items-center gap-2 px-2 py-1 rounded-[8px] cursor-pointer hover:bg-[#E8EDF2] dark:hover:bg-[#282A2C]`}
              onClick={() => setExpanded(!expanded)}
              onKeyDown={(event) => { if (event.key === 'Enter' || event.key === ' ') { event.preventDefault(); setExpanded(!expanded); } }}
            >
              <Wrench size={12} className={iconColor} />
              <span className={`text-[12px] ${mutedColor}`}>{item.name}</span>
              {summary
                ? <span className={`text-[12px] flex-1 truncate ${mutedColor}`}>{summary}</span>
                : <span className="flex-1" />}
              {isRunning && <span className={`text-[11px] ${statusColor}`}>{t.toolRunning}</span>}
              {isFailed && <span className={`text-[11px] ${statusColor}`}>{t.toolFailed}</span>}
              {cancelButton}
              <ChevronDown size={12} className={`transition-transform ${expanded ? 'rotate-180' : ''} ${mutedColor}`} />
            </div>
            {detail}
          </div>
        );
      }

      // 有产出类：保留醒目卡片，标题行带摘要。
      return (
        <div className={`rounded-[16px] overflow-hidden border bg-[#F0F4F9] border-black/5 dark:bg-[#1E1F20] dark:border-white/5`}>
          {/* biome-ignore lint/a11y/useSemanticElements: the tool card collapse header hosts a multi-child layout; a button would break existing styles */}
          <div
            role="button"
            tabIndex={0}
            className={`flex items-center gap-3 px-4 py-3 cursor-pointer hover:bg-[#E8EDF2] dark:hover:bg-[#282A2C]`}
            onClick={() => setExpanded(!expanded)}
            onKeyDown={(event) => { if (event.key === 'Enter' || event.key === ' ') { event.preventDefault(); setExpanded(!expanded); } }}
          >
            <Wrench size={14} className={statusColor} />
            <span className={`text-[13px] font-medium text-[#1F1F1F] dark:text-[#E3E3E3]`}>
              {item.name}
            </span>
            {summary
              ? <span className={`text-[12px] flex-1 truncate ${mutedColor}`}>{summary}</span>
              : <span className="flex-1" />}
            <span className={`text-[12px] ${statusColor}`}>{statusText}</span>
            {cancelButton}
            <ChevronDown size={14} className={`transition-transform ${expanded ? 'rotate-180' : ''} text-[#444746] dark:text-[#C4C7C5]`} />
          </div>
          {shellCancelError && (
            <div className={`px-4 pb-2 text-[11px] text-[#C5221F] dark:text-[#F28B82]`}>
              {shellCancelError}
            </div>
          )}
          {detail}
        </div>
      );
    };

    // ==========================================
    // Plan / 待办 步骤渲染
    // ==========================================
    const STEP_SYM = { completed: '●', in_progress: '◎', pending: '○' };
    const PlanLayer = ({ label, explanation, items, field }) => {
      if (!items || items.length === 0) return null;
      return (
        <section className="mb-2">
          <div className={`text-[12px] font-semibold mb-1 text-[#0B57D0] dark:text-[#A8C7FA]`}>{label}</div>
          {explanation && <p className={`text-[13px] mb-1.5 leading-relaxed text-[#444746] dark:text-[#C4C7C5]`}>{explanation}</p>}
          <ol className="space-y-1">
            {items.map((it, i) => (
              <li key={i} className={`text-[13px] flex gap-2 leading-relaxed ${it.status === 'completed' ? 'opacity-60' : ''} text-[#1F1F1F] dark:text-[#E3E3E3]`}>
                <span className={it.status === 'in_progress' ? 'text-[#E37400] dark:text-[#FDD663]' : ''}>{STEP_SYM[it.status] || '○'}</span>
                <span>{it[field] || ''}</span>
              </li>
            ))}
          </ol>
        </section>
      );
    };

    const cardBoxCls = (accent) =>
      `rounded-[16px] border p-4 my-1 bg-[#F0F4F9] border-black/5 dark:bg-[#1E1F20] dark:border-white/10 ${accent || ''}`;
    const cardBtnCls = (variant) => {
      const base = 'px-3 py-1.5 rounded-full text-[13px] font-medium transition-colors disabled:opacity-50 disabled:cursor-not-allowed';
      if (variant === 'primary') return `${base} bg-[#0B57D0] text-white hover:bg-[#0A4BB8] dark:bg-[#A8C7FA] dark:text-[#062E6F] dark:hover:bg-[#C2DBFF]`;
      // 危险确认（如首切 YOLO）：红底白字，深浅色同配色（红色在两种主题下对比度都够）。
      if (variant === 'danger') return `${base} bg-[#C5221F] text-white hover:bg-[#A50E0E]`;
      return `${base} bg-white text-[#1F1F1F] hover:bg-[#E1E5EA] border border-black/10 dark:border-transparent dark:bg-[#333537] dark:text-[#E3E3E3] dark:hover:bg-[#444746]`;
    };

    // 品悟角色配色（与产物卡一致）：品=盾·橙 #FF9500/#FF9F0A，悟=闪光·紫 #5E5CE6。
    // 返回 { name, accentHex(inline-style 原色,品需 isDark), text(类), softBg(类), Icon }。
    const pvRole = (isWu, isDark) => isWu
      ? { name: '悟', accentHex: '#5E5CE6', text: 'text-[#5E5CE6]',
          softBg: 'bg-[#5E5CE6]/[0.10] dark:bg-[#5E5CE6]/15', Icon: AcSparkles }
      : { name: '品', accentHex: isDark ? '#FF9F0A' : '#FF9500', text: 'text-[#FF9500] dark:text-[#FF9F0A]',
          softBg: 'bg-[#FF9500]/[0.10] dark:bg-[#FF9F0A]/15', Icon: AcShieldCheck };

    // ==========================================
    // PinvouSummonCard — 🧭 召唤式检阅（Boss 主动呼叫 Pinvou）
    // 自报家门人格(单主+alternates,§3.3) + trace + issues(severity 分色)。
    // ==========================================
    // 逐条裁决行(§2 + kind 分流):每条按本质给对应动作,不再一刀切判断题——
    //   recommendation(决策点/缺信息,Boss 才能定)→ 采纳建议 / 让 AI 问我;
    //   issue.needs_verify(外部事实,AI 无知识)→ 让 AI 核实 / 我确认没问题;
    //   issue 其他(产物缺陷,AI 改得动)→ 让 AI 改 / 接受现状(high 默认勾)。
    // 「交给 AI 处理」按各条动作组装定向指令走 B1。单独子组件:useState 放这避 hooks 错位。
    const PinvouRows = ({ review, t, role }) => {
      const roleLabel = role || pvRole(false, false);
      const body = 'text-[#000] dark:text-[#fff]';
      const muted = 'text-[#3C3C43]/60 dark:text-[#EBEBF5]/60';
      // iOS 语义色：high 红 / medium 橙 / low 灰
      const sevDot = (s) => s === 'high' ? '#FF3B30' : s === 'medium' ? '#FF9500' : '#C7C7CC';
      const rows = [
        ...(review.recommendations || []).map((x, i) => ({
          k: 'r' + i, raw: x, kind: 'rec', dot: '#FF9500',
          head: (x.topic ? x.topic + '：' : '') + t.pvSuggest + x.pick, sub: x.why,
        })),
        ...(review.issues || []).map((x, i) => ({
          k: 'i' + i, raw: x, kind: x.kind === 'needs_verify' ? 'verify' : 'fix',
          dot: sevDot(x.severity), sev: x.severity, nv: x.kind === 'needs_verify',
          head: x.text, sub: x.suggestion,
        })),
        ...(review.coverage || []).map((x, i) => ({
          k: 'c' + i, raw: x, kind: 'gap', dot: '#5E5CE6',
          sev: x.severity, head: x.dimension + (x.text ? '：' + x.text : ''), sub: x.suggestion,
        })),
      ];
      // 每类二选一 [值,文案]:第一个=「要 AI 做」(高亮),第二个=Boss 自己消化(灰)。
      const ACT = {
        rec: [['adopt', t.pvActAdopt], ['ask', t.pvActAsk]],
        verify: [['verify', t.pvActVerify], ['confirmed', t.pvActConfirmed]],
        fix: [['modify', t.pvActModify], ['accept', t.pvActAccept]],
        gap: [['fill', t.pvActFill], ['skip', t.pvActSkip]],
      };
      const ACTIVE = { adopt: 1, ask: 1, verify: 1, modify: 1, fill: 1 }; // 需转交给 AI 的动作
      const [res, setRes] = useState(() => {
        const m = {};
        rows.forEach(it => {
          let def = null;
          if (it.sev === 'high') def = it.kind === 'fix' ? 'modify' : it.kind === 'gap' ? 'fill' : null;
          m[it.k] = it.raw.resolution || def;
        });
        return m;
      });
      const setOne = (k, v) => setRes(p => ({ ...p, [k]: p[k] === v ? null : v }));
      const activeCount = rows.filter(it => ACTIVE[res[it.k]]).length;
      // iOS 分段按钮风：选中且需转交 AI=填充角色色(背景走 style)；选中但自行消化=灰填充；未选=描边。
      const chip = (on, active) => `text-[12px] px-2.5 py-1 rounded-full font-medium transition-all active:scale-[0.96] ${on
        ? (active ? 'text-white border border-transparent'
                  : 'bg-black/[0.08] text-[#000] border border-transparent dark:bg-white/15 dark:text-[#fff]')
        : 'border border-black/[0.12] text-[#3C3C43]/80 hover:bg-black/5 dark:border-white/15 dark:text-[#EBEBF5]/70 dark:hover:bg-white/5'}`;
      const onResolve = () => {
        if (!bridge.available) return;
        // 弹窗里 review 是 notify 深拷贝,写它的 resolution 落不到原 state;把裁决按下标传给 bridge,
        // 由 bridge 在 state.pinvouModal.review(原 state)上写、再落盘(根治 resolution 不持久化)。
        const resolutions = {
          recs: (review.recommendations || []).map((_, i) => res['r' + i] || 'pending'),
          issues: (review.issues || []).map((_, i) => res['i' + i] || 'pending'),
          coverage: (review.coverage || []).map((_, i) => res['c' + i] || 'pending'),
        };
        const actions = [];
        rows.forEach(it => {
          const a = res[it.k];
          if (a === 'modify') actions.push({ t: 'fix', text: it.head + (it.sub ? '（' + it.sub + '）' : '') });
          else if (a === 'verify') actions.push({ t: 'verify', text: it.head + (it.sub ? '（' + it.sub + '）' : '') });
          else if (a === 'adopt') actions.push({ t: 'adopt', topic: it.raw.topic || '', pick: it.raw.pick || '' });
          else if (a === 'ask') actions.push({ t: 'ask', topic: it.raw.topic || it.head });
          else if (a === 'fill') actions.push({ t: 'fill', dimension: it.raw.dimension || '', suggestion: it.raw.suggestion || '' });
        });
        bridge.interaction.resolvePinvouReview(resolutions, actions);
      };
      return (
        <div>
          <div className="space-y-2">
            {rows.map(it => {
              const decided = res[it.k];
              const passive = ['accept', 'confirmed', 'skip'].includes(decided);
              return (
                <div key={it.k} className={`rounded-[12px] px-3 py-2.5 transition-opacity ${passive ? 'opacity-40' : ''} bg-[#F2F2F7] dark:bg-white/[0.06]`}>
                  <div className="flex gap-2.5">
                    <span className="mt-[7px] w-[7px] h-[7px] rounded-full shrink-0" style={{ background: it.dot }} />
                    <div className="flex-1 min-w-0">
                      <div className={`text-[14px] leading-relaxed ${body}`}>
                        {it.nv && <span className={`text-[10.5px] font-medium mr-1.5 px-1.5 py-px rounded-full align-[1px] bg-[#FFF8E1] text-[#B25000] dark:bg-[#FFD60A]/20 dark:text-[#FFD60A]`}>{t.pvNeedsVerify}</span>}
                        {it.head}
                      </div>
                      {it.sub && <div className={`text-[13px] mt-0.5 ${muted}`}>{it.sub}</div>}
                      <div className="flex gap-2 mt-2">
                        {ACT[it.kind].map(([v, label]) => (
                          <button type="button" key={v} onClick={() => setOne(it.k, v)} className={chip(decided === v, !!ACTIVE[v])}
                            style={decided === v && ACTIVE[v] ? { background: roleLabel.accentHex } : undefined}>{label}</button>
                        ))}
                      </div>
                    </div>
                  </div>
                </div>
              );
            })}
          </div>
          <div className="flex items-center gap-2 mt-4 pt-1">
            {activeCount > 0 && (
              <button type="button" onClick={onResolve}
                className="px-4 py-2 rounded-full text-[14px] font-semibold text-white active:scale-[0.97] transition-transform"
                style={{ background: roleLabel.accentHex }}>
                {t.pvHandToAi(activeCount)}
              </button>
            )}
            <button type="button" onClick={() => bridge.available && bridge.interaction.dismissPinvouReview()} title={t.pvSkipTitle}
              className={`px-4 py-2 rounded-full text-[14px] font-medium transition-colors text-[#3C3C43]/70 hover:bg-black/5 dark:text-[#EBEBF5]/70 dark:hover:bg-white/5`}>
              {t.pvSkip}
            </button>
          </div>
        </div>
      );
    };

    // 检阅 loading:本地模型 5-30s / 在线模型通常更快,iOS 旋转菊花 spinner + 计时 + 安抚文字,别让 Boss 干等焦虑。
    const PinvouLoading = ({ isWu, isDark, t, isLocal }) => {
      const [secs, setSecs] = useState(0);
      useEffect(() => {
        const b = setInterval(() => setSecs(s => s + 1), 1000);
        return () => clearInterval(b);
      }, []);
      const role = pvRole(isWu, isDark);
      // isDark 保留——SVG circle stroke 与 pvRole 品·accentHex(inline-style 原色)仍需它。
      const muted = 'text-[#3C3C43]/60 dark:text-[#EBEBF5]/60';
      return (
        <div className="py-8 flex flex-col items-center text-center">
          {/* iOS activity spinner：底环 + 角色色弧，匀速旋转 */}
          <svg aria-hidden="true" className="w-9 h-9" viewBox="0 0 24 24" fill="none" style={{ animation: 'tsSpinner 0.8s linear infinite' }}>
            <circle cx="12" cy="12" r="9" stroke={isDark ? 'rgba(255,255,255,.12)' : 'rgba(0,0,0,.08)'} strokeWidth="3" />
            <path d="M12 3a9 9 0 0 1 9 9" stroke={role.accentHex} strokeWidth="3" strokeLinecap="round" />
          </svg>
          <div className={`flex items-center gap-1.5 mt-4 text-[15px] font-semibold ${role.text}`}>
            <role.Icon className="w-[18px] h-[18px]" />
            <span>{isWu ? t.pvLoadingWu : t.pvLoadingPin}</span>
          </div>
          <div className={`text-[13px] mt-1.5 ${muted}`}>
            {isWu ? t.pvLoadingWuSub : t.pvLoadingPinSub}
            {secs > 0 && <span className="ml-1.5 tabular-nums opacity-70">{secs}s</span>}
          </div>
          <div className={`text-[12px] mt-1 ${muted}`} style={{ opacity: 0.6 }}>{t.pvLoadingHint(isLocal)}</div>
        </div>
      );
    };

    // 检阅结果卡（在底部 sheet 内渲染，无外层卡框；品=橙 / 悟=紫，与产物卡一致）。
    const PinvouSummonCard = ({ item, theme, t, isLocal }) => {
      const isDark = theme === 'dark';
      const isWu = !!item.coverage; // 悟=发散(coverage)；品=查错
      const role = pvRole(isWu, isDark);
      const muted = 'text-[#3C3C43]/60 dark:text-[#EBEBF5]/60';
      const body = 'text-[#000] dark:text-[#fff]';
      // isDark 保留——pvRole 品·accentHex 与 PinvouLoading SVG stroke 仍需它。
      if (item.loading) return <PinvouLoading isWu={isWu} isDark={isDark} t={t} isLocal={isLocal} />;
      if (item.error) return (
        <div className="py-2">
          <div className={`flex items-center gap-1.5 text-[15px] font-semibold ${role.text}`}><role.Icon className="w-[18px] h-[18px]" /><span>Pinvou {role.name}</span></div>
          <div className={`text-[14px] mt-2 text-[#FF3B30] dark:text-[#FF453A]`}>{t.pvFail}{item.error}</div>
        </div>
      );
      const r = item.review || {};
      if (r.dismissed) return (
        <div className={`py-2 flex items-center gap-1.5 text-[14px] ${muted}`}><role.Icon className="w-4 h-4" /><span>{'Pinvou · ' + role.name + ' · ' + t.pvSkipped}</span></div>
      );
      const personas = r.personas || [];
      const primary = personas.find(p => p && p.primary) || personas[0] || {};
      const alts = r.alternates || [];
      const hasRows = (r.recommendations || []).length > 0 || (r.issues || []).length > 0 || (r.coverage || []).length > 0;
      return (
        <div>
          <div className="flex items-center flex-wrap gap-x-2 gap-y-1 mb-2.5">
            <span className={`inline-flex items-center justify-center w-7 h-7 rounded-full ${role.softBg}`}>
              <role.Icon className={`w-[17px] h-[17px] ${role.text}`} />
            </span>
            <span className={`text-[16px] font-semibold ${body}`}>
              {'Pinvou · ' + role.name}
              {primary.label && <span className={`text-[14px] font-normal ${muted}`}> · {primary.label + t.pvPerspective}</span>}
            </span>
            {r.verdict === 'pass' && <span className={`text-[11px] font-semibold px-2 py-0.5 rounded-full bg-[#34C759]/15 text-[#248A3D] dark:bg-[#30D158]/20 dark:text-[#30D158]`}>{t.pvVerdictPass}</span>}
          </div>
          {alts.length > 0 && <div className={`text-[12px] -mt-1 mb-2 ${muted}`}>{t.pvAlsoInvolves} {alts.join(' / ')}</div>}
          {r.trace && <div className={`text-[14px] leading-relaxed mb-3 ${body}`}>{r.trace}</div>}
          {(r.framework || []).length > 0 && (
            <div className={`text-[12px] mb-3 px-3 py-2 rounded-[12px] leading-relaxed ${role.softBg} ${role.text}`}>
              <span className="opacity-70">{t.pvFramework} · {(r.framework || []).length}{t.pvDims}: </span>{(r.framework || []).join(' · ')}
            </div>
          )}
          {hasRows && <PinvouRows review={r} t={t} role={role} />}
        </div>
      );
    };

    // ==========================================
    // PlanCard — ✨ 方案准备好
    // ==========================================
    const PlanCard = ({ item, t, onPrefill }) => {
      const webReadOnly = multiAgentWebReadOnly();
      const active = item.cardState === 'active' && !item.resolved && !!item.planId;
      return (
        <div className={cardBoxCls('border-[#0B57D0]/20 dark:border-[#A8C7FA]/30')}>
          <div className={`text-[14px] font-semibold mb-3 text-[#1F1F1F] dark:text-[#E3E3E3]`}>{t.planReady}</div>
          {(!item.plan && !item.todos)
            ? <div className={`text-[13px] text-[#444746] dark:text-[#C4C7C5]`}>{t.planEmpty}</div>
            : <>
                <PlanLayer label={t.planLabel} explanation={item.plan && item.plan.explanation} items={item.plan && item.plan.items} field="step" />
                <PlanLayer label={t.planTodos} items={item.todos && item.todos.items} field="content" />
              </>}
          <div className={`h-px my-3 bg-black/10 dark:bg-white/10`}></div>
          {active ? (
            <div className="flex items-center gap-2 flex-wrap">
              <span className={`text-[13px] mr-1 text-[#444746] dark:text-[#C4C7C5]`}>{t.planNext}</span>
              <button type="button" className={cardBtnCls('primary') + ' disabled:opacity-40 disabled:cursor-not-allowed'} disabled={webReadOnly} onClick={() => bridge.interaction.acceptPlan(item.id, item.planMarkdown, undefined, item.planId)}>{t.planGo}</button>
              <button type="button" className={cardBtnCls() + ' disabled:opacity-40 disabled:cursor-not-allowed'} disabled={webReadOnly} onClick={() => onPrefill && onPrefill(t.planRevisePrefill)}>{t.planEdit}</button>
              <button type="button" className={cardBtnCls() + ' disabled:opacity-40 disabled:cursor-not-allowed'} disabled={webReadOnly} onClick={() => bridge.interaction.discardPlan(item.id, item.planId)}>{t.planDrop}</button>
            </div>
          ) : (
            <div className={`text-[13px] font-medium text-[#137333] dark:text-[#93D5A6]`}>{item.statusLabel}</div>
          )}
        </div>
      );
    };

    // ==========================================
    // PlanStuckCard — Plan 模式 AI 撞只读保护(白名单/sandbox)的兜底卡
    // ==========================================
    const PlanStuckCard = ({ item, t, onGo }) => {
      const webReadOnly = multiAgentWebReadOnly();
      const done = item.resolved;
      return (
        <div className={cardBoxCls('border-[#E37400]/20 dark:border-[#FDD663]/30')}>
          <div className={`text-[13px] leading-relaxed mb-3 text-[#1F1F1F] dark:text-[#E3E3E3]`}>
            {t.stuckPlanPre} <code className="px-1 rounded bg-black/20">{item.toolName || t.uiToolRender.toolUnknown}</code> {t.stuckPlanPost}
          </div>
          {done ? (
            <div className={`text-[13px] text-[#444746] dark:text-[#C4C7C5]`}>{item.statusLabel || t.handled}</div>
          ) : (
            <div className="flex items-center gap-2 flex-wrap">
              <button type="button" className={cardBtnCls() + ' disabled:opacity-40 disabled:cursor-not-allowed'} disabled={webReadOnly} onClick={() => bridge.interaction.planStuckReplan(item.id)}>{t.stuckReplan}</button>
              <button type="button" className={cardBtnCls('primary') + ' disabled:opacity-40 disabled:cursor-not-allowed'} disabled={webReadOnly} onClick={() => (onGo ? onGo(item.id) : bridge.interaction.planStuckGo(item.id))}>⚡ {t.stuckGo}</button>
            </div>
          )}
        </div>
      );
    };

    // ==========================================
    // CarefulBlockedCard — 🛑 危险操作被拦（人话化：底座英文技术原因→中文人话，技术详情折叠）
    // ==========================================
    const REASON_MAP = [
      [/root filesystem|delete all root|delete root/i, 'rsRoot'],
      [/home director/i, 'rsHome'],
      [/recursiv|rm\s+-rf/i, 'rsRecursive'],
      [/forced? deletion|\bforce\b/i, 'rsForce'],
      [/fork bomb/i, 'rsForkbomb'],
      [/overwrite|\bof=|\bdd\b/i, 'rsOverwrite'],
      [/format|mkfs/i, 'rsFormat'],
    ];
    const humanizeReason = (en, t) => {
      const s = String(en);
      for (let i = 0; i < REASON_MAP.length; i++) if (REASON_MAP[i][0].test(s)) return t[REASON_MAP[i][1]];
      return t.rsDefault;
    };
    const CarefulBlockedCard = ({ item, t }) => {
      const [showTech, setShowTech] = useState(false);
      const md = item.metadata || {};
      const cmd = (item.args && (item.args.command || item.args.cmd)) || t.cbCmdUnknown;
      const rawReasons = (md.reasons && md.reasons.length) ? md.reasons : [];
      const rawSuggestions = md.suggestions || [];
      const humanReasons = [...new Set(rawReasons.map(r => humanizeReason(r, t)))];
      if (humanReasons.length === 0) humanReasons.push(t.rsDefault);
      const hasTech = rawReasons.length > 0 || rawSuggestions.length > 0;
      return (
        <div className={cardBoxCls('border-[#C5221F]/30 dark:border-[#F28B82]/40')}>
          <div className={`text-[14px] font-semibold mb-2 text-[#C5221F] dark:text-[#F28B82]`}>{t.cbTitle}</div>
          <div className={`text-[12px] mb-1 text-[#757575] dark:text-[#8E8E8E]`}>{t.cbWant}</div>
          <pre className={`text-[12px] font-mono rounded-lg p-2 mb-2 overflow-x-auto bg-white text-[#C5221F] dark:bg-[#131314] dark:text-[#F28B82]`}>{cmd}</pre>
          <div className="mb-2">
            <div className={`text-[12px] font-medium mb-1 text-[#444746] dark:text-[#C4C7C5]`}>{t.cbWhy}</div>
            <ul className={`list-disc pl-5 text-[13px] space-y-0.5 text-[#1F1F1F] dark:text-[#E3E3E3]`}>{humanReasons.map((r, i) => <li key={i}>{r}</li>)}</ul>
          </div>
          <div className={`text-[12px] leading-relaxed mb-1.5 text-[#757575] dark:text-[#8E8E8E]`}>{t.cbNote}</div>
          {hasTech && (
            <div>
              <button type="button" onClick={() => setShowTech(!showTech)} className={`text-[11px] text-[#0B57D0] dark:text-[#8AB4F8]`}>{showTech ? t.cbTechHide : t.cbTechShow}</button>
              {showTech && (
                <div className={`mt-1 text-[11px] font-mono space-y-0.5 text-[#757575] dark:text-[#8E8E8E]`}>
                  {rawReasons.map((r, i) => <div key={'r' + i}>· {r}</div>)}
                  {rawSuggestions.map((s, i) => <div key={'s' + i}>→ {s}</div>)}
                </div>
              )}
            </div>
          )}
        </div>
      );
    };

    // ==========================================
    // UserInputCard — 🤔 AI 想问你几个问题
    // ==========================================
    // isFreeTextPlaceholderOption / question normalization / answer assembly are shared with the code session's
    // NativeUserInputCard in conversation/user-input-shared.js.

    const UserInputCard = ({ item, t }) => {
      // Web 只读会话：呈现为锁定卡并说明去桌面端操作（后端漏斗是权威拦截，
      // 这里避免"能点但必败"的按钮，复核 P2）。
      const webReadOnly = multiAgentWebReadOnly();
      const questions = item.questions || [];
      const normalizedQuestions = normalizeUserInputQuestions(questions);

      function submit(groups) {
        if (webReadOnly) return;
        const answers = buildUserInputAnswers(groups, t.uiToolRender.other);
        bridge.interaction.submitUserInput(item.id, item.toolCallId, answers, questions);
      }

      const statusText = webReadOnly && !item.resolved
        ? t.uiMultiAgent.webActionHint
        : item.cardState === 'submitted' ? t.uiSubmitted
        : item.cardState === 'cancelled' ? t.uiCancelled
        : item.submitting ? t.uiSubmitting : item.error ? t.uiSubmitFailed(item.error) : '';

      return (
        <QuestionChoiceCard
          title={t.uiqTitle}
          questions={normalizedQuestions}
          initialAnswers={item.restoredAnswers || []}
          resolved={Boolean(item.resolved) || webReadOnly}
          submitting={Boolean(item.submitting)}
          statusText={statusText}
          error={Boolean(item.error)}
          submitLabel={t.uiSubmit}
          cancelLabel={t.cpCancel}
          otherPlaceholder={t.uiToolRender.other}
          otherAnswerLabel={t.uiToolRender.other}
          inputPlaceholder={t.uiConversation.inputPlaceholder}
          onSubmit={submit}
          onCancel={!item.resolved && !webReadOnly
            ? () => bridge.interaction.cancelUserInput(item.id, item.toolCallId)
            : undefined}
        />
      );
    };

export { ToolOutput, ToolCard, PlanLayer, cardBoxCls, cardBtnCls, PinvouSummonCard, PlanCard, PlanStuckCard, CarefulBlockedCard, UserInputCard };
