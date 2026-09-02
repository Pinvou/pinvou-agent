import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useId,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from 'react';
import {
  normalizeSidePanelRatio,
  resolveSidePanelLayout,
  sidePanelRatioForLegacyWidth,
  sidePanelRatioFromWidth,
} from './side-panel-layout.mjs';

const SidePanelPresenceContext = createContext(null);

/** @type {string[]} */
const EMPTY_STORAGE_KEYS = [];

function uniqueStorageKeys(keys) {
  return [...new Set((keys || []).filter(Boolean))];
}

function storedPreference(storageKey, legacyRatioStorageKeys, legacyPixelStorageKeys, fallback) {
  try {
    const value = Number(localStorage.getItem(storageKey) || '');
    if (value > 0 && value < 1) {
      return {
        ratio: normalizeSidePanelRatio(value, fallback),
        legacyPixelWidth: null,
        legacyRatioPending: false,
      };
    }
    for (const key of legacyRatioStorageKeys) {
      const legacyRatio = Number(localStorage.getItem(key) || '');
      if (legacyRatio > 0 && legacyRatio < 1) {
        return {
          ratio: normalizeSidePanelRatio(legacyRatio, fallback),
          legacyPixelWidth: null,
          legacyRatioPending: true,
        };
      }
    }
    let legacyPixelWidth = Number.NaN;
    for (const key of legacyPixelStorageKeys) {
      const valueForKey = Number(localStorage.getItem(key) || '');
      if (valueForKey > 1) {
        legacyPixelWidth = valueForKey;
        break;
      }
    }
    return {
      ratio: fallback,
      legacyPixelWidth: legacyPixelWidth > 1 ? legacyPixelWidth : null,
      legacyRatioPending: false,
    };
  } catch {
    return { ratio: fallback, legacyPixelWidth: null, legacyRatioPending: false };
  }
}

function rememberRatio(storageKey, ratio, legacyStorageKeys) {
  try {
    localStorage.setItem(storageKey, String(Number(ratio.toFixed(4))));
    for (const key of legacyStorageKeys) {
      if (key !== storageKey) localStorage.removeItem(key);
    }
  } catch {
    // When localStorage is unavailable, retain the ratio only for this window lifetime.
  }
}

/** Collect nested right-side panels so the app shell can apply one navigation/narrow policy. */
export function SidePanelLayoutProvider({ children, onPresenceChange }) {
  const panelsRef = useRef(new Set());
  const reportPresence = useCallback((panelId, present) => {
    const panels = panelsRef.current;
    const before = panels.size;
    if (present) panels.add(panelId);
    else panels.delete(panelId);
    if (panels.size !== before && onPresenceChange) onPresenceChange(panels.size);
  }, [onPresenceChange]);

  useEffect(() => () => {
    panelsRef.current.clear();
    if (onPresenceChange) onPresenceChange(0);
  }, [onPresenceChange]);

  return (
    <SidePanelPresenceContext.Provider value={reportPresence}>
      {children}
    </SidePanelPresenceContext.Provider>
  );
}

/**
 * Shared split layout for conversation-side panels. Persist the user's ratio and compute
 * only a temporary width as the container changes. When the main and panel minimum widths
 * cannot both fit, switch to a single pane instead of squeezing the main content.
 */
