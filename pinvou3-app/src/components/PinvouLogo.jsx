import { resolveAppAssetUrl } from '../shared/asset-url.mjs';

const PINVOU_BRAND_BLUE_URL = resolveAppAssetUrl('assets/brand/brand-blue.png');

export function PinvouLogo({ className = 'h-4 w-4', title }) {
  return (
    <img
      src={PINVOU_BRAND_BLUE_URL}
      alt={title || ''}
      aria-hidden={title ? undefined : true}
      className={`shrink-0 object-contain ${className}`}
    />
  );
}
