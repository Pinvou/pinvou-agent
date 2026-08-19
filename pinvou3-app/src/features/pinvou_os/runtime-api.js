import { useCallback, useEffect, useState } from 'react';

import {
  invokeTauri,
  isTauriAvailable,
  tauriEvents,
} from '../../platform/tauri/client.js';
import { applyA2uiProjection, EMPTY_A2UI_STATE } from './a2ui-runtime.js';

const INITIAL_STATE = Object.freeze({
  snapshot: null,
  events: [],
  loading: true,
  error: '',
});

function mergeRuntimeEvent(events, incoming) {
  if (!incoming || typeof incoming !== 'object') return events;
  const incomingId = incoming.eventId || incoming.event_id;
  const next = incomingId
    ? events.filter(event => (event.eventId || event.event_id) !== incomingId)
    : events;
  return [incoming, ...next].slice(0, 12);
}

export function usePinvouOsRuntime() {
  const [state, setState] = useState(INITIAL_STATE);

  const refresh = useCallback(async () => {
    if (!isTauriAvailable()) {
      setState(current => ({ ...current, loading: false, error: 'bridge_unavailable' }));
      return;
    }
    try {
      const [snapshot, events] = await Promise.all([
        invokeTauri('get_pinvou_os_snapshot'),
        invokeTauri('list_pinvou_os_events', { afterSequence: null, limit: 12 }),
      ]);
      setState({
        snapshot: snapshot || null,
        events: Array.isArray(events) ? events.slice().reverse() : [],
        loading: false,
        error: '',
      });
    } catch (error) {
      setState(current => ({
        ...current,
        loading: false,
        error: error && error.message ? error.message : String(error || 'runtime_unavailable'),
      }));
    }
  }, []);

  useEffect(() => {
    let disposed = false;
    let unlisten = null;
    void refresh();
    if (!isTauriAvailable()) return undefined;
    tauriEvents.listen('pinvou-os:event', message => {
      if (disposed) return;
      const incoming = message && message.payload ? message.payload : message;
      setState(current => ({
        ...current,
        events: mergeRuntimeEvent(current.events, incoming),
      }));
      void invokeTauri('get_pinvou_os_snapshot').then(snapshot => {
        if (!disposed) setState(current => ({ ...current, snapshot, error: '' }));
      }).catch(() => {});
    }).then(stop => {
      if (disposed) stop();
      else unlisten = stop;
    }).catch(() => {});
    return () => {
      disposed = true;
      if (unlisten) unlisten();
    };
  }, [refresh]);

  return { ...state, refresh };
}

export function usePinvouOsProjection() {
  const [state, setState] = useState({ surface: EMPTY_A2UI_STATE, loading: true, error: '' });

  useEffect(() => {
    let disposed = false;
    let unlisten = null;
    if (!isTauriAvailable()) {
      setState(current => ({ ...current, loading: false, error: 'bridge_unavailable' }));
      return undefined;
    }

    const accept = projection => {
      if (disposed) return;
      setState(current => {
        try {
          return {
            surface: applyA2uiProjection(current.surface, projection),
            loading: false,
            error: '',
          };
        } catch (error) {
          return { ...current, loading: false, error: String(error && error.message || error) };
        }
      });
    };

    tauriEvents.listen('pinvou-os:a2ui', message => {
      accept(message && message.payload ? message.payload : message);
    }).then(stop => {
      if (disposed) stop();
      else unlisten = stop;
    }).catch(() => {});

    void invokeTauri('get_pinvou_os_projection').then(accept).catch(error => {
      if (!disposed) setState(current => ({
        ...current,
        loading: false,
        error: String(error && error.message || error || 'projection_unavailable'),
      }));
    });

    return () => {
      disposed = true;
      if (unlisten) unlisten();
    };
  }, []);

  return state;
}

export function pinvouOsAgentRows(snapshot) {
  const agents = snapshot && snapshot.agents ? Object.values(snapshot.agents) : [];
  const order = [
    'agent:front',
    'agent:orchestrator',
    'agent:screen-observer',
    'agent:resource',
    'agent:connectivity',
    'agent:inference',
    'agent:device',
    'agent:capability',
    'agent:memory',
    'agent:policy',
    'agent:attention',
    'agent:asr-context',
  ];
  const rank = new Map(order.map((id, index) => [id, index]));
  return agents.sort((left, right) => {
    const leftRank = rank.has(left.agentId) ? rank.get(left.agentId) : order.length;
    const rightRank = rank.has(right.agentId) ? rank.get(right.agentId) : order.length;
    return leftRank - rightRank || String(left.agentId).localeCompare(String(right.agentId));
  });
}

export function pinvouOsEventKind(envelope) {
  return envelope && envelope.event && envelope.event.kind
    ? envelope.event.kind
    : 'unknown';
}
