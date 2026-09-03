/**
 * format-number.js — React 侧可 ESM 导入的紧凑数字 / 字节格式化。
 *
 * plain-script bridge 侧的同类实现收敛在 shared/format-utils.js（window.PinvouFormatUtils，
 * 经典脚本无法 import，故此处与其保持同语义的独立副本）；此前 ChatView / CodexAcpView /
 * MonitorView / 各列表视图各自内联的实现统一改引本模块。
 */

// 与 format-utils.js fmtTok 同语义：≥1e6 → 1.0M，≥1e3 → 1.0k，其余取整。
// 非有限值统一回退占位符（各视图原先分别返回 '—' 或直接展示 NaN）。
export function formatCompactCount(n) {
  if (n == null || !Number.isFinite(n)) return '—';
  if (n >= 1e6) return `${(n / 1e6).toFixed(1)}M`;
  if (n >= 1e3) return `${(n / 1e3).toFixed(1)}k`;
  return String(Math.round(n));
}

// 字节数 → B/KB/MB/GB（1024 进制，一位小数）。missing 用于空值占位，
// 各视图原本分别返回 '' / '—' / '0 B'，迁移时按原显示传参。
export function formatBytes(bytes, { missing = '' } = {}) {
  if (bytes == null || !Number.isFinite(bytes) || bytes < 0) return missing;
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1048576) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1073741824) return `${(bytes / 1048576).toFixed(1)} MB`;
  return `${(bytes / 1073741824).toFixed(1)} GB`;
}
