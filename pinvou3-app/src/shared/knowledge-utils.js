// Pure logic shared by the knowledge / remote-knowledge views: collection avatar coloring and
// optimistic collection-count updates after doc changes; the two copies were verbatim (identical first-6 palette and codePoint-sum hash).

const ACCENT_PALETTE = ['#3f7bf0', '#7b5fe6', '#1aa07a', '#d6873e', '#d6589a', '#4b7bd6'];

/** Stable color per collection name / category: same input always yields the same color, so both views match naturally. Accepts an extended palette. */
export function stableAccentColor(value, palette = ACCENT_PALETTE) {
  const hash = [...String(value || '')].reduce((total, ch) => total + ch.codePointAt(0), 0);
  return palette[Math.abs(hash) % palette.length];
}

/**
 * Optimistic collection-count update after document add/remove/soft-delete (docCount/chunkCount/totalBytes
 * shift together, clamped to >= 0). countDelta is +1 / -1 (recycle-bin restore / permanent delete alike).
 */
export function applyDocumentDelta(collections, collectionId, doc, countDelta) {
  return (collections || []).map((collection) => (
    collection.id === collectionId
      ? {
        ...collection,
        docCount: Math.max(0, (collection.docCount || 0) + countDelta),
        chunkCount: Math.max(0, (collection.chunkCount || 0) + countDelta * (doc.nChunks || 0)),
        totalBytes: Math.max(0, (collection.totalBytes || 0) + countDelta * (doc.size || 0)),
      }
      : collection
  ));
}
