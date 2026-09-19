// Shared ring spinner: solid ring with a transparent top notch (the
// border-X border-t-transparent family). Colors come via the tone enum (so
// className overrides cannot fight the default); exotic colors keep using inline spans.
const TONES = {
  brand: 'border-blue-500 border-t-transparent',
};

/**
 * @param {{ size?: number, tone?: string, className?: string }} props - Sizing and color tone for the spinner.
 */
export function Spinner({ size = 14, tone = 'brand', className = '' }) {
  const toneClass = TONES[tone] || TONES.brand;
  return (
    <span
      aria-hidden="true"
      className={`inline-block shrink-0 animate-spin rounded-full border-2 motion-reduce:animate-none ${toneClass} ${className}`}
      style={{ width: size, height: size }}
    />
  );
}
