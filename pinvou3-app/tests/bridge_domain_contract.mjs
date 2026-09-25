export const desktopBridgeApi = {
  lifecycle: ['init'],
  state: ['get', 'getMany', 'subscribe', 'subscribeMany'],
  platform: ['refreshConnectorAuthGates'],
  chat: ['cancelGeneration', 'cancelShellTask', 'editQueued', 'getComposerDraft', 'interruptAndSendQueued', 'prefillComposer', 'prioritizeQueued', 'removeQueued', 'retryFirstTurn', 'sendMessage', 'sendMessageToSession', 'setComposerDraft'],
  voice: ['appendVoiceText', 'cancelVoiceAsrSetup', 'cancelVoiceInput', 'clearVoiceInput', 'closeVoiceAsrSetup', 'installVoiceAsr', 'setVoiceShortcutEnabled', 'startVoiceInput', 'syncVoiceShortcutRecording'],
  knowledge: ['downloadKbModel', 'kbModelStatus', 'listCollections', 'loadKnowledgeEmbedderAfterFirstFrame', 'mountCollection', 'mountRemoteCollection', 'removeCollection', 'removeRemoteCollection', 'setCollectionEnabled', 'setRemoteCollectionEnabled', 'unmountCollection'],
  scheduled: ['createScheduledTask', 'deleteScheduledTask', 'dismissScheduledTaskError', 'exitScheduledRunChat', 'loadScheduledTaskRecentRuns', 'loadScheduledTasks', 'openScheduledRunChat', 'pauseScheduledTask', 'refreshScheduledTaskData', 'resumeScheduledTask', 'runScheduledTaskNow', 'selectScheduledTask', 'startScheduledTaskChat', 'updateScheduledTask'],
  sessions: ['archiveSession', 'createNewSession', 'deleteSession', 'exportSessionArchive', 'getSessionWorkspaceBinding', 'pickDraftWorkspace', 'renameSession', 'restoreArchivedSession', 'setDraftWorkspace', 'switchToSession', 'toggleSessionPinned'],
  monitor: ['clearMonitorStats', 'startMonitorPolling', 'stopMonitorPolling'],

  settings: ['listBuiltinFeatures', 'saveSearchSettings', 'saveSearchSettingsAndRestart', 'saveSettings', 'setBuiltinFeatureEnabled', 'setSelectedPet'],
  feedback: ['submitFeedback'],
  vllm: ['bootstrapLocalVllm', 'declineVllmSetup', 'detectLocalVllmSetup', 'dismissVllmSetup', 'discoverLocalVllm'],
  multiAgent: ['listSubagentTranscripts', 'readSubagentTranscript'],
  models: ['deleteModel', 'getImageInputCapability', 'loadSessionModel', 'probeLocalServerKind', 'revealModelApiKey', 'saveModel', 'setActiveModel', 'switchModel', 'testImageInputCapability', 'testModelConnection'],
  interaction: ['acceptPlan', 'cancelUserInput', 'confirmCodeYolo', 'discardPlan', 'dismissPinvouReview', 'editLastTurn', 'exitPlanToYolo', 'getCodePermissionPrefs', 'inspectPinvou', 'planStuckGo', 'planStuckReplan', 'resolvePinvouReview', 'setModeLane', 'setMultiAgentMode', 'setPlanModeNext', 'submitUserInput', 'summonPinvou', 'syncModeState', 'toggleSuperPerm'],
  rendering: ['renderMarkdown'],
  remoteControl: ['getWebRelaySettings', 'refreshRemoteControlQr', 'setWebRelayAddress', 'startRemoteControl', 'stopRemoteControl'],
  artifacts: ['artifactInfo', 'downloadArtifact', 'listDeliverableIndex', 'openArtifactExternal', 'openContainingFolder', 'openScheduledTaskFolder', 'openUserExternalUrl', 'readArtifactImageB64', 'readArtifactText', 'readArtifactThumbnail', 'renderArtifactVisual', 'revealSessionFolder', 'writeArtifactText'],
  attachments: ['addAttachmentByPath', 'addPasteImage', 'openConversationAttachment', 'pickAndAttach', 'removeAttachment', 'resolveConversationAttachment', 'revealConversationAttachment', 'uploadDeviceFiles'],
  files: ['pickFeedbackFiles', 'pickFiles', 'pickFolders', 'pickRebindFolder'],
  personas: ['createPersona', 'deletePersona', 'equipPersona', 'getPersonas', 'loadPersonas', 'postCardCreatorIntro', 'readPersonaBody', 'unequipPersona', 'updatePersona'],
  memory: ['confirmMemoryCandidate', 'deleteMemoryItem', 'ignoreMemoryCandidate', 'loadMemoryOverview', 'loadOrganizeHistory', 'neverMemoryCandidate', 'organizeMemory', 'saveMemoryProfilePatch', 'updateMemoryItem'],
  updater: ['cancelUpdate', 'checkForUpdate', 'downloadAndInstallUpdate', 'restartApp'],
  dependencies: ['checkDependencies', 'installDependencies'],
  projects: ['createProject', 'deleteProject', 'loadProjects', 'moveSessionToProject', 'rebindWorkspaceRoot', 'renameProject'],
  // Computer use drives the local machine: the desktop backend exposes it, the
  // web surface carries only rejecting stubs (the RPC allowlist excludes the
  // commands entirely, same policy as browser:*).
  computerUse: ['confirm', 'deny', 'getStatus', 'grant', 'refreshStatus', 'requestPermissions', 'revoke', 'setEnabled', 'stop'],
};

