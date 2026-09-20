/**
 * weak-cache.js — keyed WeakMap memoization for render-hot pure functions.
 *
 * ChatView / ConversationTimeline re-render on every composer keystroke,
 * streaming delta, and clock tick; per-item computations (markdown, bubble
 * parsing, copy-table merges) are pure functions of an object key plus a
 * tuple of identity-compared inputs. createWeakCache wraps such a function:
 * the key must be an object (WeakMap contract), and the cached value is
 * reused while the remaining arguments are `===`-equal to the previous call.
 */

export function createWeakCache(compute) {
  const cache = new WeakMap();
  return (key, ...args) => {
    const cached = cache.get(key);
    if (
      cached
      && cached.args.length === args.length
      && cached.args.every((value, index) => value === args[index])
    ) {
      return cached.value;
    }
    const value = compute(key, ...args);
    cache.set(key, { args, value });
    return value;
  };
}
