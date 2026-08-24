(function () {
  // biome-ignore lint/suspicious/noRedundantUseStrict: classic script 直拷产物,严格模式是载荷
  "use strict";

  function previewEnabled(loc) {
    if (!loc || !loc.search) return false;
    try {
      const params = new URLSearchParams(loc.search);
      return params.get("mockUpdate") === "1";
    } catch {
      return false;
    }
  }

  function previewInfo() {
    // 根据当前运行平台派生预览平台,让开发者在本机看到的更新卡片行为与真实一致。
    // navigator 可能在非浏览器环境(test vm)缺失 → 回退 linux。
    const nav = (typeof navigator === "undefined") ? null : navigator;
    const platformStr = (((nav && nav.platform) || "") + " " + ((nav && nav.userAgent) || "")).toLowerCase();
    let platform = "linux";
    if (/mac|darwin/.test(platformStr)) platform = "macos";
    else if (/win/.test(platformStr)) platform = "windows";
    return {
      available: true,
      latest_version: "1.2.0",
      current_version: "1.1.0",
      platform,
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

  function text(labels, key, fallback) {
    labels = labels || {};
    return labels[key] || fallback;
  }

  function viewModel(bs, info, fallbackVersion, labels) {
    if (!info) return { visible: false };
    const downloading = !!(bs && bs.updateDownloading);
    const ready = !!(bs && bs.updateReady);
    const progress = (bs && bs.updateProgress) || 0;
    const isWindowsUpdate = info && info.platform === "windows";
    const isMacUpdate = info && info.platform === "macos";
    // macOS 安装是原地 bundle 替换(hdiutil attach + cp -R),后端返回 Ok(false)=安装完成
    // 进程未退出。与 Linux 同型:app.restart() 按路径 exec,bundle 被替换后该路径已指向
    // 新文件,tauri-bridge 自动触发 restartApp。三者都显示 ready=true 的"立即重启"按钮,
    // 但 macOS/Linux 安装后由 tauri-bridge 自动重启,Windows 是 MSI 安装器接管(Ok(true)→exit)。
    const restartAfterInstall = info && (info.platform === "linux" || isMacUpdate);
    // 仅 Windows 启动外部 MSI 安装器(ready 时显示"安装器已启动",action=none)。
    const installerLaunch = isWindowsUpdate;
    const version = info.latest_version || info.current_version || fallbackVersion || "1.2.0";
    const label = ready && installerLaunch
      ? text(labels, "updateInstallerStarted", "安装器已启动")
      : ready
      ? text(labels, "restartNow", "立即重启")
      : (downloading
        ? (progress >= 100 ? text(labels, "installing", "安装中...") : text(labels, "downloading", "下载中") + " " + progress + "%")
        : (restartAfterInstall ? text(labels, "downloadInstallRestart", "升级并重启") : text(labels, "downloadInstall", "下载并安装")));
    return {
      visible: true,
      downloading,
      ready,
      progress,
      version,
      label,
      restartAfterInstall,
      action: ready ? (installerLaunch ? "none" : "restart") : (downloading ? "none" : "download"),
      disabled: downloading || (ready && installerLaunch),
      error: bs && bs.updateError ? String(bs.updateError) : "",
    };
  }

  window.UpdateNoticeLogic = {
    previewEnabled,
    previewInfo,
    updateInfoFor,
    versionKey,
    viewModel,
  };
})();
