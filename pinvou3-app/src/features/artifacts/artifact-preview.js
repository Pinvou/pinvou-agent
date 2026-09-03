import { bridge } from '../../hooks/useBridge.js';

// ArtifactsPanel 与 FilePreviewModal 的预览取数阶梯完全同构：
// exists 检查 → md/html/text 走 readArtifactText → 其余 kind 走 renderArtifactVisual
// （modal 另有 json pretty-print 与 image base64 两条扩展分支）。此处只共享
// 「取数 + 状态机」，返回各调用方 setPv / setPreview 直接消费的预览状态；
// 渲染 JSX 仍由各调用方自己负责（office iframe 的 sandbox 差异、可编辑 md、
// unsupported 兜底卡等不在此收敛）。

// 有文本读取通道的 kind；其余 kind 走 renderArtifactVisual 可视化渲染。
const TEXT_PREVIEW_KINDS = ['md', 'html', 'text'];

/**
 * @param {string} path - artifact 绝对路径（路径内含 session id，可唯一寻址）
 * @param {string|undefined} sessionId - 桥接会话域。按 path 取数的调用方（ArtifactsPanel）
 *   传 undefined：bridge 侧 `sessionId || activeSessionId`，与不传参逐字等价。
 * @param {{includeJson?: boolean, includeImage?: boolean, includeInfo?: boolean, isCancelled?: () => boolean}} [options] - 每调用方的阶梯扩展与取消探测，默认全关（等价 ArtifactsPanel 原阶梯）
 * @param {boolean} [options.includeJson] - .json 后缀尝试 pretty-print，kind 改判 'json'（解析失败按原文展示）
 * @param {boolean} [options.includeImage] - image kind 走 readArtifactImageB64，读失败降级为 imageError
 * @param {boolean} [options.includeInfo] - 成功态附带 info（{kind,text,info} / {kind,visual,info} / {missing,info}）；
 *   关闭时保持 FilePreviewModal 原有的无 info 形状，且 visual 分支 kind 为 `info.kind || 'other'`
 * @param {() => boolean} [options.isCancelled] - 阶梯中途查询取消：为真立即停止后续桥接调用，
 *   调用方再以自己的 cancelled 标志丢弃返回值（与原两处 effect 的 cancelled/alive 检查同位）
 * @returns {Promise<object>} 预览状态：{loading 由调用方管理} / {missing[,info]} / {kind,text[,info]} /
 *   {kind:'image',dataUrl} / {kind:'image',imageError} / {kind,visual[,info]} / {error}
 */
export async function loadArtifactPreview(path, sessionId, options = {}) {
  const { includeJson = false, includeImage = false, includeInfo = false, isCancelled } = options;
  const cancelled = () => !!(isCancelled && isCancelled());
  try {
    const info = await bridge.artifacts.artifactInfo(path, sessionId);
    if (cancelled()) return {};
    if (!info || !info.exists) return includeInfo ? { missing: true, info } : { missing: true };
    if (TEXT_PREVIEW_KINDS.includes(info.kind)) {
      let text = await bridge.artifacts.readArtifactText(path, sessionId);
      if (cancelled()) return {};
      let kind = info.kind;
      if (includeJson && /\.json$/i.test(path)) {
        try {
          text = JSON.stringify(JSON.parse(text), null, 2);
          kind = 'json';
        } catch {
          // Keep malformed JSON readable as plain text.
        }
      }
      return includeInfo ? { kind, text, info } : { kind, text };
    }
    if (includeImage && info.kind === 'image') {
      try {
        const dataUrl = await bridge.artifacts.readArtifactImageB64(path, sessionId);
        return { kind: 'image', dataUrl };
      } catch (error) {
        return { kind: 'image', imageError: String(error) };
      }
    }
    const visual = bridge.artifacts.renderArtifactVisual
      ? await bridge.artifacts.renderArtifactVisual(path, sessionId)
      : null;
    return includeInfo
      ? { kind: info.kind, visual, info }
      : { kind: info.kind || 'other', visual };
  } catch (error) {
    return { error: String(error) };
  }
}
