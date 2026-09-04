// Status dot: transcript / tool rows / session lists previously repeated the same span verbatim in 12+ places
// (1.5px dot + semantic color: gray idle / blue pulsing running / red failed / emerald done / amber waiting).
// The tone enum maps to concrete colors (so className overrides cannot fight the default); out-of-palette cases stay inline.
const TONES = {
  idle: 'bg-gray-300 dark:bg-gray-600',
  run: 'bg-blue-500 animate-pulse',
  fail: 'bg-red-500',
  ok: 'bg-emerald-500',
  okPulse: 'bg-emerald-500 animate-pulse',
  warn: 'bg-amber-500',
};

/**
 * @param {{ tone?: keyof typeof TONES, size?: 'sm'|'md', className?: string }} props
 */
export function StatusDot({ tone = 'idle', size = 'sm', className = '' }) {
  const sizeClass = size === 'md' ? 'w-2 h-2' : 'w-1.5 h-1.5';
  return (
    <span
      aria-hidden="true"
      className={`${sizeClass} shrink-0 rounded-full ${TONES[tone] || TONES.idle} ${className}`}
    />
  );
}
