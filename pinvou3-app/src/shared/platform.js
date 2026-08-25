const DEFAULT_DESKTOP_CAPABILITIES = Object.freeze({
  desktopChrome: true,
  detachWindows: true,
  pet: true,
  oauth: true,
  externalAuth: true,
  superPermission: true,
  appUpdate: true,
  dependencyInstall: true,
  localModelSetup: true,
  externalSystemOpen: true,
  webAccessAdmin: true,
  desktopNotifications: true,
  hostFilePicker: true,
  artifactDownload: false,
  browserMicrophone: true,
  sessionModelSwitch: true,
  modelManagement: true,
  toolStoreMutations: true,
  multiAgent: true,
  acpCodeMode: true,
  // 桌面端的系统选择器本就选择"本机"文件,无需浏览器上传通道;显式关闭
  // 让附件按钮在桌面保持原有单入口行为。
  deviceFileUpload: false,
});

const fallbackPlatform = Object.freeze({
  kind: 'desktop',
  isWeb: false,
  capabilities: DEFAULT_DESKTOP_CAPABILITIES,
});

export const platform = window.PinvouPlatform || fallbackPlatform;
export const isWeb = platform.kind === 'web' || platform.isWeb === true;

export function can(capability) {
  const capabilities = platform.capabilities || DEFAULT_DESKTOP_CAPABILITIES;
  // Browser capabilities are an allowlist: newly introduced desktop-only
  // features must stay hidden until the Web adapter opts in explicitly.
  if (isWeb && typeof platform.can === 'function') return platform.can(capability) === true;
  if (isWeb) return capabilities[capability] === true;
  return capabilities[capability] !== false;
}

export function canInvoke(command) {
  if (!isWeb) return true;
  return typeof platform.canInvoke === 'function' && platform.canInvoke(command) === true;
}

export function onPlatformConnectionChange(listener) {
  if (!isWeb || typeof platform.onConnectionChange !== 'function') return () => {};
  return platform.onConnectionChange(listener);
}
