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
    workflow: ["workflow"],
    memory: ["memory"],
    remoteControl: ["webAccess"],
    updater: ["updateCancelling", "updateCheckError", "updateChecking", "updateDownloading", "updateError", "updateInfo", "updateProgress", "updateReady"],
    dependencies: ["deps", "depsChecking", "depsInstallError", "depsInstalling"]
  };

  function clone(value) {
    if (typeof structuredClone === "function") {
      try { return structuredClone(value); } catch (_) {}
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
    return flat.subscribe(function () { callback(get(domainName)); });
  }

  function subscribeMany(domains, callback) {
    getMany(domains);
    return flat.subscribe(function () { callback(getMany(domains)); });
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
    interaction: domain(["toggleSuperPerm", "acceptPlan", "discardPlan", "exitPlanToYolo", "setPlanModeNext", "planStuckReplan", "planStuckGo", "submitUserInput", "cancelUserInput", "summonPinvou", "inspectPinvou", "resolvePinvouReview", "dismissPinvouReview", "editLastTurn", "compactNow"]),
    rendering: domain(["renderMarkdown"]),
    remoteControl: domain(["getWebRelaySettings", "setWebRelayAddress", "resetWebRelayAddress"], {
      startRemoteControl: "enableWebAccess",
      stopRemoteControl: "disableWebAccess",
      refreshRemoteControlQr: "rotateWebAccessLink",
      refreshRemoteControlStatus: "refreshWebAccessStatus"
    }),
    artifacts: domain(["artifactInfo", "readArtifactText", "writeArtifactText", "readArtifactImageB64", "readArtifactThumbnail", "renderArtifactVisual", "openContainingFolder", "revealSessionFolder", "openScheduledTaskFolder", "openInSystem", "openArtifactExternal", "downloadArtifact", "listDeliverables", "listDeliverableIndex", "openExternalUrl", "openUserExternalUrl"]),
    attachments: domain(["addAttachmentByPath", "addPasteImage", "removeAttachment", "clearAttachments", "pickAndAttach", "uploadDeviceFiles", "resolveConversationAttachment", "openConversationAttachment", "revealConversationAttachment"]),
    resolutions: domain(["markResolved"]),
    workflow: domain(["loadSkills", "activateSkill", "deactivateSkill", "openDemo", "closeDemo", "setCurrentPhase", "startWorkflowTask", "stopWorkflowTask", "listWorkflows", "resetWorkflowRun", "selectWorkflowRole", "closeWorkflowDrawer", "getRolePrompt", "getRoleOutputs", "getGateReport", "getRoleLogs", "submitWorkflowUserInput", "pickAndAddMaterials", "addMaterialsToSession", "attachRun", "resumeWorkflowOnBoot", "approveWorkflowGate", "rejectWorkflowGate", "retryWorkflowRole"]),
    files: domain(["pickFiles", "pickFolders", "pickFeedbackFiles"]),
    personas: domain(["loadPersonas", "getPersonas", "readPersonaBody", "equipPersona", "unequipPersona", "postCardCreatorIntro", "createPersona", "updatePersona", "deletePersona"]),
    memory: domain(["loadMemoryOverview", "saveMemoryProfilePatch", "deleteMemoryPreference", "updateMemoryItem", "deleteMemoryItem", "archiveRecentWorkMemory", "confirmMemoryCandidate", "ignoreMemoryCandidate", "neverMemoryCandidate"]),
    updater: domain(["checkForUpdate", "downloadAndInstallUpdate", "cancelUpdate", "restartApp"]),
    dependencies: domain(["checkDependencies", "installDependencies"])
  };
})();
