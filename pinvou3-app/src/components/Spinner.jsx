// Shared ring spinner: solid ring with a transparent top notch (the
// border-X border-t-transparent family). Exotic colors keep using inline spans.

/**
 * @param {{ size?: number, className?: string }} props - Sizing for the spinner.
 */
export function Spinner({ size = 14, className = '' }) {
  return (
    <span
      aria-hidden="true"
      className={`inline-block shrink-0 animate-spin rounded-full border-2 motion-reduce:animate-none border-blue-500 border-t-transparent ${className}`}
      style={{ width: size, height: size }}
    />
  );
}
