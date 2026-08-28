const DEFAULT_CONTAINER_WIDTH = 1200;
const MIN_RATIO = 0.1;
const MAX_RATIO = 0.9;

function finitePositive(value, fallback) {
  return Number.isFinite(value) && value > 0 ? value : fallback;
}

export function normalizeSidePanelRatio(value, fallback = 0.42) {
  const safeFallback = Number.isFinite(fallback) ? fallback : 0.42;
  const ratio = Number.isFinite(value) ? value : safeFallback;
  return Math.max(MIN_RATIO, Math.min(MAX_RATIO, ratio));
}

export function sidePanelWidthBounds(
  containerWidth,
  { minWidth = 360, minMainWidth = 520, maxWidthRatio = 0.65 } = {},
) {
  const width = finitePositive(containerWidth, DEFAULT_CONTAINER_WIDTH);
  const canSplit = width >= minWidth + minMainWidth;
  const maximum = canSplit
    ? Math.max(
      minWidth,
      Math.min(Math.round(width * maxWidthRatio), width - minMainWidth),
    )
    : width;
  return { containerWidth: width, canSplit, minimum: minWidth, maximum };
}

export function resolveSidePanelLayout(
  containerWidth,
  preferredRatio,
  constraints = {},
) {
  const bounds = sidePanelWidthBounds(containerWidth, constraints);
  const ratio = normalizeSidePanelRatio(preferredRatio);
  if (!bounds.canSplit) {
    return { ...bounds, overlay: true, preferredRatio: ratio, width: bounds.containerWidth };
  }
  const preferredWidth = Math.round(bounds.containerWidth * ratio);
  return {
    ...bounds,
    overlay: false,
    preferredRatio: ratio,
    width: Math.max(bounds.minimum, Math.min(preferredWidth, bounds.maximum)),
  };
}

export function sidePanelRatioFromWidth(width, containerWidth, fallback = 0.42) {
  const safeContainerWidth = finitePositive(containerWidth, DEFAULT_CONTAINER_WIDTH);
  const safeWidth = finitePositive(width, safeContainerWidth * fallback);
  return normalizeSidePanelRatio(safeWidth / safeContainerWidth, fallback);
}

/**
 * Convert a legacy absolute width only when the current split layout can
 * represent that width without clamping it at the upper bound. A minimized or
 * otherwise narrow window must not turn a temporary constraint into the
 * user's durable ratio preference; callers can retry after the container
 * grows again.
 */
export function sidePanelRatioForLegacyWidth(
  legacyPixelWidth,
  containerWidth,
  fallback = 0.42,
  constraints = {},
) {
  const pixelWidth = Number(legacyPixelWidth);
  if (!Number.isFinite(pixelWidth) || pixelWidth <= 1) return null;
  const bounds = sidePanelWidthBounds(containerWidth, constraints);
  if (!bounds.canSplit || pixelWidth > bounds.maximum) return null;
  return sidePanelRatioFromWidth(pixelWidth, bounds.containerWidth, fallback);
}
