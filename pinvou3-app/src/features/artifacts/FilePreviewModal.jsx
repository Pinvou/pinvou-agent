import React, { useCallback, useEffect, useId, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { ExternalLink, X } from '../../components/icons.jsx';
import { FileTypeIcon } from '../../components/files/FileTypeIcon.jsx';
import { bridge } from '../../hooks/useBridge.js';
import { _artifactKind } from '../../shared/artifact-utils.js';
import { can, isWeb } from '../../shared/platform.js';
import { OFFICE_HTML_STYLE } from './ArtifactsPanel.jsx';
import {
  artifactPreviewExternalUrlFromMessage,
  artifactPreviewFocusDirectionFromMessage,
  artifactPreviewRequestsCloseFromMessage,
  buildArtifactPreviewDocument,
} from './artifact-preview-navigation.js';
import {
  ARTIFACT_BROWSER_MOTION_MS,
  artifactBrowserLaunchTransform,
} from './artifact-browser-motion.js';
import './artifact-browser.css';

const MAX_ARTIFACT_TEXT_PREVIEW_BYTES = 10 * 1024 * 1024;
const MAX_ARTIFACT_IMAGE_PREVIEW_BYTES = 25_000_000;
const MAX_ARTIFACT_DOCUMENT_PREVIEW_BYTES = 50 * 1024 * 1024;

function formatBytes(value) {
  const bytes = Number(value) || 0;
  if (!bytes) return '0 B';
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(bytes < 10 * 1024 ? 1 : 0)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(bytes < 10 * 1024 * 1024 ? 1 : 0)} MB`;
}

function SandboxedArtifactHtml({ html, onFocusBoundary, onRequestClose, onRequestExternal, title }) {
  const frameRef = useRef(null);
  const documentHtml = useMemo(
    () => buildArtifactPreviewDocument(html, { isolated: true, requestClose: true }),
    [html],
  );

  useEffect(() => {
    const handleMessage = event => {
      const frameWindow = frameRef.current && frameRef.current.contentWindow;
      if (!frameWindow || event.source !== frameWindow) return;
      const url = artifactPreviewExternalUrlFromMessage(event.data);
      if (url && onRequestExternal) {
        onRequestExternal(url);
        return;
      }
      const focusDirection = artifactPreviewFocusDirectionFromMessage(event.data);
      if (focusDirection && onFocusBoundary) {
        onFocusBoundary(focusDirection);
        return;
      }
      if (artifactPreviewRequestsCloseFromMessage(event.data) && onRequestClose) onRequestClose();
    };
    window.addEventListener('message', handleMessage);
    return () => window.removeEventListener('message', handleMessage);
  }, [onFocusBoundary, onRequestClose, onRequestExternal]);

  return (
    <iframe
      ref={frameRef}
      title={title}
      sandbox="allow-scripts"
      data-testid="artifact-browser-html-frame"
      className="artifact-browser-html-frame"
      srcDoc={documentHtml}
    />
  );
}

function ArtifactBrowser({ path, sessionId, title, originRect, returnFocus, onClose, theme, t }) {
  const [preview, setPreview] = useState({ loading: true });
  const [info, setInfo] = useState(null);
  const [phase, setPhase] = useState('preparing');
  const [motionSettled, setMotionSettled] = useState(false);
  const [pendingExternalUrl, setPendingExternalUrl] = useState('');
  const [rendererReady, setRendererReady] = useState(false);
  const windowRef = useRef(null);
  const launchCloneRef = useRef(null);
  const closeButtonRef = useRef(null);
  const externalConfirmRef = useRef(null);
  const externalDialogRef = useRef(null);
  const closeTimerRef = useRef(null);
  const closeFrameRef = useRef(null);
  const closedRef = useRef(false);
  const openingCancelledRef = useRef(false);
  const reducedMotionRef = useRef(false);
  const titleId = useId();

  const name = (path || '').split(/[\\/]/).pop() || title || '';
  const displayTitle = title || name;
  const labels = t.artifactPreview;
  const canOpen = !isWeb || can('artifactDownload');
  const canOpenExternally = canOpen && (isWeb || Boolean(info && info.kind !== 'html'));
  const iconKind = _artifactKind(path);

  const finishClose = useCallback(() => {
    if (closedRef.current) return;
    closedRef.current = true;
    if (closeTimerRef.current != null) window.clearTimeout(closeTimerRef.current);
    onClose();
  }, [onClose]);

  const requestClose = useCallback(() => {
    if (phase === 'closing' || closedRef.current) return;
    openingCancelledRef.current = true;
    if (reducedMotionRef.current) {
      finishClose();
      return;
    }
    if (phase === 'preparing') {
      finishClose();
      return;
    }
    const panel = windowRef.current;
    if (panel) {
      const computed = window.getComputedStyle(panel);
      const frozenTransform = computed.transform;
      const frozenOpacity = computed.opacity;
      panel.style.transition = 'none';
      panel.style.transform = frozenTransform;
      panel.style.opacity = frozenOpacity;
      void panel.offsetWidth;
    }
    setMotionSettled(false);
    setPhase('closing');
    closeFrameRef.current = window.requestAnimationFrame(() => {
      if (!panel || closedRef.current) return;
      panel.style.transition = '';
      panel.style.transform = panel.dataset.artifactOriginTransform || 'translate3d(0, 10px, 0) scale(0.97)';
      panel.style.opacity = '0.78';
    });
    closeTimerRef.current = window.setTimeout(finishClose, ARTIFACT_BROWSER_MOTION_MS + 80);
  }, [finishClose, phase]);

  useLayoutEffect(() => {
    const panel = windowRef.current;
    if (!panel) return undefined;
    openingCancelledRef.current = false;
    reducedMotionRef.current = Boolean(window.matchMedia && window.matchMedia('(prefers-reduced-motion: reduce)').matches);
    const previousTransform = panel.style.transform;
    const previousTransition = panel.style.transition;
    panel.style.transition = 'none';
    panel.style.transform = 'none';
    const targetRect = panel.getBoundingClientRect();
    const launch = artifactBrowserLaunchTransform(originRect, targetRect);
    if (launch) {
      panel.style.setProperty('--artifact-browser-origin-transform', launch.css);
      panel.dataset.artifactOriginTransform = launch.css;
    }
    panel.style.transform = launch ? launch.css : previousTransform;
    panel.style.opacity = launch ? '0.92' : '';
    // 让浏览器在 phase 切换前提交一次起始几何，否则首次挂载时可能只观察到终态。
    void panel.offsetWidth;

    const cloneHost = launchCloneRef.current;
    if (cloneHost && returnFocus?.isConnected && originRect?.width > 0 && originRect?.height > 0) {
      const clone = returnFocus.cloneNode(true);
      clone.removeAttribute('role');
      clone.removeAttribute('tabindex');
      clone.removeAttribute('aria-label');
      clone.setAttribute('aria-hidden', 'true');
      clone.querySelectorAll('[id], button, [href], [tabindex]').forEach(element => {
        element.removeAttribute('id');
        element.removeAttribute('href');
        element.setAttribute('tabindex', '-1');
      });
      cloneHost.replaceChildren(clone);
      Object.assign(cloneHost.style, {
        top: `${originRect.top}px`,
        left: `${originRect.left}px`,
        width: `${originRect.width}px`,
        height: `${originRect.height}px`,
      });
    }
    if (reducedMotionRef.current) {
      panel.style.transition = previousTransition;
      panel.style.transform = '';
      panel.style.opacity = '';
      setPhase('open');
      setMotionSettled(true);
      setRendererReady(true);
      return undefined;
    }
    let frameOne = 0;
    let openTimer = null;
    frameOne = window.requestAnimationFrame(() => {
      panel.style.transition = previousTransition;
      void panel.offsetWidth;
      openTimer = window.setTimeout(() => {
        if (openingCancelledRef.current) return;
        panel.style.transform = 'translate3d(0, 0, 0) scale(1)';
        panel.style.opacity = '1';
        setPhase('open');
      }, 34);
    });
    return () => {
      window.cancelAnimationFrame(frameOne);
      if (openTimer != null) window.clearTimeout(openTimer);
    };
  }, [originRect, path]);

  useLayoutEffect(() => {
    closeButtonRef.current?.focus({ preventScroll: true });
  }, []);

  useEffect(() => {
    if (phase !== 'open' || rendererReady) return undefined;
    if (reducedMotionRef.current) {
      setRendererReady(true);
      return undefined;
    }
    const timer = window.setTimeout(() => setRendererReady(true), 180);
    return () => window.clearTimeout(timer);
  }, [phase, rendererReady]);

  const restorePreviewFocus = useCallback(() => {
    window.requestAnimationFrame(() => {
      const frame = windowRef.current?.querySelector('iframe');
      if (frame) frame.focus({ preventScroll: true });
      else closeButtonRef.current?.focus({ preventScroll: true });
    });
  }, []);

  const dismissExternal = useCallback(() => {
    setPendingExternalUrl('');
    restorePreviewFocus();
  }, [restorePreviewFocus]);

  const handleEscapeRequest = useCallback(() => {
    if (pendingExternalUrl) {
      dismissExternal();
      return;
    }
    requestClose();
  }, [dismissExternal, pendingExternalUrl, requestClose]);

  useEffect(() => {
    const handleDocumentKeyDown = event => {
      if (event.key !== 'Escape') return;
      event.preventDefault();
      event.stopPropagation();
      handleEscapeRequest();
    };
    document.addEventListener('keydown', handleDocumentKeyDown, true);
    return () => document.removeEventListener('keydown', handleDocumentKeyDown, true);
  }, [handleEscapeRequest]);

  useEffect(() => {
    if (pendingExternalUrl) externalConfirmRef.current?.focus({ preventScroll: true });
  }, [pendingExternalUrl]);

  useEffect(() => () => {
    if (closeTimerRef.current != null) window.clearTimeout(closeTimerRef.current);
    if (closeFrameRef.current != null) window.cancelAnimationFrame(closeFrameRef.current);
    if (returnFocus && returnFocus.isConnected && typeof returnFocus.focus === 'function') {
      returnFocus.focus({ preventScroll: true });
    }
  }, [returnFocus]);

  useEffect(() => {
    let alive = true;
    setPreview({ loading: true });
    setInfo(null);
    (async () => {
      try {
        const nextInfo = await bridge.artifacts.artifactInfo(path, sessionId);
        if (!alive) return;
        setInfo(nextInfo || null);
        if (!nextInfo || !nextInfo.exists) {
          setPreview({ missing: true });
          return;
        }
        if (['md', 'html', 'text'].includes(nextInfo.kind)) {
          if (Number(nextInfo.size) > MAX_ARTIFACT_TEXT_PREVIEW_BYTES) {
            setPreview({ tooLarge: true, kind: nextInfo.kind });
            return;
          }
          let text = await bridge.artifacts.readArtifactText(path, sessionId);
          if (!alive) return;
          let kind = nextInfo.kind;
          if (/\.json$/i.test(path)) {
            try {
              text = JSON.stringify(JSON.parse(text), null, 2);
              kind = 'json';
            } catch (_) {
              // Keep malformed JSON readable as plain text.
            }
          }
          setPreview({ kind, text });
          return;
        }
        if (nextInfo.kind === 'image') {
          if (Number(nextInfo.size) > MAX_ARTIFACT_IMAGE_PREVIEW_BYTES) {
            setPreview({ tooLarge: true, kind: nextInfo.kind });
            return;
          }
          try {
            const dataUrl = await bridge.artifacts.readArtifactImageB64(path, sessionId);
            if (alive) setPreview({ kind: 'image', dataUrl });
          } catch (error) {
            if (alive) setPreview({ kind: 'image', imageError: String(error) });
          }
          return;
        }
        if (
          ['pdf', 'docx', 'xlsx', 'legacy_office'].includes(nextInfo.kind)
          && Number(nextInfo.size) > MAX_ARTIFACT_DOCUMENT_PREVIEW_BYTES
        ) {
          setPreview({ tooLarge: true, kind: nextInfo.kind });
          return;
        }
        const visual = bridge.artifacts.renderArtifactVisual
          ? await bridge.artifacts.renderArtifactVisual(path, sessionId)
          : null;
        if (alive) setPreview({ kind: nextInfo.kind || 'other', visual });
      } catch (error) {
        if (alive) setPreview({ error: String(error) });
      }
    })();
    return () => { alive = false; };
  }, [path, sessionId]);

  const openExternal = useCallback(
    () => bridge.artifacts.openArtifactExternal?.(path, sessionId),
    [path, sessionId],
  );

  const confirmExternal = useCallback(() => {
    const url = pendingExternalUrl;
    if (!url) return;
    dismissExternal();
    void bridge.artifacts.openUserExternalUrl(url);
  }, [dismissExternal, pendingExternalUrl]);

  const handleFocusBoundary = useCallback(direction => {
    const buttons = [...(windowRef.current?.querySelectorAll('.artifact-browser-chrome button:not(:disabled)') || [])];
    const target = direction === 'previous' ? buttons[buttons.length - 1] : buttons[0];
    target?.focus({ preventScroll: true });
  }, []);

  const handleKeyDown = event => {
    if (event.key !== 'Tab') return;
    const focusRoot = pendingExternalUrl ? externalDialogRef.current : windowRef.current;
    if (!focusRoot) return;
    const focusable = [...focusRoot.querySelectorAll('button:not(:disabled), iframe, [href], [tabindex]:not([tabindex="-1"])')]
      .filter(element => !element.hasAttribute('hidden'));
    if (!focusable.length) return;
    const first = focusable[0];
    const last = focusable[focusable.length - 1];
    if (event.shiftKey && document.activeElement === first) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault();
      first.focus();
    }
  };

  const kindLabel = (t.apKinds && t.apKinds[preview.kind || info?.kind]) || labels.localArtifact;
  const sandboxedPreview = ['md', 'html'].includes(preview.kind) || preview.visual?.mode === 'html';
  const rootClass = `artifact-browser-root ${theme === 'dark' ? 'dark' : ''} is-${phase} ${motionSettled ? 'is-settled' : ''}`;
  const renderPreview = () => {
    if (preview.loading) {
      return (
        <div className="artifact-browser-loading" data-testid="artifact-browser-loading">
          <div className="artifact-browser-loading-card">
            <div className="artifact-browser-loading-mark" aria-hidden="true" />
            <div>{labels.loading}</div>
          </div>
        </div>
      );
    }
    if (preview.missing || preview.error || preview.tooLarge) {
      return (
        <div className="artifact-browser-state">
          <div className="artifact-browser-state-card" role="status">
            <p>{preview.missing
              ? labels.fileMissing
              : preview.tooLarge
                ? labels.tooLarge
                : labels.readFailed(preview.error)}</p>
            {preview.tooLarge && canOpenExternally && (
              <button type="button" onClick={openExternal} className="artifact-browser-action is-primary">
                {isWeb ? labels.downloadArtifact : labels.openExternalArtifact}
              </button>
            )}
          </div>
        </div>
      );
    }
    if (preview.kind === 'md') {
      const markdown = bridge.rendering.renderMarkdown(preview.text || '');
      return (
        <SandboxedArtifactHtml
          html={`<style>body{padding:clamp(1rem,3vw,3rem)!important;background:${theme === 'dark' ? '#11151d' : '#f2f5f9'}!important;color:${theme === 'dark' ? '#f4f7fb' : '#182230'}!important}.artifact-browser-markdown{box-sizing:border-box;width:min(900px,100%);margin:0 auto;padding:clamp(1.25rem,3vw,3rem);border-radius:22px;background:${theme === 'dark' ? '#1d212b' : '#fff'};font:15px/1.7 -apple-system,BlinkMacSystemFont,"PingFang SC",sans-serif}.artifact-browser-markdown img{max-width:100%;height:auto}</style><article class="artifact-browser-markdown">${markdown}</article>`}
          title={displayTitle}
          onFocusBoundary={handleFocusBoundary}
          onRequestExternal={setPendingExternalUrl}
          onRequestClose={handleEscapeRequest}
        />
      );
    }
    if (preview.kind === 'html') {
      return (
        <SandboxedArtifactHtml
          html={preview.text || ''}
          title={displayTitle}
          onFocusBoundary={handleFocusBoundary}
          onRequestExternal={setPendingExternalUrl}
          onRequestClose={handleEscapeRequest}
        />
      );
    }
    if (['json', 'text'].includes(preview.kind)) {
      return <div className="artifact-browser-preview-pad"><pre className="artifact-browser-code">{preview.text}</pre></div>;
    }
    if (preview.kind === 'image') {
      return preview.imageError
        ? <div className="artifact-browser-state"><div className="artifact-browser-state-card">{labels.imageReadFailed(preview.imageError)}</div></div>
        : <div className="artifact-browser-image-stage"><img className="artifact-browser-image" src={preview.dataUrl} alt={displayTitle} /></div>;
    }
    if (preview.visual?.mode === 'html') {
      return (
        <SandboxedArtifactHtml
          html={`${preview.visual.html || ''}${OFFICE_HTML_STYLE}`}
          title={displayTitle}
          onFocusBoundary={handleFocusBoundary}
          onRequestExternal={setPendingExternalUrl}
          onRequestClose={handleEscapeRequest}
        />
      );
    }
    if (preview.visual?.mode === 'images') {
      return (
        <div className="artifact-browser-pages">
          {preview.visual.warning && <div className="artifact-browser-state-card">{preview.visual.warning}</div>}
          {(preview.visual.images || []).map((src, index) => (
            <img key={`${index}-${src.slice(0, 24)}`} src={src} className="artifact-browser-page" alt={`${displayTitle} · ${index + 1}`} />
          ))}
        </div>
      );
    }
    return (
      <div className="artifact-browser-state">
        <div className="artifact-browser-state-card">
          <p>{preview.visual?.warning || labels.previewUnsupported}</p>
          {canOpenExternally && <button type="button" onClick={openExternal} className="artifact-browser-action is-primary">{isWeb ? labels.downloadArtifact : labels.openExternalArtifact}</button>}
        </div>
      </div>
    );
  };

  const browser = (
    <div className={rootClass} data-testid="artifact-browser-root">
      <div className="artifact-browser-backdrop" aria-hidden="true" onPointerDown={requestClose} />
      <div ref={launchCloneRef} className="artifact-browser-launch-clone" aria-hidden="true" />
      <section
        ref={windowRef}
        className="artifact-browser-window"
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        data-testid="artifact-browser-window"
        data-artifact-kind={preview.kind || info?.kind || 'loading'}
        onKeyDown={handleKeyDown}
        onTransitionEnd={event => {
          if (event.target !== event.currentTarget || event.propertyName !== 'transform') return;
          if (phase === 'closing') finishClose();
          else if (phase === 'open') {
            event.currentTarget.style.transform = '';
            event.currentTarget.style.opacity = '';
            setMotionSettled(true);
          }
        }}
      >
        <div className="artifact-browser-shell">
          <header className="artifact-browser-chrome" data-testid="artifact-browser-chrome" aria-hidden={pendingExternalUrl ? 'true' : undefined}>
            <div className="artifact-browser-identity">
              <span className="artifact-browser-file-icon" aria-hidden="true"><FileTypeIcon kind={iconKind} className="h-6 w-6" /></span>
              <div className="artifact-browser-title-group">
                <div className="artifact-browser-eyebrow">{labels.browserTitle}</div>
                <h2 id={titleId} className="artifact-browser-title" title={displayTitle}>{displayTitle}</h2>
              </div>
            </div>
            <div className="artifact-browser-actions">
              {canOpenExternally && (
                <button type="button" onClick={openExternal} className="artifact-browser-action is-primary" aria-label={isWeb ? labels.download : labels.openExternal}>
                  <ExternalLink size={17} />
                  <span>{isWeb ? labels.download : labels.openExternal}</span>
                </button>
              )}
              <button ref={closeButtonRef} type="button" onClick={requestClose} className="artifact-browser-action is-close" aria-label={labels.close}>
                <X size={19} />
              </button>
            </div>
          </header>
          <div className="artifact-browser-viewport" data-testid="artifact-browser-viewport" aria-hidden={pendingExternalUrl ? 'true' : undefined}>
            <div className="artifact-browser-content">
              {rendererReady ? renderPreview() : (
                <div className="artifact-browser-loading" data-testid="artifact-browser-loading">
                  <div className="artifact-browser-loading-card">
                    <div className="artifact-browser-loading-mark" aria-hidden="true" />
                    <div>{labels.loading}</div>
                  </div>
                </div>
              )}
            </div>
          </div>
          <footer className="artifact-browser-statusbar" aria-hidden={pendingExternalUrl ? 'true' : undefined}>
            <span>{kindLabel}{info?.size ? ` · ${formatBytes(info.size)}` : ''}{sandboxedPreview ? ` · ${labels.safePreview}` : ''}</span>
            <span className="artifact-browser-path" title={path}>{path}</span>
            <span>{labels.escapeHint}</span>
          </footer>
          {pendingExternalUrl && (
            <div className="artifact-browser-link-confirm-layer">
              <div ref={externalDialogRef} className="artifact-browser-link-confirm" role="alertdialog" aria-modal="true" aria-labelledby={`${titleId}-external`}>
                <div className="artifact-browser-link-icon" aria-hidden="true"><ExternalLink size={22} /></div>
                <h3 id={`${titleId}-external`}>{labels.externalLinkTitle}</h3>
                <p>{labels.externalLinkPrompt(new URL(pendingExternalUrl).host)}</p>
                <code>{pendingExternalUrl}</code>
                <div className="artifact-browser-link-actions">
                  <button ref={externalConfirmRef} type="button" className="artifact-browser-action" onClick={dismissExternal}>{labels.cancel}</button>
                  <button type="button" className="artifact-browser-action is-primary" onClick={confirmExternal}>{labels.openLink}</button>
                </div>
              </div>
            </div>
          )}
        </div>
      </section>
    </div>
  );

  return typeof document === 'undefined' ? browser : createPortal(browser, document.body);
}

// 保留旧导出名，KnowledgeView 无需跟着本次 PinvouOS 交互改造迁移。
const FilePreviewModal = ArtifactBrowser;

export { ArtifactBrowser, FilePreviewModal, SandboxedArtifactHtml, formatBytes };
