import React, { useMemo, useState } from 'react';

import {
  AlertTriangle,
  Brain,
  CheckCircle2,
  ChevronRight,
  Cpu,
  Database,
  Hexagon,
  Layers,
  MessageSquare,
  Monitor,
  Radio,
  Users,
  Wrench,
} from '../../components/icons.jsx';
import { PinvouLogo } from '../../components/PinvouLogo.jsx';
import {
  pinvouOsAgentRows,
  pinvouOsEventKind,
  usePinvouOsRuntime,
} from './runtime-api.js';

const AGENT_ICONS = {
  'agent:front': MessageSquare,
  'agent:orchestrator': Layers,
  'agent:screen-observer': Monitor,
  'agent:resource': Cpu,
  'agent:connectivity': Radio,
  'agent:inference': Brain,
  'agent:device': Wrench,
  'agent:capability': Hexagon,
  'agent:memory': Database,
  'agent:policy': CheckCircle2,
  'agent:attention': Brain,
};

function statePresentation(agent, copy) {
  const state = agent && agent.observedState;
  if (state === 'running' || state === 'idle') {
    return { label: copy.ready, dot: '#34C759', glow: 'rgba(52,199,89,.35)' };
  }
  if (state === 'paused') {
    return { label: copy.paused, dot: '#FF9500', glow: 'rgba(255,149,0,.3)' };
  }
  if (state === 'failed' || state === 'stopped') {
    return { label: copy.stopped, dot: '#FF3B30', glow: 'rgba(255,59,48,.28)' };
  }
  return { label: copy.scaffold, dot: '#8E8E93', glow: 'rgba(142,142,147,.25)' };
}

function formatMetric(value, suffix) {
  return Number.isFinite(value) ? `${Math.round(value)}${suffix}` : '—';
}

function formatClock(timestampMs) {
  if (!Number.isFinite(timestampMs)) return '—';
  return new Intl.DateTimeFormat(undefined, {
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
  }).format(new Date(timestampMs));
}

function formatLatency(value) {
  if (!Number.isFinite(value)) return '—';
  if (value < 1000) return `${Math.round(value)}ms`;
  return `${(value / 1000).toFixed(value < 10_000 ? 1 : 0)}s`;
}

function modelDisplayName(value, fallback) {
  const name = String(value || '').trim();
  if (!name) return fallback;
  return name.replace(/^glm(?=-|\b)/i, 'GLM');
}

function connectivityPresentation(connectivity, copy) {
  const status = connectivity && connectivity.status ? connectivity.status : 'unknown';
  if (status === 'online') return { label: copy.online, color: '#34C759' };
  if (status === 'degraded') return { label: copy.degraded, color: '#FF9500' };
  if (status === 'offline') return { label: copy.offline, color: '#FF3B30' };
  return { label: copy.unknown, color: '#8E8E93' };
}

function inferencePresentation(inference, copy) {
  const status = inference && inference.status ? inference.status : 'unknown';
  if (status === 'ready') return { label: copy.modelReady, color: '#34C759' };
  if (status === 'degraded') return { label: copy.degraded, color: '#FF9500' };
  if (status === 'unavailable') return { label: copy.unavailable, color: '#FF3B30' };
  return { label: copy.unknown, color: '#8E8E93' };
}

function RuntimeHealth({ snapshot, copy, dark }) {
  const connectivity = snapshot && snapshot.connectivity ? snapshot.connectivity : {};
  const inference = snapshot && snapshot.inference ? snapshot.inference : {};
  const network = connectivityPresentation(connectivity, copy);
  const model = inferencePresentation(inference, copy);
  const reason = value => copy.reasons[value] || value || copy.none;
  const rows = [
    {
      id: 'network',
      label: copy.network,
      value: network.label,
      color: network.color,
      detail: `${copy.lastCheck} ${formatClock(connectivity.checkedAtMs)} · ${formatLatency(connectivity.latencyMs)}`,
      failure: connectivity.reasonCode,
    },
    {
      id: 'model',
      label: copy.model,
      value: `${modelDisplayName(inference.model, copy.modelUnknown)} ${model.label}`,
      color: model.color,
      detail: inference.lastSuccessAtMs
        ? `${copy.lastSuccess} ${formatClock(inference.lastSuccessAtMs)} · ${formatLatency(inference.lastSuccessLatencyMs)}`
        : copy.awaitingFirstSuccess,
      failure: inference.reasonCode,
    },
  ];
  return (
    <div data-testid="pinvou-os-runtime-health" className="mt-2 space-y-1.5">
      {rows.map(row => (
        <div key={row.id} className={`rounded-xl px-3 py-2.5 ${dark ? 'bg-white/[0.04]' : 'bg-white/65'}`}>
          <div className="flex items-center justify-between gap-3 text-[11px]">
            <span className={dark ? 'text-[#AEB4BC]' : 'text-[#666B72]'}>{row.label}</span>
            <span className="truncate font-semibold" style={{ color: row.color }}>{row.value}</span>
          </div>
          <div className={`mt-1 text-[9px] ${dark ? 'text-[#777D84]' : 'text-[#8E8E93]'}`}>{row.detail}</div>
          {row.failure && (
            <div className="mt-1 text-[9px] text-[#FF6961]">{copy.failureReason} · {reason(row.failure)}</div>
          )}
        </div>
      ))}
    </div>
  );
}

