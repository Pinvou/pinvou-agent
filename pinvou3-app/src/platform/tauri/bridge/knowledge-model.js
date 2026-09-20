/**
 * knowledge-model feature for the Tauri bridge.
 * Registered before bridge.js builds the backwards-compatible facade.
 */
(function (root) {
  // biome-ignore lint/suspicious/noRedundantUseStrict: verbatim classic-script artifact; strict mode is part of the payload
  "use strict";
  // biome-ignore lint/suspicious/noAssignInExpressions: registry bootstrap of the verbatim payload; splitting statements would diverge from the artifact
  const registry = root.__PINVOU_TAURI_BRIDGE_FEATURES__ = root.__PINVOU_TAURI_BRIDGE_FEATURES__ || {};
  registry["knowledge-model"] = function (context) {let pinvouSharedtauriKnowledgeModelCache = null;
function pinvouSharedtauriKnowledgeModel() {
  if (!pinvouSharedtauriKnowledgeModelCache) pinvouSharedtauriKnowledgeModelCache = window.PinvouBridgeShared.create("tauriKnowledgeModel", { state, notify, invoke });
  return pinvouSharedtauriKnowledgeModelCache;
}


    const state = context.state;
    const notify = context.notify;
    const invoke = context.invoke;
    const listen = context.listen;

  // Model files may be installed by the bundled shared-knowledge host after
  // desktop startup. Keep the authoritative bridge snapshot synchronized with
  // status queries and peer-process installs so stale startup state cannot win.
  listen("kb_model:status", function (e) {
    const status = e && e.payload;
    if (!status) return;
    state.kbModelSetup = Object.assign({}, state.kbModelSetup, {
      startupLoading: !!status.loading,
      startupReady: typeof status.ready === "boolean" ? status.ready : state.kbModelSetup.startupReady,
      status,
    });
    notify();
  });
  // 知识库 embedding 模型按需下载（下载 → 校验 → 解压部署 → 热加载），进度走
  // kb_model:progress 事件。repair=true 时重新下载并验证候选模型，成功后原子替换旧目录。
async function downloadKbModel(repair) { return pinvouSharedtauriKnowledgeModel().downloadKbModel(repair); }

    return {
      downloadKbModel
    };
  };
})(window);
