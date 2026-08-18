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
    var discardManagedAttachment = context.discardManagedAttachment || function () { return Promise.resolve(); };
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
  // ACP 消息/产物预览里由用户亲自点击的 HTTP(S) 外链；后端与工具白名单入口分开校验。
  function openUserExternalUrl(url) { return invoke("open_user_external_url", { url: url }).catch(function (e) { addSystemItem(bt("openFailed") + e); }); }
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
  // 新产物在 chat:done/save_session_artifacts 前也能立刻出现在「产出物」一级入口。
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
  function downloadArtifact(path, sessionId) {
    return openArtifactExternal(path, sessionId);
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
  function encodeBase64Bytes(bytes) {
    var binary = "";
    var stride = 0x8000;
    for (var offset = 0; offset < bytes.length; offset += stride) {
      binary += String.fromCharCode.apply(null, bytes.subarray(offset, offset + stride));
    }
    return btoa(binary);
  }

  async function addDroppedFileAttachment(file) {
    if (!file) return;
    var id = ++attachIdSeq;
    var att = { id: id, basename: file.name || "attachment", status: "parsing", result: null, error: null, cancelled: false, uploadId: null };
    var commitAcknowledged = false;
    state.attachments.push(att);
    notify();
    try {
      if (!file.size || file.size > 20 * 1024 * 1024) {
        throw new Error(file.size ? bt("attachTooLarge") : bt("attachEmptyFile"));
      }
      var uploadId = "desktop_attach_" + Date.now().toString(36) + "_" + Math.random().toString(36).slice(2, 12);
      att.uploadId = uploadId;
      var offset = 0;
      var result = null;
      while (offset < file.size) {
        if (att.cancelled) throw new Error(bt("attachAddCancelled"));
        var end = Math.min(offset + 192 * 1024, file.size);
        var bytes = await file.slice(offset, end).arrayBuffer();
        result = await invoke("ingest_draft_file_chunk", {
          uploadId: uploadId,
          filename: file.name || "attachment",
          offset: offset,
          total: file.size,
          dataBase64: encodeBase64Bytes(new Uint8Array(bytes)),
          commit: end === file.size
        });
        if (end === file.size) commitAcknowledged = true;
        offset = end;
      }
      if (att.cancelled) throw new Error(bt("attachAddCancelled"));
      if (!result || !result.basename) throw new Error(bt("attachInvalidResult"));
      Object.defineProperty(result, "__pinvouManagedDraftAttachmentId", {
        configurable: true,
        value: uploadId,
        enumerable: false,
      });
      att.basename = result.basename || att.basename;
      att.status = "ready";
      att.result = result;
    } catch (e) {
      // The command already removes staging on backend errors. Only the user's
      // explicit cancellation, or an acknowledged commit with an invalid
      // response, may delete the completed directory.
      if (att.uploadId && (att.cancelled || commitAcknowledged)) {
        await invoke("cancel_draft_file_upload", {
          uploadId: att.uploadId,
        }).catch(function () {});
      }
      att.status = "error";
      att.error = String(e);
    }
    notify();
  }

  function conversationAttachmentArgs(reference) {
    reference = reference || {};
    return {
      sessionId: reference.sessionId || state.activeSessionId,
      messageIndex: Number(reference.messageIndex),
      attachmentIndex: Number(reference.attachmentIndex),
      basename: String(reference.basename || ""),
      displayText: String(reference.displayText || ""),
    };
  }
  function resolveConversationAttachment(reference) {
    return invoke("resolve_conversation_attachment", conversationAttachmentArgs(reference));
  }
  function openConversationAttachment(reference) {
    return invoke("open_conversation_attachment", conversationAttachmentArgs(reference))
      .catch(function (e) { addSystemItem(bt("openFailed") + e); return false; });
  }
  function revealConversationAttachment(reference) {
    return invoke("reveal_conversation_attachment", conversationAttachmentArgs(reference))
      .catch(function (e) { addSystemItem(bt("openFailed") + e); return false; });
  }

  async function addPasteImage(filename, bytes) {
    try {
      var path = await invoke("save_paste_image", { filename: filename, bytes: bytes });
      await addAttachmentByPath(path);
    } catch (e) { addSystemItem(bt("pasteImageFailed") + e); }
  }
  function removeAttachment(id) {
    var removed = state.attachments.find(function (a) { return a.id === id; });
    if (removed) {
      removed.cancelled = true;
      if (removed.status === "ready" && removed.result) {
        discardManagedAttachment(removed.result);
      }
    }
    state.attachments = state.attachments.filter(function (a) { return a.id !== id; });
    notify();
  }
  function clearAttachments() {
    state.attachments.forEach(function (attachment) {
      attachment.cancelled = true;
      if (attachment.status === "ready" && attachment.result) {
        discardManagedAttachment(attachment.result);
      }
    });
    state.attachments = [];
  }
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
  // 文件选择按钮在桌面仍走原生路径；HTML5 拖放拿不到路径时通过同一域方法
  // 分块写入 sessionless 草稿区，直到实际发送才归属到目标会话。
  async function uploadDeviceFiles(files) {
    var list = Array.prototype.slice.call(files || []).filter(Boolean);
    for (var index = 0; index < list.length; index++) {
      await addDroppedFileAttachment(list[index]);
    }
  }

  async function adoptManagedAttachments(attachments, sessionId) {
    var list = Array.prototype.slice.call(attachments || []);
    for (var index = 0; index < list.length; index++) {
      var attachment = list[index];
      var result = attachment && attachment.result;
      var uploadId = result && result.__pinvouManagedDraftAttachmentId;
      if (!uploadId) continue;
      var adopted = await invoke("adopt_draft_attachment", {
        sessionId: sessionId,
        uploadId: uploadId,
      });
      Object.defineProperty(adopted, "__pinvouManagedAttachmentSessionId", {
        configurable: true,
        value: sessionId,
        enumerable: false,
      });
      attachment.result = adopted;
      attachment.basename = adopted.basename || attachment.basename;
    }
    return list;
  }


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
      downloadArtifact: downloadArtifact,
      listDeliverableIndex: listDeliverableIndex,
      openExternalUrl: openExternalUrl,
      openUserExternalUrl: openUserExternalUrl,
      addAttachmentByPath: addAttachmentByPath,
      addPasteImage: addPasteImage,
      removeAttachment: removeAttachment,
      clearAttachments: clearAttachments,
      pickAndAttach: pickAndAttach,
      uploadDeviceFiles: uploadDeviceFiles,
      adoptManagedAttachments: adoptManagedAttachments,
      resolveConversationAttachment: resolveConversationAttachment,
      openConversationAttachment: openConversationAttachment,
      revealConversationAttachment: revealConversationAttachment
    };
  };
})(window);
