/**
 * artifacts feature for the Tauri bridge.
 * Registered before bridge.js builds the backwards-compatible facade.
 */
(function (root) {
  "use strict";
  var registry = root.__PINVOU_TAURI_BRIDGE_FEATURES__ = root.__PINVOU_TAURI_BRIDGE_FEATURES__ || {};
  registry["artifacts"] = function (context) {
    var state = context.state;
    var notify = context.notify;
    var invoke = context.invoke;
    var bt = context.bt;
    var addSystemItem = context.addSystemItem;
    var dialogOpen = context.dialogOpen;
    var basename = context.basename;
    var isDeliverable = context.isDeliverable;
    var isAbsPath = context.isAbsPath;
    var sessionStates = context.sessionStates;
    var TAURI = context.TAURI;
    var listen = context.listen;
    var attachIdSeq = 0;
  // ── 产物面板 ─────────────────────────────────────────────────────
  function artifactInfo(path) { return invoke("artifact_info", { path: path }); }
  function readArtifactText(path) { return invoke("read_artifact_text", { path: path }); }
  function writeArtifactText(path, content) { return invoke("write_artifact_text", { path: path, content: content }); }
  function readArtifactImageB64(path) { return invoke("read_artifact_image_b64", { path: path }); }
  // pptx 封面缩略图：读 docProps/thumbnail.jpeg → data URL（无则 null）。本地数据、无外链。
  function readArtifactThumbnail(path) { return invoke("read_artifact_thumbnail", { path: path }).catch(function () { return null; }); }
  function renderArtifactVisual(path) { return invoke("render_artifact_visual", { path: path }); }
  function openContainingFolder(path) { return invoke("open_containing_folder", { path: path }).catch(function (e) { addSystemItem(bt("openFailed") + e); }); }
  function revealSessionFolder(sessionId) { return invoke("reveal_session_folder", { sessionId: sessionId }).catch(function (e) { addSystemItem(bt("openFailed") + e); }); }
  function openScheduledTaskFolder(automationId) { return invoke("open_scheduled_task_folder", { automationId: automationId }).catch(function (e) { addSystemItem(bt("openFailed") + e); }); }
  function openInSystem(path) { return invoke("open_in_system", { path: path }).catch(function (e) { addSystemItem(bt("openFailed") + e); }); }
  // 仅放白名单 URL (metaso.cn / open.bochaai.com),后端 open_external_url 强制校验。
  function openExternalUrl(url) { return invoke("open_external_url", { url: url }).catch(function (e) { addSystemItem(bt("openFailed") + e); }); }
  // 奏折宝箱:列 run 成品文档(deliverables/ 下文件,二进制成品排前)
  function listDeliverables(projectDir) {
    return invoke("list_deliverables", { projectDir: projectDir }).catch(function () { return []; });
  }
  function deliverableCategory(path) {
    var ext = (String(path || "").split(".").pop() || "").toLowerCase();
    if (ext === "html" || ext === "htm" || ext === "mhtml" || ext === "mht") return "web";
    if (ext === "ppt" || ext === "pptx" || ext === "odp" || ext === "dps") return "ppt";
    if (["png", "jpg", "jpeg", "gif", "webp", "svg", "bmp", "heic"].indexOf(ext) >= 0) return "img";
    return "doc";
  }
  function sessionTitleById(sid) {
    var m = state.sessions.find(function (s) { return s.id === sid; });
    return (m && m.title) || "";
  }
  function currentMemoryArtifacts() {
    var rows = [];
    function addFrom(sid, arts) {
      (arts || []).forEach(function (a) {
        var path = a && a.path;
        if (!path || !isDeliverable(path)) return;
        rows.push({ path: path, sessionId: sid || state.activeSessionId, source: sessionTitleById(sid || state.activeSessionId), name: basename(path) });
      });
    }
    addFrom(state.activeSessionId, state.artifacts);
    Object.keys(sessionStates).forEach(function (sid) { addFrom(sid, sessionStates[sid] && sessionStates[sid].artifacts); });
    return rows;
  }
  // 跨会话产出物索引:磁盘 session JSON 为主,再合并当前内存工作集。
  // 新产物在 chat:done/save_session_artifacts 前也能立刻出现在「本地知识 → 产出物」。
  async function listDeliverableIndex() {
    var disk = await invoke("list_deliverable_index").catch(function () { return []; });
    var byPath = {};
    (disk || []).forEach(function (x) { if (x && x.path) byPath[x.path] = x; });
    var mem = currentMemoryArtifacts().filter(function (x) { return x.path && !byPath[x.path]; });
    var hydrated = await Promise.all(mem.map(async function (x) {
      var path = x.path;
      if (!isAbsPath(path) && x.sessionId) {
        try {
          var ws = await invoke("list_workspace_files", { sessionId: x.sessionId });
          var bn = basename(path);
          var resolved = (ws || []).find(function (p) { return basename(p) === bn; });
          if (resolved) path = resolved;
        } catch (_) {}
      }
      var info = null;
      try { info = await artifactInfo(path); } catch (_) {}
      var ext = (String(path).split(".").pop() || "").toLowerCase();
      return {
        name: x.name || basename(path),
        path: path,
        ext: ext,
        category: deliverableCategory(path),
        sessionId: x.sessionId || "",
        source: x.source || sessionTitleById(x.sessionId) || "",
        mtime: info && info.modified ? info.modified : 0,
        size: info && info.size ? info.size : 0,
      };
    }));
    hydrated.forEach(function (x) { if (x && x.path) byPath[x.path] = x; });
    return Object.keys(byPath).map(function (p) { return byPath[p]; }).sort(function (a, b) {
      return (b.mtime || 0) - (a.mtime || 0) || String(a.name || "").localeCompare(String(b.name || ""));
    });
  }
  // 外部打开产物：HTML 走 Tauri 独立窗口（绕沙箱），其他走系统应用。
  // sessionId = 卡片携带的产物所属 session。后端 resolve_artifact_path 用它(而非全局
  // active_id)解析相对路径 —— 切回「有 buffer」的会话后端 active 不更新,只有卡片自带
  // session 才解析得准(否则相对路径被拼到错的 workspace 报 not a file)。绝对路径无视它。
  function openArtifactExternal(path, sessionId) {
    var ext = (String(path).split(".").pop() || "").toLowerCase();
    var cmd = (ext === "html" || ext === "htm") ? "open_artifact_window" : "open_in_system";
    return invoke(cmd, { path: path, sessionId: sessionId || null }).catch(function (e) { addSystemItem(bt("openFailed") + e); });
  }

  // ── 附件 ────────────────────────────────────────────────────────
  async function addAttachmentByPath(path) {
    var id = ++attachIdSeq;
    var att = { id: id, basename: basename(path), status: "parsing", result: null, error: null };
    state.attachments.push(att); notify();
    try {
      var result = await invoke("ingest_file", { path: path });
      att.status = "ready"; att.result = result;
    } catch (e) { att.status = "error"; att.error = String(e); }
    notify();
  }
  var recentDroppedPaths = {};
  var DROP_DEDUP_MS = 1500;
  function dropPathKey(path) {
    return String(path || "").toLowerCase();
  }
  function droppedFilePaths(payload) {
    if (!payload) return [];
    if (Array.isArray(payload)) return payload.filter(Boolean);
    if (payload.payload) return droppedFilePaths(payload.payload);
    if (payload.type && payload.type !== "drop") return [];
    if (Array.isArray(payload.paths)) return payload.paths.filter(Boolean);
    if (Array.isArray(payload.files)) return payload.files.filter(Boolean);
    if (typeof payload.path === "string") return [payload.path];
    if (typeof payload === "string") return [payload];
    return [];
  }
  async function addDroppedAttachments(paths) {
    var now = Date.now();
    var seen = {};
    var list = (paths || []).filter(function (p) {
      var key = dropPathKey(p);
      if (!p || seen[key]) return false;
      seen[key] = true;
      if (recentDroppedPaths[key] && now - recentDroppedPaths[key] < DROP_DEDUP_MS) return false;
      recentDroppedPaths[key] = now;
      return true;
    });
    Object.keys(recentDroppedPaths).forEach(function (key) {
      if (now - recentDroppedPaths[key] > DROP_DEDUP_MS * 4) delete recentDroppedPaths[key];
    });
    for (var i = 0; i < list.length; i++) {
      await addAttachmentByPath(list[i]);
    }
  }
  function initAttachmentDrop() {
    if (initAttachmentDrop.done) return;
    initAttachmentDrop.done = true;

    var currentWindow = TAURI.window && TAURI.window.getCurrentWindow ? TAURI.window.getCurrentWindow() : null;
    if (currentWindow && typeof currentWindow.onDragDropEvent === "function") {
      currentWindow.onDragDropEvent(function (event) {
        var paths = droppedFilePaths(event);
        if (paths.length) addDroppedAttachments(paths);
      }).catch(function (e) { console.warn("[attachment] drag-drop listener failed", e); });
    }

    listen("tauri://file-drop", function (event) {
      var paths = droppedFilePaths(event);
      if (paths.length) addDroppedAttachments(paths);
    }).catch(function () {});
    listen("tauri://drag-drop", function (event) {
      var paths = droppedFilePaths(event);
      if (paths.length) addDroppedAttachments(paths);
    }).catch(function () {});

    document.addEventListener("dragover", function (e) {
      if (e.dataTransfer && Array.prototype.indexOf.call(e.dataTransfer.types || [], "Files") >= 0) {
        e.preventDefault();
        e.dataTransfer.dropEffect = "copy";
      }
    });
    document.addEventListener("drop", function (e) {
      var files = e.dataTransfer && e.dataTransfer.files;
      if (!files || files.length === 0) return;
      e.preventDefault();
      var paths = [];
      for (var i = 0; i < files.length; i++) {
        if (files[i] && files[i].path) paths.push(files[i].path);
      }
      if (paths.length) addDroppedAttachments(paths);
    });
  }
  async function addPasteImage(filename, bytes) {
    try {
      var path = await invoke("save_paste_image", { filename: filename, bytes: bytes });
      await addAttachmentByPath(path);
    } catch (e) { addSystemItem(bt("pasteImageFailed") + e); }
  }
  function removeAttachment(id) {
    state.attachments = state.attachments.filter(function (a) { return a.id !== id; });
    notify();
  }
  function clearAttachments() { state.attachments = []; }
  // 打开系统文件选择器并摄入为附件
  async function pickAndAttach() {
    if (!dialogOpen) { addSystemItem(bt("filePickUnavailable")); return; }
    try {
      var selected = await dialogOpen({ multiple: true });
      if (!selected) return;
      var paths = Array.isArray(selected) ? selected : [selected];
      for (var i = 0; i < paths.length; i++) { await addAttachmentByPath(paths[i]); }
    } catch (e) { addSystemItem(bt("filePickFailed") + e); }
  }
  initAttachmentDrop();


    return {
      artifactInfo: artifactInfo,
      readArtifactText: readArtifactText,
      writeArtifactText: writeArtifactText,
      readArtifactImageB64: readArtifactImageB64,
      readArtifactThumbnail: readArtifactThumbnail,
      renderArtifactVisual: renderArtifactVisual,
      openContainingFolder: openContainingFolder,
      revealSessionFolder: revealSessionFolder,
      openScheduledTaskFolder: openScheduledTaskFolder,
      openInSystem: openInSystem,
      openArtifactExternal: openArtifactExternal,
      listDeliverables: listDeliverables,
      listDeliverableIndex: listDeliverableIndex,
      openExternalUrl: openExternalUrl,
      addAttachmentByPath: addAttachmentByPath,
      addPasteImage: addPasteImage,
      removeAttachment: removeAttachment,
      clearAttachments: clearAttachments,
      pickAndAttach: pickAndAttach
    };
  };
})(window);
