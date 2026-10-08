// Backend VisualResult pairs its Chinese `warning` text with a stable
// `warning_code`. Neither preview surface ever renders the raw string: a known
// code maps to the tri-lingual artifactPreview label, and an unknown or
// missing code falls back (banners stay hidden; the unsupported card shows
// `apUnsupported`). `convertFailed` is the LibreOffice conversion path, so it
// doubles as the signal for the dependency-install shortcut.
export const VISUAL_WARNING_LABEL_KEYS = {
  truncatedPages: 'visualWarningTruncatedPages',
  convertFailed: 'visualWarningConvertFailed',
};

export function localizedVisualWarning(visual, labels, fallback = null) {
  if (!visual || !visual.warning_code) return fallback;
  return labels[VISUAL_WARNING_LABEL_KEYS[visual.warning_code]] || fallback;
}

export function visualWarningNeedsDependencyCheck(visual) {
  return visual?.warning_code === 'convertFailed';
}
