export function createAcpSessionOperationTracker(initialSessionId = null) {
  let activeSessionId = initialSessionId || null;
  let sequence = 0;
  let current = null;
  const isCurrent = operation => Boolean(
    operation
      && current
      && operation.token === current.token
      && operation.sessionId === activeSessionId,
  );

  return Object.freeze({
    switchSession(sessionId) {
      const nextSessionId = sessionId || null;
      if (nextSessionId === activeSessionId) return false;
      activeSessionId = nextSessionId;
      sequence += 1;
      current = null;
      return true;
    },
    begin(sessionId, key) {
      const operationSessionId = sessionId || null;
      if (operationSessionId !== activeSessionId) return null;
      const operation = Object.freeze({
        token: ++sequence,
        sessionId: operationSessionId,
        key,
      });
      current = operation;
      return operation;
    },
    isCurrent,
    finish(operation) {
      if (!isCurrent(operation)) return false;
      current = null;
      return true;
    },
  });
}

function partitionOwnedItems(items, ownedItems, keyOf) {
  const counts = new Map();
  for (const item of ownedItems || []) {
    const key = keyOf(item);
    counts.set(key, (counts.get(key) || 0) + 1);
  }
  const owned = [];
  const remaining = [];
  for (const item of items || []) {
    const key = keyOf(item);
    const count = counts.get(key) || 0;
    if (count > 0) {
      owned.push(item);
      counts.set(key, count - 1);
    } else {
      remaining.push(item);
    }
  }
  return { owned, remaining };
}

export function transferAcpDraftItems(drafts, sourceKey, targetKey, ownedItems, keyOf) {
  if (!sourceKey || !targetKey || sourceKey === targetKey || !(ownedItems || []).length) {
    return drafts;
  }
  const identify = keyOf || (item => item?.id ?? item);
  const { owned, remaining } = partitionOwnedItems(drafts[sourceKey], ownedItems, identify);
  if (!owned.length) return drafts;
  const next = {
    ...drafts,
    [targetKey]: [...(drafts[targetKey] || []), ...owned],
  };
  if (remaining.length) next[sourceKey] = remaining;
  else delete next[sourceKey];
  return next;
}

export function removeAcpDraftItems(drafts, ownerKey, ownedItems, keyOf) {
  if (!ownerKey || !(ownedItems || []).length) return drafts;
  const identify = keyOf || (item => item?.id ?? item);
  const { owned, remaining } = partitionOwnedItems(drafts[ownerKey], ownedItems, identify);
  if (!owned.length) return drafts;
  const next = { ...drafts };
  if (remaining.length) next[ownerKey] = remaining;
  else delete next[ownerKey];
  return next;
}
