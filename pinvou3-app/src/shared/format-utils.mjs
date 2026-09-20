/**
 * format-utils.mjs — ESM sibling of format-utils.js.
 *
 * format-utils.js is a classic UMD script (window.PinvouFormatUtils) consumed
 * by the plain-script bridges, which cannot `import`. ESM modules therefore
 * re-declared identical helpers (e.g. background-tasks.js formatElapsedMs);
 * this module is the single ESM export surface for those shared formatters.
 * window.PinvouFormatUtils.fmtDuration stays as-is for the bridges: it takes
 * seconds and keeps its own display contract.
 */

// 耗时格式化：秒级以下显示秒，分钟级显示 "Xm Ys"，更长显示 "Xh Ym"。
export function formatElapsedMs(ms) {
  const totalSeconds = Math.max(0, Math.floor((Number(ms) || 0) / 1000));
  const hours = Math.floor(totalSeconds / 3600);
  const minutes = Math.floor((totalSeconds % 3600) / 60);
  const seconds = totalSeconds % 60;
  if (hours > 0) return `${hours}h ${minutes}m`;
  if (minutes > 0) return `${minutes}m ${seconds}s`;
  return `${seconds}s`;
}
