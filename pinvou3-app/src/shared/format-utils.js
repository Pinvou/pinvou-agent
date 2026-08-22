/**
 * format-utils.js — 供 plain-script bridge 共用的监控指标格式化函数。
 *
 * 此前 fmtMiB / fmtKiB / fmtDuration / fmtTok 在 platform/web/bridge.js 与
 * platform/tauri/bridge/monitor.js 中逐字重复，现收敛到本文件，
 * 由两处统一引用 window.PinvouFormatUtils。
 */
(function (root) {
  "use strict";

  function fmtMiB(mib) {
    if (mib == null) return "—";
    return mib >= 1024 ? (mib / 1024).toFixed(1) + " GB" : mib + " MB";
  }
  function fmtKiB(kib) {
    if (kib == null) return "—";
    if (kib >= 1024 * 1024) return (kib / 1024 / 1024).toFixed(1) + " GB";
    if (kib >= 1024) return (kib / 1024).toFixed(0) + " MB";
    return kib + " KB";
  }
  function fmtDuration(secs) {
    if (secs == null || secs < 0) return "—";
    var h = Math.floor(secs / 3600), m = Math.floor((secs % 3600) / 60);
    if (h > 0) return h + "h " + m + "m";
    if (m > 0) return m + "m " + (secs % 60) + "s";
    return secs + "s";
  }
  function fmtTok(n) {
    if (n == null) return "—";
    if (n >= 1e6) return (n / 1e6).toFixed(1) + "M";
    if (n >= 1e3) return (n / 1e3).toFixed(1) + "k";
    return String(Math.round(n));
  }

  root.PinvouFormatUtils = Object.freeze({
    fmtMiB: fmtMiB,
    fmtKiB: fmtKiB,
    fmtDuration: fmtDuration,
    fmtTok: fmtTok,
  });
})(typeof window !== "undefined" ? window : this);
