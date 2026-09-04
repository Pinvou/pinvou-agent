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
    sessions: ["sessions", "archivedSessions", "activeSessionId", "sessionBusy", "draftEpoch"],
    chat: ["activeSkill", "artifacts", "artifactChange", "attachments", "busy", "chatItems", "composerDraft", "composerPrefill", "messages", "modeState", "planSnapshot", "queued", "thinking", "tokens", "turnDirtyArtifacts", "turnPresentedArtifacts", "turnTimeline"],
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

  window.TauriBridge = {
    available: true,
    lifecycle: { init: flat.init },
    state: { get, getMany, subscribe, subscribeMany },
    platform: {},
    chat: domain(["sendMessage", "sendMessageToSession", "getComposerDraft", "setComposerDraft", "retryFirstTurn", "prefillComposer", "removeQueued", "prioritizeQueued", "editQueued", "cancelGeneration", "cancelShellTask"]),
    voice: domain(["startVoiceInput", "installVoiceAsr", "closeVoiceAsrSetup", "cancelVoiceInput", "clearVoiceInput", "appendVoiceText", "runVoiceInputDebugAssertions"]),
    knowledge: domain(["downloadKbModel", "cancelKbModel", "mountCollection", "setCollectionEnabled", "removeCollection", "unmountCollection", "listCollections", "kbModelStatus"]),
    scheduled: domain(["loadScheduledTasks", "readScheduledTask", "loadScheduledTaskRuns", "loadScheduledTaskRecentRuns", "selectScheduledTask", "refreshScheduledTaskData", "clearScheduledTaskSelection", "dismissScheduledTaskError", "createScheduledTask", "updateScheduledTask", "pauseScheduledTask", "resumeScheduledTask", "toggleScheduledTaskPinned", "deleteScheduledTask", "runScheduledTaskNow", "pickFolder", "startScheduledTaskChat", "confirmScheduledTaskDraft", "clearScheduledTaskDraft", "openScheduledRunChat", "exitScheduledRunChat"]),
    sessions: domain(["createNewSession", "switchToSession", "deleteSession", "renameSession", "toggleSessionPinned", "archiveSession", "restoreArchivedSession", "getSessionWorkspaceBinding"]),
    monitor: domain(["startMonitorPolling", "stopMonitorPolling", "clearMonitorStats"]),
    settings: domain(["setSelectedPet", "saveSettings", "saveSettingsAndRestart", "saveSearchSettings", "saveSearchSettingsAndRestart", "testSearchProvider"]),
    feedback: domain(["submitFeedback"]),
    vllm: domain(["discoverLocalVllm", "detectLocalVllmSetup", "bootstrapLocalVllm", "dismissVllmSetup", "declineVllmSetup"]),
    models: domain(["getEffectiveModelConfig", "loadModels", "saveModel", "revealModelApiKey", "deleteModel", "setActiveModel", "loadSessionModel", "switchModel", "testModelConnection", "getImageInputCapability", "testImageInputCapability", "probeLocalServerKind"]),
    interaction: domain(["toggleSuperPerm", "acceptPlan", "discardPlan", "exitPlanToYolo", "setPlanModeNext", "setDraftMode", "setModeLane", "refreshModeDefaults", "getCodePermissionPrefs", "confirmCodeYolo", "syncModeState", "planStuckReplan", "planStuckGo", "submitUserInput", "cancelUserInput", "summonPinvou", "inspectPinvou", "resolvePinvouReview", "dismissPinvouReview", "editLastTurn", "compactNow"]),
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
