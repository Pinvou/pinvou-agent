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
    var listen = context.listen;

  // Model files may be installed by the bundled shared-knowledge host after
  // desktop startup. Keep the authoritative bridge snapshot synchronized with
  // status queries and peer-process installs so stale startup state cannot win.
  listen("kb_model:status", function (e) {
    var status = e && e.payload;
    if (!status) return;
    state.kbModelSetup = Object.assign({}, state.kbModelSetup, {
      startupLoading: !!status.loading,
      startupReady: typeof status.ready === "boolean" ? status.ready : state.kbModelSetup.startupReady,
      status: status,
    });
    notify();
  });
  // 知识库 embedding 模型按需下载（下载 → 校验 → 解压部署 → 热加载），进度走
  // kb_model:progress 事件。repair=true 时重新下载并验证候选模型，成功后原子替换旧目录。
  async function downloadKbModel(repair) {
    if (state.kbModelSetup.downloading) return state.kbModelSetup.status;
    state.kbModelSetup = Object.assign({}, state.kbModelSetup, { downloading: true, error: null, progress: { stage: "start" } });
    notify();
    try {
      var st = await invoke("kb_model_download", { repair: !!repair });
      state.kbModelSetup = Object.assign({}, state.kbModelSetup, {
        downloading: false,
        startupLoading: false,
        startupReady: st && typeof st.ready === "boolean" ? st.ready : true,
        status: st,
        progress: { stage: "done" },
      });
      notify();
      return st;
    } catch (e) {
      var failedStatus = await invoke("kb_model_status").catch(function () { return null; });
      state.kbModelSetup = Object.assign({}, state.kbModelSetup, {
        downloading: false,
        startupLoading: false,
        startupReady: failedStatus && typeof failedStatus.ready === "boolean" ? failedStatus.ready : false,
        status: failedStatus || state.kbModelSetup.status,
        error: String(e),
      });
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
