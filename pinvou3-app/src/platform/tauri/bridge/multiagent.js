/**
 * multiagent feature for the Tauri bridge.
 * Registered before bridge.js builds the backwards-compatible facade.
 *
 * After ADR-0006 this is a thin layer: multi-agent = plain-session capability
 * + proactive delegation. Subagent identity/status/records are persisted by
 * the foundation (worker ledger + transcripts); this domain only reads the
 * subagent list and transcripts and forwards subagent bridge events as DOM
 * events for the running overlay, the spawn count rows, and the transcript
 * panel to subscribe to (no run-state machine of its own). The old startRun
 * standalone entry was retired with the session-level switch (see the
 * interaction domain).
 */
(function (root) {
  // biome-ignore lint/suspicious/noRedundantUseStrict: verbatim classic-script artifact; strict mode is part of the payload
  "use strict";
  // biome-ignore lint/suspicious/noAssignInExpressions: registry bootstrap of the verbatim payload; splitting statements would diverge from the artifact
  const registry = root.__PINVOU_TAURI_BRIDGE_FEATURES__ = root.__PINVOU_TAURI_BRIDGE_FEATURES__ || {};
  registry["multiagent"] = function (context) {
    const invoke = context.invoke;
    const listen = context.listen;

    /**
     * Subagent list (read-only projection: the foundation worker ledger as the
     * main table, transcripts as the attached one). Available in swarm sessions
     * and Pinvou native Code sessions; survives restarts. A failed read
     * returns null instead of []: downgrading a permission error / corrupted
     * file / command failure to an empty list would make the UI disguise the
     * fault as "no subagents" (review P2). Callers keep their last valid data;
     * the transcript panel shows a read-failure notice, and the overlay heals
     * through its poll, which retries automatically.
     */
    async function listSubagentTranscripts(runId) {
      try {
        return (await invoke("list_subagent_transcripts", { runId })) || [];
      } catch (err) {
        console.warn("list_subagent_transcripts failed", err);
        return null;
      }
    }

    async function readSubagentTranscript(runId, agentId, cursor) {
      try {
        const args = { runId, agentId };
        if (cursor && Number.isSafeInteger(cursor.offset) && cursor.offset >= 0 && typeof cursor.revision === "string") {
          args.offset = cursor.offset;
          args.revision = cursor.revision;
        }
        return (await invoke("read_subagent_transcript", args)) || null;
      } catch (err) {
        console.warn("read_subagent_transcript failed", err);
        return null;
      }
    }

    // Subagent progress/completion → DOM events. The running overlay, the
    // count rows' fallbacks, and the transcript panel subscribe by agent_id;
    // no global store (that would rebuild a run-state machine).
    function dispatchSubagentUpdate(payload) {
      if (typeof root.dispatchEvent !== "function" || typeof root.CustomEvent !== "function") return;
      try {
        root.dispatchEvent(new root.CustomEvent("pinvou:subagent-update", { detail: payload }));
      } catch {
        // No CustomEvent (very old webview): degrade silently — the UI still
        // has the poll fallback.
      }
    }

    listen("multiagent:agent_progress", function (e) {
      const p = e.payload || {};
      if (!p.session_id || !p.agent_id) return;
      dispatchSubagentUpdate({
        sessionId: p.session_id,
        agentId: p.agent_id,
        role: p.role_id && p.role_id !== p.agent_id ? p.role_id : null,
        status: p.status || null,
        done: false,
        failed: false,
      });
    });

    listen("multiagent:agent_complete", function (e) {
      const p = e.payload || {};
      if (!p.session_id || !p.agent_id) return;
      dispatchSubagentUpdate({
        sessionId: p.session_id,
        agentId: p.agent_id,
        role: p.role_id && p.role_id !== p.agent_id ? p.role_id : null,
        status: null,
        done: true,
        failed: !!p.failed,
      });
    });

    return {
      listSubagentTranscripts,
      readSubagentTranscript,
    };
  };
})(typeof window === "undefined" ? globalThis : window);
