import { useEffect, useRef, useState } from 'react';

/**
 * Time-based throttle for values that change far faster than they need to be
 * rendered — streaming markdown text is the motivating case: re-running
 * marked.parse + DOMPurify + hljs over the full growing text on every chunk is
 * O(n²) over the message, so the render must follow a time budget instead of
 * the chunk rate. While `active`, the returned value trails `value` and is
 * refreshed at most once per `delayMs`; when `active` is false the raw value
 * passes straight through, so the very commit that ends the stream renders the
 * finished message verbatim — including coalesced snapshots where the final
 * text and the streaming flag change together, which a flush driven by a
 * timer or an effect could never guarantee. The interval samples the value
 * through a ref that is re-synced after every commit, so bursts of chunks
 * arriving faster than `delayMs` can never starve the trailing update, and
 * equal string samples are a free no-op re-render.
 * @template T
 * @param {T} value - the fast-changing raw value (typically streaming markdown text)
 * @param {number} delayMs - minimum interval between throttled updates while active
 * @param {boolean} active - throttle window; false passes the raw value through
 * @returns {T} the throttled value while active, otherwise the raw value
 */
export function useThrottledValue(value, delayMs = 200, active = true) {
  const [throttled, setThrottled] = useState(value);
  // Latest raw value; a ref lets the sampler interval read the freshest value
  // without being recreated (and thus reset) on every chunk.
  const valueRef = useRef(value);
  useEffect(() => {
    // Re-sync after every commit that carries a new value: the interval fires
    // between commits, so this keeps samples current.
    valueRef.current = value;
  }, [value]);
  useEffect(() => {
    if (active) return;
    // Converge the stored state with the raw value once the stream ends, so a
    // later re-activation can never resurface a pre-end sample.
    setThrottled(valueRef.current);
  }, [active]);
  useEffect(() => {
    if (!active) return;
    const timer = window.setInterval(() => setThrottled(valueRef.current), Math.max(0, delayMs));
    return () => window.clearInterval(timer);
  }, [active, delayMs]);
  return active ? throttled : value;
}
