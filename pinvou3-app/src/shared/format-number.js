/**
 * format-number.js — compact-count / byte formatting importable as ESM from React code.
 *
 * The plain-script bridge keeps its counterpart in shared/format-utils.js (window.PinvouFormatUtils —
 * classic scripts cannot import, so this module stays a semantically-equal standalone copy); the
 * inline copies previously forked across ChatView / CodexAcpView / MonitorView / list views now import this module.
 */

// Same semantics as fmtTok in format-utils.js: >=1e6 -> 1.0M, >=1e3 -> 1.0k, otherwise rounded.
// Non-finite values uniformly fall back to the missing sentinel (views previously returned '—' or rendered raw NaN).
export function formatCompactCount(n) {
  if (n == null || !Number.isFinite(n)) return '—';
  if (n >= 1e6) return `${(n / 1e6).toFixed(1)}M`;
  if (n >= 1e3) return `${(n / 1e3).toFixed(1)}k`;
  return String(Math.round(n));
}

// Bytes -> B/KB/MB/GB (1024-based, one decimal). `missing` is the empty-value sentinel;
// views originally returned '' / '—' / '0 B' respectively — pass each site's original display when migrating.
export function formatBytes(bytes, { missing = '' } = {}) {
  if (bytes == null || !Number.isFinite(bytes) || bytes < 0) return missing;
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1048576) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1073741824) return `${(bytes / 1048576).toFixed(1)} MB`;
  return `${(bytes / 1073741824).toFixed(1)} GB`;
}
