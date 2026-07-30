export const desktopBridgeApi = {
  lifecycle: ['init'],
  state: ['get', 'getMany', 'subscribe', 'subscribeMany'],
  platform: ['loadPlatformCapabilities', 'refreshConnectorAuthGates'],
  chat: ['cancelGeneration', 'cancelShellTask', 'getComposerDraft', 'prefillComposer', 'removeQueued', 'retryFirstTurn', 'sendMessage', 'sendMessageToSession', 'setComposerDraft'],
  voice: ['appendVoiceText', 'cancelVoiceAsrSetup', 'cancelVoiceInput', 'clearVoiceInput', 'closeVoiceAsrSetup', 'installVoiceAsr', 'runVoiceInputDebugAssertions', 'startVoiceInput'],
  knowledge: ['cancelKbModel', 'downloadKbModel', 'kbModelStatus', 'listCollections', 'loadKnowledgeEmbedderAfterFirstFrame', 'mountCollection', 'removeCollection', 'setCollectionEnabled', 'unmountCollection'],
  scheduled: ['clearScheduledTaskDraft', 'clearScheduledTaskSelection', 'confirmScheduledTaskDraft', 'createScheduledTask', 'deleteScheduledTask', 'dismissScheduledTaskError', 'exitScheduledRunChat', 'loadScheduledTaskRecentRuns', 'loadScheduledTaskRuns', 'loadScheduledTasks', 'openScheduledRunChat', 'pauseScheduledTask', 'pickFolder', 'readScheduledTask', 'refreshScheduledTaskData', 'resumeScheduledTask', 'runScheduledTaskNow', 'selectScheduledTask', 'startScheduledTaskChat', 'toggleScheduledTaskPinned', 'updateScheduledTask'],
  sessions: ['archiveSession', 'createNewSession', 'deleteSession', 'renameSession', 'restoreArchivedSession', 'switchToSession', 'toggleSessionPinned'],
  monitor: ['clearMonitorStats', 'startMonitorPolling', 'stopMonitorPolling'],
  settings: ['saveSearchSettings', 'saveSearchSettingsAndRestart', 'saveSettings', 'saveSettingsAndRestart', 'setSelectedPet', 'testSearchProvider'],
  feedback: ['submitFeedback'],
  vllm: ['bootstrapLocalVllm', 'declineVllmSetup', 'detectLocalVllmSetup', 'dismissVllmSetup', 'discoverLocalVllm'],
  models: ['deleteModel', 'getEffectiveModelConfig', 'loadModels', 'loadSessionModel', 'revealModelApiKey', 'saveModel', 'setActiveModel', 'switchModel', 'testModelConnection'],
  interaction: ['acceptPlan', 'cancelUserInput', 'compactNow', 'discardPlan', 'dismissPinvouReview', 'editLastTurn', 'exitPlanToYolo', 'inspectPinvou', 'planStuckGo', 'planStuckReplan', 'resolvePinvouReview', 'setPlanModeNext', 'submitUserInput', 'summonPinvou', 'toggleSuperPerm'],
  rendering: ['renderMarkdown'],
  remoteControl: ['getWebRelaySettings', 'refreshRemoteControlQr', 'refreshRemoteControlStatus', 'resetWebRelayAddress', 'setWebRelayAddress', 'startRemoteControl', 'stopRemoteControl'],
  collaboration: ['collaborationAcceptTask', 'collaborationCompleteLocalTask', 'collaborationCreateLocalTask', 'collaborationCreateProject', 'collaborationCreateTask', 'collaborationExportMemberInvite', 'collaborationGetTaskContext', 'collaborationJoinProject', 'collaborationListLocalTasks', 'collaborationRegisterIdentity', 'collaborationRejectTask', 'collaborationStart', 'collaborationUpdateLocalTask', 'openCollaborationMockSession', 'openCollaborationTaskSession', 'refreshCollaborationConfig', 'refreshCollaborationStatus'],
  artifacts: ['artifactInfo', 'downloadArtifact', 'listDeliverableIndex', 'listDeliverables', 'openArtifactExternal', 'openContainingFolder', 'openExternalUrl', 'openInSystem', 'openScheduledTaskFolder', 'openUserExternalUrl', 'readArtifactImageB64', 'readArtifactText', 'readArtifactThumbnail', 'renderArtifactVisual', 'revealSessionFolder', 'writeArtifactText'],
  attachments: ['addAttachmentByPath', 'addPasteImage', 'clearAttachments', 'openConversationAttachment', 'pickAndAttach', 'removeAttachment', 'resolveConversationAttachment', 'revealConversationAttachment', 'uploadDeviceFiles'],
  resolutions: ['markResolved'],
  workflow: ['activateSkill', 'addMaterialsToSession', 'approveWorkflowGate', 'attachRun', 'closeDemo', 'closeWorkflowDrawer', 'deactivateSkill', 'getGateReport', 'getRoleLogs', 'getRoleOutputs', 'getRolePrompt', 'listWorkflows', 'loadSkills', 'openDemo', 'pickAndAddMaterials', 'rejectWorkflowGate', 'resetWorkflowRun', 'resumeWorkflowOnBoot', 'retryWorkflowRole', 'selectWorkflowRole', 'setCurrentPhase', 'startWorkflowTask', 'stopWorkflowTask', 'submitWorkflowUserInput'],
  files: ['pickFeedbackFiles', 'pickFiles', 'pickFolders'],
  personas: ['createPersona', 'deletePersona', 'equipPersona', 'getPersonas', 'loadPersonas', 'postCardCreatorIntro', 'readPersonaBody', 'unequipPersona', 'updatePersona'],
  memory: ['archiveRecentWorkMemory', 'confirmMemoryCandidate', 'deleteMemoryItem', 'deleteMemoryPreference', 'ignoreMemoryCandidate', 'loadMemoryOverview', 'neverMemoryCandidate', 'saveMemoryProfilePatch', 'updateMemoryItem'],
  updater: ['cancelUpdate', 'checkForUpdate', 'downloadAndInstallUpdate', 'restartApp'],
  dependencies: ['checkDependencies', 'installDependencies'],
};

// These methods intentionally depend on desktop lifecycle or local machine
// resources. Web may omit them, but every other desktop method must exist.
export const desktopOnlyBridgeApi = {
  platform: ['loadPlatformCapabilities', 'refreshConnectorAuthGates'],
  voice: ['cancelVoiceAsrSetup'],
  knowledge: ['loadKnowledgeEmbedderAfterFirstFrame'],
};

export function expectedWebBridgeApi() {
  return Object.fromEntries(Object.entries(desktopBridgeApi).map(([domain, methods]) => {
    const desktopOnly = new Set(desktopOnlyBridgeApi[domain] || []);
    return [domain, methods.filter(method => !desktopOnly.has(method))];
  }));
}
