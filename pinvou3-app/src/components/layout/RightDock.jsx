import React, {
  createContext,
  useCallback,
  useContext,
  useLayoutEffect,
  useMemo,
  useReducer,
  useRef,
  useState,
} from 'react';
import { createPortal } from 'react-dom';
import { ResizableSidePanel } from './ResizableSidePanel.jsx';
import {
  createRightDockState,
  reduceRightDockState,
  rightDockSnapshot,
} from './right-dock-state.mjs';

const RightDockContext = createContext(null);

const LEGACY_RIGHT_DOCK_RATIO_KEYS = [
  'pinvou_browser_panel_ratio',
  'pinvou_artifact_panel_ratio',
  'pinvou_subagent_panel_ratio',
  'pinvou_codex_workspace_ratio',
];

const LEGACY_RIGHT_DOCK_WIDTH_KEYS = [
  'pinvou_browser_panel_width',
  'pinvou_artifactW',
  'pinvou_subagent_panel_width',
  'pinvou_codex_workspace_width',
];

export function RightDockProvider({
  children,
  onStateChange,
  onBeforeOcclusionPublish,
  onOcclusionRelease,
}) {
  const [state, dispatch] = useReducer(reduceRightDockState, undefined, createRightDockState);
  const [portalRoot, setPortalRoot] = useState(null);
  const snapshot = useMemo(() => rightDockSnapshot(state), [state]);

  const mountPanel = useCallback((panelId) => {
    dispatch({ type: 'mount', panelId });
    return () => dispatch({ type: 'unmount', panelId });
  }, []);
  const activatePanel = useCallback((panelId) => {
    dispatch({ type: 'activate', panelId });
  }, []);
  const hidePanel = useCallback((panelId) => {
    dispatch({ type: 'hide', panelId });
  }, []);
  const setOccluded = useCallback((occlusionId, active) => {
    dispatch({ type: 'occlude', occlusionId, active });
  }, []);
  const publishOcclusion = useCallback((occlusionId, publish) => {
    const commit = () => {
      const published = publish();
      if (published === false) return false;
      dispatch({ type: 'occlude', occlusionId, active: true });
      return true;
    };
    return onBeforeOcclusionPublish
      ? onBeforeOcclusionPublish(occlusionId, commit)
      : commit();
  }, [onBeforeOcclusionPublish]);
  const releaseOcclusion = useCallback((occlusionId) => {
    dispatch({ type: 'occlude', occlusionId, active: false });
    if (onOcclusionRelease) onOcclusionRelease(occlusionId);
  }, [onOcclusionRelease]);

  useLayoutEffect(() => {
    if (onStateChange) onStateChange(snapshot);
  }, [onStateChange, snapshot]);

  const value = useMemo(() => ({
    ...snapshot,
    portalRoot,
    setPortalRoot,
    mountPanel,
    activatePanel,
    hidePanel,
    setOccluded,
    publishOcclusion,
    releaseOcclusion,
  }), [
    activatePanel,
    hidePanel,
    mountPanel,
    portalRoot,
    publishOcclusion,
    releaseOcclusion,
    setOccluded,
    snapshot,
  ]);

  return <RightDockContext.Provider value={value}>{children}</RightDockContext.Provider>;
}

/**
 * A logical dock panel. Its subtree stays mounted in the shared portal while
 * another logical panel is active, so switching tools does not reset state.
 */
export function RightDockPanel({
  panelId,
  activationKey,
  visible = true,
  className = '',
  dataTestId,
  onActiveChange,
  children,
}) {
  const dock = useContext(RightDockContext);

  useLayoutEffect(() => {
    if (!dock || !panelId) return undefined;
    return dock.mountPanel(panelId);
  }, [dock?.mountPanel, panelId]);

  useLayoutEffect(() => {
    if (!dock || !panelId) return undefined;
    if (visible) dock.activatePanel(panelId);
    else dock.hidePanel(panelId);
    return () => dock.hidePanel(panelId);
  }, [dock?.activatePanel, dock?.hidePanel, panelId, visible]);

  useLayoutEffect(() => {
    if (dock && visible && panelId) dock.activatePanel(panelId);
  }, [activationKey, dock?.activatePanel, panelId, visible]);

  const active = visible && (!dock || dock.activePanelId === panelId);
  useLayoutEffect(() => {
    if (onActiveChange) onActiveChange(active);
  }, [active, onActiveChange]);

  const content = (
    <section
      aria-hidden={!active}
      className={`${active ? 'flex' : 'hidden'} h-full min-h-0 min-w-0 flex-col ${className}`}
      data-right-dock-panel={panelId}
      data-testid={dataTestId}
    >
      {children}
    </section>
  );

  if (!dock) return content;
  return dock.portalRoot ? createPortal(content, dock.portalRoot) : null;
}

/**
 * Hide the physical dock and suspend native child surfaces while an overlay owns
 * the canvas. `true` is returned only after the native hide ACK has allowed the
 * overlay state to be published. A failed or stale attempt remains fail-closed.
 */
export function useRightDockOcclusion(occlusionId, active) {
  const dock = useContext(RightDockContext);
  const [publicationReady, setPublicationReady] = useState(false);
  const attemptRef = useRef(0);
  useLayoutEffect(() => {
    if (!dock || !occlusionId) return undefined;
    const attempt = attemptRef.current + 1;
    attemptRef.current = attempt;
    let disposed = false;

    if (!active) {
      setPublicationReady(false);
      dock.releaseOcclusion(occlusionId);
      return undefined;
    }

    setPublicationReady(false);
    const publish = () => {
      if (disposed || attemptRef.current !== attempt) return false;
      setPublicationReady(true);
      return true;
    };
    try {
      const result = dock.publishOcclusion(occlusionId, publish);
      if (result && typeof result.then === 'function') {
        void Promise.resolve(result).catch(() => false);
      }
    } catch {
      // The native transition coordinator reports the error. Keeping the permit
      // false is the required fail-closed UI behavior.
    }

    return () => {
      disposed = true;
      attemptRef.current += 1;
      dock.releaseOcclusion(occlusionId);
    };
  }, [active, dock?.publishOcclusion, dock?.releaseOcclusion, occlusionId]);

  return !active ? false : (!dock || !occlusionId ? true : publicationReady);
}

export function RightDockHost({
  resizeLabel,
  resizeHint,
  onResizeActiveChange,
  className = '',
}) {
  const dock = useContext(RightDockContext);
  if (!dock || dock.mountedPanelCount === 0) return null;

  return (
    <ResizableSidePanel
      panelId="right-dock"
      visible={dock.openSidePanelCount === 1}
      storageKey="pinvou_right_dock_ratio"
      legacyRatioStorageKeys={LEGACY_RIGHT_DOCK_RATIO_KEYS}
      legacyPixelStorageKeys={LEGACY_RIGHT_DOCK_WIDTH_KEYS}
      defaultRatio={0.45}
      minWidth={420}
      minMainWidth={520}
      maxWidthRatio={0.65}
      resizeLabel={resizeLabel}
      resizeHint={resizeHint}
      onResizeActiveChange={onResizeActiveChange}
      className={`overflow-hidden ${className}`}
      dataTestId="right-dock-host"
    >
      <div
        ref={dock.setPortalRoot}
        className="h-full min-h-0 min-w-0 flex-1"
        data-active-panel={dock.activePanelId || ''}
      />
    </ResizableSidePanel>
  );
}
