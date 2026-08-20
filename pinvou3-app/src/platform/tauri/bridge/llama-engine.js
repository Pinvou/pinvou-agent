/**
 * llama-engine feature for the Tauri bridge.
 * Registered before bridge.js builds the backwards-compatible facade.
 *
 * 本地多模态引擎：一键下载 llama.cpp 引擎 + GGUF 视觉模型，启动本地
 * llama-server 提供 OpenAI 兼容端点，供底座 image_analyze 工具向纯文本
 * LLM 描述图像。进度走 llama-engine:progress，生命周期状态走
 * llama-engine:state（启动/停止/崩溃/自愈时推送）。
 */
(function (root) {
  "use strict";
  var registry = root.__PINVOU_TAURI_BRIDGE_FEATURES__ = root.__PINVOU_TAURI_BRIDGE_FEATURES__ || {};
  registry["llama-engine"] = function (context) {
    var state = context.state;
    var notify = context.notify;
    var invoke = context.invoke;

    function merge(extra) {
      state.llamaEngineSetup = Object.assign({}, state.llamaEngineSetup, extra);
    }

    function refreshStatus() {
      return invoke("llama_engine_status").then(function (st) {
        merge({ status: st, error: null, progress: null });
        notify();
        return st;
      }).catch(function (e) {
        merge({ error: String(e) });
        notify();
        throw e;
      });
    }

    async function installEngine() {
      if (state.llamaEngineSetup.downloading) return;
      merge({ downloading: true, downloadingItem: "engine", error: null, progress: { stage: "start", item: "engine" } });
      notify();
      try {
        await invoke("llama_engine_install_engine");
        await refreshStatus();
      } catch (e) {
        merge({ downloading: false, downloadingItem: null, error: String(e), progress: null });
        notify();
        throw e;
      }
    }

    async function installModel(modelId) {
      if (state.llamaEngineSetup.downloading) return;
      merge({ downloading: true, downloadingItem: "model", error: null, progress: { stage: "start", item: "model", modelId: modelId } });
      notify();
      try {
        await invoke("llama_engine_install_model", { model: modelId });
        await refreshStatus();
      } catch (e) {
        merge({ downloading: false, downloadingItem: null, error: String(e), progress: null });
        notify();
        throw e;
      }
    }

    function cancelDownload() {
      invoke("llama_engine_cancel_download").catch(function () {});
      merge({ downloading: false, downloadingItem: null });
      notify();
    }

    async function startEngine(modelId, device) {
      if (state.llamaEngineSetup.starting) return;
      merge({ starting: true, error: null });
      notify();
      try {
        await invoke("llama_engine_start", { model: modelId, device: device });
        await refreshStatus();
      } catch (e) {
        merge({ starting: false, error: String(e) });
        notify();
        throw e;
      }
    }

    async function stopEngine() {
      try {
        await invoke("llama_engine_stop");
      } catch (e) {
        merge({ error: String(e) });
        notify();
        throw e;
      }
      await refreshStatus();
    }

    async function deleteModel(modelId) {
      try {
        await invoke("llama_engine_delete_model", { model: modelId });
      } catch (e) {
        merge({ error: String(e) });
        notify();
        throw e;
      }
      await refreshStatus();
    }

    return {
      refreshStatus: refreshStatus,
      installEngine: installEngine,
      installModel: installModel,
      cancelDownload: cancelDownload,
      startEngine: startEngine,
      stopEngine: stopEngine,
      deleteModel: deleteModel
    };
  };
})(window);
