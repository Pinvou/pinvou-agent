// Closed host DATA and UI request ownership. This never grants trusted time,
// installation rights, cleanup completion or permission to configure the host.
export const TRUSTED_TIME_STATES = Object.freeze([
  'unsupported', 'setup_required', 'missing_policy', 'unknown',
  'source_unavailable', 'cleanup_pending', 'cleanup_complete', 'integrity_fault',
]);

/** @typedef {{title: string, reading: string, notRead: string, readStatus: string, refresh: string, refreshing: string, accepted: string, refreshBusy: string, networkNotice: string, states: Record<string, string>, errors: Record<string, string>}} TrustedTimeCopy */
/** @typedef {{protocolVersion: number, state: string, refreshing: boolean, refreshFailure: 'busy'|null}} TrustedTimeStatus */
/** @typedef {{inspectTrustedTime: () => Promise<unknown>, requestTrustedTimeRefresh: () => Promise<unknown>}} TrustedTimeAdapter */
/** @typedef {{status: TrustedTimeStatus|null, busy: boolean, accepted: boolean, error: 'busy'|'invalidResponse'|'unavailable'|null}} DiagnosticView */

/**
 * @param {unknown} value - Untrusted host DATA.
 * @param {string[]} keys - Required closed object keys.
 * @returns {value is Record<string, unknown>} Exact object shape.
 */
function exact(value, keys) {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
    && Object.keys(value).length === keys.length
    // Safari's supported floor does not include Object.hasOwn.
    && keys.every(key => Object.prototype.hasOwnProperty.call(value, key));
}

/**
 * @param {unknown} value - Untrusted host DATA.
 * @returns {TrustedTimeStatus} Closed diagnostic DATA.
 */
export function parseTrustedTimeStatus(value) {
  if (!exact(value, ['protocolVersion', 'state', 'refreshing', 'refreshFailure'])
      || value.protocolVersion !== 1 || typeof value.state !== 'string'
      || !TRUSTED_TIME_STATES.includes(value.state)
      || typeof value.refreshing !== 'boolean'
      || (value.refreshFailure !== null && value.refreshFailure !== 'busy')
      || (value.refreshing && value.refreshFailure !== null)) {
    throw new Error('TRUSTED_TIME_RESPONSE_INVALID');
  }
  return Object.freeze({ protocolVersion: 1, state: value.state, refreshing: value.refreshing, refreshFailure: value.refreshFailure });
}

/** @param {unknown} value - Untrusted refresh acknowledgement. */
function accepted(value) {
  if (!exact(value, ['protocolVersion', 'state'])
      || value.protocolVersion !== 1 || value.state !== 'accepted') {
    throw new Error('TRUSTED_TIME_RESPONSE_INVALID');
  }
}

/**
 * @param {unknown} error - Failure from the host bridge or local parser.
 * @returns {'busy'|'invalidResponse'|'unavailable'} Localized error selector.
 */
export function trustedTimeErrorCode(error) {
  const code = typeof error === 'string' ? error : (error instanceof Error ? error.message : null);
  if (code === 'HOST_BUSY') return 'busy';
  if (code === 'TRUSTED_TIME_RESPONSE_INVALID') return 'invalidResponse';
  return 'unavailable';
}

/**
 * @param {TrustedTimeStatus|null} status - Latest validated diagnostic observation.
 * @returns {boolean} UI availability, never host permission.
 */
export function canRefreshTrustedTime(status) {
  return !!status && !status.refreshing
    && ['unknown', 'source_unavailable', 'cleanup_pending', 'cleanup_complete', 'integrity_fault'].includes(status.state);
}

/**
 * One mounted view owns one serial request lane; the bridge also deduplicates
 * actual invokes across mounts. Disposal rejects every late UI publication.
 * @param {TrustedTimeAdapter} adapter - Semantic updater bridge; no raw invoke/global access.
 * @param {(view: DiagnosticView) => void} publish - Receives local view state, never host authority.
 * @returns {{read: () => Promise<void>, refresh: () => Promise<void>, dispose: () => void}} Read/refresh and lifetime disposal.
 */
export function createTrustedTimeDiagnostics(adapter, publish) {
  let disposed = false;
  /** @type {Promise<void>|null} */
  let pending = null;
  let timer = null;
  let pollsRemaining = 0;
  /** @type {DiagnosticView} */
  let view = { status: null, busy: false, accepted: false, error: null };
  /** @param {Partial<DiagnosticView>} patch - Changes to this mounted view. */
  const emit = patch => {
    view = { ...view, ...patch };
    if (!disposed) publish(view);
  };
  /**
   * @param {boolean} refresh - Whether this explicit action requests fresh time.
   * @returns {Promise<void>} The same in-flight local request lane.
   */
  const run = (refresh, polling = false) => {
    if (disposed) return Promise.resolve();
    if (pending) return pending;
    if (timer !== null) { clearTimeout(timer); timer = null; }
    if (!polling) pollsRemaining = 30;
    emit({ status: null, busy: true, accepted: false, error: null });
    pending = Promise.resolve().then(async () => {
      let inspecting = false;
      try {
        if (disposed) return;
        if (refresh) {
          accepted(await adapter.requestTrustedTimeRefresh());
          if (disposed) return;
          emit({ accepted: true });
        }
        inspecting = true;
        const status = parseTrustedTimeStatus(await adapter.inspectTrustedTime());
        emit({ status });
      } catch (error) {
        emit({ status: null, error: trustedTimeErrorCode(error) });
      } finally {
        pending = null;
        emit({ busy: false });
        // Poll only local diagnostics, never issue another refresh. Keep
        // observing long operations at a lower rate while this view is open.
        if (!disposed && (view.status?.refreshing || ((inspecting || refresh) && view.error === 'busy'))) {
          const delay = pollsRemaining > 0 ? 1000 : 10000;
          if (pollsRemaining > 0) pollsRemaining -= 1;
          timer = setTimeout(() => { timer = null; void run(false, true); }, delay);
        }
      }
    });
    return pending;
  };
  return {
    read: () => run(false),
    refresh: () => run(true),
    dispose: () => { disposed = true; if (timer !== null) clearTimeout(timer); timer = null; },
  };
}
