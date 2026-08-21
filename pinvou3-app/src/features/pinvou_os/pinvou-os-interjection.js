function normalizedText(value) {
  return String(value == null ? '' : value).trim().replace(/\s+/g, ' ');
}

export function queuedMessageText(item) {
  if (!item || typeof item !== 'object') return '';
  return String(item.displayText == null ? item.text || '' : item.displayText).trim();
}

export function queuedMessagePresentations(queued) {
  return (Array.isArray(queued) ? queued : [])
    .map((item, index) => ({
      id: item && item.id != null ? item.id : `queued-${index}`,
      text: queuedMessageText(item),
    }))
    .filter(item => Boolean(item.text));
}

export function visibleUnqueuedUtterance(lastUtterance, queued) {
  const utterance = String(lastUtterance || '').trim();
  if (!utterance) return '';
  const normalizedUtterance = normalizedText(utterance);
  const duplicated = (Array.isArray(queued) ? queued : []).some(item => (
    normalizedText(queuedMessageText(item)) === normalizedUtterance
  ));
  return duplicated ? '' : utterance;
}
