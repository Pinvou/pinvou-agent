(function () {
  // biome-ignore lint/suspicious/noRedundantUseStrict: verbatim copy of a classic-script artifact; strict mode is part of the payload
  "use strict";

  // biome-ignore lint/suspicious/noAssignInExpressions: registry bootstrap of the verbatim payload; splitting the statement would diverge from the artifact
  const registry = window.__PINVOU_TAURI_BRIDGE_FEATURES__ = window.__PINVOU_TAURI_BRIDGE_FEATURES__ || {};
  registry["artifact-tracker"] = function (context) {
    const state = context.state;
    const invoke = context.invoke;
    const notify = context.notify;
    const isScheduledRunSession = context.isScheduledRunSession;

  // ── 产物跟踪 ─────────────────────────────────────────────────────
  function basename(p) {
    if (!p) return "";
    const parts = String(p).split(/[\\/]/);
    return parts[parts.length - 1] || p;
  }
  function isAbsPath(p) {
    return typeof p === "string" && (p.charAt(0) === "/" || /^[A-Za-z]:[\\/]/.test(p));
  }
  function normalizedPath(p) {
    return String(p || "").replaceAll('\\', "/");
  }
  function noteArtifactChange(path, event, sessionId) {
    if (!path) return;
    state.artifactChange = {
      seq: (state.artifactChange && state.artifactChange.seq || 0) + 1,
      path,
      event: event || "modified",
      sessionId: sessionId || "",
      at: Date.now(),
    };
    notify();
  }
  function isSharedMcpArtifactPath(path) {
    return normalizedPath(path).includes("/sessions/default/artifacts/");
  }
  function artifactBelongsToSession(path, sid) {
    if (!path || !sid) return false;
    if (!isAbsPath(path)) return true;
    if (isSharedMcpArtifactPath(path)) return true;
    const normalized = normalizedPath(path);
    if (normalized.includes("/sessions/")) {
      return normalized.includes("/sessions/" + sid + "/workspace/") ||
        normalized.includes("/sessions/" + sid + "/artifacts/");
    }
    return true;
  }
  function filterSessionArtifacts(artifacts, sid) {
    return (Array.isArray(artifacts) ? artifacts : []).filter(function (a) {
      return artifactBelongsToSession(a && a.path, sid);
    });
  }
  // 「成品型」扩展名:write_file 写出这类文件即自动当成品进面板(模型常忘 present_artifact)。
  // 办公文档 + markdown 报告 + 数据表 + 图片 + 打包件都算成品(覆盖 AI 常见产出格式)。
  // 中间/草稿(.txt/.json/.xml 等)刻意不在此列 → 不进面板,避免一堆过程文件污染产物列表;
  // 这类格式若确是成品,靠模型 present_artifact 显式挂出(present 过的不受扩展名门控)。
  const DELIVERABLE_EXTS = [
    "pptx", "ppt", "docx", "doc", "pdf", "html", "htm", "xlsx", "xls",
    "md", "csv", "png", "jpg", "jpeg", "svg", "gif", "webp", "zip",
  ];
  // tmp/ 是提示词约定的中间文件目录(instructions.md:中间/临时文件一律写 tmp/,
  // 不进产出物列表)。tmp/ 下的文件即使扩展名是成品型(.md/.html 等)也不算自动成品;
  // 确需展示只能靠模型显式 present_artifact(显式 present 不经 isDeliverable 门控)。
  function isTmpPath(path) {
    const segs = normalizedPath(path).split("/");
    for (let i = 0; i < segs.length; i++) {
      if (segs[i] === "tmp") return true;
    }
    return false;
  }
  function isDeliverable(path) {
    if (isTmpPath(path)) return false;
    const ext = (String(path || "").split(".").pop() || "").toLowerCase();
    return DELIVERABLE_EXTS.includes(ext);
  }
  function trackArtifact(path) {
    if (!path) return;
    const bn = basename(path);
    for (let i = 0; i < state.artifacts.length; i++) {
      if (basename(state.artifacts[i].path) === bn) {
        // 已有同名:write_file 跟踪的是相对路径、disk watcher 推的是绝对路径——同一文件
        // 两种 path 会重复。新 path 绝对而旧的相对则用绝对替换(open 可靠),否则忽略重复。
        if (isAbsPath(path) && !isAbsPath(state.artifacts[i].path)) {
          state.artifacts[i] = { path, basename: bn };
          notify();
        }
        return;
      }
    }
    state.artifacts.push({ path, basename: bn });
    notify();
  }
  function markTurnDirtyArtifact(path) {
    const bn = basename(path);
    if (!bn) return;
    if ((state.turnDirtyArtifacts || []).some(function (p) { return basename(p) === bn; })) return;
    state.turnDirtyArtifacts.push(path);
  }
  function untrackArtifact(path) {
    const before = state.artifacts.length;
    state.artifacts = state.artifacts.filter(function (a) { return a.path !== path; });
    if (state.artifacts.length !== before) notify();
  }
  // Artifact cards use the basename as their existing presentation identity.
  // This matches persisted artifact reconciliation, where a relative write path
  // and the absolute watcher path must resolve to the same visible card.
  function findPresentedArtifact(path) {
    const bn = basename(path);
    if (!bn) return null;
    for (let i = state.chatItems.length - 1; i >= 0; i--) {
      const it = state.chatItems[i];
      if (it.type === "artifact_card" && basename(it.path) === bn) return it;
    }
    return null;
  }
  function updatePresentedArtifact(card) {
    if (!card || !card.path) return null;
    const existing = findPresentedArtifact(card.path);
    if (!existing) return null;
    const stableId = existing.id;
    const stableAbsolutePath = isAbsPath(existing.path) && !isAbsPath(card.path)
      ? existing.path
      : null;
    Object.assign(existing, card, { type: "artifact_card" });
    if (stableId !== undefined) existing.id = stableId;
    if (stableAbsolutePath) existing.path = stableAbsolutePath;
    return existing;
  }
  // 切换 session 时对账:扫 workspace 磁盘,把实际存在、但跟踪列表里没有的文件补进来。
  // 修「文件已生成在盘上、却因 app 中途重启/跟踪遗漏而不在产物面板」(以磁盘为准)。
  async function reconcileArtifacts(sid) {
    if (!sid) return;
    if (isScheduledRunSession(sid)) return;
    try {
      const files = await invoke("list_workspace_files", { sessionId: sid });
      if (sid !== state.activeSessionId) return; // 已切走,放弃(避免写错 session)
      const byName = {};
      state.artifacts.forEach(function (a) { byName[basename(a.path)] = a; });
      let added = false;
      files.forEach(function (p) {
        const bn = basename(p);
        const ex = byName[bn];
        // 已 present_artifact 过的成品在 saved.artifacts(ex 命中);扫盘只「新增」成品型文件,
        // 不再把所有过程文件全扫进面板(修「飞书 CLI scratch 全暴露成产物」)。
        if (!ex) {
          if (!isDeliverable(p)) return;
          const na = { path: p, basename: bn }; state.artifacts.push(na); byName[bn] = na; added = true;
        }
        else if (isAbsPath(p) && !isAbsPath(ex.path)) { ex.path = p; added = true; } // 相对→绝对,open 可靠
      });
      if (added) {
        notify();
        try { await invoke("save_session_artifacts", { id: sid, paths: state.artifacts.map(function (a) { return a.path; }) }); } catch { /* disk-write failure must not block the frontend update */ }
      }
    } catch { /* workspace 不存在(新 session)等,忽略 */ }
  }
  function pushArtifactPath(paths, path) {
    if (typeof path !== "string" || !path.trim()) return;
    path = path.trim();
    if (!paths.includes(path)) paths.push(path);
  }
  function extractPatchHeaderPaths(patch, paths) {
    String(patch || "").split(/\r?\n/).forEach(function (line) {
      // eslint-disable-next-line sonarjs/super-linear-regex -- input is split by line and line length is bounded by the patch header; backtracking is negligible
      const custom = /^\*\*\* (?:Add|Update|Delete) File:\s*(.+?)\s*$/.exec(line);
      if (custom) { pushArtifactPath(paths, custom[1]); return; }
      // eslint-disable-next-line sonarjs/super-linear-regex -- input is split by line and line length is bounded by the patch header; backtracking is negligible
      const unified = /^\+\+\+\s+(?:b\/)?(.+?)\s*$/.exec(line);
      if (unified && unified[1] !== "/dev/null") pushArtifactPath(paths, unified[1]);
    });
  }
  // File.write / File.edit / File.patch 的 args 里提取全部产物路径。
  function extractArtifactPaths(args) {
    if (!args) return [];
    if (typeof args === "string") {
      try { args = JSON.parse(args); } catch { return []; }
    }
    const paths = [];
    pushArtifactPath(paths, args.path || args.file_path || args.filename);
    [args.replace, args.changes].forEach(function (changes) {
      if (!Array.isArray(changes)) return;
      changes.forEach(function (change) {
        if (!change || typeof change !== "object") return;
        pushArtifactPath(paths, change.path || change.file_path || change.filename);
      });
    });
    extractPatchHeaderPaths(args.patch, paths);
    return paths;
  }
  function extractArtifactPath(args) {
    return extractArtifactPaths(args)[0] || null;
  }

  function fileMutationAction(name, args) {
    if (typeof args === "string") {
      try { args = JSON.parse(args); } catch { args = null; }
    }
    if (String(name || "").toLowerCase() === "file") {
      const action = String(args && args.action || "").toLowerCase();
      return ["write", "edit", "patch"].includes(action) ? action : null;
    }
    if (name === "write_file") return "write";
    if (name === "edit_file") return "edit";
    return null;
  }


  function isPresentArtifactTool(name) {
    return name === "present_artifact" ||
      (typeof name === "string" && name.endsWith("present_artifact"));
  }

  // 成品卡路径:优先用 server(present_artifact_server.py)解析并验证过的绝对路径 abs_path——
  // 模型常给相对路径,直接拿 args.path 渲染会让卡片 path 是相对,点 Open 报「path must be
  // absolute」,且模型可能重试再 present 一次出双卡。取不到 abs_path 才回退原始 path。
  // 兼容两种结果格式:直接 payload {abs_path} / MCP content 数组 {content:[{text}]} 包一层。
  function parseToolResultPayload(toolResultContent) {
    try {
      const raw = typeof toolResultContent === "string" ? toolResultContent : JSON.stringify(toolResultContent || {});
      const obj = JSON.parse(raw);
      if (obj && obj.content && obj.content[0] && typeof obj.content[0].text === "string") {
        try {
          const inner = JSON.parse(obj.content[0].text);
          if (inner && typeof inner === "object") return inner;
        } catch { /* use the outer object when the inner value is not JSON */ }
      }
      return obj;
    } catch {
      return null;
    }
  }
  function artifactPathFromToolOutput(toolResultContent) {
    const obj = parseToolResultPayload(toolResultContent);
    if (!obj || typeof obj !== "object") return null;
    const p = obj.abs_path || obj.path || obj.file_path || obj.local_path;
    return typeof p === "string" && p ? p : null;
  }
  function shouldUseToolOutputAsArtifact(name) {
    if (!name || isPresentArtifactTool(name)) return false;
    // Only MCP-style producer tools should be parsed from result JSON. Shell/read
    // tools often return diagnostic JSON with a `path` field, which is not a
    // newly created artifact.
    return typeof name === "string" && name.indexOf("mcp_") === 0;
  }
  function presentArtifactAbsPath(toolResultContent, fallbackPath) {
    fallbackPath = fallbackPath || "";
    const parsed = artifactPathFromToolOutput(toolResultContent);
    if (parsed) return parsed;
    return fallbackPath;
  }


    return {
      basename,
      isAbsPath,
      normalizedPath,
      noteArtifactChange,
      isSharedMcpArtifactPath,
      artifactBelongsToSession,
      filterSessionArtifacts,
      isDeliverable,
      trackArtifact,
      markTurnDirtyArtifact,
      untrackArtifact,
      findPresentedArtifact,
      updatePresentedArtifact,
      reconcileArtifacts,
      extractArtifactPaths,
      extractArtifactPath,
      fileMutationAction,
      isPresentArtifactTool,
      parseToolResultPayload,
      artifactPathFromToolOutput,
      shouldUseToolOutputAsArtifact,
      presentArtifactAbsPath,
    };
  };
})();
