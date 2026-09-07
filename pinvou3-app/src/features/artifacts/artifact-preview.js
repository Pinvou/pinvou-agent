import { bridge } from '../../hooks/useBridge.js';

// ArtifactsPanel and FilePreviewModal had isomorphic preview-fetch ladders:
// exists check -> md/html/text via readArtifactText -> other kinds via renderArtifactVisual
// (the modal adds two extension branches: json pretty-print and image base64). Only the
// fetch + state machine is shared here, returning preview state consumed directly by each caller's setPv / setPreview;
// rendering JSX stays with the callers (office iframe sandbox differences, editable md,
// the unsupported fallback card, etc. are not consolidated here).

// Kinds with a text-read channel; all other kinds render visually via renderArtifactVisual.
const TEXT_PREVIEW_KINDS = ['md', 'html', 'text'];

/**
 * @param {string} path - absolute artifact path (embeds the session id, so it is uniquely addressable)
 * @param {string|undefined} sessionId - bridge session scope. Callers fetching by path (ArtifactsPanel)
 *   pass undefined: the bridge resolves `sessionId || activeSessionId`, verbatim-equal to omitting the argument.
 * @param {{includeJson?: boolean, includeImage?: boolean, includeInfo?: boolean, isCancelled?: () => boolean}} [options] - per-caller ladder extensions and cancellation probe, all off by default (equals the original FilePreviewModal ladder; the original ArtifactsPanel ladder attached info on every branch and must pass includeInfo: true)
 * @param {boolean} [options.includeJson] - try pretty-print for the .json suffix, re-kind as 'json' (falls back to raw text on parse failure)
 * @param {boolean} [options.includeImage] - image kinds go through readArtifactImageB64, degrading to imageError on read failure
 * @param {boolean} [options.includeInfo] - attach info on success ({kind,text,info} / {kind,visual,info} / {missing,info});
 *   when off, the shape stays the original info-less FilePreviewModal one, and visual branches use kind `info.kind || 'other'`
 * @param {() => boolean} [options.isCancelled] - mid-ladder cancellation probe: when true, stop further bridge calls immediately;
 *   the caller additionally drops the result with its own cancelled flag (same slot as the original effects' cancelled/alive checks)
 * @returns {Promise<object>} preview state: {loading managed by the caller} / {missing[,info]} / {kind,text[,info]} /
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
