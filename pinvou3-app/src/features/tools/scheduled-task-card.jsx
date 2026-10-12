// Scheduled-task result card, lazy chunk (the vite.config
// lazyChunkContracts 'scheduledTaskCard' entry): the card renders only when
// a done app-automations tool item exists on the timeline, so it and its
// parser live outside the startup entry (the 500 kB main-entry budget;
// startup-budget policy in vite.config.mjs — new entry work is split or
// lazy-loaded).
import { parseScheduledTaskToolOutput, scheduledTaskPromptExcerpt } from './scheduled-task-card-logic.js';
import { unwrapMcpTextEnvelope } from './tool-common.jsx';

export const ScheduledTaskToolCard = ({ op, parsed, args, t }) => {
  const copy = t.uiScheduledTaskTool;
  const rrule = typeof args?.rrule === 'string' ? args.rrule.trim() : '';
  const excerpt = scheduledTaskPromptExcerpt(args);
  const doneLine = parsed.kind === 'pending' ? null : (
    <div className={parsed.kind === 'deleted' ? 'text-[#C5221F] dark:text-[#F28B82]' : 'text-[#137333] dark:text-[#93D5A6]'}>
      {copy[parsed.kind]}
      {parsed.taskId ? <span className="ml-1 font-mono text-[11px] opacity-70">{parsed.taskId}</span> : null}
    </div>
  );
  return (
    <div data-testid="scheduled-task-tool-card" className="my-1 text-[12px] leading-relaxed">
      {doneLine}
      {parsed.kind === 'pending' ? <div className="text-[#757575] dark:text-[#8E8E8E]">{copy.pending}</div> : null}
      {parsed.duplicate ? <div className="mt-0.5 text-[#757575] dark:text-[#8E8E8E]">{copy.duplicateNote}</div> : null}
      {/* Round-8 minor 11: the CARD carries the localized user copy — the
          server's model-directed note (with its "use a new
          idempotency_key" tail) stays in the raw transcript, not the
          timeline. */}
      {parsed.payloadMismatch ? <div className="mt-0.5 text-[#C5221F] dark:text-[#F28B82]" data-testid="scheduled-task-mismatch-note">{copy.mismatchNote}</div> : null}
      {/* Round-10 M2: the watcher writes task_name into the marker on EVERY
          successful update, so gating on parsed.taskName claimed "Renamed
          to X" for paused/rrule-only updates too. The row fires only when
          the REQUEST actually carried a name; the shown value stays the
          applied outcome (parsed.taskName). */}
      {typeof args?.name === 'string' && args.name.trim() !== '' && parsed.kind === 'updated' ? <div className="mt-0.5 text-[#757575] dark:text-[#8E8E8E]"><span data-testid="scheduled-task-updated-name">{copy.updatedName}</span> {parsed.taskName}</div> : null}
      {rrule ? <div className="mt-0.5 break-all font-mono text-[11px] text-[#757575] dark:text-[#8E8E8E]">{rrule}</div> : null}
      {excerpt ? <div className="mt-0.5 text-[#757575] dark:text-[#8E8E8E]">{copy.promptLabel}: {excerpt}</div> : null}
      {op === 'deleted' && parsed.kind !== 'pending'
        ? <div className="mt-0.5 text-[#757575] dark:text-[#8E8E8E]">{copy.deletedNote}</div> : null}
    </div>
  );
};

// The done-branch body, moved verbatim from tool-renderers.jsx: parse
// THROUGH the unwrap (round-9 BLOCKER 1: production tool-result text
// carries the serialized {"content":[{"type":"text","text":"{...}"}]}
// wrapper — parsing the raw output returned null for EVERY successful
// create/update/delete and dead-carded the feature; the sibling
// weather/stock cards unwrap first for exactly this reason). A parse miss
// renders the caller-provided fallback, preserving the old fall-through to
// the default raw output view.
export const ScheduledTaskToolResult = ({ op, out, args, t, fallback }) => {
  const parsed = parseScheduledTaskToolOutput(unwrapMcpTextEnvelope(out));
  if (!parsed) return fallback;
  return <ScheduledTaskToolCard op={op} parsed={parsed} args={args} t={t} />;
};
