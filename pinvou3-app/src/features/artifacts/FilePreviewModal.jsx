import { useEffect, useState, useSyncExternalStore } from 'react';
import { bridge } from '../../hooks/useBridge.js';
import { can, isWeb } from '../../shared/platform.js';
import { getSyntaxHighlightVersion, subscribeSyntaxHighlight } from '../../shared/syntax-highlighter.js';
import { OFFICE_HTML_STYLE } from './ArtifactsPanel.jsx';
import { ScaledHtmlPreview } from '../settings/composer-shared.jsx';

// eslint-disable-next-line sonarjs/cognitive-complexity -- single-file preview: each branch maps to one kind (md/json/image/visual/error state); splitting would thread 5+ intermediate loading states through
const FilePreviewModal = ({ path, sessionId, onClose, t }) => {
  const [preview, setPreview] = useState({ loading: true });
  // md 预览一次性 innerHTML:懒语言注册完成后 bump 版本号,重新计算恢复高亮。
  useSyncExternalStore(subscribeSyntaxHighlight, getSyntaxHighlightVersion);

  useEffect(() => {
    let alive = true;
    (async () => {
      try {
        const info = await bridge.artifacts.artifactInfo(path, sessionId);
        if (!alive) return;
        if (!info || !info.exists) {
          setPreview({ missing: true });
          return;
        }
        if (['md', 'html', 'text'].includes(info.kind)) {
          let text = await bridge.artifacts.readArtifactText(path, sessionId);
          if (!alive) return;
          let kind = info.kind;
          if (/\.json$/i.test(path)) {
            try {
              text = JSON.stringify(JSON.parse(text), null, 2);
              kind = 'json';
            } catch {
              // Keep malformed JSON readable as plain text.
            }
          }
          setPreview({ kind, text });
        } else if (info.kind === 'image') {
          try {
            const dataUrl = await bridge.artifacts.readArtifactImageB64(path, sessionId);
            if (alive) setPreview({ kind: 'image', dataUrl });
          } catch (error) {
            if (alive) setPreview({ kind: 'image', imageError: String(error) });
          }
        } else {
          const visual = bridge.artifacts.renderArtifactVisual
            ? await bridge.artifacts.renderArtifactVisual(path, sessionId)
            : null;
          if (alive) setPreview({ kind: info.kind || 'other', visual });
        }
      } catch (error) {
        if (alive) setPreview({ error: String(error) });
      }
    })();
    return () => { alive = false; };
  }, [path, sessionId]);

  const name = (path || '').split('/').pop();
  const labels = t.artifactPreview;
  const canOpen = !isWeb || can('artifactDownload');
  const open = () => bridge.artifacts.openArtifactExternal?.(path, sessionId);
  // 懒语言注册完成后重算 md 预览(其余 kind 与语法无关,重算只会
  // 重新生成相同字符串,代价可忽略)。
  const mdHtml = preview.kind === 'md'
    ? bridge.rendering.renderMarkdown(preview.text || '')
    : null;

  return (
    <div className="absolute inset-0 z-[60] flex items-center justify-center pointer-events-auto">
      {/* Modal backdrop click-to-close layer; the keyboard path is handled by the title-bar ✕ close button (the real <button type="button"> below). */}
      {/* biome-ignore lint/a11y/useKeyWithClickEvents: backdrop click-to-close layer; keyboard path handled by the title-bar close button */}
      {/* biome-ignore lint/a11y/noStaticElementInteractions: backdrop click-to-close layer, non-interactive container */}
      <div className="absolute inset-0 bg-black/50" onClick={onClose} />
      <div className="relative w-[860px] max-w-[92vw] h-[82vh] flex flex-col rounded-[16px] shadow-2xl overflow-hidden bg-white dark:bg-[#1E1F20]">
        <div className="flex items-center justify-between px-4 py-3 border-b border-black/10 dark:border-white/10">
          <span className="text-[14px] font-medium truncate text-[#1F1F1F] dark:text-[#E3E3E3]" title={path}>{name}</span>
          <div className="flex items-center gap-2">
            {canOpen && <button type="button" onClick={open} className="px-2 py-1 text-[12px] rounded text-[#444746] dark:text-[#C4C7C5] hover:bg-[#F0F4F9] dark:hover:bg-[#333537]">{isWeb ? labels.download : labels.openExternal}</button>}
            <button type="button" onClick={onClose} className="w-7 h-7 rounded-full flex items-center justify-center hover:bg-[#F0F4F9] dark:hover:bg-[#333537] text-[#444746] dark:text-[#C4C7C5]">✕</button>
          </div>
        </div>
        <div className="flex-1 overflow-y-auto custom-scrollbar p-4 min-w-0">
          {preview.loading ? <div className="text-[13px] text-[#757575] dark:text-[#8E8E8E]">{labels.loading}</div>
            : preview.missing ? <div className="text-[13px] text-[#757575] dark:text-[#8E8E8E]">{labels.fileMissing}</div>
            : preview.error ? <div className="text-[13px] text-[#F28B82]">{labels.readFailed(preview.error)}</div>
            : preview.kind === 'md' ? <div className="msg-md text-[14px] leading-relaxed light-code dark-code text-[#1F1F1F] dark:text-[#E3E3E3]" dangerouslySetInnerHTML={{ __html: mdHtml }} />
            : preview.kind === 'html' ? <ScaledHtmlPreview html={preview.text || ''} title={name} onOpenExternal={(url) => bridge.artifacts.openUserExternalUrl(url)} />
            : ['json', 'text'].includes(preview.kind) ? <pre className="text-[12px] whitespace-pre-wrap break-words font-mono leading-relaxed text-[#444746] dark:text-[#C4C7C5]">{preview.text}</pre>
            : preview.kind === 'image' ? (preview.imageError ? <div className="text-[13px] text-[#F28B82]">{labels.imageReadFailed(preview.imageError)}</div> : <img className="max-w-full max-h-[70vh] object-contain mx-auto rounded-lg" src={preview.dataUrl} alt={name} />)
            : preview.visual?.mode === 'html' ? <iframe sandbox="allow-same-origin" title={name} className="w-full min-h-[68vh] border-0 block bg-[#15171a]" style={{ colorScheme: 'dark' }} srcDoc={(preview.visual.html || '') + OFFICE_HTML_STYLE} />
            : preview.visual?.mode === 'images' ? <div className="flex flex-col items-center gap-3">{(preview.visual.images || []).map((src, index) => <img key={src} src={src} className="max-w-full h-auto rounded-lg shadow-sm" alt={`page-${index + 1}`} />)}</div>
            : <div><p className="text-[13px] mb-2 text-[#444746] dark:text-[#C4C7C5]">{labels.previewUnsupported}</p>{canOpen && <button type="button" onClick={open} className="px-3 py-1.5 rounded-full text-[13px] bg-[#0B57D0] dark:bg-[#A8C7FA] text-white dark:text-[#062E6F]">{isWeb ? labels.downloadArtifact : labels.openExternalArtifact}</button>}</div>}
        </div>
      </div>
    </div>
  );
};

export { FilePreviewModal };
