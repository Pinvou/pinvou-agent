import { useEffect, useMemo, useRef, useState } from 'react';
import { canRefreshTrustedTime, createTrustedTimeDiagnostics } from './trusted-time-diagnostics.js';

/**
 * Diagnostic DATA only. The real host owns permissions, time and recovery.
 * @param {{copy: import('./trusted-time-diagnostics.js').TrustedTimeCopy, supported: boolean, updater: import('./trusted-time-diagnostics.js').TrustedTimeAdapter|undefined}} props - Localized copy, semantic capability and updater API.
 * @returns {import('react').ReactNode} Read-only status and explicit refresh.
 */
export default function TrustedTimeDiagnostics({ copy, supported, updater }) {
  const controller = useRef(/** @type {ReturnType<typeof createTrustedTimeDiagnostics>|null} */ (null));
  const source = useMemo(() => ({ updater, supported }), [updater, supported]);
  const [observation, setObservation] = useState(/** @type {{source: typeof source, view: import('./trusted-time-diagnostics.js').DiagnosticView}|null} */ (null));
  const available = supported && !!updater?.inspectTrustedTime && !!updater?.requestTrustedTimeRefresh;
  // A replacement adapter starts with no observation. Old results cannot appear
  // under the new source while its first asynchronous read is still pending.
  const view = observation?.source === source
    ? observation.view
    : { status: null, busy: available, accepted: false, error: null };
  useEffect(() => {
    if (!source.supported || !source.updater?.inspectTrustedTime || !source.updater?.requestTrustedTimeRefresh) return;
    const current = createTrustedTimeDiagnostics(source.updater, view => setObservation({ source, view }));
    controller.current = current;
    let disposed = false;
    // Start the external read after subscription setup. Cleanup can cancel a
    // not-yet-started read, including React's development remount probe.
    Promise.resolve().then(() => { if (!disposed) return current.read(); });
    return () => {
      disposed = true;
      current.dispose();
      if (controller.current === current) controller.current = null;
    };
  }, [source]);
  const reason = available ? view.status?.state : 'unsupported';
  return (
    <section aria-labelledby="trusted-time-diagnostics-title" aria-busy={available && view.busy} className="mb-6">
      <h3 id="trusted-time-diagnostics-title" className="px-3 mb-2 text-[12px] font-semibold text-[#8A8A8E] dark:text-[#8E8E93]">{copy.title}</h3>
      <div className="rounded-[18px] p-4 bg-white dark:bg-[#2C2C2E] text-[#1C1C1E] dark:text-[#F2F2F7]">
        <div role="status" aria-live="polite" aria-atomic="true" className="text-[14px] leading-6">
          {available && view.busy ? copy.reading : (reason ? copy.states[reason] : copy.notRead)}
          {available && view.status?.refreshing && <p>{copy.refreshing}</p>}
          {available && view.accepted && <p>{copy.accepted}</p>}
          {available && view.status?.refreshFailure === 'busy' && <p>{copy.refreshBusy}</p>}
        </div>
        {available && view.error && <p role="alert" className="mt-2 text-[12px] leading-5 text-[#EA4335]">{copy.errors[view.error]}</p>}
        <p className="mt-2 text-[12px] leading-5 text-gray-500 dark:text-gray-400">{copy.networkNotice}</p>
        <div className="mt-3 flex flex-wrap gap-2">
          <button type="button" onClick={() => controller.current?.read()} disabled={!available || view.busy}
            className="h-9 px-4 rounded-full text-[14px] bg-gray-100 dark:bg-white/10 disabled:opacity-50 disabled:cursor-not-allowed">{copy.readStatus}</button>
          <button type="button" onClick={() => controller.current?.refresh()} disabled={!available || view.busy || !canRefreshTrustedTime(view.status)}
            className="h-9 px-4 rounded-full text-[14px] bg-[#007AFF] text-white disabled:opacity-50 disabled:cursor-not-allowed">{copy.refresh}</button>
        </div>
      </div>
    </section>
  );
}
