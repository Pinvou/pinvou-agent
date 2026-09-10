/**
 * projects feature for the Tauri bridge.
 * Registered before bridge.js builds the backwards-compatible facade.
 *
 * Keeps the authoritative project list + session assignment map in bridge
 * state (state.projectsList = { projects, assignments, loadedAt }) so the
 * sidebar grouping re-derives from bs.projectsList on every notify. Backend
 * mutations broadcast projects:list_changed; we refetch on the event and also
 * after each mutation we initiate (event arrival order is not guaranteed
 * relative to the invoke response).
 */
(function (root) {
  // biome-ignore lint/suspicious/noRedundantUseStrict: verbatim classic-script artifact; strict mode is part of the payload
  "use strict";
  // biome-ignore lint/suspicious/noAssignInExpressions: registry bootstrap of the verbatim payload; splitting statements would diverge from the artifact
  const registry = root.__PINVOU_TAURI_BRIDGE_FEATURES__ = root.__PINVOU_TAURI_BRIDGE_FEATURES__ || {};
  registry["projects"] = function (context) {
    const state = context.state;
    const notify = context.notify;
    const invoke = context.invoke;
    const listen = context.listen;

    let fetchInFlight = false;
    // 飞行中的 fetch 之后又来了个变更事件 → 结束后补一轮,防止快照滞留
    // (评审 #448 finding 14:事件在 fetch 期间到达会被吞,侧栏一直用旧值)。
    let refetchNeeded = false;

    function applySnapshot(snapshot) {
      if (!snapshot || !Array.isArray(snapshot.projects)) return;
      state.projectsList = {
        projects: snapshot.projects,
        assignments: snapshot.assignments || {},
        loadedAt: Date.now(),
      };
      notify();
    }

    async function loadProjects() {
      if (fetchInFlight) {
        refetchNeeded = true;
        return state.projectsList;
      }
      fetchInFlight = true;
      try {
        applySnapshot(await invoke("list_projects"));
      } catch (e) {
        // 归属是纯偏好数据,失败不打断 UI:首次拉取失败时侧栏回落隐式分组;
        // 已有快照则继续显示旧快照(与最新无从区分),直到下一个
        // projects:list_changed 事件才重试。
        console.warn("[projects] list_projects failed:", e);
      } finally {
        fetchInFlight = false;
        if (refetchNeeded) {
          refetchNeeded = false;
          loadProjects();
        }
      }
      return state.projectsList;
    }

    listen("projects:list_changed", function () {
      loadProjects();
    });

    async function createProject(name, roots) {
      const created = await invoke("create_project", { name, roots: roots || [] });
      await loadProjects();
      return created;
    }

    async function renameProject(projectId, name) {
      const updated = await invoke("update_project", { projectId, name });
      await loadProjects();
      return updated;
    }

    async function deleteProject(projectId) {
      const report = await invoke("delete_project", { projectId });
      await loadProjects();
      return report;
    }

    async function moveSessionToProject(sessionId, projectId, addWorkspaceRoot) {
      const outcome = await invoke("move_session_to_project", {
        sessionId,
        projectId: projectId === undefined ? null : projectId,
        addWorkspaceRoot: !!addWorkspaceRoot,
      });
      await loadProjects();
      return outcome;
    }

    // 目录重绑定(修断链):confirmExisting 由前端两阶段控制——先不带确认
    // 调用,后端在旧目录仍存在时报特定错误,前端升级为强确认后重试。
    async function rebindWorkspaceRoot(from, to, confirmExisting) {
      const report = await invoke("rebind_workspace_root", {
        from,
        to,
        confirmExisting: !!confirmExisting,
      });
      await loadProjects();
      return report;
    }

    return {
      loadProjects,
      createProject,
      renameProject,
      deleteProject,
      moveSessionToProject,
      rebindWorkspaceRoot
    };
  };
})(window);
