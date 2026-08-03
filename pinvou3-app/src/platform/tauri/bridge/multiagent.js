/**
 * multiagent feature for the Tauri bridge.
 * Registered before bridge.js builds the backwards-compatible facade.
 *
 * ADR-0006 之后这里只剩薄薄一层：多智能体 = 普通会话能力 + 主动委派。
 * 子智能体的身份/状态/记录由底座落盘（worker ledger + transcripts），
 * 本域只做两件事：读取子智能体列表与对话记录、把子智能体相关的桥事件
 * 转成 DOM 事件供行内专家卡自订阅（不维护任何运行状态机）。旧的
 * startRun 独立入口已随会话级开关上线退役（开关见 interaction 域）。
 */
(function (root) {
  "use strict";
  var registry = root.__PINVOU_TAURI_BRIDGE_FEATURES__ = root.__PINVOU_TAURI_BRIDGE_FEATURES__ || {};
  registry["multiagent"] = function (context) {
    var invoke = context.invoke;
    var listen = context.listen;

    /**
     * 子智能体列表（底座 worker ledger 为主表、transcripts 为附表的只读投影）。
     * 任意会话可用；重启后依然可查。读取失败返回 null 而不是 []：权限错误/
     * 文件损坏/命令失败若降级成空表，界面会把故障伪装成"没有子智能体"
     * （复核 P2）。调用方保留上次有效数据并显示重试提示，轮询自动重试。
     */
    async function listSubagentTranscripts(runId) {
      try {
        return (await invoke("list_subagent_transcripts", { runId: runId })) || [];
      } catch (err) {
        console.warn("list_subagent_transcripts failed", err);
        return null;
      }
    }

    async function readSubagentTranscript(runId, agentId) {
      try {
        return (await invoke("read_subagent_transcript", { runId: runId, agentId: agentId })) || [];
      } catch (err) {
        console.warn("read_subagent_transcript failed", err);
        return null;
      }
    }

    // 子智能体进展/完成 → DOM 事件。行内专家卡与面板按 agent_id 自订阅，
    // 不经全局 store（避免重建一套运行状态机）。
    function dispatchSubagentUpdate(payload) {
      if (typeof root.dispatchEvent !== "function" || typeof root.CustomEvent !== "function") return;
      try {
        root.dispatchEvent(new root.CustomEvent("pinvou:subagent-update", { detail: payload }));
      } catch (err) {
        // CustomEvent 不可用（极旧 webview）时静默降级：界面还有轮询兜底。
      }
    }

    listen("workflow:agent_progress", function (e) {
      var p = e.payload || {};
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

    listen("workflow:agent_complete", function (e) {
      var p = e.payload || {};
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
      listSubagentTranscripts: listSubagentTranscripts,
      readSubagentTranscript: readSubagentTranscript,
    };
  };
})(typeof window !== "undefined" ? window : globalThis);
