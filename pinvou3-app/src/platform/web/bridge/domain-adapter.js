/**
 * Adapt the Web transport to the same domain API and state slices consumed by
 * the desktop UI. The legacy flat object stays private to this platform layer.
 */
(function () {
  "use strict";

  var platform = window.PinvouPlatform;
  if (!platform || (platform.kind !== "web" && platform.isWeb !== true)) return;

  var flat = window.TauriBridge;
  if (!flat || !flat.available || typeof flat.getState !== "function") return;

  var fields = {
    platform: ["appVersion", "backendOnline", "platformCapabilities"],
    sessions: ["sessions", "archivedSessions", "activeSessionId", "sessionBusy", "draftEpoch"],
    chat: ["activeSkill", "artifacts", "artifactChange", "attachmentDragActive", "attachments", "busy", "chatItems", "composerDraft", "composerPrefill", "messages", "modeState", "planSnapshot", "queued", "thinking", "tokens", "turnDirtyArtifacts", "turnPresentedArtifacts", "turnTimeline"],
    voice: ["voiceInput", "voiceAsrSetup"],
    knowledge: ["kbModelSetup", "mountedCollection", "mountedCollections", "mountedCollectionsRevision"],
    scheduled: ["scheduledRunContext", "scheduledTaskAutoOpenId", "scheduledTaskBusyAction", "scheduledTaskCreationSessionId", "scheduledTaskDetail", "scheduledTaskDraft", "scheduledTaskError", "scheduledTaskErrorKind", "scheduledTaskLoading", "scheduledTaskPendingGuide", "scheduledTaskRecentRuns", "scheduledTaskRuns", "scheduledTasks", "scheduledTaskSelectionGeneration", "selectedScheduledTaskId"],
    monitor: ["monitor", "monitorError"],
    settings: ["settings", "selectedPet"],
    models: ["activeModelId", "currentSessionModelId", "effectiveModelConfig", "savedModels"],
    vllm: ["vllmBootstrapDone", "vllmBootstrapError", "vllmBootstrapping", "vllmSetup", "vllmSetupAttempt", "vllmSetupDismissed", "vllmSetupPhase"],
    interaction: ["pinvouModal", "pinvouReviews", "pinvouSummoning", "superPermEnabled"],
    personas: ["activePersona", "personaEvents", "personaPool"],
    memory: ["memory"],
    remoteControl: ["webAccess"],
    updater: ["updateCancelling", "updateCheckError", "updateChecking", "updateDownloading", "updateError", "updateInfo", "updateProgress", "updateReady"],
    dependencies: ["deps", "depsChecking", "depsInstallError", "depsInstalling"]
  };

  function clone(value) {
    if (typeof structuredClone === "function") {
      try { return structuredClone(value); } catch (_) {} // safari14-ok: typeof-guarded with JSON fallback
    }
    return JSON.parse(JSON.stringify(value));
  }

  function pick(full, domainName) {
    var names = fields[domainName];
    if (!names) throw new Error("Unknown Tauri bridge state slice: " + domainName);
    var result = {};
    names.forEach(function (name) { result[name] = full[name]; });
    return result;
  }

  // 订阅回调每次通知都会 pick 出全新外层对象：任何一处状态变化（如流式
  // token）都让所有域订阅者拿到新引用、全量重渲染。逐订阅者缓存上次的
  // (full, slice)：full 未换引用时复用上次 slice，保持身份稳定（桌面端
  // bridge 的 revision-cache 同款契约）。full 由通知方按变更重建，同一 full
  // 引用意味着本域字段集合不可能变化；字段值仍是 full 上的原引用，与 flat
  // 订阅者共享子树的身份共享契约（见 web_bridge_domain_contract 测试）不受影响。
  function stablePick() {
    var lastFull = null;
    var lastSlice = null;
    return function (full, domainName) {
      if (full === lastFull) return lastSlice;
      lastFull = full;
      lastSlice = Object.freeze(pick(full, domainName));
      return lastSlice;
    };
  }

  function get(domainName) {
    return clone(pick(flat.getState(), domainName));
  }

  function getMany(domains) {
    if (!Array.isArray(domains) || domains.length === 0) throw new Error("Tauri bridge state.getMany requires at least one domain");
    var full = flat.getState();
    var result = {};
    domains.forEach(function (domainName) { Object.assign(result, pick(full, domainName)); });
    return clone(result);
  }

  function subscribe(domainName, callback) {
    get(domainName);
    var stable = stablePick();
    return flat.subscribe(function (full) {
      callback(stable(full, domainName));
    });
  }

  function subscribeMany(domains, callback) {
    getMany(domains);
    // 每个域独立 stable 缓存：stablePick 闭包按 (lastFull,lastSlice) 单槽记忆，
    // 共用一个实例会在多域间互相覆盖。
    var stables = {};
    domains.forEach(function (domainName) { stables[domainName] = stablePick(); });
    return flat.subscribe(function (full) {
      var result = {};
      domains.forEach(function (domainName) { Object.assign(result, stables[domainName](full, domainName)); });
      callback(Object.freeze(result));
    });
  }

  function domain(names, aliases) {
    var result = {};
    names.forEach(function (name) { if (typeof flat[name] === "function") result[name] = flat[name]; });
    Object.keys(aliases || {}).forEach(function (name) {
      var fn = flat[aliases[name]];
      if (typeof fn === "function") result[name] = fn;
    });
    return result;
  }

  window.TauriBridge = {
    available: true,
    lifecycle: { init: flat.init },
    state: { get: get, getMany: getMany, subscribe: subscribe, subscribeMany: subscribeMany },
    platform: {},
    chat: domain(["sendMessage", "sendMessageToSession", "getComposerDraft", "setComposerDraft", "retryFirstTurn", "prefillComposer", "removeQueued", "cancelGeneration", "cancelShellTask"]),
    voice: domain(["startVoiceInput", "installVoiceAsr", "closeVoiceAsrSetup", "cancelVoiceInput", "clearVoiceInput", "appendVoiceText", "runVoiceInputDebugAssertions"]),
    knowledge: domain(["downloadKbModel", "cancelKbModel", "mountCollection", "setCollectionEnabled", "removeCollection", "unmountCollection", "listCollections", "kbModelStatus"]),
    scheduled: domain(["loadScheduledTasks", "readScheduledTask", "loadScheduledTaskRuns", "loadScheduledTaskRecentRuns", "selectScheduledTask", "refreshScheduledTaskData", "clearScheduledTaskSelection", "dismissScheduledTaskError", "createScheduledTask", "updateScheduledTask", "pauseScheduledTask", "resumeScheduledTask", "toggleScheduledTaskPinned", "deleteScheduledTask", "runScheduledTaskNow", "pickFolder", "startScheduledTaskChat", "confirmScheduledTaskDraft", "clearScheduledTaskDraft", "openScheduledRunChat", "exitScheduledRunChat"]),
    sessions: domain(["createNewSession", "switchToSession", "deleteSession", "renameSession", "toggleSessionPinned", "archiveSession", "restoreArchivedSession"]),
    monitor: domain(["startMonitorPolling", "stopMonitorPolling", "clearMonitorStats"]),
    settings: domain(["setSelectedPet", "saveSettings", "saveSettingsAndRestart", "saveSearchSettings", "saveSearchSettingsAndRestart", "testSearchProvider"]),
    feedback: domain(["submitFeedback"]),
    vllm: domain(["discoverLocalVllm", "detectLocalVllmSetup", "bootstrapLocalVllm", "dismissVllmSetup", "declineVllmSetup"]),
    models: domain(["getEffectiveModelConfig", "loadModels", "saveModel", "revealModelApiKey", "deleteModel", "setActiveModel", "loadSessionModel", "switchModel", "testModelConnection", "getImageInputCapability", "testImageInputCapability"]),
    interaction: domain(["toggleSuperPerm", "acceptPlan", "discardPlan", "exitPlanToYolo", "setPlanModeNext", "setDraftMode", "setModeLane", "refreshModeDefaults", "syncModeState", "planStuckReplan", "planStuckGo", "submitUserInput", "cancelUserInput", "summonPinvou", "inspectPinvou", "resolvePinvouReview", "dismissPinvouReview", "editLastTurn", "compactNow"]),
    rendering: domain(["renderMarkdown"]),
    remoteControl: domain(["getWebRelaySettings", "setWebRelayAddress", "resetWebRelayAddress"], {
      startRemoteControl: "enableWebAccess",
      stopRemoteControl: "disableWebAccess",
      refreshRemoteControlQr: "rotateWebAccessLink",
      refreshRemoteControlStatus: "refreshWebAccessStatus"
    }),
    artifacts: domain(["artifactInfo", "readArtifactText", "writeArtifactText", "readArtifactImageB64", "readArtifactThumbnail", "renderArtifactVisual", "openContainingFolder", "revealSessionFolder", "openScheduledTaskFolder", "openInSystem", "openArtifactExternal", "downloadArtifact", "listDeliverableIndex", "openExternalUrl", "openUserExternalUrl"]),
    attachments: domain(["addAttachmentByPath", "addPasteImage", "removeAttachment", "clearAttachments", "pickAndAttach", "uploadDeviceFiles", "resolveConversationAttachment", "openConversationAttachment", "revealConversationAttachment"]),
    resolutions: domain(["markResolved"]),
    files: domain(["pickFiles", "pickFolders", "pickFeedbackFiles"]),
    personas: domain(["loadPersonas", "getPersonas", "readPersonaBody", "equipPersona", "unequipPersona", "postCardCreatorIntro", "createPersona", "updatePersona", "deletePersona"]),
    memory: domain(["loadMemoryOverview", "saveMemoryProfilePatch", "deleteMemoryPreference", "updateMemoryItem", "deleteMemoryItem", "archiveRecentWorkMemory", "confirmMemoryCandidate", "ignoreMemoryCandidate", "neverMemoryCandidate"]),
    updater: domain(["checkForUpdate", "downloadAndInstallUpdate", "cancelUpdate", "restartApp"]),
    dependencies: domain(["checkDependencies", "installDependencies"])
  };
})();
