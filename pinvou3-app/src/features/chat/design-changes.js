function createDesignChange({ element, type, property, oldValue, newValue, groupId, groupLabel }) {
  const selector = element && element.selector ? element.selector : '';
  return {
    id: `design-change-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`, // eslint-disable-line sonarjs/pseudo-random -- not a security context; only generates display IDs
    elementId: element && element.id ? element.id : '',
    selector,
    elementLabel: element && element.label ? element.label : '',
    type,
    property,
    oldValue: oldValue == null ? '' : String(oldValue),
    newValue: newValue == null ? '' : String(newValue),
    groupId: groupId || '',
    groupLabel: groupLabel || '',
    status: 'todo',
    createdAt: new Date().toISOString(),
  };
}

// 两条改动是否完全同值(同 selector/type/property 且新旧值一致):
// add 去重与 ChatView 应用前的预检共用同一比较,避免两处字段漂移。
function sameDesignChange(a, b) {
  return !!a && !!b
    && a.selector === b.selector
    && a.type === b.type
    && a.property === b.property
    && a.oldValue === b.oldValue
    && a.newValue === b.newValue;
}

function reduceDesignChanges(state, action) {
  const current = Array.isArray(state) ? state : [];
  if (!action || typeof action !== 'object') return current;
  switch (action.type) {
    case 'add':
      if (!action.change) return current;
      if (current.some((change) => sameDesignChange(change, action.change))) return current;
      return [...current, action.change];
    case 'mark-applied':
      return current.map((change) => (
        change.id === action.changeId
          ? { ...change, status: action.ok === false ? 'failed' : 'applied', error: action.error || undefined }
          : change
      ));
    case 'clear':
      return [];
    default:
      return current;
  }
}

function uniqueDesignChanges(changes) {
  const seen = new Set();
  return (Array.isArray(changes) ? changes : []).filter((change) => {
    const key = [
      change.selector,
      change.type,
      change.property || '',
      change.oldValue,
      change.newValue,
    ].join('\u0000');
    if (seen.has(key)) return false;
    seen.add(key);
    return true;
  });
}

function createDesignChangeScopeKey(sessionId, artifactPath) {
  return `${sessionId || 'draft'}::${artifactPath || ''}`;
}

function reduceScopedDesignChanges(scopes, scopeKey, action) {
  const currentScopes = scopes && typeof scopes === 'object' ? scopes : {};
  const key = scopeKey || createDesignChangeScopeKey();
  const nextChanges = reduceDesignChanges(currentScopes[key] || [], action);
  if (nextChanges === currentScopes[key]) return currentScopes;
  const nextScopes = { ...currentScopes };
  if (nextChanges.length) nextScopes[key] = nextChanges;
  else delete nextScopes[key];
  return nextScopes;
}

export {
  createDesignChange,
  createDesignChangeScopeKey,
  reduceDesignChanges,
  reduceScopedDesignChanges,
  sameDesignChange,
  uniqueDesignChanges,
};
