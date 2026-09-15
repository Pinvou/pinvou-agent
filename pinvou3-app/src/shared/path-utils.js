// Path utils: the codex view / tool store / settings / reader each previously inlined split(/[\\/]/).pop()
// basename extraction with drifting trailing-slash and Windows-separator handling; consolidated here.

/**
 * Returns the last path segment. Both '\\' and '/' are separators (Windows paths).
 * @param {string|null} path - file / directory path; null/undefined is treated as ''
 * @param {{ collapseTrailing?: boolean, fallback?: string }} [opts]
 *   collapseTrailing — strip trailing separators first (a directory path yields its own name, e.g. /a/b/ -> b);
 *   fallback — sentinel used when the result is '' (e.g. t.unknownDirectory).
 */
export function pathBasename(path, { collapseTrailing = false, fallback = '' } = {}) {
  let value = String(path || '');
  if (collapseTrailing) {
    let end = value.length;
    while (end > 0 && (value[end - 1] === '/' || value[end - 1] === '\\')) end -= 1;
    value = value.slice(0, end);
  }
  const segments = value.split(/[\\/]/);
  return segments[segments.length - 1] || fallback;
}
