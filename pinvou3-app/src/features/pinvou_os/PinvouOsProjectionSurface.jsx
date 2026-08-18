import React, { useMemo } from 'react';

import { Brain, CheckCircle2, Radio, Users } from '../../components/icons.jsx';
import { resolveA2uiBinding } from './a2ui-runtime.js';
import { usePinvouOsProjection } from './runtime-api.js';

function statusLabel(status, copy) {
  return copy.interactionStates[status] || copy.interactionStates.idle;
}

function healthLabel(kind, value, copy) {
  if (kind === 'resourcePressure') return copy.pressures[value] || copy.unknown;
  if (kind === 'connectivity') {
    return ({ online: copy.online, offline: copy.offline, degraded: copy.degraded })[value] || copy.unknown;
  }
  return ({ ready: copy.modelReady, degraded: copy.degraded, unavailable: copy.unavailable })[value] || copy.unknown;
}

function bound(component, key, dataModel) {
  return resolveA2uiBinding(component && component[key], dataModel);
}

export function PinvouOsProjectionSurface({ t, compact = false }) {
  const copy = t.uiPinvouOs;
  const { surface, loading, error } = usePinvouOsProjection();
  const model = surface.dataModel || {};
  const components = surface.components || {};
  const root = components.root;
  const children = useMemo(
    () => (root && Array.isArray(root.children) ? root.children.map(id => components[id]).filter(Boolean) : []),
    [components, root],
  );
  const identity = children.find(component => component.component === 'PinvouIdentityCard');
  const interaction = children.find(component => component.component === 'PinvouInteractionCard');
  const health = children.find(component => component.component === 'PinvouRuntimeHealth');

  if (loading || error || !root) return null;

  const interactionState = String(bound(interaction, 'status', model) || 'idle');
  const runningAgents = Number(bound(health, 'runningAgents', model) || 0);
  const totalAgents = Number(bound(health, 'totalAgents', model) || 0);
  const activeMissions = Number(bound(health, 'activeMissions', model) || 0);
  const resourcePressure = String(bound(health, 'resourcePressure', model) || 'normal');
  const connectivity = String(bound(health, 'connectivity', model) || 'unknown');
  const inference = String(bound(health, 'inference', model) || 'unknown');

  return (
    <section
      className={`pinvou-os-projection ${compact ? 'is-compact' : ''}`}
      data-testid="pinvou-os-a2ui-surface"
      data-a2ui-surface-id={surface.surfaceId}
      data-a2ui-basis-sequence={surface.basisSequence}
      aria-label={copy.canvasTitle}
    >
      <div className="pinvou-os-projection-identity">
        <span className="pinvou-os-projection-mark" aria-hidden="true" />
        <span className="pinvou-os-projection-copy">
          <strong>{bound(identity, 'displayName', model) || 'Pinvou'}</strong>
          <small>{copy.canvasTitle}</small>
        </span>
      </div>
      <div className={`pinvou-os-interaction-state is-${interactionState}`}>
        <span aria-hidden="true" />
        {statusLabel(interactionState, copy)}
      </div>
      <div className="pinvou-os-projection-metrics" aria-label={copy.canvasRuntimeSummary}>
        <span title={copy.agentsLabel}><Users size={14} />{runningAgents}/{totalAgents}</span>
        <span title={copy.activeMissions}><CheckCircle2 size={14} />{activeMissions}</span>
        <span title={copy.network}><Radio size={14} />{healthLabel('connectivity', connectivity, copy)}</span>
        <span title={copy.model}><Brain size={14} />{healthLabel('inference', inference, copy)}</span>
        <span className={`is-pressure-${resourcePressure}`} title={copy.pressure}>{healthLabel('resourcePressure', resourcePressure, copy)}</span>
      </div>
    </section>
  );
}
