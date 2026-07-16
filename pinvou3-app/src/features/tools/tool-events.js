const notifyComposerToolsChanged = () => {
  try { window.dispatchEvent(new CustomEvent('pinvou:tools-changed')); } catch (_) {}
};

export { notifyComposerToolsChanged };
