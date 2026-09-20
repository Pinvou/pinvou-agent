(function () {
  // biome-ignore lint/suspicious/noRedundantUseStrict: verbatim copy of a classic-script artifact; strict mode is part of the payload
  "use strict";

  // biome-ignore lint/suspicious/noAssignInExpressions: registry bootstrap of the verbatim payload; splitting the statement would diverge from the artifact
  const registry = window.__PINVOU_TAURI_BRIDGE_FEATURES__ = window.__PINVOU_TAURI_BRIDGE_FEATURES__ || {};
  registry["artifact-tracker"] = function (context) {let pinvouSharedtauriArtifactTrackerCache = null;
function pinvouSharedtauriArtifactTracker() {
  if (!pinvouSharedtauriArtifactTrackerCache) pinvouSharedtauriArtifactTrackerCache = window.PinvouBridgeShared.create("tauriArtifactTracker", { state, notify, DELIVERABLE_EXTS, extractArtifactPaths, parseToolResultPayload });
  return pinvouSharedtauriArtifactTrackerCache;
}


    const state = context.state;
    const invoke = context.invoke;
    const notify = context.notify;
    const isScheduledRunSession = context.isScheduledRunSession;

  // ── 产物跟踪 ─────────────────────────────────────────────────────
function basename(p) { return pinvouSharedtauriArtifactTracker().basename(p); }
function isAbsPath(p) { return pinvouSharedtauriArtifactTracker().isAbsPath(p); }
function normalizedPath(p) { return pinvouSharedtauriArtifactTracker().normalizedPath(p); }
function noteArtifactChange(path, event, sessionId) { return pinvouSharedtauriArtifactTracker().noteArtifactChange(path, event, sessionId); }
function isSharedMcpArtifactPath(path) { return pinvouSharedtauriArtifactTracker().isSharedMcpArtifactPath(path); }
function artifactBelongsToSession(path, sid) { return pinvouSharedtauriArtifactTracker().artifactBelongsToSession(path, sid); }
function filterSessionArtifacts(artifacts, sid) { return pinvouSharedtauriArtifactTracker().filterSessionArtifacts(artifacts, sid); }
  // 「成品型」扩展名:write_file 写出这类文件即自动当成品进面板(模型常忘 present_artifact)。
  // 办公文档 + markdown 报告 + 数据表 + 图片 + 打包件都算成品(覆盖 AI 常见产出格式)。
  // 中间/草稿(.txt/.json/.xml 等)刻意不在此列 → 不进面板,避免一堆过程文件污染产物列表;
  // 这类格式若确是成品,靠模型 present_artifact 显式挂出(present 过的不受扩展名门控)。
  const DELIVERABLE_EXTS = new Set([
    "pptx", "ppt", "docx", "doc", "pdf", "html", "htm", "xlsx", "xls",
    "md", "csv", "png", "jpg", "jpeg", "svg", "gif", "webp", "zip",
  ]);

function isDeliverable(path) { return pinvouSharedtauriArtifactTracker().isDeliverable(path); }
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
function markTurnDirtyArtifact(path) { return pinvouSharedtauriArtifactTracker().markTurnDirtyArtifact(path); }
function untrackArtifact(path) { return pinvouSharedtauriArtifactTracker().untrackArtifact(path); }
  // Prefer an exact normalized path so an older card is not hidden by a newer
  // same-named artifact from another directory. Fall back to the basename for
  // persisted relative paths that must reconcile with an absolute watcher path.
function findPresentedArtifact(path) { return pinvouSharedtauriArtifactTracker().findPresentedArtifact(path); }
  // Updates an existing presentation card in place (stable id and position)
  // instead of appending a duplicate card. Returns null when the caller must
  // append a fresh card instead:
  // - no card for this basename yet, or the matching card's absolute path
  //   differs from the presented path (same-named files in different
  //   directories are distinct artifacts, never rewritten into each other);
  // - a user message is newer than the existing card with no file mutation
  //   after it: the model is answering a fresh "show it again" request, and
  //   replaying that turn must stay a visible new card.
function updatePresentedArtifact(card) { return pinvouSharedtauriArtifactTracker().updatePresentedArtifact(card); }
function sessionRecentlyRebound(sid) { return pinvouSharedtauriArtifactTracker().sessionRecentlyRebound(sid); }
function rebaseArtifactPathsForRebind(sid, paths) { return pinvouSharedtauriArtifactTracker().rebaseArtifactPathsForRebind(sid, paths); }
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
        else if (isAbsPath(p) && (!isAbsPath(ex.path) || (normalizedPath(ex.path) !== normalizedPath(p) && sessionRecentlyRebound(sid)))) {
          // Relative→absolute opens reliably; or stale absolute → live
          // workspace file, matched by basename — ONLY for a session the
          // rebind command just moved (the workspace_rebound mark, review
          // #463 round-10 Major 2). After a folder rebind the persisted
          // entry keeps the vanished root, and the relative→absolute escape
          // hatch never fires for an already-absolute stale entry, so the
          // freshness-windowed mark is what lets the reconcile heal it.
          ex.path = p; added = true;
        }
      });
      if (added) {
        notify();
        try { await invoke("save_session_artifacts", { id: sid, paths: rebaseArtifactPathsForRebind(sid, state.artifacts.map(function (a) { return a.path; })) }); } catch { /* disk-write failure must not block the frontend update */ }
      }
    } catch { /* workspace 不存在(新 session)等,忽略 */ }
  }
function pushArtifactPath(paths, path) { return pinvouSharedtauriArtifactTracker().pushArtifactPath(paths, path); }
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
function extractArtifactPath(args) { return pinvouSharedtauriArtifactTracker().extractArtifactPath(args); }

function fileMutationAction(name, args) { return pinvouSharedtauriArtifactTracker().fileMutationAction(name, args); }


function isPresentArtifactTool(name) { return pinvouSharedtauriArtifactTracker().isPresentArtifactTool(name); }

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
function artifactPathFromToolOutput(toolResultContent) { return pinvouSharedtauriArtifactTracker().artifactPathFromToolOutput(toolResultContent); }
function shouldUseToolOutputAsArtifact(name) { return pinvouSharedtauriArtifactTracker().shouldUseToolOutputAsArtifact(name); }
function presentArtifactAbsPath(toolResultContent, fallbackPath) { return pinvouSharedtauriArtifactTracker().presentArtifactAbsPath(toolResultContent, fallbackPath); }


    return {
      basename,
      isAbsPath,
      normalizedPath,
      sessionRecentlyRebound,
      rebaseArtifactPathsForRebind,
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
