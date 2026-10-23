// Shared presentation of scheduled-message destinations in task detail.
export function ScheduledMessageTarget({ target, label, mutedClass, bodyClass }) {
  if (!target) return null;
  return (
    <div data-testid="scheduled-detail-target" className="flex items-center justify-between gap-3">
      <span className={mutedClass}>{label}</span>
      <span className={`truncate text-right font-mono text-[12px] font-medium ${bodyClass}`}>{target}</span>
    </div>
  );
}

export function scheduledRunDescription(item, kind, copy, statusLabel) {
  return item.error
    || (item.sessionId ? copy.viewRunResult : null)
    || (item.deliveredTarget ? copy.deliveredTo(item.deliveredTarget) : null)
    || (['failed', 'canceled'].includes(item.status) ? statusLabel(item.status) : null)
    || (kind === 'session_message' ? copy.noDeliveryTarget : copy.noRunSession);
}
