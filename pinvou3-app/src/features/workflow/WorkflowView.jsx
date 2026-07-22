import React, { useEffect, useRef, useState } from 'react';
import { bridge } from '../../hooks/useBridge.js';
import { useCompactViewport } from '../../hooks/useViewport.js';
import { can, isWeb } from '../../shared/platform.js';
import { OFFICE_HTML_STYLE } from '../artifacts/ArtifactsPanel.jsx';
import { ScaledHtmlPreview } from '../settings/SettingsView.jsx';
import { cardBtnCls } from '../tools/tool-renderers.jsx';

const WidgetCard = ({ title, children, theme }) => {
      const isDark = theme === 'dark';
      return (
        <div className={`rounded-[24px] p-8 flex flex-col transition-shadow hover:shadow-md ${isDark ? 'bg-[#1E1F20]' : 'bg-[#F0F4F9]'}`}>
          <div className={`text-[14px] font-medium tracking-wide mb-6 ${isDark ? 'text-[#A8C7FA]' : 'text-[#0B57D0]'}`}>
            {title}
          </div>
          <div className="flex-1 flex flex-col">
            {children}
          </div>
        </div>
      );
    };

    const ProgressBar = ({ label, value, subValue, percentage, theme, color = "#0B57D0" }) => {
      const isDark = theme === 'dark';
      return (
        <div>
          <div className="flex justify-between items-end mb-2">
            <span className={`text-[14px] ${isDark ? 'text-[#E3E3E3]' : 'text-[#1F1F1F]'}`}>{label}</span>
            <div className="text-right">
              <span className={`text-[16px] font-medium ${isDark ? 'text-[#E3E3E3]' : 'text-[#1F1F1F]'}`}>{value}</span>
              {subValue && <span className={`text-[13px] ml-2 ${isDark ? 'text-[#C4C7C5]' : 'text-[#444746]'}`}>{subValue}</span>}
            </div>
          </div>
          <div className={`h-2 w-full rounded-full overflow-hidden ${isDark ? 'bg-[#333537]' : 'bg-[#E1E5EA]'}`}>
            <div
              className="h-full rounded-full transition-all duration-1000 ease-out"
              style={{ width: `${percentage > 0 ? percentage : 100}%`, backgroundColor: percentage > 0 ? color : (isDark ? '#444746' : '#C4C7C5') }}
            ></div>
          </div>
        </div>
      );
    };

    const ListRow = ({ label, value, border = true, theme }) => {
      const isDark = theme === 'dark';
      return (
        <div className="flex justify-between items-center px-4 py-3 relative">
          <span className={`text-[14px] ${isDark ? 'text-[#E3E3E3]' : 'text-[#1F1F1F]'}`}>{label}</span>
          <span className={`text-[14px] font-mono ${isDark ? 'text-[#C4C7C5]' : 'text-[#444746]'}`}>{value}</span>
          {border && <div className={`absolute bottom-0 right-4 left-4 h-[1px] ${isDark ? 'bg-white/5' : 'bg-black/5'}`}></div>}
        </div>
      );
    };

    // 卡片流工作流看板（v1）—— AgentAvatar/AgentCard/AgentPipelineView(白浪原 swim-lane 设计) /
    // CardDrawer / InteractionArea(WfUserInputCard+GateApprovalCard) / NewTaskModal / TemplateCard
    // 数据全从 bs.workflow.run 来；动作走 bridge.* 包装（见 tauri-bridge.js）。
    // ==========================================

    // [工作流分离 Stage D] 角色卡定义(id/名/描述/色/头像)和泳道布局不再前端硬编码——
    // 全部住在各工作流 workflow.json 的 ui 块里,由后端 list_workflows(模板页)/
    // get_workflow_state(run.ui,看板)下发。加新工作流前端零改动。
    // 4 主态 chip
    const UI_STATES = {
      PENDING:      { label: '等待中',   emoji: '💤', color: '#9ca3af', accent: '#81c784' },
      EXECUTING:    { label: '执行中',   emoji: '⚡', color: '#ffb74d', accent: '#4fc3f7' },
      GATE_PENDING: { label: '等待确认', emoji: '🔔', color: '#f06292', accent: '#f06292' },
      COMPLETED:    { label: '已完成',   emoji: '✅', color: '#aed581', accent: '#aed581' },
    };
    function toUiState(raw) {
      switch (raw) {
        case 'running': case 'reviewing': case 'briefing': return 'EXECUTING';
        case 'gate_waiting': case 'gate_approval': case 'waiting_human': return 'GATE_PENDING';
        case 'completed': case 'complete': return 'COMPLETED';
        case 'failed': case 'blocked': case 'blocked-upstream': case 'stopped': return null;
        default: return 'PENDING';
      }
    }
    // id→名映射(waitingFor 依赖标签等处用)。由 layoutForRun 每次算布局时累积——
    // 同一 run 内 defs 先算后用;跨工作流 id 不重叠,残留无害。
    const AGENT_NAME_MAP = {};

    // 按 run 的 ui(workflow.json 下发)+ 实际 agents 算泳道布局,对所有工作流通用:
    // - 静态期:run.agents 含全部注册角色 → 每条泳道过滤出在场角色照常画
    // - [B2 E1] 尚书省派单后出现差事节点(id 含 ~,静态部门角色从 run 消失):
    //   被取代的静态泳道在原位置换成按 wave 分层的"第N批"泳道;
    //   差事卡复用所属部(bu)的头像/配色,标题用差事名。
    function layoutForRun(ui, agents) {
      const defsSrc = (ui && ui.agentDefs) || [];
      const byId = {};
      defsSrc.forEach(a => { byId[a.id] = a; });
      const ids = Object.keys(agents || {});
      const taskIds = ids.filter(id => id.indexOf('~') >= 0);
      const defs = [];
      const lanes = [];
      let laneNo = 0;
      let wavesDone = false;
      const pushWaveLanes = () => {
        const waves = {};
        taskIds.forEach(id => {
          const a = agents[id] || {};
          const w = a.wave || 1;
          (waves[w] = waves[w] || []).push(id);
          const bu = a.bu || id.split('~')[0];
          const base = byId[bu] || { color: '#888' };
          defs.push({ id, name: a.name || id, color: base.color, avatar: base.avatar });
        });
        Object.keys(waves).map(Number).sort((x, y) => x - y).forEach(w => {
          lanes.push({ lane: laneNo++, title: '第' + w + '批', agents: waves[w] });
        });
        wavesDone = true;
      };
      ((ui && ui.lanes) || []).forEach(l => {
        // run 还没角色(刚建项目)→ 全静态预览;有角色 → 只画在场的
        const present = ids.length ? (l.agents || []).filter(a => agents[a]) : (l.agents || []);
        if (present.length) {
          present.forEach(a => defs.push(byId[a] || { id: a, name: a, color: '#888' }));
          lanes.push({ lane: laneNo++, title: l.title, agents: present });
        } else if (taskIds.length && !wavesDone) {
          pushWaveLanes();
        }
      });
      if (taskIds.length && !wavesDone) pushWaveLanes();
      defs.forEach(d => { AGENT_NAME_MAP[d.id] = d.name; });
      return { defs, lanes };
    }

    // Agent 头像。
    // 有 avatar（三省六部古风头像：10 张人物像 + 回奏奏折静物）→ 渲染圆形图，但必须保留状态语义：
    //   running/reviewing/briefing → 彩色 + 主题色脉冲描边
    //   pending/skipped（休息态）  → 降饱和 + 压暗 + 灰罩 + 角标 Z（"未激活"）
    //   completed                  → 正常彩色（无脉冲）
    //   failed/blocked 等           → 轻度去色（区别于满血）
    // 无 avatar 的角色 → 走下方默认 SVG 机器人逻辑，向后兼容。
    const AgentAvatar = ({ color, status, size = 92, avatar }) => {
      const isActive = status === 'running' || status === 'reviewing' || status === 'briefing';
      const isSleeping = status === 'pending' || status === 'skipped';
      if (avatar) {
        const isDone = status === 'completed' || status === 'complete';
        const isDimmed = status === 'failed' || status === 'stale' || status === 'stopped'
          || status === 'blocked' || status === 'blocked-upstream';
        // 状态 → 滤镜/描边
        let filter, ring, opacity = 1;
        if (isActive) {
          filter = 'saturate(1.08) contrast(1.03)';
          ring = `0 0 0 2.5px ${color}, 0 0 14px ${color}99`;
        } else if (isSleeping) {
          // [白浪] 未开工别灰成死人样 → 轻微虚化 + 半透明,保留本色(像在打盹,不是死了)
          filter = 'blur(1.1px) saturate(0.9)';
          ring = `0 0 0 2px ${color}33`;
          opacity = 0.5;
        } else if (isDone) {
          filter = 'saturate(1)';
          ring = '0 0 0 2px #137333aa';
        } else if (isDimmed) {
          filter = 'grayscale(0.55) brightness(0.85)';
          ring = '0 0 0 2px #99999955';
        } else { // ready / gate_waiting 等中间态
          filter = 'saturate(0.95) brightness(0.95)';
          ring = `0 0 0 2px ${color}55`;
        }
        return (
          <div className={'relative select-none' + (isActive ? ' animate-pulse' : '')}
               style={{ width: size, height: size }}>
            <img src={avatar} width={size} height={size} alt=""
                 className="select-none rounded-full object-cover"
                 style={{ width: size, height: size, borderRadius: '50%',
                          objectFit: 'cover', filter, boxShadow: ring,
                          opacity, transition: 'filter .3s, box-shadow .3s, opacity .3s' }} />
            {isSleeping && (
              <span className="absolute" style={{
                top: -2, right: 2, fontSize: Math.round(size*0.16),
                fontWeight: 700, color: '#bbb', textShadow: '0 1px 2px #000a',
                fontFamily: 'sans-serif', lineHeight: 1 }}>Z</span>
            )}
          </div>
        );
      }
      const sc = isActive ? color : (isSleeping ? '#555' : '#888');
      const lines = isActive
        ? `<rect x="36" y="22" width="28" height="3" rx="1.5" fill="${color}" opacity="0.7"/>
           <rect x="36" y="29" width="20" height="3" rx="1.5" fill="${color}" opacity="0.5"/>
           <rect x="36" y="36" width="35" height="3" rx="1.5" fill="${color}" opacity="0.3"/>`
        : '';
      const zzz = isSleeping
        ? `<text x="78" y="18" fill="#888" font-size="10" font-family="sans-serif">Z</text>
           <text x="85" y="12" fill="#666" font-size="8" font-family="sans-serif">z</text>`
        : '';
      const eyes = isSleeping
        ? `<path d="M53 80 Q56 78 59 80" stroke="#888" stroke-width="1.5" fill="none"/>
           <path d="M61 80 Q64 78 67 80" stroke="#888" stroke-width="1.5" fill="none"/>`
        : `<circle cx="55" cy="80" r="2" fill="white"/>
           <circle cx="65" cy="80" r="2" fill="white"/>`;
      const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="120" height="120" viewBox="0 0 120 120" fill="none">
        <rect x="25" y="8" width="70" height="50" rx="6" fill="#222" stroke="${sc}" stroke-width="2"/>
        <rect x="30" y="13" width="60" height="38" rx="3" fill="${isActive ? color : '#333'}" opacity="${isActive ? '0.15' : '0.05'}"/>
        ${lines}${zzz}
        <rect x="55" y="58" width="10" height="6" fill="#333"/>
        <rect x="45" y="64" width="30" height="3" rx="1.5" fill="#333"/>
        <circle cx="60" cy="82" r="14" fill="#1a1a1a"/>
        <path d="M48 70 L44 58 L54 66 Z" fill="#1a1a1a"/>
        <path d="M72 70 L76 58 L66 66 Z" fill="#1a1a1a"/>
        <path d="M52 88 L60 96 L68 88" fill="${color}" opacity="0.9"/>
        <ellipse cx="38" cy="82" rx="8" ry="5" fill="#1a1a1a" transform="rotate(-20 38 82)"/>
        <ellipse cx="82" cy="82" rx="8" ry="5" fill="#1a1a1a" transform="rotate(20 82 82)"/>
        ${eyes}
      </svg>`;
      const dataUri = 'data:image/svg+xml,' + encodeURIComponent(svg);
      return <img src={dataUri} width={size} height={size} alt="" className="select-none" />;
    };

    // [per_page] fan-out chip 网格：把一个 per_page 节点展开成 N 个 SubAgent 实时状态格子。
    const FanoutGrid = ({ fanout, isDark }) => {
      if (!fanout || !fanout.pages || !fanout.pages.length) return null;
      const COLORS = {
        running:  { bg: '#1A73E8', fg: '#fff', label: '写' },
        done:     { bg: isDark ? '#1E3A2A' : '#137333', fg: '#fff', label: '✓' },
        retrying: { bg: '#E8710A', fg: '#fff', label: '↻' },
        queued:   { bg: isDark ? '#3C4043' : '#DADCE0', fg: isDark ? '#9AA0A6' : '#5F6368', label: '·' },
      };
      const pages = [...fanout.pages].sort((a, b) => a.page - b.page);
      const doneN = pages.filter(p => p.status === 'done').length;
      const runN = pages.filter(p => p.status === 'running' || p.status === 'retrying').length;
      return (
        <div className="mt-2 w-full">
          <div className={`text-[10px] mb-1 text-center ${isDark ? 'text-[#8E8E8E]' : 'text-[#757575]'}`}>
            {doneN}/{pages.length} 页完成 · {runN} 个 SubAgent 在写
          </div>
          <div className="flex flex-wrap gap-1 justify-center">
            {pages.map(p => {
              const c = COLORS[p.status] || COLORS.queued;
              const pulse = (p.status === 'running' || p.status === 'retrying') ? ' animate-pulse' : '';
              return (
                <div key={p.page}
                  title={'p' + p.page + ' · ' + p.status}
                  className={'flex items-center justify-center rounded text-[9px] font-semibold' + pulse}
                  style={{ width: 22, height: 18, background: c.bg, color: c.fg }}>
                  {p.page}
                </div>
              );
            })}
          </div>
        </div>
      );
    };

    const AgentCard = ({ agent, status, waitingFor, fanout, progress, tokens, theme, onApprove, onRetry, onClick }) => {
      const isDark = theme === 'dark';
      const st = status || 'pending';
      const uiState = toUiState(st);
      const isActive = st === 'running' || st === 'reviewing' || st === 'briefing';
      const isSleeping = st === 'pending' || st === 'skipped';
      const isDone = st === 'completed' || st === 'complete';
      const isFailed = st === 'failed';
      const isWaiting = st === 'gate_waiting' || st === 'gate_approval' || st === 'waiting_human';
      const isBlocked = st === 'blocked' || st === 'blocked-upstream';

      const cardBg = isDark
        ? (isActive ? 'bg-[#1E2530]' : isDone ? 'bg-[#1A2420]' : isFailed ? 'bg-[#2A1A1A]' : isWaiting ? 'bg-[#2A2418]' : 'bg-[#1E1F20]')
        : (isActive ? 'bg-white' : isDone ? 'bg-[#F8FFF8]' : isFailed ? 'bg-[#FFF5F5]' : isWaiting ? 'bg-[#FFFAF0]' : 'bg-[#F8F9FA]');
      const borderColor = isDark
        ? (isActive ? 'border-white/10' : isDone ? 'border-[#81C995]/20' : isFailed ? 'border-[#F28B82]/30' : 'border-white/5')
        : (isActive ? 'border-[#1A73E8]/20' : isDone ? 'border-[#137333]/10' : isFailed ? 'border-[#C5221F]/15' : 'border-black/5');

      const statusLabels = {
        pending: '待机休息', ready: '准备就绪', running: '正在工作…',
        reviewing: '品悟评审中', gate_waiting: '等待你确认', gate_approval: '等待你确认', waiting_human: '等待你确认',
        completed: '已完成', complete: '已完成', failed: '执行失败', stale: '需要重跑', skipped: '已跳过',
        blocked: '已阻塞', 'blocked-upstream': '上游阻塞',
        stopped: '已停止',
      };
      const statusEmoji = {
        pending: '💤', ready: '🟢', running: '⚡', reviewing: '🔍',
        gate_waiting: '🔔', gate_approval: '🔔', waiting_human: '🔔', completed: '✅', complete: '✅',
        failed: '❌', stale: '🔄', skipped: '⏭️', blocked: '🚫', 'blocked-upstream': '🚫',
        stopped: '⏹️',
      };
      const waitingLabel = (waitingFor && waitingFor.length > 0 && (isSleeping || isBlocked))
        ? '等待 ' + waitingFor.map(id => AGENT_NAME_MAP[id] || id).join('、') + ' 交付'
        : null;
      const uiInfo = uiState ? UI_STATES[uiState] : null;

      return (
        <div onClick={onClick ? () => onClick(agent.id) : undefined}
             className={`relative flex flex-col items-center rounded-[20px] border p-4 transition-all duration-300 ${cardBg} ${borderColor} ${isActive ? 'shadow-lg' : 'shadow-sm'} ${onClick ? 'cursor-pointer hover:scale-[1.015] hover:shadow-md' : ''}`}
             style={isActive ? { borderColor: '#1A73E866', boxShadow: '0 4px 24px #1A73E822' } : {}}>
          {uiInfo && (
            <div className="absolute top-2 left-2 text-[10px] px-2 py-0.5 rounded-full font-medium tracking-wide"
                 style={{ background: uiInfo.color + '22', color: uiInfo.color }}>
              {uiInfo.label}
            </div>
          )}
          <div className={`mb-2 transition-transform duration-700 ${isActive ? 'scale-105' : ''}`}>
            <AgentAvatar color={agent.color} status={st} size={92} avatar={agent.avatar} />
          </div>
          <div className={`text-[14px] font-semibold mb-0.5 ${isDark ? 'text-[#E3E3E3]' : 'text-[#1F1F1F]'}`}>{agent.name}</div>
          <div className={`text-[11px] text-center mb-2 leading-relaxed line-clamp-2 ${isDark ? 'text-[#8E8E8E]' : 'text-[#757575]'}`}>{agent.desc}</div>
          {waitingLabel && (
            <div className={`text-[10px] text-center mb-1 px-1 ${isDark ? 'text-[#FCAD70]' : 'text-[#E8710A]'}`}>{waitingLabel}</div>
          )}
          <div className={`text-[11px] font-medium px-2 py-0.5 rounded-full flex items-center gap-1 ${
            isActive ? (isDark ? 'bg-white/10 text-white' : 'bg-black/5 text-[#1F1F1F]')
            : isDone ? (isDark ? 'bg-[#81C995]/10 text-[#81C995]' : 'bg-[#E6F4EA] text-[#137333]')
            : isFailed ? (isDark ? 'bg-[#F28B82]/10 text-[#F28B82]' : 'bg-[#FCE8E6] text-[#C5221F]')
            : isWaiting ? (isDark ? 'bg-[#FCAD70]/10 text-[#FCAD70]' : 'bg-[#FFF3E0] text-[#E8710A]')
            : (isDark ? 'bg-white/5 text-[#5F6368]' : 'bg-black/5 text-[#9AA0A6]')
          }`}>
            <span>{statusEmoji[st] || '💤'}</span>
            <span>{statusLabels[st] || '待机'}</span>
          </div>
          {progress ? (
            <div style={{ fontSize: 11, opacity: 0.75, marginTop: 4, maxWidth: "100%",
                          whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis",
                          textAlign: "center" }}>
              {progress}
            </div>
          ) : null}
          {tokens ? (
            <div style={{ fontSize: 10, opacity: 0.6, marginTop: 2 }}>
              ⬡ {((tokens.input + tokens.output) / 1000).toFixed(1)}k tok · {tokens.calls} 次调用
            </div>
          ) : null}
          <FanoutGrid fanout={fanout} isDark={isDark} />
          {isWaiting && onApprove && (
            <button onClick={(e) => { e.stopPropagation(); onApprove(agent.id); }}
              className="mt-2 text-[11px] px-4 py-1 rounded-full font-medium bg-[#1A73E8] text-white hover:bg-[#1557B0] transition-colors">
              确认通过
            </button>
          )}
          {(isFailed || st === 'stale' || isBlocked) && onRetry && (
            <button onClick={(e) => { e.stopPropagation(); onRetry(agent.id); }}
              className="mt-2 text-[11px] px-4 py-1 rounded-full font-medium bg-[#C5221F] text-white hover:bg-[#A50E0E] transition-colors">
              🔄 重跑
            </button>
          )}
        </div>
      );
    };

    // 自上而下 路由图：层层向下，符合古代权力分布(皇上→太子→三省→六部→回奏)。
    // 卡片居中；卡片之间按【实际路由依赖】用曲线相连——无明确路由的卡片不连线。
    const AgentPipelineView = ({ ui, agents, agentStates, agentDeps, fanout, progress, tokens, theme, onApprove, onRetry, onCardClick }) => {
      const isDark = theme === 'dark';
      // 布局全按 run.ui(workflow.json)+ 实际 agents 算;差事动态分批在 layoutForRun 内处理。
      const { defs, lanes } = layoutForRun(ui, agents);
      const containerRef = useRef(null);
      const cardRefs = useRef({});
      const [edges, setEdges] = useState([]);
      const [dims, setDims] = useState({ w: 0, h: 0 });
      const compactViewport = useCompactViewport();

      // 渲染出的卡片 id → 所在层
      const rendered = {};
      lanes.forEach((lane, li) => lane.agents.forEach(rid => { if (defs.find(a => a.id === rid)) rendered[rid] = li; }));
      // run 是否已有真实依赖数据
      const hasRunDeps = agentDeps && Object.keys(agentDeps).length > 0;
      // 某卡的路由上游:有 run 数据 → 只认真实依赖(含 B2 wave,准确);无 run 数据(静态预览)→
      // 按层兜底=连上一层。返回必须是已渲染卡;空 = 无明确路由 → 不连线。
      function upstreamOf(rid, laneIdx) {
        if (hasRunDeps) {
          const d = agentDeps[rid];
          return Array.isArray(d) ? d.filter(x => rendered[x] !== undefined) : [];
        }
        return laneIdx > 0 ? lanes[laneIdx - 1].agents.filter(x => rendered[x] !== undefined) : [];
      }

      React.useLayoutEffect(() => {
        const cont = containerRef.current;
        if (!cont) return;
        const measure = () => {
          const cr = cont.getBoundingClientRect();
          const out = [];
          lanes.forEach((lane, li) => lane.agents.forEach(rid => {
            const node = cardRefs.current[rid];
            if (!node) return;
            const a = node.getBoundingClientRect();
            upstreamOf(rid, li).forEach(dep => {
              const dn = cardRefs.current[dep];
              if (!dn) return;
              const b = dn.getBoundingClientRect();
              out.push({
                x1: b.left + b.width / 2 - cr.left, y1: b.bottom - cr.top,   // 上游卡底部中心
                x2: a.left + a.width / 2 - cr.left, y2: a.top - cr.top,        // 本卡顶部中心
              });
            });
          }));
          setDims({ w: cont.offsetWidth, h: cont.offsetHeight });
          setEdges(out);
        };
        measure();
        const ro = new ResizeObserver(measure);
        ro.observe(cont);
        window.addEventListener('resize', measure);
        return () => { ro.disconnect(); window.removeEventListener('resize', measure); };
      }, [JSON.stringify((ui && ui.lanes) || null), JSON.stringify(agentStates), JSON.stringify(agentDeps)]);

      const lineColor = isDark ? 'rgba(168,199,250,0.40)' : 'rgba(11,87,208,0.30)';
      // 紧凑视口：画布式连线在手机上放不下也看不清，降级为按层纵向列表
      //（卡片全宽、不画 SVG）；层标题保留，执行顺序仍然自上而下可读。
      if (compactViewport) {
        return (
          <div data-testid="workflow-pipeline-compact" className="flex flex-col gap-5 py-1">
            {lanes.map((lane, i) => (
              <div key={i}>
                <div className={`text-[10px] uppercase tracking-wider mb-1.5 ${isDark ? 'text-[#8E8E8E]' : 'text-[#9AA0A6]'}`}>{lane.title}</div>
                <div className="flex flex-col gap-3">
                  {lane.agents.map(rid => {
                    const agent = defs.find(a => a.id === rid);
                    if (!agent) return null;
                    return (
                      <div key={rid} className="w-full">
                        <AgentCard agent={agent}
                          status={(agentStates || {})[rid]}
                          waitingFor={(agentDeps || {})[rid]}
                          fanout={(fanout || {})[rid]}
                          progress={(progress || {})[rid]}
                          tokens={(tokens || {})[rid]}
                          theme={theme} onApprove={onApprove} onRetry={onRetry} onClick={onCardClick} />
                      </div>
                    );
                  })}
                </div>
              </div>
            ))}
          </div>
        );
      }
      return (
        <div ref={containerRef} className="relative flex flex-col gap-6 items-stretch py-1">
          <svg className="absolute inset-0 pointer-events-none" width={dims.w} height={dims.h} style={{ zIndex: 0, overflow: 'visible' }}>
            {edges.map((e, i) => (
              <path key={i} fill="none" stroke={lineColor} strokeWidth="1.5"
                d={`M ${e.x1} ${e.y1} C ${e.x1} ${(e.y1 + e.y2) / 2}, ${e.x2} ${(e.y1 + e.y2) / 2}, ${e.x2} ${e.y2}`} />
            ))}
          </svg>
          {lanes.map((lane, i) => (
            <div key={i} className="relative" style={{ zIndex: 1 }}>
              <div className={`text-[10px] uppercase tracking-wider text-center mb-1.5 ${isDark ? 'text-[#8E8E8E]' : 'text-[#9AA0A6]'}`}>{lane.title}</div>
              <div className="flex flex-row flex-wrap gap-4 justify-center">
                {lane.agents.map(rid => {
                  const agent = defs.find(a => a.id === rid);
                  if (!agent) return null;
                  return (
                    <div key={rid} ref={el => { cardRefs.current[rid] = el; }} className="w-[176px] shrink-0">
                      <AgentCard agent={agent}
                        status={(agentStates || {})[rid]}
                        waitingFor={(agentDeps || {})[rid]}
                        fanout={(fanout || {})[rid]}
                        progress={(progress || {})[rid]}
                        tokens={(tokens || {})[rid]}
                        theme={theme} onApprove={onApprove} onRetry={onRetry} onClick={onCardClick} />
                    </div>
                  );
                })}
              </div>
            </div>
          ))}
        </div>
      );
    };

    // —— 右侧角色详情抽屉 ——
    // [2026-06-07] 通用文件预览浮层:产出文件/产物点文件名内联看,不再甩浏览器。
    // json 自动 parse+缩进+解 \u 转义;md→markdown、html→iframe、text/json→<pre>;其余给外部打开兜底。
    const FilePreviewModal = ({ path, sessionId, theme, onClose }) => {
      const isDark = theme === 'dark';
      const [pv, setPv] = useState({ loading: true });
      useEffect(() => {
        let alive = true;
        (async () => {
          try {
            const info = await bridge.artifactInfo(path, sessionId);
            if (!alive) return;
            if (!info || !info.exists) { setPv({ missing: true }); return; }
            if (info.kind === 'md' || info.kind === 'html' || info.kind === 'text') {
              let text = await bridge.readArtifactText(path, sessionId);
              if (!alive) return;
              let kind = info.kind;
              if (/\.json$/i.test(path)) {
                try { text = JSON.stringify(JSON.parse(text), null, 2); kind = 'json'; } catch (_) {}
              }
              setPv({ kind, text });
            } else if (info.kind === 'image') {
              try { const dataUrl = await bridge.readArtifactImageB64(path, sessionId); if (alive) setPv({ kind: 'image', dataUrl: dataUrl }); }
              catch (e2) { if (alive) setPv({ kind: 'image', imgErr: String(e2) }); }
            } else {
              const visual = bridge.renderArtifactVisual ? await bridge.renderArtifactVisual(path, sessionId) : null;
              if (!alive) return;
              setPv({ kind: info.kind || 'other', visual });
            }
          } catch (e) { if (alive) setPv({ error: String(e) }); }
        })();
        return () => { alive = false; };
      }, [path, sessionId]);
      const base = (path || '').split('/').pop();
      const dim = isDark ? 'text-[#8E8E8E]' : 'text-[#757575]';
      return (
        <div className="absolute inset-0 z-[60] flex items-center justify-center pointer-events-auto">
          <div className="absolute inset-0 bg-black/50" onClick={onClose}></div>
          <div className={`relative w-[860px] max-w-[92vw] h-[82vh] flex flex-col rounded-[16px] shadow-2xl overflow-hidden ${isDark ? 'bg-[#1E1F20]' : 'bg-white'}`}>
            <div className={`flex items-center justify-between px-4 py-3 border-b ${isDark ? 'border-white/10' : 'border-black/10'}`}>
              <span className={`text-[14px] font-medium truncate ${isDark ? 'text-[#E3E3E3]' : 'text-[#1F1F1F]'}`} title={path}>{base}</span>
              <div className="flex items-center gap-2">
                {(!isWeb || can('artifactDownload')) && <button onClick={() => bridge.openArtifactExternal && bridge.openArtifactExternal(path, sessionId)} className={`px-2 py-1 text-[12px] rounded ${isDark ? 'text-[#C4C7C5] hover:bg-[#333537]' : 'text-[#444746] hover:bg-[#F0F4F9]'}`}>{isWeb ? '↓ 下载' : '↗ 外部'}</button>}
                <button onClick={onClose} className={`w-7 h-7 rounded-full flex items-center justify-center ${isDark ? 'hover:bg-[#333537] text-[#C4C7C5]' : 'hover:bg-[#F0F4F9] text-[#444746]'}`}>✕</button>
              </div>
            </div>
            <div className="flex-1 overflow-y-auto custom-scrollbar p-4 min-w-0">
              {pv.loading ? <div className={`text-[13px] ${dim}`}>加载中…</div>
                : pv.missing ? <div className={`text-[13px] ${dim}`}>文件不存在或已被删除</div>
                : pv.error ? <div className="text-[13px] text-[#F28B82]">读取失败: {pv.error}</div>
                : pv.kind === 'md' ? <div className={`msg-md text-[14px] leading-relaxed ${isDark ? 'dark-code text-[#E3E3E3]' : 'light-code text-[#1F1F1F]'}`} dangerouslySetInnerHTML={{ __html: bridge.renderMarkdown(pv.text || '') }} />
                : pv.kind === 'html' ? <ScaledHtmlPreview html={pv.text || ''} />
                : (pv.kind === 'json' || pv.kind === 'text') ? <pre className={`text-[12px] whitespace-pre-wrap break-words font-mono leading-relaxed ${isDark ? 'text-[#C4C7C5]' : 'text-[#444746]'}`}>{pv.text}</pre>
                : pv.kind === 'image' ? (pv.imgErr ? <div className="text-[13px] text-[#F28B82]">图片读取失败: {pv.imgErr}</div> : <img className="max-w-full max-h-[70vh] object-contain mx-auto rounded-lg" src={pv.dataUrl} alt={base} />)
                : pv.visual && pv.visual.mode === 'html' ? <iframe sandbox="allow-same-origin" className="w-full min-h-[68vh] border-0 block bg-[#15171a]" style={{ colorScheme: 'dark' }} srcDoc={(pv.visual.html || '') + OFFICE_HTML_STYLE} />
                : pv.visual && pv.visual.mode === 'images' ? <div className="flex flex-col items-center gap-3">{(pv.visual.images || []).map((src, i) => <img key={i} src={src} className="max-w-full h-auto rounded-lg shadow-sm" alt={`page-${i + 1}`} />)}</div>
                : <div><p className={`text-[13px] mb-2 ${isDark ? 'text-[#C4C7C5]' : 'text-[#444746]'}`}>此类型暂不支持预览</p>{(!isWeb || can('artifactDownload')) && <button onClick={() => bridge.openArtifactExternal(path, sessionId)} className={`px-3 py-1.5 rounded-full text-[13px] ${isDark ? 'bg-[#A8C7FA] text-[#062E6F]' : 'bg-[#0B57D0] text-white'}`}>{isWeb ? '↓ 下载产物' : '↗ 外部打开'}</button>}</div>}
            </div>
          </div>
        </div>
      );
    };

    // 🏛️ 最终奏折：三省六部办差完毕、太子准奏后展开的结案呈报(回奏 final_report.md)。代入感拉满。
    const ImperialMemorialModal = ({ projectDir, sessionId, theme, onClose }) => {
      const [st, setSt] = useState({ loading: true, text: '', error: null });
      const [closing, setClosing] = useState(false);
      // 御赐宝箱:两层——products=真成品(题目命名的最终报告+二进制成品),装箱;
      // papers=六部过程文书,折叠降级展示(审计找得到,客户视角不被淹)。
      const [deliv, setDeliv] = useState({ products: [], papers: [] });
      const [chestOpen, setChestOpen] = useState(false);
      useEffect(() => {
        let alive = true;
        const dir = (projectDir || '').replace(/\/$/, '');
        if (dir && bridge.listDeliverables) {
          bridge.listDeliverables(dir, sessionId).then((r) => {
            if (alive) setDeliv({ products: (r && r.products) || [], papers: (r && r.papers) || [] });
          });
        }
        return () => { alive = false; };
      }, [projectDir, sessionId]);
      const closingRef = useRef(false);
      // 收卷:先放反向卷轴动画,卷合后再真正卸载
      const requestClose = () => {
        if (closingRef.current) return;
        closingRef.current = true;
        setClosing(true);
        setTimeout(onClose, 580);
      };
      useEffect(() => {
        let alive = true;
        (async () => {
          try {
            const path = (projectDir || '').replace(/\/$/, '') + '/final_report.md';
            const text = await bridge.readArtifactText(path, sessionId);
            if (alive) setSt({ loading: false, text: text || '', error: null });
          } catch (e) { if (alive) setSt({ loading: false, text: '', error: String(e) }); }
        })();
        return () => { alive = false; };
      }, [projectDir, sessionId]);
      const ease = closing ? '.55s cubic-bezier(.5,0,.75,.4) both' : '.65s cubic-bezier(.22,.8,.3,1) both';
      // 两根轴杆:横向渐变带深色轴头,比纸面宽出一截,从中线滚向上下边缘
      const roller = (top) => ({
        position: 'absolute', left: '-14px', right: '-14px', height: '18px', borderRadius: '9px', zIndex: 2,
        background: 'linear-gradient(90deg,#2e1f0e 0%,#6b4a23 6%,#4a3217 50%,#6b4a23 94%,#2e1f0e 100%)',
        boxShadow: '0 2px 8px rgba(0,0,0,.5), inset 0 1px 0 rgba(255,235,190,.25)',
        animation: `memorial-roller-${top ? 'top' : 'bottom'}-${closing ? 'close' : 'open'} ${ease}`,
      });
      return (
        <div className="fixed inset-0 z-[70] flex items-center justify-center">
          <div className="absolute inset-0 bg-black/70" onClick={requestClose}
            style={{ animation: 'memorial-fade .35s ease both', opacity: closing ? 0 : undefined, transition: 'opacity .5s ease' }}></div>
          <div className="relative w-[640px] max-w-[94vw] max-h-[88vh] flex">
            <div className="relative flex-1 min-w-0 min-h-0 flex flex-col rounded-[8px] overflow-hidden"
              style={{ background: 'linear-gradient(180deg,#f7eed8 0%,#f0e1bf 100%)', color: '#3a2a18', border: '1px solid #b8893f', boxShadow: '0 24px 70px rgba(0,0,0,.55)',
                animation: `memorial-paper-${closing ? 'close' : 'open'} ${ease}` }}>
              <div className="relative px-8 pt-6 pb-3 text-center shrink-0">
                <div style={{ color: '#8a1c1c', letterSpacing: '8px' }} className="text-[22px] font-semibold">奏　折</div>
                <div style={{ color: '#7a5a2a' }} className="text-[12px] mt-1">三省六部办差完毕 · 回奏呈报</div>
                <div style={{ position: 'absolute', top: '12px', right: '22px', color: '#b21e1e', border: '3px double #b21e1e', borderRadius: '6px', padding: '5px 9px', fontWeight: 700, fontSize: '19px', letterSpacing: '3px', fontFamily: 'serif',
                  animation: closing ? 'none' : 'memorial-stamp .5s cubic-bezier(.3,1.4,.5,1) .55s both', transform: 'rotate(-14deg)', opacity: .82 }}>准奏</div>
              </div>
              <div style={{ borderTop: '1px solid #c9a96a' }}></div>
              <div className="flex-1 overflow-y-auto custom-scrollbar px-8 py-5 min-w-0">
                {st.loading ? <div style={{ color: '#7a5a2a' }} className="text-[13px] text-center py-10">展卷中…</div>
                  : st.error ? <div className="text-[13px] text-center py-10" style={{ color: '#8a1c1c' }}>奏折读取失败：{st.error}</div>
                  : st.text ? <div className="msg-md light-code text-[14px] leading-[1.9]" style={{ color: '#3a2a18' }} dangerouslySetInnerHTML={{ __html: bridge.renderMarkdown(st.text) }} />
                  : <div style={{ color: '#7a5a2a' }} className="text-[13px] text-center py-10">尚无回奏内容</div>}
              </div>
              {deliv.products.length > 0 && (
                <div className="shrink-0 px-8 pt-2 pb-1 text-center" style={{ borderTop: '1px dashed #c9a96a' }}>
                  {chestOpen && (
                    <div className="flex flex-wrap justify-center gap-3 mb-2">
                      {deliv.products.concat(deliv.papers).map((f, i) => {
                        const isProduct = i < deliv.products.length;
                        const ext = (String(f.name).split('.').pop() || '').toLowerCase();
                        const chip = (ext === 'pptx' || ext === 'ppt') ? 'PPT' : ext === 'xlsx' ? '表格' : ext === 'pdf' ? 'PDF'
                          : (ext === 'html' || ext === 'htm') ? '网页' : (ext === 'png' || ext === 'jpg' || ext === 'jpeg') ? '图' : '文书';
                        const sz = f.size > 1048576 ? (f.size / 1048576).toFixed(1) + ' MB' : Math.max(1, Math.round(f.size / 1024)) + ' KB';
                        if (isWeb && !can('artifactDownload')) return null;
                        return (
                          <button key={f.path} onClick={() => bridge.openArtifactExternal(f.path, sessionId)} title={(isWeb ? '下载:' : '打开:') + (f.title || f.name)}
                            style={{ background: 'none', border: 'none', padding: 0, cursor: 'pointer',
                              animation: `chest-item-pop .45s cubic-bezier(.3,1.4,.5,1) ${i * 0.09}s both` }}>
                            {/* 展开的小卷轴(斜 45° 视角):两端轴杆 + 中间纸面写标题 */}
                            <div style={{ transform: 'perspective(520px) rotateY(-14deg) rotateX(7deg)', transformStyle: 'preserve-3d' }}>
                              <div className="relative flex items-stretch" style={{ width: '168px', height: '92px', filter: 'drop-shadow(3px 5px 6px rgba(58,42,24,.35))' }}>
                                <div style={{ width: '10px', margin: '-5px 0', borderRadius: '5px', background: 'linear-gradient(90deg,#2e1f0e,#6b4a23 55%,#3a2812)' }}></div>
                                <div className="flex-1 flex flex-col items-center justify-center px-2" style={{
                                  background: 'linear-gradient(90deg,#e8d9b4 0%,#f7eed8 12%,#f7eed8 88%,#e8d9b4 100%)',
                                  borderTop: '1px solid #c9a96a', borderBottom: '1px solid #c9a96a', position: 'relative' }}>
                                  {isProduct && <div style={{ position: 'absolute', top: '3px', right: '4px', color: '#b21e1e', border: '1.5px solid #b21e1e',
                                    borderRadius: '3px', padding: '0 3px', fontSize: '9px', fontFamily: 'serif', opacity: .85, transform: 'rotate(-8deg)' }}>成品</div>}
                                  <div className="text-[12px] leading-[1.5] font-medium" style={{ color: '#3a2a18',
                                    display: '-webkit-box', WebkitLineClamp: 3, WebkitBoxOrient: 'vertical', overflow: 'hidden',
                                    maxHeight: '54px', wordBreak: 'break-all' }}>{f.title || f.name}</div>
                                  <div className="text-[10px] mt-1" style={{ color: '#9a7b45' }}>{chip} · {sz}</div>
                                </div>
                                <div style={{ width: '10px', margin: '-5px 0', borderRadius: '5px', background: 'linear-gradient(90deg,#3a2812,#6b4a23 45%,#2e1f0e)' }}></div>
                              </div>
                            </div>
                          </button>
                        );
                      })}
                    </div>
                  )}
                  {/* 宝箱(白浪选的钥匙孔款):点击开箱,金光迸出、成品弹出 */}
                  <button onClick={() => setChestOpen((o) => !o)} title={chestOpen ? '收箱' : '点击开箱'}
                    className="relative inline-block" style={{ background: 'none', border: 'none', cursor: 'pointer', padding: 0 }}>
                    {chestOpen && <div style={{ position: 'absolute', left: '50%', top: '-14px', width: '170px', height: '64px',
                      background: 'radial-gradient(ellipse at 50% 100%, rgba(255,200,60,.7) 0%, rgba(255,200,60,0) 70%)',
                      animation: 'chest-glow .5s ease both', pointerEvents: 'none', zIndex: 0 }}></div>}
                    <img src="avatars/chengpin_chest.png" alt="成品箱"
                      style={{ width: '118px', display: 'block', borderRadius: '8px', position: 'relative', zIndex: 1,
                        boxShadow: '0 4px 12px rgba(58,42,24,.3)',
                        animation: chestOpen ? 'chest-open-bounce .5s cubic-bezier(.34,1.45,.6,1) both' : 'none' }} />
                    <div className="text-[10px] mt-0.5" style={{ color: '#7a5a2a' }}>
                      {chestOpen ? '▲ 收箱' : `▼ 点击开箱 · 文书 ${deliv.products.length + deliv.papers.length} 件`}
                    </div>
                  </button>
                </div>
              )}
              <div className="shrink-0 text-center py-3" style={{ borderTop: '1px solid #c9a96a' }}>
                <div className="text-[15px]" style={{ color: '#8a1c1c', letterSpacing: '6px' }}>钦　此</div>
                <button onClick={requestClose} className="mt-2 px-5 py-1.5 rounded-full text-[13px]" style={{ background: '#8a1c1c', color: '#f7eed6' }}>朕已阅 · 收卷</button>
              </div>
            </div>
            <div style={roller(true)}></div>
            <div style={roller(false)}></div>
          </div>
        </div>
      );
    };

    // [2026-06-07 #18/#20] 生图引擎面板：客户选 provider + 填自己的 key（不用白浪的）。
    // (ImageProviderPanel 已随 h3c-ppt 工作流 2026-06-11 存档下线:仅 illustrator 角色用)

    const CardDrawer = ({ roleId, projectDir, sessionId, theme, onClose }) => {
      const isDark = theme === 'dark';
      const [info, setInfo] = useState({ loading: false, error: null, data: null });
      const [outputs, setOutputs] = useState({ loading: false, error: null, data: null });
      const [gate, setGate] = useState({ loading: false, error: null, data: null });
      const [logs, setLogs] = useState({ loading: false, error: null, data: null });
      const [previewPath, setPreviewPath] = useState(null);  // [2026-06-07] 产出文件内联预览
      useEffect(() => {
        if (!roleId || !bridge.available) return;
        let alive = true;
        const run = (fn, setter) => {
          setter({ loading: true, error: null, data: null });
          Promise.resolve().then(fn)
            .then((d) => { if (alive) setter({ loading: false, error: null, data: d }); })
            .catch((e) => { if (alive) setter({ loading: false, error: String(e), data: null }); });
        };
        run(() => bridge.getRolePrompt(roleId, projectDir), setInfo);
        run(() => bridge.getRoleOutputs(roleId), setOutputs);
        run(() => bridge.getGateReport(roleId), setGate);
        run(() => bridge.getRoleLogs(roleId, 60), setLogs);
        return () => { alive = false; };
      }, [roleId, projectDir]);
      if (!roleId) return null;
      const normOutputs = (raw) => {
        let arr = raw;
        if (raw && !Array.isArray(raw) && Array.isArray(raw.files)) arr = raw.files;
        if (!Array.isArray(arr)) return [];
        return arr.map((it) => {
          if (typeof it === 'string') { const base = it.split('/').pop() || it; return { path: it, basename: base }; }
          const path = it.path || it.file || it.fullpath || '';
          const base = it.basename || it.name || (path ? path.split('/').pop() : '') || String(it);
          return { path, basename: base };
        });
      };
      const fileExt = (name) => (String(name).split('.').pop() || '').toLowerCase();
      const fileIcon = (name) => {
        const e = fileExt(name);
        if (['md', 'markdown', 'txt'].includes(e)) return '📄';
        if (['html', 'htm'].includes(e)) return '🌐';
        if (['png', 'jpg', 'jpeg', 'gif', 'webp', 'svg'].includes(e)) return '🖼️';
        if (['json', 'yaml', 'yml'].includes(e)) return '🔢';
        return '📎';
      };
      const logText = (raw) => {
        if (raw == null) return '';
        if (typeof raw === 'string') return raw;
        if (Array.isArray(raw)) return raw.map((l) => (typeof l === 'string' ? l : JSON.stringify(l))).join('\n');
        if (raw.lines && Array.isArray(raw.lines)) return raw.lines.join('\n');
        if (raw.text) return String(raw.text);
        return JSON.stringify(raw, null, 2);
      };
      const verdictStyle = (v) => {
        const s = String(v || '').toLowerCase();
        if (['pass', 'passed', 'approve', 'approved', 'ok'].includes(s)) return isDark ? 'bg-[#1E3A2A] text-[#93D5A6]' : 'bg-[#E6F4EA] text-[#137333]';
        if (['fail', 'failed', 'reject', 'rejected', 'block', 'blocked'].includes(s)) return isDark ? 'bg-[#3A1E1E] text-[#F28B82]' : 'bg-[#FCE8E6] text-[#C5221F]';
        return isDark ? 'bg-[#333537] text-[#C4C7C5]' : 'bg-[#F0F4F9] text-[#444746]';
      };
      const fmtTs = (ts) => {
        if (!ts) return '';
        const d = new Date(typeof ts === 'number' && ts < 1e12 ? ts * 1000 : ts);
        return isNaN(d.getTime()) ? String(ts) : d.toLocaleString();
      };
      const titleCls = isDark ? 'text-[#E3E3E3]' : 'text-[#1F1F1F]';
      const bodyCls = isDark ? 'text-[#C4C7C5]' : 'text-[#444746]';
      const dimCls = isDark ? 'text-[#8E8E8E]' : 'text-[#757575]';
      const secHeadCls = `text-[12px] font-semibold mb-2 ${isDark ? 'text-[#A8C7FA]' : 'text-[#0B57D0]'}`;
      const StateLine = ({ st, empty }) => {
        if (st.loading) return <div className={`text-[13px] ${dimCls}`}>加载中…</div>;
        if (st.error) return <div className={`text-[13px] ${isDark ? 'text-[#F28B82]' : 'text-[#C5221F]'}`}>⚠️ {st.error}</div>;
        if (empty) return <div className={`text-[13px] ${dimCls}`}>{empty}</div>;
        return null;
      };
      // [2026-06-07] 产出文件按文件名自然排序(p01<p02<…<p10<…<p30,不再按完成/glob 乱序)
      const files = outputs.data ? normOutputs(outputs.data).slice().sort(function (a, b) {
        return (a.basename || '').localeCompare(b.basename || '', undefined, { numeric: true, sensitivity: 'base' });
      }) : [];
      const gd = gate.data || {};
      const findings = Array.isArray(gd.findings) ? gd.findings : [];
      const tail = logText(logs.data);
      const meta = (info.data && info.data.registry_meta) || {};
      const promptMd = (info.data && info.data.prompt_md) || '';
      const inputSection = (() => { const m = promptMd.match(/##\s*你的输入[\s\S]*?(?=\n##\s|$)/); return m ? m[0].replace(/##\s*你的输入\s*/, '').trim() : ''; })();
      return (
        <div className="fixed inset-0 z-50 flex justify-end">
          <div className="absolute inset-0 bg-black/60" onClick={onClose}></div>
          <div className={`relative w-[420px] max-w-[92vw] h-full flex flex-col shadow-2xl ${isDark ? 'bg-[#1E1F20]' : 'bg-white'} animate-in slide-in-from-right duration-200`}>
            <div className={`flex items-center justify-between px-4 py-3 border-b shrink-0 ${isDark ? 'border-white/10' : 'border-black/10'}`}>
              <div className="min-w-0">
                <div className={`text-[15px] font-medium truncate ${titleCls}`}>⚙️ {roleId}</div>
                {projectDir && <div className={`text-[11px] font-mono truncate ${dimCls}`} title={projectDir}>{projectDir}</div>}
              </div>
              <button onClick={onClose} className={`w-8 h-8 rounded-full flex items-center justify-center shrink-0 ml-2 ${isDark ? 'hover:bg-[#333537] text-[#C4C7C5]' : 'hover:bg-[#F0F4F9] text-[#444746]'}`}>✕</button>
            </div>
            <div className="flex-1 overflow-y-auto custom-scrollbar p-4 space-y-5">
              <section>
                <div className={secHeadCls}>🛠 角色说明</div>
                <StateLine st={info} empty={!info.loading && !info.error && !info.data ? '暂无角色信息' : null} />
                {info.data && (
                  <div className="space-y-3 text-[13px]">
                    <div className={bodyCls}>
                      <span className={dimCls}>名称：</span>{meta.name || roleId}
                      <span className={dimCls}> · max_steps {meta.max_steps != null ? meta.max_steps : '—'} · gate {meta.gate || '—'}{meta.timeout_secs ? ' · timeout ' + meta.timeout_secs + 's' : ''}</span>
                    </div>
                    <div>
                      <div className={`${dimCls} mb-1`}>🔧 工具</div>
                      <div className="flex flex-wrap gap-1">
                        {(meta.tools || []).length ? (meta.tools).map((t, i) => (
                          <span key={i} className={`text-[11px] px-1.5 py-0.5 rounded font-mono ${isDark ? 'bg-[#333537] text-[#C4C7C5]' : 'bg-[#F0F4F9] text-[#444746]'}`}>{t}</span>
                        )) : <span className={dimCls}>—</span>}
                      </div>
                    </div>
                    {inputSection && (
                      <div>
                        <div className={`${dimCls} mb-1`}>⬇ 输入要求</div>
                        <div className={`${bodyCls} whitespace-pre-wrap text-[12px] leading-relaxed`}>{inputSection}</div>
                      </div>
                    )}
                    <div>
                      <div className={`${dimCls} mb-1`}>⬆ 输出</div>
                      <div className={bodyCls}>{(meta.outputs || []).join('、') || '—'}</div>
                      {meta.output_schema && (
                        <ul className="mt-1 space-y-0.5">
                          {Object.keys(meta.output_schema).map((k, i) => (
                            <li key={i} className={`text-[12px] ${bodyCls}`}>
                              <code className={`text-[11px] ${isDark ? 'text-[#A8C7FA]' : 'text-[#0B57D0]'}`}>{k}</code>
                              <span className={dimCls}> — {typeof meta.output_schema[k] === 'string' ? meta.output_schema[k] : JSON.stringify(meta.output_schema[k])}</span>
                            </li>
                          ))}
                        </ul>
                      )}
                    </div>
                    {promptMd && (
                      <details>
                        <summary className={`cursor-pointer text-[12px] ${isDark ? 'text-[#A8C7FA]' : 'text-[#0B57D0]'}`}>📜 查看完整提示词</summary>
                        <pre className={`mt-2 text-[11px] whitespace-pre-wrap break-words font-mono max-h-[300px] overflow-y-auto custom-scrollbar rounded-[12px] p-3 border ${isDark ? 'border-white/10 bg-[#131314] text-[#C4C7C5]' : 'border-black/10 bg-[#F8FAFC] text-[#444746]'}`}>{promptMd}</pre>
                      </details>
                    )}
                  </div>
                )}
              </section>
              <section>
                <div className={secHeadCls}>📄 产出文件</div>
                <StateLine st={outputs} empty={!outputs.loading && !outputs.error && files.length === 0 ? '暂无产出' : null} />
                {files.length > 0 && (
                  <div className="space-y-1">
                    {files.map((f, i) => (
                      <div key={i} title={f.path || f.basename} className={`flex items-center gap-2 px-2.5 py-2 rounded-[12px] border ${isDark ? 'border-white/10 bg-[#131314]' : 'border-black/10 bg-[#F8FAFC]'}`}>
                        <span className="shrink-0">{fileIcon(f.basename)}</span>
                        <span onClick={() => f.path && setPreviewPath(f.path)} title="点击预览" className={`flex-1 truncate text-[13px] cursor-pointer hover:underline ${titleCls}`}>{f.basename || '(未命名)'}</span>
                        {f.path && (!isWeb || can('artifactDownload')) && (
                          <button title={isWeb ? '下载产物' : '外部打开'} onClick={() => bridge.available && bridge.openArtifactExternal && bridge.openArtifactExternal(f.path, sessionId)} className={`shrink-0 text-[13px] ${dimCls} hover:opacity-80`}>{isWeb ? '↓' : '↗'}</button>
                        )}
                      </div>
                    ))}
                  </div>
                )}
              </section>
              <section>
                <div className={secHeadCls}>📋 评审结果</div>
                <StateLine st={gate} empty={!gate.loading && !gate.error && !gd.verdict && findings.length === 0 ? '尚无评审记录' : null} />
                {!gate.loading && !gate.error && (gd.verdict || findings.length > 0) && (
                  <div className="space-y-2">
                    <div className="flex items-center gap-2 flex-wrap">
                      {gd.verdict && <span className={`text-[12px] font-medium px-2 py-0.5 rounded-full ${verdictStyle(gd.verdict)}`}>{gd.verdict}</span>}
                      {gd.ts && <span className={`text-[11px] ${dimCls}`}>⏱ {fmtTs(gd.ts)}</span>}
                    </div>
                    {gd.summary && <div className={`text-[13px] ${bodyCls}`}>{gd.summary}</div>}
                    {findings.length > 0 && (
                      <ul className="space-y-1">
                        {findings.map((f, i) => {
                          const txt = typeof f === 'string' ? f : (f.message || f.text || f.detail || JSON.stringify(f));
                          return (<li key={i} className={`flex gap-2 text-[13px] ${bodyCls}`}><span className={`shrink-0 ${dimCls}`}>›</span><span className="min-w-0">{txt}</span></li>);
                        })}
                      </ul>
                    )}
                  </div>
                )}
              </section>
              <section>
                <div className={secHeadCls}>📋 运行日志(尾 60 行)</div>
                <StateLine st={logs} empty={!logs.loading && !logs.error && !tail ? '暂无日志' : null} />
                {!logs.loading && !logs.error && tail && (
                  <pre className={`text-[11px] leading-relaxed whitespace-pre-wrap break-words font-mono max-h-[320px] overflow-y-auto custom-scrollbar rounded-[12px] p-3 border ${isDark ? 'border-white/10 bg-[#131314] text-[#C4C7C5]' : 'border-black/10 bg-[#F8FAFC] text-[#444746]'}`}>{tail}</pre>
                )}
              </section>
            </div>
          </div>
          {previewPath && <FilePreviewModal path={previewPath} sessionId={sessionId} theme={theme} onClose={() => setPreviewPath(null)} />}
        </div>
      );
    };

    // —— 底部交互区：问答卡 / gate 卡 / 系统卡 ——
    const WfUserInputCard = ({ card, theme }) => {
      const isDark = theme === 'dark';
      const questions = card.questions || [];
      const [answers, setAnswers] = useState(() => questions.map(() => null));
      const [matState, setMatState] = useState({ busy: false, names: [] }); // [2026-06-06] 素材上传反馈
      const [otherOpen, setOtherOpen] = useState(() => questions.map(() => false));
      const [otherText, setOtherText] = useState(() => questions.map(() => ''));
      const locked = !!card.resolved;
      function pick(qi, opt) {
        if (locked) return;
        const next = answers.slice(); next[qi] = { id: questions[qi].id, label: opt.label, value: opt.label }; setAnswers(next);
        const oo = otherOpen.slice(); oo[qi] = false; setOtherOpen(oo);
      }
      function toggleOther(qi) {
        if (locked) return;
        const oo = otherOpen.slice(); oo[qi] = !oo[qi]; setOtherOpen(oo);
        if (oo[qi]) { const next = answers.slice(); next[qi] = null; setAnswers(next); }
      }
      function setOther(qi, val) {
        const ot = otherText.slice(); ot[qi] = val; setOtherText(ot);
        const next = answers.slice(); next[qi] = val.trim() ? { id: questions[qi].id, label: '其他', value: val.trim() } : null; setAnswers(next);
      }
      function submit() { if (locked) return; if (!answers.every(a => a != null)) return; bridge.submitWorkflowUserInput(card.cardId, card.toolCallId, answers); }
      const canSubmit = answers.length > 0 && answers.every(a => a != null);
      if (locked) {
        const cancelled = card.cardState === 'cancelled';
        return (
          <div className={`rounded-[16px] border p-4 ${isDark ? 'bg-[#1E1F20] border-white/10' : 'bg-white border-black/10'}`}>
            <div className={`text-[14px] font-semibold mb-1 ${isDark ? 'text-[#E3E3E3]' : 'text-[#1F1F1F]'}`}>🤔 AI 想问你几个问题</div>
            <div className={`text-[13px] ${cancelled ? (isDark ? 'text-[#8E8E8E]' : 'text-[#757575]') : (isDark ? 'text-[#93D5A6]' : 'text-[#137333]')}`}>{cancelled ? '✕ 已取消' : '✓ 已提交'}</div>
          </div>
        );
      }
      return (
        <div className={`rounded-[16px] border p-4 ${isDark ? 'bg-[#1E1F20] border-[#A8C7FA]/30' : 'bg-white border-[#0B57D0]/20'}`}>
          <div className={`text-[14px] font-semibold mb-3 ${isDark ? 'text-[#E3E3E3]' : 'text-[#1F1F1F]'}`}>🤔 AI 想问你几个问题</div>
          {/* [2026-06-06] 素材上传：选文件 → 拷进当前 run 配套材料/ → 再选「已补充」让审计员重扫 */}
          <div className="mb-3 flex items-center flex-wrap gap-2">
            {(!isWeb || can('hostFilePicker')) && <button disabled={matState.busy}
              onClick={async () => {
                setMatState({ busy: true, names: matState.names });
                try { const added = await bridge.pickAndAddMaterials(); setMatState({ busy: false, names: matState.names.concat(added || []) }); }
                catch (e) { setMatState({ busy: false, names: matState.names }); }
              }}
              className={`px-3 py-1.5 rounded-[10px] text-[13px] border transition-colors disabled:opacity-50 ${isDark ? 'border-[#A8C7FA]/40 text-[#A8C7FA] hover:bg-[#A8C7FA]/10' : 'border-[#0B57D0]/30 text-[#0B57D0] hover:bg-[#0B57D0]/5'}`}>
              {matState.busy ? '上传中…' : '📎 上传素材文件'}
            </button>}
            {matState.names.length > 0 && <span className={`text-[12px] ${isDark ? 'text-[#93D5A6]' : 'text-[#137333]'}`}>已上传 {matState.names.length} 个：{matState.names.join('、')}</span>}
          </div>
          <div className="space-y-4">
            {questions.map((q, qi) => (
              <div key={q.id || qi}>
                <div className={`text-[12px] font-semibold ${isDark ? 'text-[#A8C7FA]' : 'text-[#0B57D0]'}`}>{q.header || ('Q' + (qi + 1))}</div>
                <div className={`text-[13px] mb-2 ${isDark ? 'text-[#E3E3E3]' : 'text-[#1F1F1F]'}`}>{q.question || ''}</div>
                <div className="flex flex-col gap-1.5">
                  {(q.options || []).map((opt, oi) => {
                    const sel = answers[qi] && answers[qi].label === opt.label && answers[qi].value === opt.label;
                    return (
                      <button key={oi} onClick={() => pick(qi, opt)}
                        className={`text-left px-3 py-2 rounded-[12px] border transition-colors ${sel ? (isDark ? 'border-[#A8C7FA] bg-[#A8C7FA]/10' : 'border-[#0B57D0] bg-[#0B57D0]/5') : (isDark ? 'border-white/10 hover:bg-[#282A2C]' : 'border-black/10 hover:bg-[#E8EDF2]')}`}>
                        <div className={`text-[13px] font-medium ${isDark ? 'text-[#E3E3E3]' : 'text-[#1F1F1F]'}`}>{opt.label}</div>
                        {opt.description && <div className={`text-[12px] ${isDark ? 'text-[#8E8E8E]' : 'text-[#757575]'}`}>{opt.description}</div>}
                      </button>
                    );
                  })}
                  <button onClick={() => toggleOther(qi)}
                    className={`text-left px-3 py-2 rounded-[12px] border transition-colors ${answers[qi] && answers[qi].label === '其他' ? (isDark ? 'border-[#A8C7FA] bg-[#A8C7FA]/10' : 'border-[#0B57D0] bg-[#0B57D0]/5') : (isDark ? 'border-white/10 hover:bg-[#282A2C]' : 'border-black/10 hover:bg-[#E8EDF2]')}`}>
                    <div className={`text-[13px] font-medium ${isDark ? 'text-[#E3E3E3]' : 'text-[#1F1F1F]'}`}>💬 其他(自己写)</div>
                  </button>
                  {otherOpen[qi] && (
                    <textarea rows="2" value={otherText[qi]} onChange={e => setOther(qi, e.target.value)} placeholder="写下你想说的..."
                      className={`w-full rounded-[10px] p-2 text-[13px] outline-none border ${isDark ? 'bg-[#131314] border-white/10 text-[#E3E3E3]' : 'bg-white border-black/10 text-[#1F1F1F]'}`} />
                  )}
                </div>
              </div>
            ))}
          </div>
          <div className="flex justify-end mt-3">
            <button disabled={!canSubmit} onClick={submit} className={`${cardBtnCls(isDark, 'primary')} ${canSubmit ? '' : 'opacity-50 cursor-not-allowed'}`}>提交</button>
          </div>
        </div>
      );
    };

    const GateApprovalCard = ({ card, theme }) => {
      const isDark = theme === 'dark';
      const findings = card.findings || [];
      const locked = !!card.resolved;
      const [rejecting, setRejecting] = useState(false);
      const [reason, setReason] = useState('');
      function fText(f) {
        if (f == null) return '';
        if (typeof f === 'string') return f;
        if (typeof f === 'object') return f.message || f.text || f.detail || f.title || JSON.stringify(f);
        return String(f);
      }
      if (locked) {
        const approved = card.cardState === 'approved';
        return (
          <div className={`rounded-[16px] border p-4 ${isDark ? 'bg-[#1E1F20] border-white/10' : 'bg-white border-black/10'}`}>
            <div className={`text-[14px] font-semibold mb-1 ${isDark ? 'text-[#E3E3E3]' : 'text-[#1F1F1F]'}`}>🟠 {card.roleName || card.roleId} · 产出待确认</div>
            <div className={`text-[13px] ${approved ? (isDark ? 'text-[#93D5A6]' : 'text-[#137333]') : (isDark ? 'text-[#F28B82]' : 'text-[#C5221F]')}`}>{approved ? '✓ 已通过' : '✕ 已打回'}</div>
          </div>
        );
      }
      return (
        <div className={`rounded-[16px] border p-4 ${isDark ? 'bg-[#1E1F20] border-[#F9A825]/30' : 'bg-white border-[#F9A825]/30'}`}>
          <div className={`text-[14px] font-semibold mb-2 ${isDark ? 'text-[#E3E3E3]' : 'text-[#1F1F1F]'}`}>🟠 {card.roleName || card.roleId} · 产出待确认</div>
          {findings.length > 0 ? (
            <ul className="space-y-1 mb-3">
              {findings.map((f, i) => (
                <li key={i} className={`text-[13px] flex gap-1.5 ${isDark ? 'text-[#C4C7C5]' : 'text-[#444746]'}`}><span className={isDark ? 'text-[#8E8E8E]' : 'text-[#757575]'}>›</span><span className="flex-1">{fText(f)}</span></li>
              ))}
            </ul>
          ) : (
            <div className={`text-[13px] mb-3 ${isDark ? 'text-[#8E8E8E]' : 'text-[#757575]'}`}>无评审备注</div>
          )}
          {rejecting && (
            <textarea rows="2" value={reason} autoFocus onChange={e => setReason(e.target.value)} placeholder="打回原因(可选)…"
              className={`w-full rounded-[10px] p-2 text-[13px] outline-none border mb-2 ${isDark ? 'bg-[#131314] border-white/10 text-[#E3E3E3]' : 'bg-white border-black/10 text-[#1F1F1F]'}`} />
          )}
          <div className="flex items-center gap-2 flex-wrap justify-end">
            <button className={cardBtnCls(isDark)} onClick={() => card.roleId && bridge.selectWorkflowRole(card.roleId)}>📄 查看产出</button>
            {rejecting ? (
              <React.Fragment>
                <button className={cardBtnCls(isDark)} onClick={() => { setRejecting(false); setReason(''); }}>取消</button>
                <button className={cardBtnCls(isDark, 'primary')} onClick={() => bridge.rejectWorkflowGate(card.cardId, card.roleId, reason.trim())}>✕ 确认打回</button>
              </React.Fragment>
            ) : (
              <React.Fragment>
                <button className={cardBtnCls(isDark)} onClick={() => setRejecting(true)}>✕ 打回</button>
                <button className={cardBtnCls(isDark, 'primary')} onClick={() => bridge.approveWorkflowGate(card.cardId, card.roleId)}>✓ 通过</button>
              </React.Fragment>
            )}
          </div>
        </div>
      );
    };

    const InteractionArea = ({ cards, sessionId, theme }) => {
      const isDark = theme === 'dark';
      const pending = (cards || []).filter(c => !c.resolved);
      if (pending.length === 0) return null;
      return (
        <div className="flex flex-col">
          {pending.map((card, i) => {
            const stackStyle = i > 0 ? { marginTop: '-8px' } : undefined;
            return (
              <div key={card.cardId || i} style={stackStyle} className="transition-transform hover:translate-y-[-2px]">
                {card.kind === 'user_input' ? (
                  <WfUserInputCard card={card} theme={theme} />
                ) : card.kind === 'gate' ? (
                  <GateApprovalCard card={card} theme={theme} />
                ) : card.kind === 'system' ? (
                  <div className={`rounded-[12px] border px-3 py-2 text-[13px] flex items-center gap-2 ${isDark ? 'bg-[#1E1F20] border-white/10 text-[#C4C7C5]' : 'bg-white border-black/10 text-[#444746]'}`}><span>⚙️</span><span className="flex-1">{card.text || ''}</span></div>
                ) : card.kind === 'artifact' ? (
                  <div className={`rounded-[16px] border p-4 ${isDark ? 'bg-[#1E1F20] border-[#34A853]/40' : 'bg-white border-[#34A853]/40'}`}>
                    <div className={`text-[14px] font-semibold mb-2 ${isDark ? 'text-[#E3E3E3]' : 'text-[#1F1F1F]'}`}>{card.text || '🎉 工作流完成'}</div>
                    <div className={`text-[12px] mb-3 break-all ${isDark ? 'text-[#8E8E8E]' : 'text-[#757575]'}`}>{card.path}</div>
                    <div className="flex items-center gap-2 flex-wrap justify-end">
                      {can('externalSystemOpen') && <button className={cardBtnCls(isDark)} onClick={() => bridge.openContainingFolder(card.path)}>📁 打开所在文件夹</button>}
                      {(!isWeb || can('artifactDownload')) && <button className={cardBtnCls(isDark, 'primary')} onClick={() => bridge.openArtifactExternal(card.path, sessionId)}>{isWeb ? '↓ 下载成品' : '▶ 打开成品'}</button>}
                    </div>
                  </div>
                ) : null}
              </div>
            );
          })}
        </div>
      );
    };

    // —— 新建任务模态 ——
    const NewTaskModal = ({ theme, onClose, onStarted, workflow, initialBrief = '' }) => {
      const isDark = theme === 'dark';
      // [工作流分离 Stage D] 表单形态全由该工作流 workflow.json 的 ui 块决定:
      // ui.scenarioOptions 有值 → 场景下拉;ui.attachments → 附件区。
      const wfUi = (workflow && workflow.ui) || {};
      const SCENARIOS = wfUi.scenarioOptions || [];
      const defaultScenario = SCENARIOS.length ? SCENARIOS[0].value
        : ((workflow && workflow.scenarios && workflow.scenarios[0]) || '');
      const [scenario, setScenario] = useState(defaultScenario);
      const [briefText, setBriefText] = useState(initialBrief);
      const [files, setFiles] = useState([]);      // 三省六部附件路径(start 后拷进配套材料/)
      const [picking, setPicking] = useState(false);
      const [starting, setStarting] = useState(false);
      const [error, setError] = useState('');
      const baseName = (p) => { const s = String(p).replace(/\\/g, '/').split('/'); return s[s.length - 1] || p; };
      async function pickAttachments() {
        if (picking || starting || !bridge.pickFiles) return;
        setPicking(true);
        try {
          const paths = await bridge.pickFiles();
          if (paths && paths.length) setFiles(prev => { const seen = new Set(prev); return prev.concat(paths.filter(p => !seen.has(p))); });
        } catch (e) { setError('选文件失败: ' + String((e && e.message) || e)); }
        finally { setPicking(false); }
      }
      async function start() {
        if (starting) return;
        setStarting(true); setError('');
        try {
          const res = await bridge.startWorkflowTask(scenario, { user_request_raw: briefText });
          if (res) {
            if (wfUi.attachments && files.length && bridge.addMaterialsToSession) {
              try { await bridge.addMaterialsToSession(res.session_id, files); }
              catch (e) { console.warn('附件拷贝失败(不阻塞启动):', e); }   // 素材失败不挡启动
            }
            onStarted(res); onClose();
          }
          else { setError('启动失败,请重试'); setStarting(false); }
        } catch (e) { setError(String((e && e.message) || e)); setStarting(false); }
      }
      const briefEmpty = briefText.trim().length === 0;
      const titleText = wfUi.newTaskTitle || ('新建「' + ((workflow && workflow.name) || '工作流') + '」任务');
      return (
        <div className="fixed inset-0 z-50 flex items-center justify-center p-6">
          <div className="absolute inset-0 bg-black/60" onClick={() => { if (!starting) onClose(); }}></div>
          <div className={`relative w-[560px] max-w-[92vw] flex flex-col rounded-[16px] overflow-hidden shadow-2xl ${isDark ? 'bg-[#1E1F20]' : 'bg-white'}`}>
            <div className={`flex items-center justify-between px-4 py-3 border-b ${isDark ? 'border-white/10' : 'border-black/10'}`}>
              <span className={`text-[15px] font-semibold flex items-center gap-2 ${isDark ? 'text-[#E3E3E3]' : 'text-[#1F1F1F]'}`}>{titleText}</span>
              <button onClick={() => { if (!starting) onClose(); }} disabled={starting} className={`w-8 h-8 rounded-full flex items-center justify-center disabled:opacity-40 ${isDark ? 'hover:bg-[#333537] text-[#C4C7C5]' : 'hover:bg-[#F0F4F9] text-[#444746]'}`}>✕</button>
            </div>
            <div className="px-4 py-4 space-y-4">
              {SCENARIOS.length > 0 && (
                <div>
                  <label className={`block text-[12px] font-semibold mb-1.5 ${isDark ? 'text-[#A8C7FA]' : 'text-[#0B57D0]'}`}>场景</label>
                  <select value={scenario} onChange={e => setScenario(e.target.value)} disabled={starting}
                    style={{
                      appearance: 'none', WebkitAppearance: 'none', MozAppearance: 'none',
                      backgroundImage: `url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='12' height='12' viewBox='0 0 12 12'%3E%3Cpath d='M2 4l4 4 4-4' stroke='${isDark ? '%23A8C7FA' : '%230B57D0'}' stroke-width='1.5' fill='none' stroke-linecap='round' stroke-linejoin='round'/%3E%3C/svg%3E")`,
                      backgroundRepeat: 'no-repeat', backgroundPosition: 'right 12px center', paddingRight: '32px',
                      color: isDark ? '#E3E3E3' : '#1F1F1F', backgroundColor: isDark ? '#131314' : '#ffffff',
                    }}
                    className={`w-full rounded-[10px] px-3 py-2 text-[13px] outline-none border disabled:opacity-50 ${isDark ? 'border-white/10' : 'border-black/10'}`}>
                    {SCENARIOS.map(s => <option key={s.value} value={s.value} style={{ backgroundColor: isDark ? '#131314' : '#ffffff', color: isDark ? '#E3E3E3' : '#1F1F1F' }}>{s.label}</option>)}
                  </select>
                </div>
              )}
              <div>
                <label className={`block text-[12px] font-semibold mb-1.5 ${isDark ? 'text-[#A8C7FA]' : 'text-[#0B57D0]'}`}>{wfUi.briefLabel || '初始需求'}</label>
                <textarea rows="5" value={briefText} onChange={e => setBriefText(e.target.value)} disabled={starting} placeholder={wfUi.briefPlaceholder || ''}
                  className={`w-full rounded-[10px] p-2 text-[13px] outline-none border resize-y disabled:opacity-50 ${isDark ? 'bg-[#131314] border-white/10 text-[#E3E3E3]' : 'bg-white border-black/10 text-[#1F1F1F]'}`} />
                {briefEmpty && <div className={`mt-1 text-[12px] ${isDark ? 'text-[#8E8E8E]' : 'text-[#757575]'}`}>留空也能开始,但写清楚 AI 办得更准。</div>}
              </div>
              {wfUi.attachments && (!isWeb || can('hostFilePicker')) && (
                <div>
                  <label className={`block text-[12px] font-semibold mb-1.5 ${isDark ? 'text-[#A8C7FA]' : 'text-[#0B57D0]'}`}>附件(可选)</label>
                  <button onClick={pickAttachments} disabled={picking || starting} className={`${cardBtnCls(isDark)} disabled:opacity-40`}>{picking ? '选择中…' : (isWeb ? '📎 选择桌面端文件' : '📎 上传附件')}</button>
                  {files.length > 0 && (
                    <div className="mt-2 space-y-1">
                      {files.map(p => (
                        <div key={p} className={`flex items-center justify-between text-[12px] rounded px-2 py-1 ${isDark ? 'bg-[#131314] text-[#C4C7C5]' : 'bg-[#F0F4F9] text-[#444746]'}`}>
                          <span className="truncate pr-2">📄 {baseName(p)}</span>
                          <button onClick={() => setFiles(prev => prev.filter(x => x !== p))} disabled={starting} className="shrink-0 opacity-60 hover:opacity-100">✕</button>
                        </div>
                      ))}
                    </div>
                  )}
                  <div className={`mt-1 text-[12px] ${isDark ? 'text-[#8E8E8E]' : 'text-[#757575]'}`}>{isWeb ? '从桌面主机选择的文件会作为素材交给六部参考。' : '上传的文件会作为素材交给六部参考。'}</div>
                </div>
              )}
              {error && <div className={`text-[13px] ${isDark ? 'text-[#F28B82]' : 'text-[#C5221F]'}`}>⚠️ {error}</div>}
            </div>
            <div className={`flex items-center justify-end gap-2 px-4 py-3 border-t ${isDark ? 'border-white/10' : 'border-black/10'}`}>
              <button onClick={onClose} disabled={starting} className={`${cardBtnCls(isDark)} disabled:opacity-40 disabled:cursor-not-allowed`}>取消</button>
              <button onClick={start} disabled={starting} className={`${cardBtnCls(isDark, 'primary')} ${starting ? 'opacity-60 cursor-not-allowed' : ''}`}>{starting ? '启动中…' : '开始'}</button>
            </div>
          </div>
        </div>
      );
    };

    // —— 工作流模板卡（未启动时显示）——
    const TemplateCard = ({ theme, onOpen, title, badge, desc, banner }) => {
      const isDark = theme === 'dark';
      // banner 自带标题/流程文字 → 整张图即入口,点击即打开,不叠任何卡片文字。
      if (banner) {
        return (
          <button onClick={onOpen} title={title} style={{ gridColumn: '1 / -1', maxWidth: 1400 }}
            className={`group self-start block w-full rounded-[16px] overflow-hidden border text-left transition-all ${isDark ? 'border-white/10 hover:border-white/25 hover:shadow-lg hover:shadow-black/30' : 'border-black/10 hover:shadow-md'}`}>
            <img src={banner} alt={title} className="w-full h-auto block transition-transform duration-300 group-hover:scale-[1.02]" />
          </button>
        );
      }
      return (
        <div className={`flex flex-col rounded-[16px] border overflow-hidden ${isDark ? 'bg-[#1E1F20] border-white/10 hover:border-white/20' : 'bg-white border-black/10 hover:shadow-md'} transition-all`}>
          <div className="flex flex-col flex-1 p-5">
            <div className="flex items-start justify-between gap-2 mb-1">
              <h3 className={`text-[15px] font-semibold ${isDark ? 'text-[#E3E3E3]' : 'text-[#1F1F1F]'}`}>{title}</h3>
              <span className={`text-[11px] px-1.5 py-0.5 rounded shrink-0 ${isDark ? 'bg-[#333537] text-[#C4C7C5]' : 'bg-[#F0F4F9] text-[#444746]'}`}>{badge}</span>
            </div>
            <p className={`text-[12px] leading-relaxed mb-3 ${isDark ? 'text-[#C4C7C5]' : 'text-[#444746]'}`}>{desc}</p>
            <div className="mt-auto pt-2">
              <button onClick={onOpen} className={cardBtnCls(isDark, 'primary')}>打开</button>
            </div>
          </div>
        </div>
      );
    };

    const WorkflowView = ({ bs, theme, t }) => {
      const isDark = theme === 'dark';
      const wf = (bs && bs.workflow) || {};
      const run = wf.run || { active: false, agents: {}, cards: [], status: 'idle' };
      const [opened, setOpened] = useState(false);
      const [showNewTask, setShowNewTask] = useState(false);
      const [restartBrief, setRestartBrief] = useState('');
      const [stopping, setStopping] = useState(false);
      // [工作流分离 Stage D] 模板页/新建表单数据源 = 后端 list_workflows(含 ui 块)
      const [workflows, setWorkflows] = useState([]);
      const [newTaskWorkflow, setNewTaskWorkflow] = useState(null);
      useEffect(() => {
        let on = true;
        if (bridge.listWorkflows) bridge.listWorkflows().then(ws => { if (on) setWorkflows(ws || []); });
        return () => { on = false; };
      }, []);
      // 当前 run 所属的工作流对象(看板内"+新建任务"用它的表单)
      const runWorkflow = workflows.find(w => run.scenario && (w.scenarios || []).indexOf(run.scenario) >= 0) || null;
      // exited: run 还挂着也允许退回模板页(看板状态不丢,再点"打开"即回)
      const [exited, setExited] = useState(false);
      // 准奏后展开的最终奏折弹窗(ui.hasMemorial 的工作流·终审角色)
      const [memorialOpen, setMemorialOpen] = useState(false);
      // 奏折触发=状态驱动:回奏角色「未完成→完成」的瞬间自动展卷——与从哪个按钮
      // 准奏无关(底部 gate 卡的「✓ 通过」直连 approveWorkflowGate,点击路径接不到
      // approveRole,6/12 实测漏弹)。初值即已完成(刷新/重启进入已结束的 run)不自动
      // 弹,靠头部「📜 奏折」常驻入口手动看。
      const memorialUi = (run.ui && run.ui.hasMemorial && run.ui)
        || (runWorkflow && runWorkflow.ui && runWorkflow.ui.hasMemorial && runWorkflow.ui) || null;
      const memorialRoleId = memorialUi ? (memorialUi.memorialRole || 'huizou') : null;
      const memorialStatus = memorialRoleId
        ? (((run.agents || {})[memorialRoleId] || {}).status || null) : null;
      const memorialDone = memorialStatus === 'completed' || memorialStatus === 'complete';
      const prevMemorialDone = useRef(memorialDone);
      useEffect(() => {
        const was = prevMemorialDone.current;
        prevMemorialDone.current = memorialDone;
        if (memorialRoleId && memorialDone && !was) setMemorialOpen(true);
      }, [memorialDone, memorialRoleId]);
      const inBoard = (run.active || opened) && !exited;
      const containerCls = "flex-1 flex flex-col w-full h-full relative z-10 overflow-hidden animate-in fade-in duration-300";

      if (!inBoard) {
        return (
          <div className={containerCls}>
            <div className="w-full max-w-7xl mx-auto px-4 sm:px-6 md:px-10 pt-8 pb-4">
              <h1 className={`text-[32px] font-normal tracking-tight ${isDark ? 'text-[#E3E3E3]' : 'text-[#1F1F1F]'}`}>{t.workflow}</h1>
              <p className={`text-[13px] mt-1 ${isDark ? 'text-[#8E8E8E]' : 'text-[#757575]'}`}>选择一个工作流模板开始</p>
            </div>
            <div className="flex-1 overflow-y-auto custom-scrollbar pb-10">
              <div className="grid gap-4 max-w-7xl mx-auto px-4 sm:px-6 md:px-10" style={{ gridTemplateColumns: 'repeat(auto-fill, minmax(280px, 1fr))' }}>
                {/* [工作流分离 Stage D] 模板卡 = 后端 list_workflows(各 workflow.json 的
                    ui.template)。点开:当前 run 属于该工作流(scenario 命中它认领的场景)
                    → 续看板;否则弹该工作流自己的新建表单。 */}
                {workflows.map(wf => {
                  const tpl = (wf.ui && wf.ui.template) || {};
                  // banner 头图:workflow.json 的 ui.template.banner 优先;三省六部内置朝堂图兜底。
                  const banner = tpl.banner || (wf.id === 'sansheng-liubu' ? 'assets/sansheng-banner.png' : null);
                  return <TemplateCard key={wf.id} theme={theme} banner={banner}
                    title={tpl.title || wf.name || wf.id} badge={tpl.badge || ''} desc={tpl.desc || ''}
                    onOpen={() => {
                      setNewTaskWorkflow(wf);
                      const hasRun = run.scenario && (wf.scenarios || []).indexOf(run.scenario) >= 0
                        && (run.active || Object.keys(run.agents || {}).length > 0);
                      if (hasRun) { setExited(false); setOpened(true); }
                      else { setRestartBrief(''); setShowNewTask(true); }
                    }} />;
                })}
                {workflows.length === 0 && (
                  <p className={`text-[13px] ${isDark ? 'text-[#8E8E8E]' : 'text-[#757575]'}`}>没有可用的工作流(检查 bundle/workflow/*/workflow.json)</p>
                )}
              </div>
            </div>
            {showNewTask && <NewTaskModal theme={theme} workflow={newTaskWorkflow} initialBrief={restartBrief}
              onClose={() => setShowNewTask(false)} onStarted={() => { setExited(false); setOpened(true); }} />}
          </div>
        );
      }

      const statusText = run.status === 'complete' ? '✅ 已完成'
        : run.status === 'stopped' ? '⏹ 已停止'
        : run.status === 'blocked' ? '⚫ 阻塞'
        : run.active ? '🔵 运行中' : '未开始（点"新建任务"启动）';
      // run.agents{rid→{status,depends_on}} → swim-lane 需要的 agentStates/agentDeps
      const agentStates = {}, agentDeps = {};
      Object.keys(run.agents || {}).forEach((rid) => {
        const a = run.agents[rid] || {};
        agentStates[rid] = a.status;
        agentDeps[rid] = a.depends_on || [];
      });
      // [per_page] fan-out 逐页状态(base_role → {total, pages}) → 卡片展开 N 个 SubAgent chip
      const fanout = run.fanout || {};
      // [edict-obs] SubAgent 实时进展 + per-role token 计数
      const progress = run.progress || {};
      const tokens = run.tokens || {};
      // 卡片上"确认通过"→ 直接按 roleId 批准(后端只需 roleId;不依赖内存 gate 卡——
      // 刷新/重启后内存卡已清空,旧逻辑找不到卡就静默失效,正是"点了没反应"的根因)。
      const approveRole = (rid) => {
        const c = (run.cards || []).find((c) => c.kind === 'gate' && c.roleId === rid && !c.resolved);
        const p = bridge.approveWorkflowGate(c ? c.cardId : null, rid);
        // 三省六部的回奏(终审)准奏 → 立刻展卷(快路径;状态驱动的 effect 是兜底)
        if (memorialRoleId && (rid === memorialRoleId || String(rid).indexOf(memorialRoleId) === 0)) {
          Promise.resolve(p).then(() => setMemorialOpen(true));
        }
      };
      // 失败节点"🔄 重跑"→ 重置该角色为 pending(清重试)后续跑,上游已完成节点不重跑。
      const retryRole = (rid) => {
        if (bridge.available && bridge.retryWorkflowRole) bridge.retryWorkflowRole(rid);
      };
      const openRestart = (brief) => {
        setRestartBrief(brief || '');
        setNewTaskWorkflow(runWorkflow || workflows[0] || null);
        setShowNewTask(true);
      };
      const stopAndRestart = async () => {
        if (stopping || !bridge.stopWorkflowTask) return;
        if (!window.confirm('停止后，当前任务不会再继续派发。已生成的文件会保留，你可以修改原需求后重新开始。')) return;
        setStopping(true);
        try {
          const result = await bridge.stopWorkflowTask('user_stopped_for_restart');
          const brief = result && result.brief && result.brief.user_request_raw;
          openRestart(typeof brief === 'string' ? brief : '');
        } catch (e) {
          window.alert('停止工作流失败：' + String((e && e.message) || e));
        } finally {
          setStopping(false);
        }
      };

      return (
        <div className={containerCls}>
          <div className="w-full max-w-7xl mx-auto flex items-center justify-between px-4 sm:px-6 md:px-10 pt-8 pb-4">
            <div>
              <h1 className={`text-[32px] font-normal tracking-tight ${isDark ? 'text-[#E3E3E3]' : 'text-[#1F1F1F]'}`}>{(run.ui && run.ui.header) || (runWorkflow && runWorkflow.ui && runWorkflow.ui.header) || '工作流'}</h1>
              <p className={`text-[13px] mt-1 ${isDark ? 'text-[#8E8E8E]' : 'text-[#757575]'}`}>{statusText} · 卡片流</p>
            </div>
            <div className="flex items-center gap-2">
              {/* 回奏已完成的 run 常驻奏折入口(自动弹窗只在完成瞬间触发一次,事后/刷新后从这里看) */}
              {memorialRoleId && memorialDone && (
                <button onClick={() => setMemorialOpen(true)} className={cardBtnCls(isDark)}>📜 奏折</button>
              )}
              {run.status === 'stopped' ? (
                <button onClick={() => openRestart(restartBrief)} className={cardBtnCls(isDark, 'primary')}>✏️ 修改需求并重新开始</button>
              ) : run.status !== 'complete' ? (
                <button data-testid="workflow-stop-restart" onClick={stopAndRestart} disabled={stopping}
                  className={`${cardBtnCls(isDark)} ${stopping ? 'opacity-50 cursor-not-allowed' : ''}`}>{stopping ? '停止中…' : '⏹ 停止并修改'}</button>
              ) : null}
              <button onClick={() => { setOpened(false); setExited(true); }} className={cardBtnCls(isDark)}>← 模板页</button>
              {/* 看板内新建 = 跟当前看板同工作流(开机自动恢复直接进看板时没经过模板卡,
                  必须在这里按 run 反查工作流对象,否则表单回落错) */}
              <button onClick={() => { setRestartBrief(''); setNewTaskWorkflow(runWorkflow || workflows[0] || null); setShowNewTask(true); }} className={cardBtnCls(isDark, 'primary')}>+ 新建任务</button>
            </div>
          </div>
          <div className="flex-1 overflow-auto custom-scrollbar px-4 sm:px-6 md:px-10 pb-4">
            <AgentPipelineView ui={run.ui || (runWorkflow && runWorkflow.ui) || null} agents={run.agents || {}} agentStates={agentStates} agentDeps={agentDeps} fanout={fanout} progress={progress} tokens={tokens} theme={theme}
              onApprove={approveRole} onRetry={retryRole} onCardClick={(rid) => bridge.selectWorkflowRole(rid)} />
          </div>
          {(run.cards || []).some(c => !c.resolved) && (
            <div className={`shrink-0 max-h-[42vh] overflow-y-auto custom-scrollbar px-4 sm:px-6 md:px-10 py-3 border-t ${isDark ? 'border-white/10 bg-[#131314]/60' : 'border-black/10 bg-[#F8FAFC]/60'}`}>
              <InteractionArea cards={run.cards || []} sessionId={run.sessionId} theme={theme} />
            </div>
          )}
          {run.selectedRole && <CardDrawer roleId={run.selectedRole} projectDir={run.projectDir} sessionId={run.sessionId} theme={theme} onClose={() => bridge.closeWorkflowDrawer()} />}
          {memorialOpen && <ImperialMemorialModal projectDir={run.projectDir} sessionId={run.sessionId} theme={theme} onClose={() => setMemorialOpen(false)} />}
          {showNewTask && <NewTaskModal theme={theme} workflow={newTaskWorkflow} initialBrief={restartBrief} onClose={() => setShowNewTask(false)} onStarted={() => { setExited(false); setOpened(true); }} />}
        </div>
      );
    };


    // ==========================================
    // 撕离窗口(tear-off):同一个 index.html 以 ?detached=1&kind=&id= 启动,只挂载该面板,无侧边栏。
    // 窗口间强独立:各自 useBridge()/init(),不做 live 数据同步(真相源在后端,进程内共享)。
    // ==========================================

export { WidgetCard, ProgressBar, ListRow, UI_STATES, toUiState, AGENT_NAME_MAP, layoutForRun, AgentAvatar, FanoutGrid, AgentCard, AgentPipelineView, FilePreviewModal, ImperialMemorialModal, CardDrawer, WfUserInputCard, GateApprovalCard, InteractionArea, NewTaskModal, TemplateCard, WorkflowView };
