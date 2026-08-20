/**
 * artifacts feature for the Tauri bridge.
 * Registered before bridge.js builds the backwards-compatible facade.
 */
(function (root) {
  // biome-ignore lint/suspicious/noRedundantUseStrict: verbatim classic-script artifact; strict mode is part of the payload
  "use strict";
  // biome-ignore lint/suspicious/noAssignInExpressions: registry bootstrap of the verbatim payload; splitting statements would diverge from the artifact
  const registry = root.__PINVOU_TAURI_BRIDGE_FEATURES__ = root.__PINVOU_TAURI_BRIDGE_FEATURES__ || {};
  registry["artifacts"] = function (context) {
    const state = context.state;
    const notify = context.notify;
    const invoke = context.invoke;
    const bt = context.bt;
    const addSystemItem = context.addSystemItem;
    const dialogOpen = context.dialogOpen;
    const basename = context.basename;
    const isDeliverable = context.isDeliverable;
    const isAbsPath = context.isAbsPath;
    const sessionStates = context.sessionStates;
    const discardManagedAttachment = context.discardManagedAttachment || function () { return Promise.resolve(); };
    let attachIdSeq = 0;
  // ── 产物面板 ─────────────────────────────────────────────────────
  function artifactInfo(path) { return invoke("artifact_info", { path }); }
  function readArtifactText(path) { return invoke("read_artifact_text", { path }); }
  function writeArtifactText(path, content) { return invoke("write_artifact_text", { path, content }); }
  function readArtifactImageB64(path) { return invoke("read_artifact_image_b64", { path }); }
  // pptx 封面缩略图：读 docProps/thumbnail.jpeg → data URL（无则 null）。本地数据、无外链。
  function readArtifactThumbnail(path) { return invoke("read_artifact_thumbnail", { path }).catch(function () { return null; }); }
  function renderArtifactVisual(path) { return invoke("render_artifact_visual", { path }); }
  function openContainingFolder(path) { return invoke("open_containing_folder", { path }).catch(function (e) { addSystemItem(bt("openFailed") + e); }); }
  function revealSessionFolder(sessionId) { return invoke("reveal_session_folder", { sessionId }).catch(function (e) { addSystemItem(bt("openFailed") + e); }); }
  function openScheduledTaskFolder(automationId) { return invoke("open_scheduled_task_folder", { automationId }).catch(function (e) { addSystemItem(bt("openFailed") + e); }); }
  function openInSystem(path) { return invoke("open_in_system", { path }).catch(function (e) { addSystemItem(bt("openFailed") + e); }); }
  // 仅放白名单 URL (metaso.cn / open.bochaai.com),后端 open_external_url 强制校验。
  function openExternalUrl(url) { return invoke("open_external_url", { url }).catch(function (e) { addSystemItem(bt("openFailed") + e); }); }
  // ACP 消息/产物预览里由用户亲自点击的 HTTP(S) 外链；后端与工具白名单入口分开校验。
  function openUserExternalUrl(url) { return invoke("open_user_external_url", { url }).catch(function (e) { addSystemItem(bt("openFailed") + e); }); }
  function deliverableCategory(path) {
    const ext = (String(path || "").split(".").pop() || "").toLowerCase();
    if (["html", "htm", "mhtml", "mht"].includes(ext)) return "web";
    if (["ppt", "pptx", "odp", "dps"].includes(ext)) return "ppt";
    if (["png", "jpg", "jpeg", "gif", "webp", "svg", "bmp", "heic"].includes(ext)) return "img";
    return "doc";
  }
  function sessionTitleById(sid) {
    const m = state.sessions.find(function (s) { return s.id === sid; });
    return (m && m.title) || "";
  }
  function currentMemoryArtifacts() {
    const rows = [];
    function addFrom(sid, arts) {
      (arts || []).forEach(function (a) {
        const path = a && a.path;
        if (!path || !isDeliverable(path)) return;
        rows.push({ path, sessionId: sid || state.activeSessionId, source: sessionTitleById(sid || state.activeSessionId), name: basename(path) });
      });
    }
    addFrom(state.activeSessionId, state.artifacts);
    Object.keys(sessionStates).forEach(function (sid) { addFrom(sid, sessionStates[sid] && sessionStates[sid].artifacts); });
    return rows;
  }
  // 跨会话产出物索引:磁盘 session JSON 为主,再合并当前内存工作集。
  // 新产物在 chat:done/save_session_artifacts 前也能立刻出现在「产出物」一级入口。
  async function listDeliverableIndex() {
    const disk = await invoke("list_deliverable_index").catch(function () { return []; });
    const byPath = {};
    (disk || []).forEach(function (x) { if (x && x.path) byPath[x.path] = x; });
    const mem = currentMemoryArtifacts().filter(function (x) { return x.path && !byPath[x.path]; });
    const hydrated = await Promise.all(mem.map(async function (x) {
      let path = x.path;
      if (!isAbsPath(path) && x.sessionId) {
        try {
          const ws = await invoke("list_workspace_files", { sessionId: x.sessionId });
          const bn = basename(path);
          const resolved = (ws || []).find(function (p) { return basename(p) === bn; });
          if (resolved) path = resolved;
        } catch { /* keep the original path on parse failure */ }
      }
      let info = null;
      try { info = await artifactInfo(path); } catch { /* degrade to no-details when info is missing */ }
      const ext = (String(path).split(".").pop() || "").toLowerCase();
      return {
        name: x.name || basename(path),
        path,
        ext,
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
    const ext = (String(path).split(".").pop() || "").toLowerCase();
    const cmd = (ext === "html" || ext === "htm") ? "open_artifact_window" : "open_in_system";
    return invoke(cmd, { path, sessionId: sessionId || null }).catch(function (e) { addSystemItem(bt("openFailed") + e); });
  }
  function downloadArtifact(path, sessionId) {
    return openArtifactExternal(path, sessionId);
  }

  // ── 附件 ────────────────────────────────────────────────────────
  async function addAttachmentByPath(path) {
    const id = ++attachIdSeq;
    const att = { id, basename: basename(path), status: "parsing", result: null, error: null };
    state.attachments.push(att); notify();
    try {
      const result = await invoke("ingest_file", { path });
      att.status = "ready"; att.result = result;
    } catch (e) { att.status = "error"; att.error = String(e); }
    notify();
  }
  async function addDroppedFileAttachment(file) {
    if (!file) return;
    const id = ++attachIdSeq;
    const att = { id, basename: file.name || "attachment", status: "parsing", result: null, error: null, cancelled: false, uploadId: null };
    state.attachments.push(att);
    notify();
    try {
      const uploader = root.PinvouChunkedFileUpload;
      if (!uploader || typeof uploader.uploadFile !== "function") {
        throw new Error("chunked attachment uploader is unavailable");
      }
      const uploadId = uploader.uploadId("desktop_attach");
      att.uploadId = uploadId;
      const completed = await uploader.uploadFile({
        file,
        uploadId,
        isCancelled: function () { return att.cancelled; },
        sendChunk: function (chunk) {
          return invoke("ingest_draft_file_chunk", {
            uploadId: chunk.uploadId,
            filename: chunk.fileName,
            offset: chunk.offset,
            total: chunk.total,
            dataBase64: chunk.dataBase64,
            commit: chunk.commit,
            ...(chunk.sha256 ? { sha256: chunk.sha256 } : {}),
          });
        },
        validateResult: function (result) { return Boolean(result && result.basename); },
        cleanup: function (upload) {
          // 后端命令失败时已清理 staging。只有用户取消，或最后一块已确认后
          // 前端校验失败，才允许删除这个 uploadId 对应的已完成目录。
          if (att.cancelled || upload.commitAcknowledged) {
            return invoke("cancel_draft_file_upload", { uploadId: upload.uploadId });
          }
        },
      });
      const result = completed.result;
      Object.defineProperty(result, "__pinvouManagedDraftAttachmentId", {
        configurable: true,
        value: uploadId,
        enumerable: false,
      });
      att.basename = result.basename || att.basename;
      att.status = "ready";
      att.result = result;
    } catch (e) {
      att.status = "error";
      att.error = e && e.code === "device_upload_empty"
        ? bt("attachEmptyFile")
        : e && e.code === "device_upload_too_large"
          ? bt("attachTooLarge")
          : e && e.code === "device_upload_cancelled"
            ? bt("attachAddCancelled")
            : e && e.code === "device_upload_invalid_result"
              ? bt("attachInvalidResult")
              : String(e);
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
      const path = await invoke("save_paste_image", { filename, bytes });
      await addAttachmentByPath(path);
    } catch (e) { addSystemItem(bt("pasteImageFailed") + e); }
  }
  function removeAttachment(id) {
    const removed = state.attachments.find(function (a) { return a.id === id; });
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
      const selected = await dialogOpen({ multiple: true });
      if (!selected) return;
      const paths = Array.isArray(selected) ? selected : [selected];
      for (let i = 0; i < paths.length; i++) { await addAttachmentByPath(paths[i]); }
    } catch (e) { addSystemItem(bt("filePickFailed") + e); }
  }
  // 文件选择按钮在桌面仍走原生路径；HTML5 拖放拿不到路径时通过同一域方法
  // 分块写入 sessionless 草稿区，直到实际发送才归属到目标会话。
  async function uploadDeviceFiles(files) {
    const list = Array.prototype.slice.call(files || []).filter(Boolean);
    for (let index = 0; index < list.length; index++) {
      let file = list[index];
      // 发送前预缩放：超长边图片先压到 ~1500px JPEG 再入附件
      // （本地引擎视觉编码耗时随 token 线性增长）。canvas 不可用时
      // prescale 原样回落，绝不拦截上传。
      if (root.PinvouImagePrescale && file.type && file.type.indexOf("image/") === 0) {
        try {
          const scaled = await root.PinvouImagePrescale.prescaleImageFile(file);
          if (scaled.compressed) {
            const name = String(file.name || "image").replace(/\.[A-Za-z0-9]+$/, "") + ".jpg";
            file = new File([scaled.file], name, { type: "image/jpeg" });
            addSystemItem(bt("imageCompressed"));
          }
        } catch {
          // 预缩放失败不拦截上传：原文件继续走分块上传，循环不中断。
        }
      }
      await addDroppedFileAttachment(file);
    }
  }

  async function adoptManagedAttachments(attachments, sessionId) {
    const list = Array.prototype.slice.call(attachments || []);
    for (let index = 0; index < list.length; index++) {
      const attachment = list[index];
      const result = attachment && attachment.result;
      const uploadId = result && result.__pinvouManagedDraftAttachmentId;
      if (!uploadId) continue;
      const adopted = await invoke("adopt_draft_attachment", {
        sessionId,
        uploadId,
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
      artifactInfo,
      readArtifactText,
      writeArtifactText,
      readArtifactImageB64,
      readArtifactThumbnail,
      renderArtifactVisual,
      openContainingFolder,
      revealSessionFolder,
      openScheduledTaskFolder,
      openInSystem,
      openArtifactExternal,
      downloadArtifact,
      listDeliverableIndex,
      openExternalUrl,
      openUserExternalUrl,
      addAttachmentByPath,
      addPasteImage,
      removeAttachment,
      clearAttachments,
      pickAndAttach,
      uploadDeviceFiles,
      adoptManagedAttachments,
      resolveConversationAttachment,
      openConversationAttachment,
      revealConversationAttachment
    };
  };
})(window);