function AgentCard({ agent, copy, dark, index }) {
  const localized = copy.agents[agent.agentId] || {};
  const capability = Array.isArray(agent.capabilities) ? agent.capabilities[0] : null;
  const status = statePresentation(agent, copy);
  const Icon = AGENT_ICONS[agent.agentId] || Radio;
  return (
    <div
      data-testid="pinvou-os-agent-card"
      data-agent-id={agent.agentId}
      className={`group rounded-2xl border px-3 py-3 transition-all duration-300 hover:-translate-y-px ${
        dark
          ? 'border-white/[0.07] bg-white/[0.035] hover:bg-white/[0.065]'
          : 'border-black/[0.04] bg-white/55 hover:bg-white/90'
      }`}
      style={{
        animation: `pinvouOsSlideUp .42s cubic-bezier(.16,1,.3,1) ${Math.min(index, 8) * 35}ms both`,
      }}
    >
      <div className="flex items-start gap-3">
        <div className={`flex h-9 w-9 shrink-0 items-center justify-center rounded-xl ${
          dark ? 'bg-white/[0.06] text-[#A8C7FA]' : 'bg-[#EAF2FF] text-[#0A62CC]'
        }`}>
          <Icon size={17} />
        </div>
        <div className="min-w-0 flex-1">
          <div className="flex items-center justify-between gap-2">
            <div className="truncate text-[13px] font-semibold">
              {localized.name || agent.displayName || agent.agentId}
            </div>
            <div className={`flex shrink-0 items-center gap-1.5 text-[10px] font-medium ${dark ? 'text-[#AEB4BC]' : 'text-[#6E7378]'}`}>
              <span
                className="h-1.5 w-1.5 rounded-full"
                style={{ background: status.dot, boxShadow: `0 0 8px ${status.glow}` }}
              />
              {status.label}
            </div>
          </div>
          <div className={`mt-1 line-clamp-2 text-[11px] leading-[1.45] ${dark ? 'text-[#8E949C]' : 'text-[#777D84]'}`}>
            {localized.role || agent.role}
          </div>
          {capability && (
            <div className={`mt-2 inline-flex max-w-full rounded-md px-2 py-1 font-mono text-[9px] ${
              dark ? 'bg-black/20 text-[#7DBDFF]' : 'bg-[#EDF4FF] text-[#0A62CC]'
            }`}>
              <span className="truncate">{capability.capabilityId}</span>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}

export function PinvouOsAgentDock({ theme, t }) {
  const dark = theme === 'dark';
  const copy = t.uiPinvouOs;
  const { snapshot, events, loading, error, refresh } = usePinvouOsRuntime();
  const [open, setOpen] = useState(false);
  const agents = useMemo(() => pinvouOsAgentRows(snapshot), [snapshot]);
  const runningCount = agents.filter(agent => ['running', 'idle'].includes(agent.observedState)).length;
  const observation = snapshot && snapshot.resources && snapshot.resources.lastObservation;
  const pressure = snapshot && snapshot.resources ? snapshot.resources.pressure : 'normal';
  const connectivity = snapshot && snapshot.connectivity ? snapshot.connectivity : {};
  const inference = snapshot && snapshot.inference ? snapshot.inference : {};
  const networkStatus = connectivityPresentation(connectivity, copy);
  const inferenceStatus = inferencePresentation(inference, copy);

  if (!open) {
    return (
      <button
        type="button"
        data-testid="pinvou-os-agent-dock-open"
        onClick={() => setOpen(true)}
        aria-label={copy.openRuntime}
        title={copy.openRuntime}
        className={`fixed right-7 top-7 z-50 flex min-h-12 touch-manipulation items-center gap-3 rounded-2xl border px-4 py-2 text-[11px] font-semibold shadow-lg backdrop-blur-2xl transition-all hover:-translate-y-px ${
          dark
            ? 'border-white/10 bg-[#1B1C1F]/85 text-[#E8EAED]'
            : 'border-black/[0.06] bg-white/85 text-[#1D1D1F]'
        }`}
      >
        <Users size={16} />
        <span className="grid min-w-0 gap-0.5 text-left leading-tight">
          <span data-testid="pinvou-os-network-summary" className="truncate">
            {copy.network} · <span style={{ color: networkStatus.color }}>{networkStatus.label}</span>
          </span>
          <span data-testid="pinvou-os-model-summary" className="truncate">
            {copy.model} · {modelDisplayName(inference.model, copy.modelUnknown)} <span style={{ color: inferenceStatus.color }}>{inferenceStatus.label}</span>
          </span>
        </span>
      </button>
    );
  }

  return (
    <aside
      data-testid="pinvou-os-agent-dock"
      className={`fixed bottom-5 right-5 top-5 z-50 flex w-[354px] max-w-[calc(100vw-40px)] flex-col overflow-hidden rounded-[24px] border shadow-[0_18px_60px_rgba(0,0,0,.24)] backdrop-blur-[36px] ${
        dark
          ? 'border-white/[0.08] bg-[#1B1C1F]/80 text-[#F2F3F5]'
          : 'border-black/[0.05] bg-white/78 text-[#1D1D1F]'
      }`}
    >
      <style>{`@keyframes pinvouOsSlideUp{from{opacity:0;transform:translateY(10px)}to{opacity:1;transform:translateY(0)}}`}</style>
      <div className={`shrink-0 border-b px-4 pb-4 pt-4 ${dark ? 'border-white/[0.07]' : 'border-black/[0.05]'}`}>
        <div className="flex items-center gap-3">
          <div className={`flex h-10 w-10 items-center justify-center rounded-2xl ${dark ? 'bg-white/[0.06]' : 'bg-white shadow-sm'}`}>
            <PinvouLogo className="h-6 w-6" title="Pinvou" />
          </div>
          <div className="min-w-0 flex-1">
            <div className="flex items-center gap-2">
              <span className="text-[15px] font-semibold">{copy.identity}</span>
              <span className="inline-flex items-center gap-1 rounded-full bg-[#34C759]/[0.12] px-2 py-0.5 text-[9px] font-bold uppercase tracking-[.08em] text-[#30D158]">
                <span className="h-1.5 w-1.5 rounded-full bg-current" />
                {copy.continuous}
              </span>
            </div>
            <div className={`mt-0.5 text-[11px] ${dark ? 'text-[#8E949C]' : 'text-[#777D84]'}`}>{copy.oneIdentity}</div>
          </div>
          <button
            type="button"
            data-testid="pinvou-os-agent-dock-close"
            onClick={() => setOpen(false)}
            aria-label={copy.closeRuntime}
            title={copy.closeRuntime}
            className={`relative z-10 flex h-11 w-11 shrink-0 touch-manipulation items-center justify-center rounded-full transition-colors ${dark ? 'hover:bg-white/[0.08]' : 'hover:bg-black/[0.05]'}`}
          >
            <ChevronRight size={16} />
          </button>
        </div>

        <div className="mt-4 grid grid-cols-3 gap-2">
          <div className={`rounded-2xl px-3 py-2.5 ${dark ? 'bg-white/[0.04]' : 'bg-white/65'}`}>
            <div className={`text-[9px] font-semibold uppercase tracking-[.08em] ${dark ? 'text-[#787E87]' : 'text-[#8E8E93]'}`}>{copy.agentsLabel}</div>
            <div className="mt-1 text-[18px] font-semibold tabular-nums">{agents.length || '—'}</div>
          </div>
          <div className={`rounded-2xl px-3 py-2.5 ${dark ? 'bg-white/[0.04]' : 'bg-white/65'}`}>
            <div className={`text-[9px] font-semibold uppercase tracking-[.08em] ${dark ? 'text-[#787E87]' : 'text-[#8E8E93]'}`}>{copy.running}</div>
            <div className="mt-1 text-[18px] font-semibold tabular-nums text-[#34C759]">{runningCount}</div>
          </div>
          <div className={`rounded-2xl px-3 py-2.5 ${dark ? 'bg-white/[0.04]' : 'bg-white/65'}`}>
            <div className={`text-[9px] font-semibold uppercase tracking-[.08em] ${dark ? 'text-[#787E87]' : 'text-[#8E8E93]'}`}>{copy.pressure}</div>
            <div className={`mt-1 truncate text-[13px] font-semibold ${pressure === 'normal' ? 'text-[#34C759]' : 'text-[#FF9500]'}`}>
              {copy.pressures[pressure] || pressure}
            </div>
          </div>
        </div>

        <div className={`mt-2 grid grid-cols-3 gap-px overflow-hidden rounded-xl text-center ${dark ? 'bg-white/[0.06]' : 'bg-black/[0.04]'}`}>
          {[
            [copy.cpu, formatMetric(observation && observation.cpuUsagePct, '%')],
            [copy.memory, formatMetric(observation && observation.memoryUsedPct, '%')],
            [copy.temperature, formatMetric(observation && observation.temperatureC, '°')],
          ].map(([label, value]) => (
            <div key={label} className={`px-2 py-2 ${dark ? 'bg-[#202124]/90' : 'bg-white/85'}`}>
              <div className={`text-[9px] ${dark ? 'text-[#777D84]' : 'text-[#8E8E93]'}`}>{label}</div>
              <div className="mt-0.5 text-[11px] font-semibold tabular-nums">{value}</div>
            </div>
          ))}
        </div>
        <RuntimeHealth snapshot={snapshot} copy={copy} dark={dark} />
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto px-3 py-3 custom-scrollbar">
        <div className="mb-2 flex items-center justify-between px-1">
          <div>
            <div className="text-[12px] font-semibold">{copy.agentRuntime}</div>
            <div className={`mt-0.5 text-[10px] ${dark ? 'text-[#777D84]' : 'text-[#8E8E93]'}`}>{copy.runtimeHint}</div>
          </div>
          <button
            type="button"
            onClick={refresh}
            className={`flex h-8 w-8 items-center justify-center rounded-full transition-colors ${dark ? 'hover:bg-white/[0.07]' : 'hover:bg-black/[0.05]'}`}
            title={copy.refresh}
          >
            <Radio size={14} className={loading ? 'animate-pulse' : ''} />
          </button>
        </div>

        {error && (
          <div className="mb-3 flex items-start gap-2 rounded-2xl bg-[#FF3B30]/[0.1] px-3 py-2.5 text-[11px] text-[#FF6961]">
            <AlertTriangle size={14} className="mt-0.5 shrink-0" />
            <span>{error === 'bridge_unavailable' ? copy.bridgeUnavailable : copy.readFailed}</span>
          </div>
        )}

        <div className="space-y-2">
          {agents.map((agent, index) => (
            <AgentCard key={agent.agentId} agent={agent} copy={copy} dark={dark} index={index} />
          ))}
        </div>

        <div className={`mx-1 mt-4 border-t pt-3 ${dark ? 'border-white/[0.07]' : 'border-black/[0.05]'}`}>
          <div className="flex items-center justify-between">
            <div className="text-[12px] font-semibold">{copy.eventFabric}</div>
            <div className={`text-[9px] font-medium ${dark ? 'text-[#777D84]' : 'text-[#8E8E93]'}`}>{copy.sharedTruth}</div>
          </div>
          <div className="mt-2 space-y-1.5">
            {events.slice(0, 6).map(event => {
              const kind = pinvouOsEventKind(event);
              return (
                <div key={event.eventId || event.sequence} className={`flex items-center gap-2 rounded-xl px-2.5 py-2 text-[10px] ${dark ? 'bg-black/15' : 'bg-white/60'}`}>
                  <span className="h-1.5 w-1.5 shrink-0 rounded-full bg-[#0A84FF]" />
                  <span className="min-w-0 flex-1 truncate">{copy.events[kind] || kind}</span>
                  <span className={`shrink-0 font-mono ${dark ? 'text-[#666C74]' : 'text-[#9A9A9F]'}`}>#{event.sequence}</span>
                </div>
              );
            })}
          </div>
        </div>
      </div>

      <div className={`shrink-0 border-t px-4 py-3 text-[10px] leading-relaxed ${dark ? 'border-white/[0.07] text-[#777D84]' : 'border-black/[0.05] text-[#8E8E93]'}`}>
        {copy.influenceChain}
      </div>
    </aside>
  );
}
