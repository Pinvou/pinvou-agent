/**
 * Adapt the Web transport to the same domain API and state slices consumed by
 * the desktop UI. The legacy flat object stays private to this platform layer.
 */
(function () {
  // biome-ignore lint/suspicious/noRedundantUseStrict: verbatim copy of a classic script; strict mode is the payload
  "use strict";

  const platform = window.PinvouPlatform;
  if (!platform || (platform.kind !== "web" && platform.isWeb !== true)) return;

  const flat = window.TauriBridge;
  if (!flat || !flat.available || typeof flat.getState !== "function") return;

  const fields = {
    platform: ["appVersion", "backendOnline", "platformCapabilities"],
    sessions: ["sessions", "archivedSessions", "activeSessionId", "sessionBusy", "draftEpoch", "draftWorkspacePath"],
    chat: ["activeSkill", "artifacts", "artifactChange", "attachments", "busy", "chatItems", "composerDraft", "composerPrefill", "messages", "modeState", "planSnapshot", "queued", "thinking", "tokens", "turnDirtyArtifacts", "turnPresentedArtifacts", "turnTimeline"],
    voice: ["voiceInput", "voiceAsrSetup"],
    knowledge: ["kbModelSetup", "mountedCollection", "mountedCollections", "mountedRemoteCollections", "mountedCollectionsRevision"],
    scheduled: ["scheduledRunContext", "scheduledTaskAutoOpenId", "scheduledTaskBusyAction", "scheduledTaskCreationSessionId", "scheduledTaskDetail", "scheduledTaskError", "scheduledTaskErrorKind", "scheduledTaskLoading", "scheduledTaskPendingGuide", "scheduledTaskRecentRuns", "scheduledTaskRuns", "scheduledTasks", "scheduledTaskSelectionGeneration", "selectedScheduledTaskId"],
    monitor: ["monitor", "monitorError"],
    settings: ["settings", "selectedPet"],
    models: ["activeModelId", "currentSessionModelId", "effectiveModelConfig", "savedModels"],
    vllm: ["vllmBootstrapDone", "vllmBootstrapError", "vllmBootstrapping", "vllmSetup", "vllmSetupAttempt", "vllmSetupDismissed", "vllmSetupPhase"],
    interaction: ["pinvouModal", "pinvouReviews", "pinvouSummoning", "superPermEnabled"],
    computerUse: ["computerUse"],
    personas: ["activePersona", "personaEvents", "personaPool"],
    memory: ["memory"],
    remoteControl: ["webAccess"],
    // projects 域为桌面专属(整域不出现在 bridgeDomainContract 的 Web 域面),
    // 但 APP_BRIDGE_STATE_DOMAINS 的订阅列表双端共享;Web 端补一个空桩
    // (没有 bridge.projects 方法面,列表恒空),防止启动期 getMany 抛
    // "Unknown Tauri bridge state slice: projects"(栈内 #448 的根因)。
    projects: ["projectsList"],
    updater: ["updateCancelling", "updateCheckError", "updateChecking", "updateDownloading", "updateError", "updateInfo", "updateProgress", "updateReady"],
    dependencies: ["deps", "depsChecking", "depsInstallError", "depsInstallProgress", "depsInstalling"]
  };

  function clone(value) {
    if (typeof structuredClone === "function") {
      try { return structuredClone(value); } catch { /* silently fall back to JSON */ } // safari14-ok: typeof-guarded with JSON fallback
    }
    return JSON.parse(JSON.stringify(value));
  }

  function pick(full, domainName) {
    const names = fields[domainName];
    if (!names) throw new Error("Unknown Tauri bridge state slice: " + domainName);
    const result = {};
    names.forEach(function (name) { result[name] = full[name]; });
    return result;
  }

  // Subscriber callbacks pick a fresh outer object on every
  // notification: any state change anywhere (e.g. a streaming token)
  // hands every domain subscriber a new reference and a full re-render.
  // Cache the last (full, slice) per subscriber: when full keeps its
  // reference, reuse the last slice to keep identity stable. Note this
  // is whole-snapshot granularity (the web transport only reuses the
  // same full reference when nothing at all changed), weaker than the
  // desktop bridge's per-domain revision cache: a change in any domain
  // still swaps the outer object of unchanged domains' slices (inner
  // field references remain shared with flat subscribers; the identity
  // sharing contract lives in the web_bridge_domain_contract test and
  // is unaffected). full is rebuilt by the notifier per change, so the
  // same full reference implies this domain's field set cannot have
  // changed.
  function stablePick() {
    let lastFull = null;
    let lastSlice = null;
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
    const full = flat.getState();
    const result = {};
    domains.forEach(function (domainName) { Object.assign(result, pick(full, domainName)); });
    return clone(result);
  }

  function subscribe(domainName, callback) {
    get(domainName);
    const stable = stablePick();
    return flat.subscribe(function (full) {
      callback(stable(full, domainName));
    });
  }

  function subscribeMany(domains, callback) {
    getMany(domains);
    // One stable cache per domain: the stablePick closure memoizes a
    // single (lastFull,lastSlice) slot; sharing one instance across
    // domains would make them overwrite each other.
    const stables = {};
    domains.forEach(function (domainName) { stables[domainName] = stablePick(); });
    // The combined outer object is likewise memoized on the full
    // reference in a single slot: a React setState subscriber can only
    // bail out on whole-object identity, and rebuilding the combined
    // object every round would make even no-change notifications trigger
    // full re-renders, cancelling out the inner slices' identity
    // stability (see useBridge.js's subscribeMany for the consumer).
    let lastFull = null;
    let lastResult = null;
    return flat.subscribe(function (full) {
      if (full !== lastFull) {
        const result = {};
        domains.forEach(function (domainName) { Object.assign(result, stables[domainName](full, domainName)); });
        lastFull = full;
        lastResult = Object.freeze(result);
      }
      callback(lastResult);
    });
  }

  function domain(names, aliases) {
    const result = {};
    names.forEach(function (name) { if (typeof flat[name] === "function") result[name] = flat[name]; });
    Object.keys(aliases || {}).forEach(function (name) {
      const fn = flat[aliases[name]];
      if (typeof fn === "function") result[name] = fn;
    });
    return result;
  }

  // Computer use controls the desktop's own screen, mouse and keyboard, so it
  // must never be driven from the remote web client (the desktop RPC funnel
  // keeps the commands off the access-policy allowlist as well). The domain
  // still exists with rejecting stubs so shared UI code can feature-detect it.
  function computerUseUnsupported() {
    return Promise.reject(new Error("computer use is not supported on the web client"));
  }
  const computerUseStubs = {};
  ["getStatus", "refreshStatus", "grant", "revoke", "stop", "confirm", "deny", "setEnabled", "requestPermissions"]
    .forEach(function (name) { computerUseStubs[name] = computerUseUnsupported; });

  window.TauriBridge = {
    available: true,
    lifecycle: { init: flat.init },
    state: { get, getMany, subscribe, subscribeMany },
    platform: {},
    chat: domain(["sendMessage", "sendMessageToSession", "getComposerDraft", "setComposerDraft", "retryFirstTurn", "prefillComposer", "removeQueued", "prioritizeQueued", "editQueued", "cancelGeneration", "cancelShellTask"]),
    voice: domain(["startVoiceInput", "cancelVoiceAsrSetup", "closeVoiceAsrSetup", "cancelVoiceInput", "clearVoiceInput", "appendVoiceText"]),
    knowledge: domain(["mountCollection", "setCollectionEnabled", "removeCollection", "unmountCollection", "listCollections", "kbModelStatus"]),
    scheduled: domain(["loadScheduledTasks", "loadScheduledTaskRecentRuns", "selectScheduledTask", "refreshScheduledTaskData", "dismissScheduledTaskError", "createScheduledTask", "updateScheduledTask", "pauseScheduledTask", "resumeScheduledTask", "deleteScheduledTask", "runScheduledTaskNow", "startScheduledTaskChat", "openScheduledRunChat", "exitScheduledRunChat"]),
    sessions: domain(["createNewSession", "switchToSession", "deleteSession", "renameSession", "toggleSessionPinned", "archiveSession", "restoreArchivedSession", "getSessionWorkspaceBinding"]),
    monitor: domain(["startMonitorPolling", "stopMonitorPolling", "clearMonitorStats"]),
    settings: domain(["setSelectedPet", "saveSettings", "saveSearchSettings"]),
    feedback: domain(["submitFeedback"]),
    // 厂商版 vLLM 引导是桌面专属面（appUpdate/webAccessAdmin 同理）：
    // 相关命令不在 web access-policy 白名单内,能力位恒 false,Web 端整域为空桩。
    vllm: {},
    models: domain(["saveModel", "revealModelApiKey", "deleteModel", "setActiveModel", "loadSessionModel", "switchModel", "testModelConnection", "getImageInputCapability", "testImageInputCapability", "probeLocalServerKind"]),
    interaction: domain(["toggleSuperPerm", "acceptPlan", "discardPlan", "exitPlanToYolo", "setPlanModeNext", "setModeLane", "getCodePermissionPrefs", "confirmCodeYolo", "syncModeState", "planStuckReplan", "planStuckGo", "submitUserInput", "cancelUserInput", "summonPinvou", "inspectPinvou", "resolvePinvouReview", "dismissPinvouReview", "editLastTurn"]),
    rendering: domain(["renderMarkdown"]),
    // 远程控制(桌面 Web 代理管理)是桌面专属面：Web 端空桩(能力位恒 false,
    // web_access_* 命令不在白名单内)。
    remoteControl: {},
    artifacts: domain(["artifactInfo", "readArtifactText", "writeArtifactText", "readArtifactImageB64", "readArtifactThumbnail", "renderArtifactVisual", "openContainingFolder", "revealSessionFolder", "openScheduledTaskFolder", "openArtifactExternal", "downloadArtifact", "listDeliverableIndex", "openUserExternalUrl"]),
    attachments: domain(["addAttachmentByPath", "addPasteImage", "removeAttachment", "pickAndAttach", "uploadDeviceFiles", "resolveConversationAttachment", "openConversationAttachment", "revealConversationAttachment"]),
    files: domain(["pickFiles", "pickFolders", "pickRebindFolder", "pickFeedbackFiles"]),
    personas: domain(["loadPersonas", "getPersonas", "readPersonaBody", "equipPersona", "unequipPersona", "postCardCreatorIntro", "createPersona", "updatePersona", "deletePersona"]),
    memory: domain(["loadMemoryOverview", "saveMemoryProfilePatch", "updateMemoryItem", "deleteMemoryItem", "confirmMemoryCandidate", "ignoreMemoryCandidate", "neverMemoryCandidate", "organizeMemory", "loadOrganizeHistory"]),
    // 应用内升级是桌面专属面（check/download/install/restart 命令不在 web
    // access-policy 白名单内,appUpdate 能力位恒 false）,Web 端整域为空桩。
    updater: {},
    dependencies: domain(["checkDependencies"]),
    computerUse: computerUseStubs
  };
})();
