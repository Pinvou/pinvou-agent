export const EMPTY_PERSISTENCE_WARNING = Object.freeze({
  dismissed: false,
  message: '',
});

// Dismissal is local to the currently displayed occurrence. Status hydration
// repeats the backend's latest state and must not resurrect an unchanged
// warning; a new backend event is a fresh occurrence even when its text is the
// same. Remounting naturally starts from the empty state.
export function persistenceWarningReducer(state, action) {
  switch (action?.type) {
    case 'hydrate': {
      const message = typeof action.message === 'string' ? action.message : '';
      if (!message) return EMPTY_PERSISTENCE_WARNING;
      return {
        dismissed: state?.message === message ? !!state.dismissed : false,
        message,
      };
    }
    case 'report': {
      const message = typeof action.message === 'string' ? action.message : '';
      return message ? { dismissed: false, message } : EMPTY_PERSISTENCE_WARNING;
    }
    case 'dismiss':
      return state?.message ? { ...state, dismissed: true } : EMPTY_PERSISTENCE_WARNING;
    case 'clear':
      return EMPTY_PERSISTENCE_WARNING;
    default:
      return state || EMPTY_PERSISTENCE_WARNING;
  }
}

export function visiblePersistenceWarning(state) {
  return state?.dismissed ? '' : (state?.message || '');
}

// Status is a point-in-time snapshot, while warning/restored events are newer
// authoritative transitions. A response may hydrate only when no persistence
// event has arrived since that request started.
export function isPersistenceStatusCurrent(requestEventEpoch, currentEventEpoch) {
  return requestEventEpoch === currentEventEpoch;
}
