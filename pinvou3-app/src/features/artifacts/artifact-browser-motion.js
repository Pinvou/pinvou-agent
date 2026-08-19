export const ARTIFACT_BROWSER_MOTION_MS = 460;

function finitePositive(value) {
  const number = Number(value);
  return Number.isFinite(number) && number > 0 ? number : 0;
}
export function artifactBrowserLaunchTransform(originRect, targetRect) {
  if (!originRect || !targetRect) return null;
  const originWidth = finitePositive(originRect.width);
  const originHeight = finitePositive(originRect.height);
  const targetWidth = finitePositive(targetRect.width);
  const targetHeight = finitePositive(targetRect.height);
  if (!originWidth || !originHeight || !targetWidth || !targetHeight) return null;

  const originCenterX = Number(originRect.left) + originWidth / 2;
  const originCenterY = Number(originRect.top) + originHeight / 2;
  const targetCenterX = Number(targetRect.left) + targetWidth / 2;
  const targetCenterY = Number(targetRect.top) + targetHeight / 2;
  if (![originCenterX, originCenterY, targetCenterX, targetCenterY].every(Number.isFinite)) {
    return null;
  }

  const translateX = originCenterX - targetCenterX;
  const translateY = originCenterY - targetCenterY;
  const scaleX = Math.max(0.02, Math.min(1, originWidth / targetWidth));
  const scaleY = Math.max(0.02, Math.min(1, originHeight / targetHeight));
  return {
    translateX,
    translateY,
    scaleX,
    scaleY,
    css: `translate3d(${translateX}px, ${translateY}px, 0) scale(${scaleX}, ${scaleY})`,
  };
}