// These methods intentionally depend on desktop lifecycle or local machine
// resources. Web may omit them, but every other desktop method must exist.
export const desktopOnlyBridgeApi = {
  platform: ['refreshConnectorAuthGates'],
  voice: ['installVoiceAsr', 'setVoiceShortcutEnabled', 'syncVoiceShortcutRecording'],
  knowledge: ['downloadKbModel', 'loadKnowledgeEmbedderAfterFirstFrame', 'mountRemoteCollection', 'removeRemoteCollection', 'setRemoteCollectionEnabled'],
  // 多智能体开关是桌面专属操作（ADR-0006）：Web 端只读呈现。
  interaction: ['setMultiAgentMode'],
  // Draft workspace selection needs the system directory dialog (TAURI.dialog)
  // and the create_session workspacePath parameter channel; the Web side has no
  // such backend, so ChatView's method-existence guard skips rendering the
  // selector.
  // Session archive export writes the local-disk tar.xz via a native save
  // dialog over ~/.pinvou3/sessions; web keeps no local session store.
  sessions: ['exportSessionArchive', 'pickDraftWorkspace', 'setDraftWorkspace'],

  // The queued chip's zap-send goes through the foundation EnginePool and needs
  // the Tauri command channel; web has no such backend.
  chat: ['interruptAndSendQueued'],
  // Builtin feature toggles (list_builtin_features / set_builtin_feature_enabled)
  // are desktop Rust command channels with no web backend. list_builtin_features
  // is consumed by ChatView (the session-mention feature gate, PR #586);
  // set_builtin_feature_enabled is the contract hook for future per-feature
  // settings pages — no consumer yet.
  // saveSettingsAndRestart/saveSearchSettingsAndRestart restart the desktop
  // process in place; the web host has no restart channel.
  settings: ['saveSearchSettingsAndRestart', 'listBuiltinFeatures', 'setBuiltinFeatureEnabled'],
  // Vendor-edition one-click vLLM bootstrap is a vendor-edition desktop surface: the web capability bit is always
  // false and the related commands are not in the access-policy allowlist.
  vllm: ['bootstrapLocalVllm', 'declineVllmSetup', 'detectLocalVllmSetup', 'dismissVllmSetup', 'discoverLocalVllm'],
  // In-app upgrade: check/download/install/restart all depend on the local package manager; on the web
  // only the version-number read is available (the appUpdate capability bit is always false).
  updater: ['cancelUpdate', 'checkForUpdate', 'downloadAndInstallUpdate', 'restartApp'],
  // Remote control (desktop web-proxy management) depends on the web_access_* admin commands and is unavailable
  // on the web (the webAccessAdmin capability bit is always false).
  remoteControl: ['getWebRelaySettings', 'refreshRemoteControlQr', 'setWebRelayAddress', 'startRemoteControl', 'stopRemoteControl'],
  // One-click install of missing dependencies elevates via pkexec apt and is unavailable on the web (the dependencyInstall
  // capability bit is always false); the re-check (check_dependencies) is shared by both hosts.
  dependencies: ['installDependencies'],
};

// 整域桌面专属：Web 端连域都不存在（区别于 platform 这类"空域仍在"）。
// 后端 remote_control 漏斗另有权威封禁。projects 是本地会话归档分组
// （~/.pinvou3/projects/projects.json），Web 端没有对应后端。
export const desktopOnlyBridgeDomains = ['multiAgent', 'projects'];

export function expectedWebBridgeApi() {
  return Object.fromEntries(
    Object.entries(desktopBridgeApi)
      .filter(([domain]) => !desktopOnlyBridgeDomains.includes(domain))
      .map(([domain, methods]) => {
        const desktopOnly = new Set(desktopOnlyBridgeApi[domain] || []);
        return [domain, methods.filter(method => !desktopOnly.has(method))];
      }),
  );
}