export function ResizableSidePanel({
  children,
  panelId,
  storageKey,
  legacyPixelStorageKey = '',
  legacyPixelStorageKeys = EMPTY_STORAGE_KEYS,
  legacyRatioStorageKeys = EMPTY_STORAGE_KEYS,
  defaultRatio = 0.42,
  minWidth = 360,
  minMainWidth = 520,
  maxWidthRatio = 0.65,
  visible = true,
  className = '',
  dataTestId,
  resizeLabel,
  resizeHint,
  onResizeActiveChange,
}) {
  const generatedId = useId();
  const presenceId = panelId || generatedId;
  const reportPresence = useContext(SidePanelPresenceContext);
  const panelRef = useRef(null);
  const resizeCleanupRef = useRef(null);
  const normalizedDefaultRatio = normalizeSidePanelRatio(defaultRatio);
  const [legacyStorageKeys] = useState(() => ({
    pixel: uniqueStorageKeys([legacyPixelStorageKey, ...legacyPixelStorageKeys]),
    ratio: uniqueStorageKeys(legacyRatioStorageKeys),
  }));
  const [initialPreference] = useState(() => (
    storedPreference(
      storageKey,
      legacyStorageKeys.ratio,
      legacyStorageKeys.pixel,
      normalizedDefaultRatio,
    )
  ));
  const legacyPixelWidthRef = useRef(initialPreference.legacyPixelWidth);
  const initialLegacyRatioPendingRef = useRef(initialPreference.legacyRatioPending);
  const [preferredRatio, setPreferredRatio] = useState(initialPreference.ratio);
  const [containerWidth, setContainerWidth] = useState(0);
  const constraints = useMemo(() => ({ minWidth, minMainWidth, maxWidthRatio }), [
    maxWidthRatio,
    minMainWidth,
    minWidth,
  ]);
  const layout = resolveSidePanelLayout(containerWidth, preferredRatio, constraints);

  useEffect(() => {
    if (!initialLegacyRatioPendingRef.current) return;
    initialLegacyRatioPendingRef.current = false;
    rememberRatio(
      storageKey,
      initialPreference.ratio,
      [...legacyStorageKeys.ratio, ...legacyStorageKeys.pixel],
    );
  }, [initialPreference.ratio, legacyStorageKeys, storageKey]);

  useEffect(() => {
    if (!reportPresence || !visible) return;
    reportPresence(presenceId, true);
    return () => reportPresence(presenceId, false);
  }, [presenceId, reportPresence, visible]);

  useLayoutEffect(() => {
    if (!visible) return;
    const panel = panelRef.current;
    const container = panel?.parentElement;
    if (!container) return;
    let frame = 0;
    const measure = () => {
      if (frame) cancelAnimationFrame(frame);
      frame = requestAnimationFrame(() => {
        frame = 0;
        const width = Math.round(container.getBoundingClientRect().width);
        if (width > 0) {
          const legacyPixelWidth = legacyPixelWidthRef.current;
          if (legacyPixelWidth != null) {
            const migratedRatio = sidePanelRatioForLegacyWidth(
              legacyPixelWidth,
              width,
              normalizedDefaultRatio,
              constraints,
            );
            if (migratedRatio != null) {
              legacyPixelWidthRef.current = null;
              setPreferredRatio(migratedRatio);
              rememberRatio(
                storageKey,
                migratedRatio,
                [...legacyStorageKeys.ratio, ...legacyStorageKeys.pixel],
              );
            }
          }
          setContainerWidth((current) => (current === width ? current : width));
        }
      });
    };
    measure();
    const observer = typeof ResizeObserver === 'function' ? new ResizeObserver(measure) : null;
    observer?.observe(container);
    window.addEventListener('resize', measure);
    return () => {
      observer?.disconnect();
      window.removeEventListener('resize', measure);
      if (frame) cancelAnimationFrame(frame);
    };
  }, [constraints, legacyStorageKeys, normalizedDefaultRatio, storageKey, visible]);

  useEffect(() => () => {
    if (resizeCleanupRef.current) resizeCleanupRef.current();
  }, []);

  const commitWidth = useCallback((width, rootWidth) => {
    const nextRatio = sidePanelRatioFromWidth(width, rootWidth, normalizedDefaultRatio);
    setPreferredRatio(nextRatio);
    rememberRatio(
      storageKey,
      nextRatio,
      [...legacyStorageKeys.ratio, ...legacyStorageKeys.pixel],
    );
  }, [legacyStorageKeys, normalizedDefaultRatio, storageKey]);

  function startResize(event) {
    if (layout.overlay || (event.button != null && event.button !== 0)) return;
    event.preventDefault();
    const panel = panelRef.current;
    const rootRect = panel?.parentElement?.getBoundingClientRect();
    if (!panel || !rootRect) return;
    if (resizeCleanupRef.current) resizeCleanupRef.current();

    const currentLayout = resolveSidePanelLayout(rootRect.width, preferredRatio, constraints);
    const startingRatio = preferredRatio;
    let nextWidth = currentLayout.width;
    let frame = 0;
    let finished = false;
    const onMove = (moveEvent) => {
      nextWidth = Math.max(
        currentLayout.minimum,
        Math.min(rootRect.right - moveEvent.clientX, currentLayout.maximum),
      );
      if (frame) return;
      frame = requestAnimationFrame(() => {
        frame = 0;
        panel.style.width = `${nextWidth}px`;
      });
    };
    const restoreTransientWidth = () => {
      // Dragging writes only a temporary inline width. The React ratio state is
      // intentionally untouched until pointerup, so cancellation only needs to
      // put the DOM back on the layout derived from the drag-start preference.
      const currentRootWidth = panel.parentElement?.getBoundingClientRect().width
        || rootRect.width;
      const restored = resolveSidePanelLayout(currentRootWidth, startingRatio, constraints);
      panel.style.width = `${restored.width}px`;
    };
    const cleanup = (restore = false) => {
      if (finished) return;
      finished = true;
      document.removeEventListener('pointermove', onMove);
      document.removeEventListener('pointerup', onUp);
      document.removeEventListener('pointercancel', onCancel);
      document.removeEventListener('keydown', onKeyDown);
      document.removeEventListener('visibilitychange', onVisibilityChange);
      window.removeEventListener('blur', onCancel);
      window.removeEventListener('resize', onCancel);
      window.removeEventListener('pagehide', onCancel);
      if (frame) cancelAnimationFrame(frame);
      if (restore) restoreTransientWidth();
      panel.style.pointerEvents = '';
      document.body.style.cursor = '';
      document.body.style.userSelect = '';
      resizeCleanupRef.current = null;
      if (onResizeActiveChange) onResizeActiveChange(false);
    };
    const onUp = () => {
      cleanup();
      commitWidth(nextWidth, rootRect.width);
    };
    const onCancel = () => cleanup(true);
    const onKeyDown = (event) => {
      // WAI-ARIA splitter convention: Escape aborts the drag and restores the
      // pre-drag width instead of committing the transient one.
      if (event.key === 'Escape') {
        event.preventDefault();
        event.stopPropagation();
        onCancel();
      }
    };
    const onVisibilityChange = () => {
      if (document.visibilityState === 'hidden') onCancel();
    };
    resizeCleanupRef.current = onCancel;
    document.addEventListener('pointermove', onMove);
    document.addEventListener('pointerup', onUp);
    document.addEventListener('pointercancel', onCancel);
    document.addEventListener('keydown', onKeyDown);
    document.addEventListener('visibilitychange', onVisibilityChange);
    window.addEventListener('blur', onCancel);
    window.addEventListener('resize', onCancel);
    window.addEventListener('pagehide', onCancel);
    panel.style.pointerEvents = 'none';
    document.body.style.cursor = 'col-resize';
    document.body.style.userSelect = 'none';
    if (onResizeActiveChange) onResizeActiveChange(true);
  }

  function resetRatio() {
    setPreferredRatio(normalizedDefaultRatio);
    rememberRatio(
      storageKey,
      normalizedDefaultRatio,
      [...legacyStorageKeys.ratio, ...legacyStorageKeys.pixel],
    );
  }

  function handleSeparatorKeyDown(event) {
    if (layout.overlay || (event.key !== 'ArrowLeft' && event.key !== 'ArrowRight')) return;
    event.preventDefault();
    const delta = event.key === 'ArrowLeft' ? 24 : -24;
    const nextWidth = Math.max(layout.minimum, Math.min(layout.width + delta, layout.maximum));
    commitWidth(nextWidth, layout.containerWidth);
  }

  return (
    <aside
      ref={panelRef}
      style={{ width: visible ? `${layout.width}px` : undefined, maxWidth: '100%' }}
      className={`${visible ? 'flex' : 'hidden'} relative min-w-0 shrink-0 flex-col ${className}`}
      data-testid={dataTestId}
      data-layout-mode={layout.overlay ? 'single' : 'split'}
      data-preferred-ratio={preferredRatio.toFixed(4)}
    >
      {!layout.overlay && (
        // biome-ignore lint/a11y/useSemanticElements: this focusable WAI-ARIA splitter handles pointer resizing; Chromium's native hr behavior breaks the drag/release sequence
        <div
          role="separator"
          tabIndex={0}
          aria-label={resizeLabel}
          aria-orientation="vertical"
          aria-valuemin={layout.minimum}
          aria-valuemax={layout.maximum}
          aria-valuenow={layout.width}
          onPointerDown={startResize}
          onKeyDown={handleSeparatorKeyDown}
          onDoubleClick={resetRatio}
          className="absolute inset-y-0 left-0 z-20 w-1.5 -translate-x-1/2 cursor-col-resize bg-black/10 transition-colors hover:bg-[#0B57D0]/50 focus:bg-[#0B57D0]/50 focus:outline-none dark:bg-white/10 dark:hover:bg-[#A8C7FA]/60 dark:focus:bg-[#A8C7FA]/60"
          title={resizeHint}
        />
      )}
      {children}
    </aside>
  );
}
