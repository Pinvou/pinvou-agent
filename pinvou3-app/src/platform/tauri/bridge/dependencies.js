/**
 * dependencies feature for the Tauri bridge.
 * Registered before bridge.js builds the backwards-compatible facade.
 */
(function (root) {
  "use strict";
  var registry = root.__PINVOU_TAURI_BRIDGE_FEATURES__ = root.__PINVOU_TAURI_BRIDGE_FEATURES__ || {};
  registry["dependencies"] = function (context) {
    var state = context.state;
    var notify = context.notify;
    var invoke = context.invoke;
    var bt = context.bt;
  // ── 依赖体检 ─────────────────────────────────────────────────────
  // 实时检测各文件解析能力(PDF/Office/OCR/压缩包/邮件)的系统依赖是否齐全,
  // 设置页展示缺失项 + 一键 apt 命令。后端 check_dependencies 不走缓存,装完可复检。
  async function checkDependencies() {
    if (state.depsChecking) return;
    state.depsChecking = true; state.depsInstallError = null; notify();
    try {
      state.deps = await invoke("check_dependencies");
    } catch (e) { state.deps = []; }
    state.depsChecking = false; notify();
  }
  // 一键安装缺失依赖: 收集缺失项的包名 → 后端 pkexec apt 提权安装 → 装完实时重检。
  async function installDependencies() {
    var deps = state.deps || [];
    var missing = deps.filter(function (d) { return !d.installed; });
    if (!missing.length || state.depsInstalling) return;
    var pkgs = [];
    missing.forEach(function (d) {
      var parts = String(d.apt).trim().split(/\s+/).filter(Boolean);
      if (!parts.length || !parts.every(function (p) { return /^[a-z0-9][a-z0-9+.-]*$/i.test(p); })) {
        return;
      }
      parts.forEach(function (p) {
        if (pkgs.indexOf(p) < 0) pkgs.push(p);
      });
    });
    if (!pkgs.length) {
      state.depsInstallError = bt("depsInstallManual");
      notify();
      return;
    }
    state.depsInstalling = true; state.depsInstallError = null; notify();
    try {
      await invoke("install_dependencies", { packages: pkgs });
      state.deps = await invoke("check_dependencies"); // 装完实时重检,缺失项应清空
    } catch (e) {
      state.depsInstallError = String(e);
    }
    state.depsInstalling = false; notify();
  }

    return {
      checkDependencies: checkDependencies,
      installDependencies: installDependencies
    };
  };
})(window);
