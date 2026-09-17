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
    // A change event arriving while a fetch is in flight → run one more round
    // afterwards, so the snapshot never goes stale (review #448 finding 14: an
    // event landing mid-fetch would be swallowed and the sidebar would keep
    // showing old values).
    let refetchNeeded = false;

    function applySnapshot(snapshot) {
      if (!snapshot || !Array.isArray(snapshot.projects)) return;
      state.projectsList = {
        projects: snapshot.projects,
        assignments: snapshot.assignments || {},
        // Anti-materialization exclusion list (§3, an array of canonical keys;
        // on POSIX the identity key is the path itself).
        neverMaterializeRoots: snapshot.never_materialize_roots || [],
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
        // Ownership is pure preference data and a failure must not break the
        // UI: when the first fetch fails the sidebar falls back to implicit
        // grouping; with an existing snapshot the old one keeps showing
        // (indistinguishable from the latest) until the next
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

    // Folder-project auto-materialization: idempotent ensure; the backend
    // broadcasts projects:list_changed on creation (event refresh plus the
    // active refresh below, belt and braces, same as createProject). Failed
    // roots (nesting conflicts etc.) are handled by the caller per per-root
    // outcome; errors are not swallowed here.
    async function ensureFolderProjects(roots) {
      const outcomes = await invoke("ensure_folder_projects", { roots: roots || [] });
      await loadProjects();
      return outcomes;
    }

    // Manage panel (§4): whole-set roots replacement (the backend writes
    // explicit move-outs for the removed roots' auto members).
    async function updateProjectRoots(projectId, roots) {
      const updated = await invoke("update_project", { projectId, roots });
      await loadProjects();
      return updated;
    }

    // Project-remembered primary folder (§9.2): the manage panel's "set as
    // primary folder" entry.
    async function setPrimaryRoot(projectId, root) {
      const updated = await invoke("update_project", { projectId, lastPrimaryRoot: root });
      await loadProjects();
      return updated;
    }

    // Anti-materialization exclusion list (§3): never=true stops
    // auto-creating a project for this folder; false revokes it.
    async function setNeverMaterialize(root, never) {
      const updated = await invoke("projects_set_never_materialize", { root, never: !!never });
      await loadProjects();
      return updated;
    }

    // Align to project (§6/§9.7): the session keychain is replaced by the
    // owning project's full root set at that moment; typed errors
    // (ALIGN_BUSY/ALIGN_NO_WORKSPACE) are thrown as-is for the caller to map
    // to copy by marker.
    async function alignSessionToProject(sessionId) {
      return invoke("align_session_to_project", { sessionId });
    }

    // Directory rebind (fix broken links): confirmExisting is two-staged by
    // the frontend — first call without the confirmation; when the old
    // directory still exists the backend reports a specific error, and the
    // frontend upgrades to a strong confirmation and retries.
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
      ensureFolderProjects,
      updateProjectRoots,
      setPrimaryRoot,
      setNeverMaterialize,
      alignSessionToProject,
      rebindWorkspaceRoot
    };
  };
})(window);
