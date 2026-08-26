/**
 * dependencies feature for the Tauri bridge.
 * Registered before bridge.js builds the backwards-compatible facade.
 */
(function (root) {
  // biome-ignore lint/suspicious/noRedundantUseStrict: verbatim classic-script artifact; strict mode is part of the payload
  "use strict";
  // biome-ignore lint/suspicious/noAssignInExpressions: registry bootstrap of the verbatim payload; splitting statements would diverge from the artifact
  const registry = root.__PINVOU_TAURI_BRIDGE_FEATURES__ = root.__PINVOU_TAURI_BRIDGE_FEATURES__ || {};
  registry["dependencies"] = function (context) {
    const state = context.state;
    const notify = context.notify;
    const invoke = context.invoke;
    const listen = context.listen;
    const bt = context.bt;
  // ── 依赖体检 ─────────────────────────────────────────────────────
  // 实时检测各文件解析能力(PDF/Office/OCR/压缩包/邮件)的系统依赖是否齐全,
  // 设置页展示缺失项 + 一键 apt 命令。后端 check_dependencies 不走缓存,装完可复检。
  async function checkDependencies() {
    if (state.depsChecking) return;
    state.depsChecking = true; state.depsInstallError = null; notify();
    try {
      state.deps = await invoke("check_dependencies");
    } catch { state.deps = []; }
    state.depsChecking = false; notify();
  }
  // 一键安装缺失依赖: 收集缺失项的包名 → 后端 pkexec apt 提权安装 → 装完实时重检。
  async function installDependencies() {
    const deps = state.deps || [];
    const missing = deps.filter(function (d) { return !d.installed; });
    if (!missing.length || state.depsInstalling) return;
    const pkgs = [];
    const actions = [];
    missing.forEach(function (d) {
      const action = String(d.install_action || "").trim();
      if (/^[a-z0-9_]+$/i.test(action) && !actions.includes(action)) {
        actions.push(action);
      }
      const parts = String(d.apt).trim().split(/\s+/).filter(Boolean);
      if (!parts.length || parts.some(function (p) { return !/^[a-z0-9][a-z0-9+.-]*$/i.test(p); })) {
        return;
      }
      parts.forEach(function (p) {
        if (!pkgs.includes(p)) pkgs.push(p);
      });
    });
    if (!pkgs.length && !actions.length) {
      state.depsInstallError = bt("depsInstallManual");
      notify();
      return;
    }
    state.depsInstalling = true; state.depsInstallError = null; state.depsInstallProgress = null; notify();
    // 订阅后端进度事件,实时刷新「正在安装 X (n/总数)…」,避免长尾包(libreoffice)
    // 全程只有静态「安装中…」像卡死。监听器注册是异步的,必须等其注册完成后再
    // 触发安装,否则安装开始前发出的第一条进度事件会因监听器未就绪而丢失。
    // 监听注册、安装、unlisten 与状态复位统一放在 try/catch/finally 中:
    // 监听注册失败也必须复位 depsInstalling,否则界面会一直停在「安装中」。
    let unlisten = null;
    try {
      unlisten = await listen("deps:install_progress", function (event) {
        state.depsInstallProgress = event.payload;
        notify();
      });
      await invoke("install_dependencies", { packages: pkgs, actions });
    } catch (e) {
      state.depsInstallError = String(e);
    } finally {
      // 反注册尽早做:无论成功失败(含监听注册失败)都取消订阅。
      if (typeof unlisten === "function") unlisten();
    }
    // 装完独立重检:重检返回前保持安装锁(depsInstalling 仍为 true)。
    // 若先解锁再异步重检,界面会基于旧缺失项快照重新启用安装按钮,
    // 用户再次点击会触发第二个并发安装(Homebrew/winget/模型下载都可能被重复触发)。
    try {
      state.deps = await invoke("check_dependencies"); // 成功或部分成功后均实时反映当前状态
    } catch { /* keep the last successful dependency snapshot */ }
    finally {
      // 最终 dependency snapshot 更新完成后再解锁并通知。
      state.depsInstalling = false; state.depsInstallProgress = null; notify();
    }
  }

    return {
      checkDependencies,
      installDependencies
    };
  };
})(window);
