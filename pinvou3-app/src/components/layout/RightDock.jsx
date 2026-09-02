import {
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
    publishOcclusion,
    releaseOcclusion,
  }), [
    activatePanel,
    hidePanel,
    mountPanel,
    portalRoot,
    publishOcclusion,
    releaseOcclusion,
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
  const mountPanel = dock?.mountPanel;
  const activatePanel = dock?.activatePanel;
  const hidePanel = dock?.hidePanel;
  const activePanelId = dock?.activePanelId;
  const portalRoot = dock?.portalRoot;

  useLayoutEffect(() => {
    if (!mountPanel || !panelId) return;
    return mountPanel(panelId);
  }, [mountPanel, panelId]);

  useLayoutEffect(() => {
    if (!activatePanel || !hidePanel || !panelId) return;
    if (visible) activatePanel(panelId);
    else hidePanel(panelId);
    return () => hidePanel(panelId);
  }, [activatePanel, hidePanel, panelId, visible]);

  useLayoutEffect(() => {
    if (activatePanel && visible && panelId) activatePanel(panelId);
  }, [activationKey, activatePanel, panelId, visible]);

  const active = visible && (!dock || activePanelId === panelId);
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
  return portalRoot ? createPortal(content, portalRoot) : null;
}

/**
 * Hide the physical dock and suspend native child surfaces while an overlay owns
 * the canvas. `true` is returned only after the native hide ACK has allowed the
 * overlay state to be published. A failed or stale attempt remains fail-closed.
 */
export function useRightDockOcclusion(occlusionId, active) {
  const dock = useContext(RightDockContext);
  const publishOcclusion = dock?.publishOcclusion;
  const releaseOcclusion = dock?.releaseOcclusion;
  const [publicationReady, setPublicationReady] = useState(false);
  const attemptRef = useRef(0);
  useLayoutEffect(() => {
    if (!publishOcclusion || !releaseOcclusion || !occlusionId) return;
    const attempt = attemptRef.current + 1;
    attemptRef.current = attempt;
    let disposed = false;

    if (!active) {
      // This is an intentional fail-closed reset at the active-state boundary.
      // eslint-disable-next-line react-hooks/set-state-in-effect -- stale permits must be revoked before a later activation can publish
      setPublicationReady(false);
      releaseOcclusion(occlusionId);
      return;
    }

    // Hide the overlay until this activation owns a fresh native-surface ACK.
    setPublicationReady(false);
    const publish = () => {
      if (disposed || attemptRef.current !== attempt) return false;
      setPublicationReady(true);
      return true;
    };
    try {
      const result = publishOcclusion(occlusionId, publish);
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
      releaseOcclusion(occlusionId);
    };
  }, [active, occlusionId, publishOcclusion, releaseOcclusion]);

  return active ? (!dock || !occlusionId ? true : publicationReady) : false;
}

export function RightDockHost({
  resizeLabel,
  resizeHint,
  onResizeActiveChange,
  className = '',
}) {
  const dock = useContext(RightDockContext);
  const mountedPanelCount = dock?.mountedPanelCount || 0;
  const openSidePanelCount = dock?.openSidePanelCount || 0;
  const activePanelId = dock?.activePanelId || '';
  const setPortalRoot = dock?.setPortalRoot;
  if (!dock || mountedPanelCount === 0) return null;

  return (
    <ResizableSidePanel
      panelId="right-dock"
      visible={openSidePanelCount === 1}
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
        ref={setPortalRoot}
        className="h-full min-h-0 min-w-0 flex-1"
        data-active-panel={activePanelId}
      />
    </ResizableSidePanel>
  );
}
