(function () {
  "use strict";

  function previewEnabled(loc) {
    if (!loc || !loc.search) return false;
    try {
      var params = new URLSearchParams(loc.search);
      return params.get("mockUpdate") === "1";
    } catch (_) {
      return false;
    }
  }

  function previewInfo() {
    return {
      available: true,
      latest_version: "1.2.0",
      current_version: "1.1.0",
      notes: "优化了模型响应速度，并修复了部分工具调用失败的问题。",
    };
  }

  function updateInfoFor(bs, opts) {
    opts = opts || {};
    if (bs && bs.updateInfo && bs.updateInfo.available) return bs.updateInfo;
    if (opts.preview) return opts.previewInfo || previewInfo();
    return null;
  }

  function versionKey(info) {
    if (!info) return "";
    return String(info.latest_version || info.current_version || "");
  }

  function viewModel(bs, info, fallbackVersion) {
    if (!info) return { visible: false };
    var downloading = !!(bs && bs.updateDownloading);
    var ready = !!(bs && bs.updateReady);
    var progress = (bs && bs.updateProgress) || 0;
    var isWindowsUpdate = info && info.platform === "windows";
    var version = info.latest_version || info.current_version || fallbackVersion || "1.2.0";
    var label = ready && isWindowsUpdate
      ? "安装器已启动"
      : ready
      ? "立即重启"
      : (downloading ? (progress >= 100 ? "安装中..." : "下载中 " + progress + "%") : "重启升级");
    return {
      visible: true,
      downloading: downloading,
      ready: ready,
      progress: progress,
      version: version,
      label: label,
      action: ready ? (isWindowsUpdate ? "none" : "restart") : (downloading ? "none" : "download"),
      disabled: downloading || (ready && isWindowsUpdate),
      error: bs && bs.updateError ? String(bs.updateError) : "",
    };
  }

  window.UpdateNoticeLogic = {
    previewEnabled: previewEnabled,
    previewInfo: previewInfo,
    updateInfoFor: updateInfoFor,
    versionKey: versionKey,
    viewModel: viewModel,
  };
})();
