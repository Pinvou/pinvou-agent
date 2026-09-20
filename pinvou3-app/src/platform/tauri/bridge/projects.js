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
    // A change event arriving while a fetch is in flight schedules one extra
    // round after it settles, so the snapshot cannot go stale (review #448
    // finding 14: an event landing mid-fetch would be swallowed and the
    // sidebar would keep showing the old value).
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
        // Assignments are pure preference data: a fetch failure must not
        // break the UI. On the first failed pull the sidebar falls back to
        // implicit grouping; with an existing snapshot it keeps showing the
        // stale one (indistinguishable from fresh) until the next
        // projects:list_changed event retries.
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
      await invoke("delete_project", { projectId });
      await loadProjects();
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

    // Directory rebind (broken-link repair): confirmExisting is driven by the
    // frontend's two-phase handshake — the first call omits the confirmation,
    // the backend rejects it with a typed marker while the old directory still
    // exists, and the frontend escalates to the strong warning and retries.
    // previousPostBusySessionIds is the dialog's feed-back of its previous
    // report's post-busy ids (review #463 F-Major): the backend honors only
    // the intersection with its own to-lane retry population, so a
    // busy-refused carryover session is honestly reported post-busy again
    // instead of vanishing from every report field.
    async function rebindWorkspaceRoot(from, to, confirmExisting, previousPostBusySessionIds) {
      const report = await invoke("rebind_workspace_root", {
        from,
        to,
        confirmExisting: !!confirmExisting,
        previousPostBusySessionIds: previousPostBusySessionIds || [],
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
