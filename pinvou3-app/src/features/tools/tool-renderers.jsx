import React, { useEffect, useRef, useState } from 'react';
import { ChevronDown, ChevronRight, FileText, Wrench } from '../../components/icons.jsx';
import { bridge } from '../../hooks/useBridge.js';
import { QuestionChoiceCard } from '../conversation/QuestionChoiceCard.jsx';
import { AcShieldCheck, AcSparkles, ArtifactCard, DiffView, GrepView, ListDirView, OutputError, OutputPre, QUIET_TOOLS, ReceiptBlock, ShellTextView, ShellView, StockQuoteCard, TODO_TOOLS, TodoView, WeatherCard, isReceipt, isStockQuoteTool, isWeatherTool, looksDiff, outBox, parseReceipt, toolBasename, toolSummary, tryParseJson, tryTailJson } from './tool-common.jsx';

const isShellExecutionTool = name => [
  'exec_shell',
  'exec_shell_wait',
  'exec_wait',
  'task_shell_start',
  'task_shell_wait',
  'shell',
].includes(name);

const ToolOutput = ({ item, isDark, t }) => {
      const out = item.output;
      if (item.success === false) return <OutputError text={out} isDark={isDark} />;
      if (isWeatherTool(item.name)) {
        let raw = out;
        const envelope = tryParseJson(out);
        if (envelope && Array.isArray(envelope.content)) {
          const t = envelope.content.find(c => c.type === 'text');
          if (t && t.text) raw = t.text;
        }
        const w = tryParseJson(raw);
        if (w && w.type === 'weather' && !w.error) return <WeatherCard data={w} />;
      }
      // 股票报价卡片：iwencai 返回表格数据 → 映射为卡片
      if (isStockQuoteTool(item.name)) {
        let raw = out;
        const envelope = tryParseJson(out);
        if (envelope && Array.isArray(envelope.content)) {
          const t = envelope.content.find(c => c.type === 'text');
          if (t && t.text) raw = t.text;
        }
        const w = tryParseJson(raw);
        if (w && Array.isArray(w.datas) && w.datas.length > 0) {
          const d = w.datas[0];
          const findVal = (obj, keyword) => {
            for (const k of Object.keys(obj)) {
              if (k.includes(keyword)) return parseFloat(obj[k]);
            }
            return undefined;
          };
          const mapped = {
            name: d['股票简称'] || '--',
            code: (d['股票代码'] || '').replace(/\.\w+$/, ''),
            price: parseFloat(d['最新价']),
            changePercent: findVal(d, '涨跌幅'),
            open: findVal(d, '开盘价'),
            high: findVal(d, '最高价'),
            low: findVal(d, '最低价'),
          };
          return <StockQuoteCard data={mapped} isDark={isDark} />;
        }
        if (w && w.type === 'stock_quote' && !w.error) return <StockQuoteCard data={w} isDark={isDark} />;
      }
      if (isReceipt(out)) return <ReceiptBlock text={out} isDark={isDark} t={t} />;
      if (item.name === 'list_dir') { const v = tryParseJson(out); if (Array.isArray(v)) return <ListDirView items={v} isDark={isDark} t={t} />; }
      else if (item.name === 'grep_files') { const v = tryParseJson(out); if (v && Array.isArray(v.matches)) return <GrepView data={v} isDark={isDark} t={t} />; }
      else if (isShellExecutionTool(item.name)) {
        const v = tryParseJson(out);
        if (v && (v.stdout != null || v.exit_code != null || v.status)) return <ShellView data={v} isDark={isDark} t={t} />;
        return <ShellTextView cmd={item.args && item.args.command} text={out} isDark={isDark} />;
      }
      // edit_file / write_file 走 Rust similar crate 输出 unified diff,走 DiffView。
      // 注意:apply_patch 后端返回 JSON(apply_patch.rs::execute 返回 ToolResult::json),
      // looksDiff 永远 false,所以这里不把 apply_patch 加进路由 —— 加了也只是 dead code
      // (PR #195 M2)。若未来后端给 apply_patch 输出 unified diff,再把它加回来。
      else if (item.name === 'edit_file' || item.name === 'write_file') { if (looksDiff(out)) return <DiffView text={out} isDark={isDark} />; }
      else if (item.name === 'append_file') {
        const m = String(out).match(/appended (\d+) bytes[\s\S]*?\((\d+) -> (\d+) bytes\)/i);
        if (m) return <div className={outBox(isDark)}>{t.appendBytes(/^Created/i.test(out), m[1], m[2], m[3])}</div>;
      }
      else if (TODO_TOOLS.indexOf(item.name) >= 0) { const v = tryTailJson(out); if (v && Array.isArray(v.items)) return <TodoView snap={v} isDark={isDark} t={t} />; }
      return <OutputPre text={out} isDark={isDark} />;
    };

    const ToolCard = ({ item, theme, t, variant = 'legacy' }) => {
      const isDark = theme === 'dark';
      const isTimeline = variant === 'timeline';
      const isRunning = item.state === 'running';
      const [cancelling, setCancelling] = useState(false);
      const [shellCancelError, setShellCancelError] = useState('');
      // 有可视化卡片的工具(天气/股票)完成后直接展开,不折叠
      const hasCard = (isWeatherTool(item.name) || isStockQuoteTool(item.name)) && item.state === 'done';
      const hasLiveShellOutput = isShellExecutionTool(item.name)
        && isRunning
        && (item.liveOutput || item.output != null);
      const shouldAutoExpand = !isTimeline && (hasLiveShellOutput || hasCard);
      const [expanded, setExpanded] = useState(shouldAutoExpand);
      useEffect(() => {
        if (!isTimeline && shouldAutoExpand) {
          setExpanded(true);
        }
      }, [isTimeline, shouldAutoExpand]);
      const isDone = item.state === 'done';
      const isFailed = item.state === 'failed';
      const quiet = QUIET_TOOLS.has(item.name);
      const summary = toolSummary(item.name, item.args, t);

      const statusColor = isRunning
        ? (isDark ? 'text-[#A8C7FA]' : 'text-[#0B57D0]')
        : isDone
          ? (isDark ? 'text-[#93D5A6]' : 'text-[#137333]')
          : (isDark ? 'text-[#F28B82]' : 'text-[#C5221F]');

      const statusText = isRunning ? t.toolRunning
        : (item.exitCode != null ? `${isDone ? t.toolDone : t.toolFailed} · exit ${item.exitCode}` : (isDone ? t.toolDone : t.toolFailed));
      const timelineStatusText = isRunning
        ? '进行中'
        : item.exitCode != null
          ? `${isDone ? '完成' : '失败'} · exit ${item.exitCode}`
          : isDone
            ? '完成'
            : '失败';
      const mutedColor = isDark ? 'text-[#8E8E8E]' : 'text-[#757575]';
      const cancelBackground = async (event) => {
        event.stopPropagation();
        if (!item.taskId || cancelling) return;
        setCancelling(true);
        setShellCancelError('');
        try {
          await bridge.chat.cancelShellTask(item.sessionId, item.taskId);
        } catch (error) {
          console.warn('cancel shell task failed', error);
          setShellCancelError(`${t.shellCancelFailed || t.toolFailed}: ${String(error)}`);
        } finally {
          setCancelling(false);
        }
      };
      const cancelButton = item.taskId && isRunning ? (
        <button
          type="button"
          data-testid="cancel-shell-task"
          data-shell-task-id={item.taskId}
          disabled={cancelling}
          onClick={cancelBackground}
          className={`text-[11px] px-2 py-1 rounded-full disabled:opacity-50 ${isDark ? 'bg-white/10 text-[#F28B82] hover:bg-white/15' : 'bg-black/5 text-[#C5221F] hover:bg-black/10'}`}
        >
          {cancelling ? t.cancelling : t.cancel}
        </button>
      ) : null;

      const detail = expanded ? (
        <div className={`${isTimeline ? 'px-3 pb-3' : 'px-4 pb-3'} border-t ${isDark ? 'border-white/5' : 'border-black/5'}`}>
          {item.output != null
            ? <div className="mt-2"><ToolOutput item={item} isDark={isDark} t={t} /></div>
            : null}
        </div>
      ) : null;

      if (isTimeline) {
        const tone = isFailed
          ? 'text-red-500 bg-red-500/10'
          : isRunning
            ? 'text-blue-500 bg-blue-500/10'
            : 'text-gray-500 bg-black/[0.04] dark:bg-white/[0.06]';
        const meta = `${summary ? `${summary} · ` : ''}${timelineStatusText}`;
        const toggleExpanded = () => setExpanded(value => !value);
        return (
          <div
            data-tool-card-variant="timeline"
            data-tool-name={item.name}
            className={`rounded-xl border ${
              isFailed ? 'border-red-500/20' : 'border-black/[0.05] dark:border-white/[0.07]'
            } bg-white/45 dark:bg-white/[0.015]`}
          >
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
              {isRunning && <span className="w-1.5 h-1.5 rounded-full bg-blue-500 animate-pulse" />}
              {cancelButton}
              <ChevronDown size={13} className={`shrink-0 text-gray-400 transition-transform ${expanded ? 'rotate-180' : ''}`} />
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
          <div className={expanded ? `rounded-[12px] overflow-hidden border ${isDark ? 'border-white/5' : 'border-black/5'}` : ''}>
            <div
              className={`flex items-center gap-2 px-2 py-1 rounded-[8px] cursor-pointer ${isDark ? 'hover:bg-[#282A2C]' : 'hover:bg-[#E8EDF2]'}`}
              onClick={() => setExpanded(!expanded)}
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
        <div className={`rounded-[16px] overflow-hidden border ${isDark ? 'bg-[#1E1F20] border-white/5' : 'bg-[#F0F4F9] border-black/5'}`}>
          <div
            className={`flex items-center gap-3 px-4 py-3 cursor-pointer ${isDark ? 'hover:bg-[#282A2C]' : 'hover:bg-[#E8EDF2]'}`}
            onClick={() => setExpanded(!expanded)}
          >
            <Wrench size={14} className={statusColor} />
            <span className={`text-[13px] font-medium ${isDark ? 'text-[#E3E3E3]' : 'text-[#1F1F1F]'}`}>
              {item.name}
            </span>
            {summary
              ? <span className={`text-[12px] flex-1 truncate ${mutedColor}`}>{summary}</span>
              : <span className="flex-1" />}
            <span className={`text-[12px] ${statusColor}`}>{statusText}</span>
            {cancelButton}
            <ChevronDown size={14} className={`transition-transform ${expanded ? 'rotate-180' : ''} ${isDark ? 'text-[#C4C7C5]' : 'text-[#444746]'}`} />
          </div>
          {shellCancelError && (
            <div className={`px-4 pb-2 text-[11px] ${isDark ? 'text-[#F28B82]' : 'text-[#C5221F]'}`}>
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
    const PlanLayer = ({ label, explanation, items, field, isDark }) => {
      if (!items || items.length === 0) return null;
      return (
        <section className="mb-2">
          <div className={`text-[12px] font-semibold mb-1 ${isDark ? 'text-[#A8C7FA]' : 'text-[#0B57D0]'}`}>{label}</div>
          {explanation && <p className={`text-[13px] mb-1.5 leading-relaxed ${isDark ? 'text-[#C4C7C5]' : 'text-[#444746]'}`}>{explanation}</p>}
          <ol className="space-y-1">
            {items.map((it, i) => (
              <li key={i} className={`text-[13px] flex gap-2 leading-relaxed ${it.status === 'completed' ? 'opacity-60' : ''} ${isDark ? 'text-[#E3E3E3]' : 'text-[#1F1F1F]'}`}>
                <span className={it.status === 'in_progress' ? (isDark ? 'text-[#FDD663]' : 'text-[#E37400]') : ''}>{STEP_SYM[it.status] || '○'}</span>
                <span>{it[field] || ''}</span>
              </li>
            ))}
          </ol>
        </section>
      );
    };

    const cardBoxCls = (isDark, accent) =>
      `rounded-[16px] border p-4 my-1 ${isDark ? 'bg-[#1E1F20] border-white/10' : 'bg-[#F0F4F9] border-black/5'} ${accent || ''}`;
    const cardBtnCls = (isDark, variant) => {
      const base = 'px-3 py-1.5 rounded-full text-[13px] font-medium transition-colors disabled:opacity-50 disabled:cursor-not-allowed';
      if (variant === 'primary') return `${base} ${isDark ? 'bg-[#A8C7FA] text-[#062E6F] hover:bg-[#C2DBFF]' : 'bg-[#0B57D0] text-white hover:bg-[#0A4BB8]'}`;
      return `${base} ${isDark ? 'bg-[#333537] text-[#E3E3E3] hover:bg-[#444746]' : 'bg-white text-[#1F1F1F] hover:bg-[#E1E5EA] border border-black/10'}`;
    };

    // 品悟角色配色（与产物卡一致）：品=盾·橙 #FF9500/#FF9F0A，悟=闪光·紫 #5E5CE6。
    // 返回 { name, accentHex, text(类), softBg(类), Icon }。
    const pvRole = (isWu, isDark) => isWu
      ? { name: '悟', accentHex: '#5E5CE6', text: 'text-[#5E5CE6]',
          softBg: isDark ? 'bg-[#5E5CE6]/15' : 'bg-[#5E5CE6]/[0.10]', Icon: AcSparkles }
      : { name: '品', accentHex: isDark ? '#FF9F0A' : '#FF9500', text: isDark ? 'text-[#FF9F0A]' : 'text-[#FF9500]',
          softBg: isDark ? 'bg-[#FF9F0A]/15' : 'bg-[#FF9500]/[0.10]', Icon: AcShieldCheck };

    // ==========================================
    // PinvouSummonCard — 🧭 召唤式检阅（Boss 主动呼叫 Pinvou）
    // 自报家门人格(单主+alternates,§3.3) + trace + issues(severity 分色)。
    // ==========================================
    // 逐条裁决行(§2 + kind 分流):每条按本质给对应动作,不再一刀切判断题——
    //   recommendation(决策点/缺信息,Boss 才能定)→ 采纳建议 / 让 AI 问我;
    //   issue.needs_verify(外部事实,AI 无知识)→ 让 AI 核实 / 我确认没问题;
    //   issue 其他(产物缺陷,AI 改得动)→ 让 AI 改 / 接受现状(high 默认勾)。
    // 「交给 AI 处理」按各条动作组装定向指令走 B1。单独子组件:useState 放这避 hooks 错位。
    const PinvouRows = ({ review, theme, t, role }) => {
      const isDark = theme === 'dark';
      role = role || pvRole(false, isDark);
      const body = isDark ? 'text-[#fff]' : 'text-[#000]';
      const muted = isDark ? 'text-[#EBEBF5]/60' : 'text-[#3C3C43]/60';
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
                  : (isDark ? 'bg-white/15 text-[#fff] border border-transparent' : 'bg-black/[0.08] text-[#000] border border-transparent'))
        : (isDark ? 'border border-white/15 text-[#EBEBF5]/70 hover:bg-white/5' : 'border border-black/[0.12] text-[#3C3C43]/80 hover:bg-black/5')}`;
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
              const passive = decided === 'accept' || decided === 'confirmed' || decided === 'skip';
              return (
                <div key={it.k} className={`rounded-[12px] px-3 py-2.5 transition-opacity ${passive ? 'opacity-40' : ''} ${isDark ? 'bg-white/[0.06]' : 'bg-[#F2F2F7]'}`}>
                  <div className="flex gap-2.5">
                    <span className="mt-[7px] w-[7px] h-[7px] rounded-full shrink-0" style={{ background: it.dot }} />
                    <div className="flex-1 min-w-0">
                      <div className={`text-[14px] leading-relaxed ${body}`}>
                        {it.nv && <span className={`text-[10.5px] font-medium mr-1.5 px-1.5 py-px rounded-full align-[1px] ${isDark ? 'bg-[#FFD60A]/20 text-[#FFD60A]' : 'bg-[#FFF8E1] text-[#B25000]'}`}>{t.pvNeedsVerify}</span>}
                        {it.head}
                      </div>
                      {it.sub && <div className={`text-[13px] mt-0.5 ${muted}`}>{it.sub}</div>}
                      <div className="flex gap-2 mt-2">
                        {ACT[it.kind].map(([v, label]) => (
                          <button key={v} onClick={() => setOne(it.k, v)} className={chip(decided === v, !!ACTIVE[v])}
                            style={decided === v && ACTIVE[v] ? { background: role.accentHex } : undefined}>{label}</button>
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
              <button onClick={onResolve}
                className="px-4 py-2 rounded-full text-[14px] font-semibold text-white active:scale-[0.97] transition-transform"
                style={{ background: role.accentHex }}>
                {t.pvHandToAi(activeCount)}
              </button>
            )}
            <button onClick={() => bridge.available && bridge.interaction.dismissPinvouReview()} title={t.pvSkipTitle}
              className={`px-4 py-2 rounded-full text-[14px] font-medium transition-colors ${isDark ? 'text-[#EBEBF5]/70 hover:bg-white/5' : 'text-[#3C3C43]/70 hover:bg-black/5'}`}>
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
      const muted = isDark ? 'text-[#EBEBF5]/60' : 'text-[#3C3C43]/60';
      return (
        <div className="py-8 flex flex-col items-center text-center">
          {/* iOS activity spinner：底环 + 角色色弧，匀速旋转 */}
          <svg className="w-9 h-9" viewBox="0 0 24 24" fill="none" style={{ animation: 'tsSpinner 0.8s linear infinite' }}>
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
      const muted = isDark ? 'text-[#EBEBF5]/60' : 'text-[#3C3C43]/60';
      const body = isDark ? 'text-[#fff]' : 'text-[#000]';
      if (item.loading) return <PinvouLoading isWu={isWu} isDark={isDark} t={t} isLocal={isLocal} />;
      if (item.error) return (
        <div className="py-2">
          <div className={`flex items-center gap-1.5 text-[15px] font-semibold ${role.text}`}><role.Icon className="w-[18px] h-[18px]" /><span>Pinvou {role.name}</span></div>
          <div className={`text-[14px] mt-2 ${isDark ? 'text-[#FF453A]' : 'text-[#FF3B30]'}`}>{t.pvFail}{item.error}</div>
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
            {r.verdict === 'pass' && <span className={`text-[11px] font-semibold px-2 py-0.5 rounded-full ${isDark ? 'bg-[#30D158]/20 text-[#30D158]' : 'bg-[#34C759]/15 text-[#248A3D]'}`}>{t.pvVerdictPass}</span>}
          </div>
          {alts.length > 0 && <div className={`text-[12px] -mt-1 mb-2 ${muted}`}>{t.pvAlsoInvolves} {alts.join(' / ')}</div>}
          {r.trace && <div className={`text-[14px] leading-relaxed mb-3 ${body}`}>{r.trace}</div>}
          {(r.framework || []).length > 0 && (
            <div className={`text-[12px] mb-3 px-3 py-2 rounded-[12px] leading-relaxed ${role.softBg} ${role.text}`}>
              <span className="opacity-70">{t.pvFramework} · {(r.framework || []).length}{t.pvDims}: </span>{(r.framework || []).join(' · ')}
            </div>
          )}
          {hasRows && <PinvouRows review={r} theme={theme} t={t} role={role} />}
        </div>
      );
    };

    // ==========================================
    // PlanCard — ✨ 方案准备好
    // ==========================================
    const PlanCard = ({ item, theme, t, onPrefill }) => {
      const isDark = theme === 'dark';
      const active = item.cardState === 'active' && !item.resolved && !!item.planId;
      return (
        <div className={cardBoxCls(isDark, isDark ? 'border-[#A8C7FA]/30' : 'border-[#0B57D0]/20')}>
          <div className={`text-[14px] font-semibold mb-3 ${isDark ? 'text-[#E3E3E3]' : 'text-[#1F1F1F]'}`}>{t.planReady}</div>
          {(!item.plan && !item.todos)
            ? <div className={`text-[13px] ${isDark ? 'text-[#C4C7C5]' : 'text-[#444746]'}`}>{t.planEmpty}</div>
            : <>
                <PlanLayer label={t.planLabel} explanation={item.plan && item.plan.explanation} items={item.plan && item.plan.items} field="step" isDark={isDark} />
                <PlanLayer label={t.planTodos} items={item.todos && item.todos.items} field="content" isDark={isDark} />
              </>}
          <div className={`h-px my-3 ${isDark ? 'bg-white/10' : 'bg-black/10'}`}></div>
          {active ? (
            <div className="flex items-center gap-2 flex-wrap">
              <span className={`text-[13px] mr-1 ${isDark ? 'text-[#C4C7C5]' : 'text-[#444746]'}`}>{t.planNext}</span>
              <button className={cardBtnCls(isDark, 'primary')} onClick={() => bridge.interaction.acceptPlan(item.id, item.planMarkdown, undefined, item.planId)}>{t.planGo}</button>
              <button className={cardBtnCls(isDark)} onClick={() => onPrefill && onPrefill(t.planRevisePrefill)}>{t.planEdit}</button>
              <button className={cardBtnCls(isDark)} onClick={() => bridge.interaction.discardPlan(item.id, item.planId)}>{t.planDrop}</button>
            </div>
          ) : (
            <div className={`text-[13px] font-medium ${isDark ? 'text-[#93D5A6]' : 'text-[#137333]'}`}>{item.statusLabel}</div>
          )}
        </div>
      );
    };

    // ==========================================
    // PlanStuckCard — Plan 模式 AI 撞只读保护(白名单/sandbox)的兜底卡
    // ==========================================
    const PlanStuckCard = ({ item, theme, t }) => {
      const isDark = theme === 'dark';
      const done = item.resolved;
      return (
        <div className={cardBoxCls(isDark, isDark ? 'border-[#FDD663]/30' : 'border-[#E37400]/20')}>
          <div className={`text-[13px] leading-relaxed mb-3 ${isDark ? 'text-[#E3E3E3]' : 'text-[#1F1F1F]'}`}>
            {t.stuckPlanPre} <code className="px-1 rounded bg-black/20">{item.toolName || '(unknown)'}</code> {t.stuckPlanPost}
          </div>
          {done ? (
            <div className={`text-[13px] ${isDark ? 'text-[#C4C7C5]' : 'text-[#444746]'}`}>{item.statusLabel || t.handled}</div>
          ) : (
            <div className="flex items-center gap-2 flex-wrap">
              <button className={cardBtnCls(isDark)} onClick={() => bridge.interaction.planStuckReplan(item.id)}>{t.stuckReplan}</button>
              <button className={cardBtnCls(isDark, 'primary')} onClick={() => bridge.interaction.planStuckGo(item.id)}>⚡ {t.stuckGo}</button>
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
    const CarefulBlockedCard = ({ item, theme, t }) => {
      const isDark = theme === 'dark';
      const [showTech, setShowTech] = useState(false);
      const md = item.metadata || {};
      const cmd = (item.args && (item.args.command || item.args.cmd)) || t.cbCmdUnknown;
      const rawReasons = (md.reasons && md.reasons.length) ? md.reasons : [];
      const rawSuggestions = md.suggestions || [];
      const humanReasons = [...new Set(rawReasons.map(r => humanizeReason(r, t)))];
      if (humanReasons.length === 0) humanReasons.push(t.rsDefault);
      const hasTech = rawReasons.length > 0 || rawSuggestions.length > 0;
      return (
        <div className={cardBoxCls(isDark, isDark ? 'border-[#F28B82]/40' : 'border-[#C5221F]/30')}>
          <div className={`text-[14px] font-semibold mb-2 ${isDark ? 'text-[#F28B82]' : 'text-[#C5221F]'}`}>{t.cbTitle}</div>
          <div className={`text-[12px] mb-1 ${isDark ? 'text-[#8E8E8E]' : 'text-[#757575]'}`}>{t.cbWant}</div>
          <pre className={`text-[12px] font-mono rounded-lg p-2 mb-2 overflow-x-auto ${isDark ? 'bg-[#131314] text-[#F28B82]' : 'bg-white text-[#C5221F]'}`}>{cmd}</pre>
          <div className="mb-2">
            <div className={`text-[12px] font-medium mb-1 ${isDark ? 'text-[#C4C7C5]' : 'text-[#444746]'}`}>{t.cbWhy}</div>
            <ul className={`list-disc pl-5 text-[13px] space-y-0.5 ${isDark ? 'text-[#E3E3E3]' : 'text-[#1F1F1F]'}`}>{humanReasons.map((r, i) => <li key={i}>{r}</li>)}</ul>
          </div>
          <div className={`text-[12px] leading-relaxed mb-1.5 ${isDark ? 'text-[#8E8E8E]' : 'text-[#757575]'}`}>{t.cbNote}</div>
          {hasTech && (
            <div>
              <button onClick={() => setShowTech(!showTech)} className={`text-[11px] ${isDark ? 'text-[#8AB4F8]' : 'text-[#0B57D0]'}`}>{showTech ? t.cbTechHide : t.cbTechShow}</button>
              {showTech && (
                <div className={`mt-1 text-[11px] font-mono space-y-0.5 ${isDark ? 'text-[#8E8E8E]' : 'text-[#757575]'}`}>
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
    const isFreeTextPlaceholderOption = (option) => {
      const label = String(option?.label || '').trim();
      return /^(?:其他|其它|other)(?:\s*[\(（][^()（）]*[\)）])?$/i.test(label);
    };

    const UserInputCard = ({ item, t }) => {
      const questions = item.questions || [];
      const normalizedQuestions = questions.map((question, index) => {
        const allowOther = question.allow_free_text !== false;
        return {
          id: question.id || `question-${index + 1}`,
          header: question.header || `Q${index + 1}`,
          question: question.question || '',
          options: (question.options || [])
            .filter(option => !allowOther || !isFreeTextPlaceholderOption(option))
            .map(option => ({
              value: option.label,
              label: option.label,
              description: option.description || '',
            })),
          allowOther,
          multiSelect: Boolean(question.multi_select),
          required: !question.multi_select,
        };
      });

      function submit(groups) {
        const answers = groups.flatMap(group => group.answers.map(answer => ({
          id: group.questionId,
          label: answer.other ? '其他' : answer.label,
          value: String(answer.value),
        })));
        bridge.interaction.submitUserInput(item.id, item.toolCallId, answers, questions);
      }

      const statusText = item.cardState === 'submitted' ? t.uiSubmitted
        : item.cardState === 'cancelled' ? t.uiCancelled
        : item.submitting ? t.uiSubmitting : item.error ? t.uiSubmitFailed(item.error) : '';

      return (
        <QuestionChoiceCard
          title={t.uiqTitle}
          questions={normalizedQuestions}
          initialAnswers={item.restoredAnswers || []}
          resolved={Boolean(item.resolved)}
          submitting={Boolean(item.submitting)}
          statusText={statusText}
          error={Boolean(item.error)}
          submitLabel={t.uiSubmit}
          cancelLabel={t.cpCancel}
          otherPlaceholder="Other"
          onSubmit={submit}
          onCancel={!item.resolved
            ? () => bridge.interaction.cancelUserInput(item.id, item.toolCallId)
            : undefined}
        />
      );
    };

    // ==========================================
    // ArtifactsPanel — 产物面板（右侧抽屉 + 预览）
    // ==========================================
    // 产物列表/预览的 iOS 风类型图标:配色圆角 tile + 白色字形。
    // 复用成品卡那套 _ARTIFACT_FMT / _artifactKind / AcFmtIcon(line 3048+),列表与卡片视觉统一。

export { ToolOutput, ToolCard, STEP_SYM, PlanLayer, cardBoxCls, cardBtnCls, pvRole, PinvouRows, PinvouLoading, PinvouSummonCard, PlanCard, PlanStuckCard, REASON_MAP, humanizeReason, CarefulBlockedCard, UserInputCard };
