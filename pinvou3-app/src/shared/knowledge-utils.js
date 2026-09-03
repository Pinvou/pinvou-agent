// 知识库本地 / 远程两个视图共用的纯逻辑：集合头像取色、文档增删后的集合计数
// 乐观更新。此前两处逐字重复（取色板前 6 色与 codePoint 求和哈希完全一致）。

const ACCENT_PALETTE = ['#3f7bf0', '#7b5fe6', '#1aa07a', '#d6873e', '#d6589a', '#4b7bd6'];

/** 按集合名 / 类别稳定取色：同输入恒同色，两视图因此天然同色。可传入加长调色板。 */
export function stableAccentColor(value, palette = ACCENT_PALETTE) {
  const hash = [...String(value || '')].reduce((total, ch) => total + ch.codePointAt(0), 0);
  return palette[Math.abs(hash) % palette.length];
}

/**
 * 文档增删 / 移入回收站后的集合计数乐观更新（docCount/chunkCount/totalBytes 一起平移，
 * 钳制到 ≥0）。countDelta 为 +1 / -1（回收站恢复 / 彻底删除同理）。
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
