/**
 * updater feature for the Tauri bridge.
 * Registered before bridge.js builds the backwards-compatible facade.
 */
(function (root) {
  "use strict";
  var registry = root.__PINVOU_TAURI_BRIDGE_FEATURES__ = root.__PINVOU_TAURI_BRIDGE_FEATURES__ || {};
  registry["updater"] = function (context) {
    var state = context.state;
    var notify = context.notify;
    var invoke = context.invoke;
    var refreshHistoryList = context.refreshHistoryList;
    var listen = context.listen;
    var publishRemoteLiveSnapshot = context.publishRemoteLiveSnapshot;
    var getBuffer = context.getBuffer;
    var bt = context.bt;
  // ── 应用内升级 ───────────────────────────────────────────────────
  // 链路: check_for_update(对比服务器 latest.json) → download_update(流式下载+sha256,
  // 进度走 update:progress 事件) → install_update(pkexec apt) → restart_app。
  listen("update:progress", function (e) {
    var p = e.payload || {};
    state.updateProgress = p.total ? Math.round((p.downloaded / p.total) * 100) : 0;
    notify();
  });
  listen("remote_control:status", function (e) {
    state.remoteControl = Object.assign({}, state.remoteControl, e.payload || {});
    notify();
  });
  listen("remote_control:snapshot_requested", function (e) {
    var sid = e && e.payload && e.payload.session_id;
    if (!sid) return;
    publishRemoteLiveSnapshot(sid).catch(function () {});
  });
  listen("remote_control:session_created", function (e) {
    var s = e && e.payload && e.payload.session;
    if (s && s.id) {
      getBuffer(s.id);
      if (!state.sessions.some(function (item) { return item.id === s.id; })) {
        state.sessions.unshift({
          id: s.id,
          title: s.title || bt("newChatFallbackTitle"),
          updated_at: s.updated_at || "",
          message_count: s.message_count || 0,
        });
      }
      notify();
    }
    refreshHistoryList().then(function () { notify(); }).catch(function () {});
  });
  async function loadAppVersion() {
    try {
      state.appVersion = await invoke("get_app_version");
    } catch (_) {}
  }
  // 启动静默检查: 失败全吞(网络差/更新源挂了不打扰用户)。结果不管新旧都存——
  // available 驱动红点,current_version 给设置页显示当前版本用。
  async function checkForUpdateSilently() {
    try {
      var info = await invoke("check_for_update");
      if (info && info.current_version) state.appVersion = info.current_version;
      if (info) { state.updateInfo = info; notify(); }
    } catch (e) { /* 静默 */ }
  }
  // 设置页手动检查: 错误和「已是最新」都要反馈。
  async function checkForUpdate() {
    state.updateChecking = true; state.updateCheckError = null; notify();
    try {
      var info = await invoke("check_for_update");
      if (info && info.current_version) state.appVersion = info.current_version;
      state.updateInfo = info;
      if (!info.available) state.updateCheckError = "latest"; // 前端按 i18n 显示「已是最新」
    } catch (e) {
      state.updateCheckError = String(e);
    }
    state.updateChecking = false; notify();
  }
  // 下载+安装一条龙: Linux 下载 deb 后 pkexec apt 并自动重启;Windows 下载 zip 后解析 MSI,
  // 安装器启动成功后 Windows 退出当前进程；Linux/macOS 在安装后由前端重启。
  async function downloadAndInstallUpdate() {
    if (!state.updateInfo || !state.updateInfo.available || state.updateDownloading) return false;
    // 入口捕获发起时的更新信息：下载/安装期间静默检查可能替换 updateInfo，
    // download/install 参数须仍指向发起时的版本（审计）。当前基线的 install_update
    // 为 unsupported 桩（info 未参与行为），此配对为面向完整平台实现的防御性修复。
    // invoke 文本保持原样。
    var info = state.updateInfo;
    var shouldRestartAfterInstall = info.platform !== "windows";
    var installed = false;
    state.updateDownloading = true; state.updateCancelling = false;
    state.updateProgress = 0; state.updateError = null; notify();
    try {
      var downloadResult = await invoke("download_update", { info: state.updateInfo });
      state.updateProgress = 100; notify();
      state.updateInfo = info; // 复原：install 用发起时的版本元数据，不随静默检查漂移
      if (downloadResult && typeof downloadResult === "object" && downloadResult.installer_path) {
        await invoke("install_update", { installerPath: downloadResult.installer_path, info: state.updateInfo });
      } else {
        await invoke("install_update", { debPath: downloadResult, info: state.updateInfo });
      }
      state.updateReady = true;
      installed = true;
    } catch (e) {
      // 用户主动取消下载时后端返回「已取消下载」,当正常处理不弹错误
      if (state.updateCancelling) state.updateProgress = 0;
      else state.updateError = String(e);
    }
    state.updateDownloading = false; state.updateCancelling = false; notify();
    if (installed && shouldRestartAfterInstall) restartApp();
    return installed;
  }
  // 取消进行中的下载: 置前端标志 + 通知后端中断下载循环。仅下载阶段有效;
  // 已进入 install(pkexec/apt)则无效(系统接管,装一半不能停)。
  function cancelUpdate() {
    if (!state.updateDownloading || state.updateCancelling) return;
    state.updateCancelling = true; notify();
    invoke("cancel_download").catch(function () { /* 忽略,下载循环超时也会退 */ });
  }
  function restartApp() {
    invoke("restart_app").catch(function () { /* restart 成功不会返回 */ });
  }
  function reportPendingUpdateResult() {
    invoke("report_pending_update_result").catch(function () { /* 静默重试,不阻塞启动 */ });
  }

    return {
      loadAppVersion: loadAppVersion,
      checkForUpdateSilently: checkForUpdateSilently,
      checkForUpdate: checkForUpdate,
      downloadAndInstallUpdate: downloadAndInstallUpdate,
      cancelUpdate: cancelUpdate,
      restartApp: restartApp,
      reportPendingUpdateResult: reportPendingUpdateResult
    };
  };
})(window);
