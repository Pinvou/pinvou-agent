// Consolidates the 15+ hand-rolled ring spinners. Two visual forms:
//   arc — thin ring + same-hue bright arc (the border-current/20 border-t-current family)
//   top — solid ring + transparent top notch (the border-X border-t-transparent family)
// Colors come via the tone enum (so className overrides cannot fight the default); exotic colors keep using inline spans.
const TONES = {
  arc: {
    current: 'border-current/20 border-t-current',
    brand: 'border-blue-500/20 border-t-blue-500',
  },
  top: {
    brand: 'border-blue-500 border-t-transparent',
    ios: 'border-[#007AFF] border-t-transparent dark:border-[#0A84FF]',
    inverse: 'border-white/70 border-t-transparent',
    web: 'border-[#8AB4F8] border-t-transparent',
  },
};

/**
 * @param {{ size?: number, variant?: 'arc'|'top', tone?: string, className?: string }} props
 */
export function Spinner({ size = 14, variant = 'top', tone, className = '' }) {
  const variants = TONES[variant] || TONES.top;
  const toneClass = variants[tone] || (variant === 'arc' ? variants.current : variants.brand);
  return (
    <span
      aria-hidden="true"
      className={`inline-block shrink-0 animate-spin rounded-full border-2 motion-reduce:animate-none ${toneClass} ${className}`}
      style={{ width: size, height: size }}
    />
  );
}
