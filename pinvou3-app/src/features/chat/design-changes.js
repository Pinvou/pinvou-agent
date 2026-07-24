function createDesignChange({ element, type, property, oldValue, newValue }) {
  const selector = element && element.selector ? element.selector : '';
  return {
    id: `design-change-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
    elementId: element && element.id ? element.id : '',
    selector,
    type,
    property,
    oldValue: oldValue == null ? '' : String(oldValue),
    newValue: newValue == null ? '' : String(newValue),
    status: 'todo',
    createdAt: new Date().toISOString(),
  };
}

function reduceDesignChanges(state, action) {
  const current = Array.isArray(state) ? state : [];
  if (!action || typeof action !== 'object') return current;
  switch (action.type) {
    case 'add':
      if (!action.change) return current;
      if (current.some((change) => (
        change.selector === action.change.selector &&
        change.type === action.change.type &&
        change.property === action.change.property &&
        change.oldValue === action.change.oldValue &&
        change.newValue === action.change.newValue
      ))) return current;
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

export { createDesignChange, reduceDesignChanges, uniqueDesignChanges };
