/**
 * knowledge-model feature for the Tauri bridge.
 * Registered before bridge.js builds the backwards-compatible facade.
 */
(function (root) {
  "use strict";
  var registry = root.__PINVOU_TAURI_BRIDGE_FEATURES__ = root.__PINVOU_TAURI_BRIDGE_FEATURES__ || {};
  registry["knowledge-model"] = function (context) {
    var state = context.state;
    var notify = context.notify;
    var invoke = context.invoke;
  // 知识库 embedding 模型按需下载（下载 → 校验 → 解压部署 → 热加载），进度走
  // kb_model:progress 事件。resolve 时模型已就绪，调用方据 status.installed 收起 gate。
  async function downloadKbModel() {
    if (state.kbModelSetup.downloading) return state.kbModelSetup.status;
    state.kbModelSetup = Object.assign({}, state.kbModelSetup, { downloading: true, error: null, progress: { stage: "start" } });
    notify();
    try {
      var st = await invoke("kb_model_download");
      state.kbModelSetup = Object.assign({}, state.kbModelSetup, { downloading: false, status: st, progress: { stage: "done" } });
      notify();
      return st;
    } catch (e) {
      state.kbModelSetup = Object.assign({}, state.kbModelSetup, { downloading: false, error: String(e) });
      notify();
      throw e;
    }
  }

  function cancelKbModel() {
    invoke("kb_model_cancel").catch(function () {});
  }
    return {
      downloadKbModel: downloadKbModel,
      cancelKbModel: cancelKbModel
    };
  };
})(window);
