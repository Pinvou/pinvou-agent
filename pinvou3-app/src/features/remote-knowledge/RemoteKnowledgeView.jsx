import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import {
  AlertTriangle, BookOpen, Check, CheckCircle2, ChevronDown, Copy, Database, Download, FolderPlus,
  Link, Plus, RefreshCw, Search, Server, Trash2, Upload, Users, X,
} from '../../components/icons.jsx';
import { PinvouLogo } from '../../components/PinvouLogo.jsx';
import { FileTypeIcon } from '../../components/files/FileTypeIcon.jsx';
import { invokeTauri, isTauriAvailable, listenTauri, openTauriDialog, saveTauriDialog } from '../../platform/tauri/client.js';
import { copyClipboardText } from '../../shared/clipboard.js';
import remoteKnowledgeHeroImage from '../../assets/remote-knowledge/remote-knowledge-hero.webp';

const panel = 'border border-[#ececf1] bg-white dark:border-white/10 dark:bg-[#1E1F20]';
const panelShadow = 'shadow-[0_1px_2px_rgba(24,24,40,0.04),0_8px_24px_rgba(24,24,40,0.04)] dark:shadow-none';
const ink = 'text-[#1F1F1F] dark:text-[#E3E3E3]';
const muted = 'text-[#444746] dark:text-[#C4C7C5]';
const button = 'inline-flex h-9 items-center justify-center gap-1.5 whitespace-nowrap rounded-full px-4 text-[13px] font-semibold transition-colors disabled:cursor-not-allowed disabled:opacity-45';
const primary = `${button} bg-[#0B57D0] text-white hover:bg-[#0848ad] dark:bg-[#A8C7FA] dark:text-[#062E6F] dark:hover:bg-[#c2d8fb]`;
const soft = `${button} bg-[#F0F4F9] text-[#0B57D0] hover:bg-[#E1E5EA] dark:bg-[#2A2B2D] dark:text-[#A8C7FA] dark:hover:bg-[#333537]`;
const quiet = `${button} px-3 text-[#444746] hover:bg-[#F0F4F9] dark:text-[#C4C7C5] dark:hover:bg-[#333537]`;
const danger = `${button} px-3 text-[#d63a3a] hover:bg-[#d63a3a]/10`;
const iconButton = 'grid h-8 w-8 shrink-0 place-items-center rounded-full text-[#444746] transition-colors hover:bg-[#E1E5EA] dark:text-[#C4C7C5] dark:hover:bg-[#333537] disabled:cursor-not-allowed disabled:opacity-45';
const field = 'h-10 w-full rounded-xl border border-[#dfe3ea] bg-white px-3.5 text-[13px] text-[#1F1F1F] outline-none transition-shadow placeholder:text-[#8b8d94] focus:border-[#0B57D0] focus:ring-2 focus:ring-[#0B57D0]/10 dark:border-white/10 dark:bg-[#171719] dark:text-[#E3E3E3] dark:focus:border-[#A8C7FA]';
const ownerSection = 'rounded-2xl border border-[#e3e7ee] bg-white p-4 dark:border-white/10 dark:bg-[#1E1F20] sm:p-5';
const ownerTab = 'inline-flex h-10 min-w-0 items-center justify-center gap-2 rounded-lg px-3 text-[13px] font-semibold transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[#0B57D0]/25';
const collectionPalette = ['#3f7bf0', '#7b5fe6', '#1aa07a', '#d6873e', '#d6589a', '#4b7bd6'];
const documentPageSize = 200;
const documentStatusBatchSize = 500;
const uploadIndexPollIntervalMs = 1000;
const uploadIndexPollTimeoutMs = 60000;

const wait = duration => new Promise(resolve => window.setTimeout(resolve, duration));

function uploadPollSetting(name, fallback) {
  const value = Number(window[name]);
  return Number.isFinite(value) && value >= 0 ? value : fallback;
}

async function settleBeforeDeadline(promise, deadline) {
  const remaining = Math.max(0, deadline - Date.now());
  if (!remaining) throw new Error('index status polling timed out');
  let timer;
  try {
    return await Promise.race([
      promise,
      new Promise((_, reject) => {
        timer = window.setTimeout(() => reject(new Error('index status polling timed out')), remaining);
      }),
    ]);
  } finally {
    if (timer) window.clearTimeout(timer);
  }
}

async function settleDocumentStatusBatches(documentIds, deadline, requestBatch) {
  const batches = [];
  for (let offset = 0; offset < documentIds.length; offset += documentStatusBatchSize) {
    batches.push(documentIds.slice(offset, offset + documentStatusBatchSize));
  }
  const settled = await Promise.allSettled(batches.map(batch => settleBeforeDeadline(
    requestBatch(batch),
    deadline,
  )));
  return settled.flatMap(result => (result.status === 'fulfilled' && Array.isArray(result.value)
    ? result.value
    : []));
}

function stableColor(value) {
  const hash = [...String(value || '')].reduce((total, character) => total + character.charCodeAt(0), 0);
  return collectionPalette[Math.abs(hash) % collectionPalette.length];
}

function formatBytes(bytes) {
  if (!Number.isFinite(bytes) || bytes <= 0) return '0 B';
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(bytes < 10240 ? 1 : 0)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

function OverlayDialog({
  testId,
  title,
  description,
  icon: Icon,
  onClose,
  closeLabel,
  closeDisabled = false,
  widthClassName = 'max-w-[560px]',
  scrollBody = false,
  children,
}) {
  const dialog = (
    <div
      className="fixed inset-0 z-[80] flex items-center justify-center bg-[#17181b]/35 p-4 backdrop-blur-[2px] animate-in fade-in duration-150 motion-reduce:animate-none"
      onMouseDown={() => { if (!closeDisabled) onClose(); }}
    >
      <section
        role="dialog"
        aria-modal="true"
        aria-label={title}
        data-testid={testId}
        className={`max-h-[calc(100vh-2rem)] w-full ${widthClassName} ${scrollBody ? 'flex flex-col overflow-hidden' : 'overflow-y-auto'} rounded-3xl p-5 sm:p-6 animate-in zoom-in-95 duration-200 motion-reduce:animate-none ${panel} ${panelShadow}`}
        onMouseDown={event => event.stopPropagation()}
      >
        <div className="flex shrink-0 items-start gap-3">
          <div className="grid h-11 w-11 shrink-0 place-items-center rounded-xl bg-[#eaf2ff] text-[#0B57D0] dark:bg-[#172b49] dark:text-[#A8C7FA]">
            <Icon size={20} />
          </div>
          <div className="min-w-0 flex-1">
            <h2 className={`text-[16px] font-bold ${ink}`}>{title}</h2>
            {description && <p className={`mt-1 text-[12px] leading-5 ${muted}`}>{description}</p>}
          </div>
          <button className={iconButton} onClick={onClose} disabled={closeDisabled} aria-label={closeLabel}><X size={16} /></button>
        </div>
        <div className={`mt-5 ${scrollBody ? 'min-h-0 overflow-y-auto pr-1' : ''}`}>{children}</div>
      </section>
    </div>
  );
  return typeof document === 'undefined' ? dialog : createPortal(dialog, document.body);
}

function RemoteKnowledgeView({ t, embedded = false }) {
  const [connections, setConnections] = useState([]);
  const [connectionsLoaded, setConnectionsLoaded] = useState(false);
  const [selectedServerId, setSelectedServerId] = useState('');
  const [collections, setCollections] = useState([]);
  const [selectedCollectionId, setSelectedCollectionId] = useState(null);
  const [documents, setDocuments] = useState([]);
  const [documentsHasMore, setDocumentsHasMore] = useState(false);
  const [documentsLoadingPage, setDocumentsLoadingPage] = useState(false);
  const [invitation, setInvitation] = useState('');
  const [deviceName, setDeviceName] = useState('');
  const [pendingJoins, setPendingJoins] = useState([]);
  const [joinFeedback, setJoinFeedback] = useState(null);
  const [nearbyHosts, setNearbyHosts] = useState([]);
  const [discoveryStatus, setDiscoveryStatus] = useState('idle');
  const [connectorError, setConnectorError] = useState('');
  const [identityProbe, setIdentityProbe] = useState(null);
  const [hostStatus, setHostStatus] = useState(null);
  const [showConnector, setShowConnector] = useState(false);
  const [showOwnerPanel, setShowOwnerPanel] = useState(false);
  const [ownerPanelTab, setOwnerPanelTab] = useState('people');
  const [ownerShares, setOwnerShares] = useState([]);
  const [ownerJoinRequests, setOwnerJoinRequests] = useState([]);
  const [ownerDevices, setOwnerDevices] = useState([]);
  const [ownerModelStatus, setOwnerModelStatus] = useState(null);
  const [ownerIdentity, setOwnerIdentity] = useState(null);
  const [shareLink, setShareLink] = useState('');
  const [autoApproveRead, setAutoApproveRead] = useState(false);
  const [shareEndpoint, setShareEndpoint] = useState('');
  const [showCollectionCreator, setShowCollectionCreator] = useState(false);
  const [showPublishDialog, setShowPublishDialog] = useState(false);
  const [localCollections, setLocalCollections] = useState([]);
  const [publishCollectionId, setPublishCollectionId] = useState('');
  const [publishDraft, setPublishDraft] = useState(null);
  const [newCollectionName, setNewCollectionName] = useState('');
  const [query, setQuery] = useState('');
  const [results, setResults] = useState([]);
  const [uploadQueue, setUploadQueue] = useState([]);
  const [uploadDiscovery, setUploadDiscovery] = useState(null);
  const [showUploadSourceMenu, setShowUploadSourceMenu] = useState(false);
  const [showUploadDialog, setShowUploadDialog] = useState(false);
  const [showRecoveryCode, setShowRecoveryCode] = useState(false);
  const [showRestoreDialog, setShowRestoreDialog] = useState(false);
  const [restoreSource, setRestoreSource] = useState('');
  const [restoreCode, setRestoreCode] = useState('');
  const [backupRecoveryCode, setBackupRecoveryCode] = useState('');
  const [documentToTrash, setDocumentToTrash] = useState(null);
  const [confirmation, setConfirmation] = useState(null);
  const [includeTrash, setIncludeTrash] = useState(false);
  const [hostProgress, setHostProgress] = useState(null);
  const [busyCounts, setBusyCounts] = useState({});
  const [uploadInProgress, setUploadInProgress] = useState(false);
  const [notice, setNotice] = useState(null);
  const [recoveryCopyFeedback, setRecoveryCopyFeedback] = useState(null);
  const selectedServerRef = useRef('');
  const selectedCollectionRef = useRef(null);
  const collectionsRequestRef = useRef(0);
  const documentsRequestRef = useRef(0);
  const documentsRef = useRef([]);
  const documentsPageInFlightRef = useRef(null);
  const searchRequestRef = useRef(0);
  const connectionsRequestRef = useRef(0);
  const ownerPanelRequestRef = useRef(0);
  const ownerPanelTriggerRef = useRef(null);
  const ownerPeopleTabRef = useRef(null);
  const ownerHostTabRef = useRef(null);
  const confirmationResolverRef = useRef(null);
  const confirmationTriggerRef = useRef(null);
  const connectInFlightRef = useRef(false);
  const pendingRefreshInFlightRef = useRef(false);
  const ownerRequestsRefreshInFlightRef = useRef(false);
  const hostOperationInFlightRef = useRef(false);
  const uploadSourceMenuRef = useRef(null);

  const selectedConnection = useMemo(
    () => connections.find(item => item.serverId === selectedServerId) || null,
    [connections, selectedServerId],
  );
  const selectedCollection = useMemo(
    () => collections.find(item => item.id === selectedCollectionId) || null,
    [collections, selectedCollectionId],
  );
  const isOwner = selectedConnection?.scope === 'owner';
  const canManage = selectedConnection?.scope === 'manage' || isOwner;
  const isLocalHostOwner = Boolean(isOwner && hostStatus?.installed && selectedConnection?.endpoint === hostStatus?.endpoint);
  const hasLocalHostOwner = Boolean(hostStatus?.installed && connections.some(
    item => item.scope === 'owner' && item.endpoint === hostStatus.endpoint,
  ));
  const isBusy = useCallback(key => Boolean(busyCounts[key]), [busyCounts]);
  const anyBusy = Object.keys(busyCounts).length > 0;
  const connecting = isBusy('connect');
  const invitationIsShareLink = invitation.trim().startsWith('pinvou-knowledge://share');
  const connectionDetailsReady = Boolean(deviceName.trim() && invitation.trim());
  const visibleNearbyHosts = useMemo(
    () => nearbyHosts.filter(item => !connections.some(connection => connection.serverId === item.serverId)),
    [connections, nearbyHosts],
  );
  const pendingOwnerJoinRequests = useMemo(
    () => ownerJoinRequests.filter(item => item.status === 'pending'),
    [ownerJoinRequests],
  );
  const totalDocuments = collections.reduce((total, collection) => total + (collection.docCount || 0), 0);
  const totalChunks = collections.reduce((total, collection) => total + (collection.chunkCount || 0), 0);
  const uploadHasStarted = uploadQueue.some(item => item.status !== 'queued');
  const uploadCloseLabel = !uploadHasStarted
    ? t.remoteKbCancel
    : (uploadInProgress ? t.remoteKbCollapse : t.remoteKbDone);

  const documentStatusLabel = status => ({
    pending: t.remoteKbStatusPending,
    ready: t.remoteKbStatusReady,
    failed: t.remoteKbStatusFailed,
  })[status] || t.remoteKbStatusUnknown;

  const hostProgressLabel = phase => ({
    prepare: t.remoteKbHostProgressPrepare,
    install: t.remoteKbHostProgressInstall,
    connect: t.remoteKbHostProgressConnect,
    complete: t.remoteKbHostProgressComplete,
    failed: t.remoteKbHostProgressFailed,
  })[phase] || t.remoteKbHostProgressPrepare;

  const run = useCallback(async (key, action, success) => {
    setBusyCounts(current => ({ ...current, [key]: (current[key] || 0) + 1 }));
    setNotice(null);
    try {
      const value = await action();
      if (success) setNotice({ type: 'success', text: success });
      return value;
    } catch (error) {
      setNotice({ type: 'error', text: String(error) });
      return undefined;
    } finally {
      setBusyCounts(current => {
        const count = current[key] || 0;
        if (count > 1) return { ...current, [key]: count - 1 };
        const next = { ...current };
        delete next[key];
        return next;
      });
    }
  }, []);

  async function copyWithFeedback(value, successMessage, local = false) {
    const copied = await copyClipboardText(value);
    const feedback = {
      type: copied ? 'success' : 'error',
      text: copied ? successMessage : t.remoteKbCopyFailed,
    };
    if (local) setRecoveryCopyFeedback(feedback);
    else setNotice(feedback);
  }

  useEffect(() => {
    if (!isTauriAvailable()) return undefined;
    let disposed = false;
    let unlisten = null;
    listenTauri('shared-knowledge-host-progress', event => {
      if (disposed || !event?.payload) return;
      const progress = event.payload;
      setHostProgress(current => {
        if (!current || current.operation !== progress.operation) return current;
        return { ...current, ...progress };
      });
    }).then(dispose => {
      if (disposed) dispose();
      else unlisten = dispose;
    }).catch(() => {});
    return () => {
      disposed = true;
      if (unlisten) unlisten();
    };
  }, []);

  useEffect(() => {
    if (!showOwnerPanel || !ownerModelStatus?.downloading || !selectedServerId) return undefined;
    const serverId = selectedServerId;
    const requestId = ownerPanelRequestRef.current;
    const timer = window.setInterval(async () => {
      try {
        const status = await invokeTauri('remote_kb_model_status', { serverId });
        if (requestId !== ownerPanelRequestRef.current || serverId !== selectedServerRef.current) return;
        setOwnerModelStatus(status);
        if (!status.downloading) await loadConnections();
      } catch {
        // The visible refresh action remains available when a transient poll fails.
      }
    }, 2000);
    return () => window.clearInterval(timer);
  }, [showOwnerPanel, ownerModelStatus?.downloading, selectedServerId]);

  const closeOwnerPanel = useCallback(() => {
    ownerPanelRequestRef.current += 1;
    setShowOwnerPanel(false);
    window.requestAnimationFrame(() => ownerPanelTriggerRef.current?.focus());
  }, []);

  const requestConfirmation = useCallback(options => new Promise(resolve => {
    confirmationResolverRef.current?.(false);
    confirmationResolverRef.current = resolve;
    const activeElement = document.activeElement;
    confirmationTriggerRef.current = activeElement && typeof activeElement.focus === 'function' ? activeElement : null;
    setConfirmation({ dangerous: false, ...options });
  }), []);

  const finishConfirmation = useCallback(confirmed => {
    const resolve = confirmationResolverRef.current;
    confirmationResolverRef.current = null;
    setConfirmation(null);
    window.requestAnimationFrame(() => {
      if (confirmationTriggerRef.current?.isConnected) confirmationTriggerRef.current.focus();
      confirmationTriggerRef.current = null;
    });
    resolve?.(confirmed);
  }, []);

  useEffect(() => () => {
    confirmationResolverRef.current?.(false);
    confirmationResolverRef.current = null;
  }, []);

  useEffect(() => {
    if (!showOwnerPanel || showRecoveryCode || showRestoreDialog || confirmation) return undefined;
    const frame = window.requestAnimationFrame(() => {
      (ownerPanelTab === 'host' ? ownerHostTabRef : ownerPeopleTabRef).current?.focus();
    });
    return () => window.cancelAnimationFrame(frame);
  }, [confirmation, ownerPanelTab, showOwnerPanel, showRecoveryCode, showRestoreDialog]);

  const resetDocumentPaging = useCallback(() => {
    documentsRef.current = [];
    documentsPageInFlightRef.current = null;
    setDocuments([]);
    setDocumentsHasMore(false);
    setDocumentsLoadingPage(false);
  }, []);

  const selectCollection = useCallback((collectionId) => {
    if (selectedCollectionRef.current === collectionId) return;
    selectedCollectionRef.current = collectionId;
    documentsRequestRef.current += 1;
    searchRequestRef.current += 1;
    setSelectedCollectionId(collectionId);
    resetDocumentPaging();
    setResults([]);
  }, [resetDocumentPaging]);

  const selectServer = useCallback((serverId) => {
    if (selectedServerRef.current === serverId) return;
    selectedServerRef.current = serverId;
    selectedCollectionRef.current = null;
    collectionsRequestRef.current += 1;
    documentsRequestRef.current += 1;
    searchRequestRef.current += 1;
    setSelectedServerId(serverId);
    setCollections([]);
    setSelectedCollectionId(null);
    ownerPanelRequestRef.current += 1;
    setShowOwnerPanel(false);
    setOwnerShares([]);
    setOwnerJoinRequests([]);
    setOwnerDevices([]);
    setOwnerModelStatus(null);
    resetDocumentPaging();
    setResults([]);
    setNotice(null);
  }, [resetDocumentPaging]);

  const loadConnections = useCallback(async () => {
    if (!isTauriAvailable()) return;
    const requestId = ++connectionsRequestRef.current;
    const next = await run('connections', () => invokeTauri('remote_kb_connections'));
    if (!next || requestId !== connectionsRequestRef.current) return;
    setConnections(next);
    setConnectionsLoaded(true);
    const current = selectedServerRef.current;
    const nextServerId = current && next.some(item => item.serverId === current)
      ? current
      : (next[0]?.serverId || '');
    if (nextServerId !== current) selectServer(nextServerId);
  }, [run, selectServer]);

  const loadHostStatus = useCallback(async () => {
    if (!isTauriAvailable()) return;
    try {
      setHostStatus(await invokeTauri('shared_kb_host_status'));
    } catch {
      setHostStatus(null);
    }
  }, []);

  const refreshPendingJoins = useCallback(async () => {
    if (!isTauriAvailable() || pendingRefreshInFlightRef.current) return;
    pendingRefreshInFlightRef.current = true;
    try {
      const pending = (await invokeTauri('remote_kb_pending_joins')) || [];
      if (!pending.length) {
        setPendingJoins([]);
        return;
      }
      let connected = null;
      let connectedRequestId = null;
      await Promise.all(pending.map(async item => {
        try {
          const outcome = await invokeTauri('remote_kb_refresh_join', { requestId: item.requestId });
          if (outcome?.connection) {
            connected = outcome.connection;
            connectedRequestId = item.requestId;
          }
        } catch {
          // A transient offline server leaves the durable request visible for retry.
        }
      }));
      setPendingJoins((await invokeTauri('remote_kb_pending_joins')) || []);
      if (connected) {
        await loadConnections();
        selectServer(connected.serverId);
        if (joinFeedback?.requestId === connectedRequestId) {
          setJoinFeedback(current => ({
            ...current,
            status: 'approved',
            serverName: connected.name || current?.serverName,
          }));
          await wait(550);
          setShowConnector(false);
          setJoinFeedback(null);
        }
        setNotice({ type: 'success', text: t.remoteKbConnected });
      }
    } catch {
      // A transient bridge or server failure must not escape the polling loop;
      // durable requests stay visible and the next interval retries them.
    } finally {
      pendingRefreshInFlightRef.current = false;
    }
  }, [joinFeedback?.requestId, loadConnections, selectServer, t.remoteKbConnected]);

  const loadCollections = useCallback(async (serverId) => {
    const requestId = ++collectionsRequestRef.current;
    if (!serverId) {
      if (serverId === selectedServerRef.current) {
        setCollections([]);
        selectCollection(null);
      }
      return;
    }
    try {
      const next = await invokeTauri('remote_kb_collections', {
        serverId,
        includeDeleted: includeTrash,
      });
      if (requestId !== collectionsRequestRef.current || serverId !== selectedServerRef.current) return;
      setCollections(next);
      const current = selectedCollectionRef.current;
      const nextCollectionId = current && next.some(item => item.id === current)
        ? current
        : (next.find(item => !item.deletedAt)?.id || next[0]?.id || null);
      if (nextCollectionId !== current) selectCollection(nextCollectionId);
    } catch (error) {
      if (requestId === collectionsRequestRef.current && serverId === selectedServerRef.current) {
        setNotice({ type: 'error', text: String(error) });
      }
    }
  }, [includeTrash, selectCollection]);

  const loadDocuments = useCallback(async (serverId, collectionId, reset = true) => {
    if (!reset && documentsPageInFlightRef.current !== null) return { loaded: false };
    const requestId = ++documentsRequestRef.current;
    const offset = reset ? 0 : documentsRef.current.length;
    if (!serverId || !collectionId) {
      if (serverId === selectedServerRef.current && collectionId === selectedCollectionRef.current) resetDocumentPaging();
      return { loaded: false };
    }
    documentsPageInFlightRef.current = requestId;
    setDocumentsLoadingPage(true);
    try {
      const next = await invokeTauri('remote_kb_documents', {
        serverId,
        collectionId,
        includeDeleted: includeTrash,
        limit: documentPageSize,
        offset,
      });
      if (requestId !== documentsRequestRef.current
        || serverId !== selectedServerRef.current
        || collectionId !== selectedCollectionRef.current) return { loaded: false };
      const merged = reset
        ? next
        : [...new Map([...documentsRef.current, ...next].map(document => [document.id, document])).values()];
      documentsRef.current = merged;
      setDocuments(merged);
      setDocumentsHasMore(next.length === documentPageSize);
      return { loaded: true };
    } catch (error) {
      if (requestId === documentsRequestRef.current
        && serverId === selectedServerRef.current
        && collectionId === selectedCollectionRef.current) {
        setNotice({ type: 'error', text: String(error) });
      }
      return { loaded: false, error: String(error) };
    } finally {
      if (documentsPageInFlightRef.current === requestId) {
        documentsPageInFlightRef.current = null;
        setDocumentsLoadingPage(false);
      }
    }
  }, [includeTrash, resetDocumentPaging]);

  const refreshRemoteKnowledge = useCallback(() => run('remote-refresh', async () => {
    await Promise.all([loadConnections(), loadHostStatus()]);
    const serverId = selectedServerRef.current;
    const collectionId = selectedCollectionRef.current;
    await loadCollections(serverId);
    if (serverId !== selectedServerRef.current) return;
    if (collectionId === selectedCollectionRef.current) {
      await loadDocuments(serverId, collectionId, true);
    }
  }), [loadCollections, loadConnections, loadDocuments, loadHostStatus, run]);

  useEffect(() => { loadConnections(); }, [loadConnections]);
  useEffect(() => { loadHostStatus(); }, [loadHostStatus]);
  useEffect(() => {
    refreshPendingJoins();
    const timer = window.setInterval(refreshPendingJoins, 2000);
    return () => window.clearInterval(timer);
  }, [refreshPendingJoins]);
  useEffect(() => {
    if (!isOwner || !selectedServerId) {
      setOwnerJoinRequests([]);
      return undefined;
    }
    let disposed = false;
    const serverId = selectedServerId;
    const refresh = async () => {
      if (ownerRequestsRefreshInFlightRef.current) return;
      ownerRequestsRefreshInFlightRef.current = true;
      try {
        const requests = await invokeTauri('remote_kb_join_requests', { serverId });
        if (!disposed && serverId === selectedServerRef.current) {
          setOwnerJoinRequests(requests || []);
        }
      } catch {
        // Keep the current list visible; the next poll retries automatically.
      } finally {
        ownerRequestsRefreshInFlightRef.current = false;
      }
    };
    refresh();
    const timer = window.setInterval(refresh, 2000);
    return () => {
      disposed = true;
      window.clearInterval(timer);
    };
  }, [isOwner, selectedServerId]);
  useEffect(() => { loadCollections(selectedServerId); }, [loadCollections, selectedServerId]);
  useEffect(() => { loadDocuments(selectedServerId, selectedCollectionId, true); }, [loadDocuments, selectedCollectionId, selectedServerId]);
  useEffect(() => {
    if (selectedConnection && !canManage && includeTrash) setIncludeTrash(false);
  }, [canManage, includeTrash, selectedConnection]);
  useEffect(() => {
    if (!showConnector && !showOwnerPanel && !showCollectionCreator && !showPublishDialog && !showUploadDialog && !showRecoveryCode && !showRestoreDialog && !documentToTrash && !confirmation) return undefined;
    const closeOnEscape = event => {
      if (event.key !== 'Escape') return;
      if (confirmation) finishConfirmation(false);
      else if (documentToTrash) setDocumentToTrash(null);
      else if (showRecoveryCode) setShowRecoveryCode(false);
      else if (showRestoreDialog && !isBusy('restore-host')) setShowRestoreDialog(false);
      else if (showUploadDialog && !isBusy('upload')) {
        if (!uploadHasStarted) setPublishDraft(null);
        setShowUploadDialog(false);
      }
      else if (showPublishDialog && !isBusy('prepare-publish')) setShowPublishDialog(false);
      else if (showCollectionCreator) setShowCollectionCreator(false);
      else if (showOwnerPanel) closeOwnerPanel();
      else if (showConnector && !isBusy('connect') && joinFeedback?.status !== 'approved') {
        setShowConnector(false);
        setJoinFeedback(null);
        setIdentityProbe(null);
      }
    };
    window.addEventListener('keydown', closeOnEscape);
    return () => window.removeEventListener('keydown', closeOnEscape);
  }, [closeOwnerPanel, confirmation, documentToTrash, finishConfirmation, isBusy, joinFeedback?.status, showCollectionCreator, showConnector, showOwnerPanel, showPublishDialog, showRecoveryCode, showRestoreDialog, showUploadDialog, uploadHasStarted]);
  useEffect(() => {
    if (!showUploadSourceMenu) return undefined;
    const closeMenu = event => {
      if (event.key === 'Escape' || !uploadSourceMenuRef.current?.contains(event.target)) {
        setShowUploadSourceMenu(false);
      }
    };
    window.addEventListener('keydown', closeMenu);
    window.addEventListener('pointerdown', closeMenu);
    return () => {
      window.removeEventListener('keydown', closeMenu);
      window.removeEventListener('pointerdown', closeMenu);
    };
  }, [showUploadSourceMenu]);

  async function connectServer() {
    if (!connectionDetailsReady || connecting || connectInFlightRef.current) return;
    connectInFlightRef.current = true;
    try {
      setConnectorError('');
      if (!invitationIsShareLink && !identityProbe) {
        const probe = await run('connect', async () => {
          try {
            return await invokeTauri('remote_kb_probe_private_endpoint', { source: invitation.trim() });
          } catch (error) {
            setConnectorError(String(error));
            throw error;
          }
        });
        if (probe) setIdentityProbe(probe);
        return;
      }
      const outcome = await run('connect', async () => {
        try {
          return await (invitationIsShareLink
            ? invokeTauri('remote_kb_request_join', {
              source: invitation.trim(),
              deviceName: deviceName.trim(),
            })
            : invokeTauri('remote_kb_request_join_confirmed', {
              probe: identityProbe,
              deviceName: deviceName.trim(),
              confirmedCaFingerprint: identityProbe.caFingerprint,
              confirmedIdentityCode: identityProbe.identityCode,
            }));
        } catch (error) {
          setConnectorError(String(error));
          throw error;
        }
      });
      if (!outcome) return;
      if (outcome.connection) {
        setInvitation('');
        setDeviceName('');
        setJoinFeedback({
          requestId: outcome.request?.id || '',
          serverName: outcome.connection.name,
          status: 'approved',
        });
        await loadConnections();
        selectServer(outcome.connection.serverId);
        setNotice({ type: 'success', text: t.remoteKbConnected });
        await wait(550);
        setShowConnector(false);
        setJoinFeedback(null);
      } else {
        setJoinFeedback(outcome.pending ? { ...outcome.pending, status: 'pending' } : null);
        if (outcome.pending) {
          setPendingJoins(current => [
            outcome.pending,
            ...current.filter(item => item.requestId !== outcome.pending.requestId),
          ]);
        }
        setNotice({ type: 'success', text: t.remoteKbJoinRequested });
      }
    } finally {
      connectInFlightRef.current = false;
    }
  }

  async function discoverNearby() {
    if (discoveryStatus === 'discovering') return;
    setDiscoveryStatus('discovering');
    setConnectorError('');
    try {
      const discovered = (await invokeTauri('shared_kb_discover_nearby')) || [];
      setNearbyHosts(discovered);
      setDiscoveryStatus('done');
    } catch (error) {
      setNearbyHosts([]);
      setDiscoveryStatus('failed');
      setConnectorError(String(error));
    }
  }

  function chooseNearbyHost(probe) {
    setInvitation(probe.endpoint);
    setIdentityProbe(probe);
    setConnectorError('');
  }

  function closeConnector() {
    setShowConnector(false);
    setJoinFeedback(null);
    setIdentityProbe(null);
    setConnectorError('');
  }

  function openConnector() {
    setJoinFeedback(null);
    setIdentityProbe(null);
    setConnectorError('');
    setShowConnector(true);
    void discoverNearby();
  }

  async function cancelPendingJoin(requestId) {
    const done = await run(`cancel-join-${requestId}`, () => invokeTauri('remote_kb_cancel_join', { requestId }));
    if (!done) return;
    await refreshPendingJoins();
  }

  async function installHost() {
    if (hostOperationInFlightRef.current) return;
    hostOperationInFlightRef.current = true;
    setBusyCounts(current => ({ ...current, 'install-host': (current['install-host'] || 0) + 1 }));
    setNotice(null);
    setHostProgress({ operation: 'install', phase: 'prepare', percent: 5, error: null });
    try {
      const connection = await invokeTauri('shared_kb_host_install');
      await Promise.all([loadHostStatus(), loadConnections()]);
      selectServer(connection.serverId);
      setHostProgress(current => (current?.operation === 'install'
        ? { operation: 'install', phase: 'complete', percent: 100, error: null }
        : null));
      setNotice({ type: 'success', text: t.remoteKbHostCreated });
      window.setTimeout(() => setHostProgress(current => (
        current?.operation === 'install' && current.phase === 'complete' ? null : current
      )), 1200);
    } catch (error) {
      const message = String(error);
      setHostProgress(current => (current?.operation === 'install' ? {
        operation: 'install', phase: 'failed', percent: current.percent || 5, error: message,
      } : null));
      setNotice({ type: 'error', text: message });
    } finally {
      hostOperationInFlightRef.current = false;
      setBusyCounts(current => {
        const next = { ...current };
        delete next['install-host'];
        return next;
      });
    }
  }

  async function upgradeHost() {
    if (hostOperationInFlightRef.current) return;
    hostOperationInFlightRef.current = true;
    setBusyCounts(current => ({ ...current, 'upgrade-host': (current['upgrade-host'] || 0) + 1 }));
    setNotice(null);
    setHostProgress({ operation: 'upgrade', phase: 'prepare', percent: 5, error: null });
    try {
      await invokeTauri('shared_kb_host_upgrade');
      await Promise.all([loadHostStatus(), loadConnections()]);
      setHostProgress(current => (current?.operation === 'upgrade'
        ? { operation: 'upgrade', phase: 'complete', percent: 100, error: null }
        : null));
      setNotice({ type: 'success', text: t.remoteKbHostUpgraded });
      window.setTimeout(() => setHostProgress(current => (
        current?.operation === 'upgrade' && current.phase === 'complete' ? null : current
      )), 1200);
    } catch (error) {
      const message = String(error);
      setHostProgress(current => (current?.operation === 'upgrade' ? {
        operation: 'upgrade', phase: 'failed', percent: current.percent || 5, error: message,
      } : null));
      setNotice({ type: 'error', text: message });
    } finally {
      hostOperationInFlightRef.current = false;
      setBusyCounts(current => {
        const next = { ...current };
        delete next['upgrade-host'];
        return next;
      });
    }
  }

  async function reconnectHost() {
    if (hostOperationInFlightRef.current) return;
    hostOperationInFlightRef.current = true;
    setBusyCounts(current => ({ ...current, 'reconnect-host': (current['reconnect-host'] || 0) + 1 }));
    setNotice(null);
    setHostProgress({ operation: 'reconnect', phase: 'prepare', percent: 5, error: null });
    try {
      const connection = await invokeTauri('shared_kb_host_reconnect');
      await Promise.all([loadHostStatus(), loadConnections()]);
      selectServer(connection.serverId);
      setHostProgress(current => (current?.operation === 'reconnect'
        ? { operation: 'reconnect', phase: 'complete', percent: 100, error: null }
        : null));
      setNotice({ type: 'success', text: t.remoteKbHostReconnected });
      window.setTimeout(() => setHostProgress(current => (
        current?.operation === 'reconnect' && current.phase === 'complete' ? null : current
      )), 1200);
    } catch (error) {
      const message = String(error);
      setHostProgress(current => (current?.operation === 'reconnect'
        ? { operation: 'reconnect', phase: 'failed', percent: 100, error: message }
        : null));
      setNotice({ type: 'error', text: message });
    } finally {
      hostOperationInFlightRef.current = false;
      setBusyCounts(current => {
        const next = { ...current };
        delete next['reconnect-host'];
        return next;
      });
    }
  }

  async function openOwnerPanel() {
    if (!selectedServerId || !isOwner) return;
    const serverId = selectedServerId;
    const requestId = ++ownerPanelRequestRef.current;
    if (!showOwnerPanel) {
      setOwnerPanelTab('people');
      setNotice(null);
    }
    setShowOwnerPanel(true);
    setShareLink('');
    setOwnerShares([]);
    setOwnerJoinRequests([]);
    setOwnerDevices([]);
    setOwnerModelStatus(null);
    setOwnerIdentity(null);
    const [shares, requests, devices, modelStatus, identity] = await Promise.all([
      run('owner-shares', () => invokeTauri('remote_kb_shares', { serverId })),
      run('owner-requests', () => invokeTauri('remote_kb_join_requests', { serverId })),
      run('owner-devices', () => invokeTauri('remote_kb_devices', { serverId })),
      run('owner-model', () => invokeTauri('remote_kb_model_status', { serverId })),
      run('owner-identity', () => invokeTauri('remote_kb_connection_identity', { serverId })),
    ]);
    if (requestId !== ownerPanelRequestRef.current || serverId !== selectedServerRef.current) return;
    if (shares) setOwnerShares(shares);
    if (requests) setOwnerJoinRequests(requests);
    if (devices) setOwnerDevices(devices);
    if (modelStatus) setOwnerModelStatus(modelStatus);
    if (identity) setOwnerIdentity(identity);
  }

  function moveOwnerPanelTab(event) {
    let nextTab = null;
    if (event.key === 'Home') nextTab = 'people';
    else if (event.key === 'End') nextTab = 'host';
    else if (event.key === 'ArrowLeft' || event.key === 'ArrowRight') nextTab = ownerPanelTab === 'people' ? 'host' : 'people';
    if (!nextTab) return;
    event.preventDefault();
    setOwnerPanelTab(nextTab);
  }

  async function createShare() {
    const created = await run('create-share', async () => {
      let endpoints = [selectedConnection.endpoint];
      if (isLocalHostOwner) {
        endpoints = await invokeTauri('shared_kb_host_lan_endpoints');
      }
      if (shareEndpoint.trim()) endpoints.push(shareEndpoint.trim());
      if (!endpoints.length) throw new Error(t.remoteKbNoLanEndpoint);
      return invokeTauri('remote_kb_create_share', {
        serverId: selectedServerId,
        endpoints,
        autoApproveRead,
      });
    });
    if (!created) return;
    setShareLink(created.share);
    setOwnerShares(await invokeTauri('remote_kb_shares', { serverId: selectedServerId }));
  }

  async function resolveJoinRequest(requestId, scope) {
    if (isBusy(`owner-request-${requestId}`)) return;
    const command = scope ? 'remote_kb_approve_join_request' : 'remote_kb_reject_join_request';
    const resolved = await run(`owner-request-${requestId}`, () => invokeTauri(command, {
      serverId: selectedServerId,
      requestId,
      ...(scope ? { scope } : {}),
    }));
    if (!resolved) return;
    setOwnerJoinRequests(await invokeTauri('remote_kb_join_requests', { serverId: selectedServerId }));
    setOwnerDevices(await invokeTauri('remote_kb_devices', { serverId: selectedServerId }));
  }

  async function stopShare(shareId) {
    const stopped = await run(`stop-share-${shareId}`, () => invokeTauri('remote_kb_stop_share', { serverId: selectedServerId, shareId }));
    if (stopped) setOwnerShares(await invokeTauri('remote_kb_shares', { serverId: selectedServerId }));
  }

  async function updateMember(device, patch) {
    const updated = await run(`member-${device.id}`, () => invokeTauri('remote_kb_update_device', {
      serverId: selectedServerId,
      deviceId: device.id,
      name: null,
      scope: null,
      revoked: null,
      ...patch,
    }));
    if (updated) setOwnerDevices(current => current.map(item => (item.id === updated.id ? updated : item)));
  }

  async function removeMember(device) {
    const confirmed = await requestConfirmation({
      testId: 'remote-remove-member-confirm',
      title: t.remoteKbRemoveMemberAction,
      description: t.remoteKbRemoveMemberConfirm.replace('{name}', device.name),
      confirmLabel: t.remoteKbRemoveMemberAction,
      dangerous: true,
    });
    if (!confirmed) return;
    const done = await run(`member-${device.id}`, () => invokeTauri('remote_kb_remove_device', { serverId: selectedServerId, deviceId: device.id }));
    if (done !== undefined) setOwnerDevices(current => current.filter(item => item.id !== device.id));
  }

  async function changeOwner(device, owner) {
    if (!isLocalHostOwner) return;
    const confirmed = await requestConfirmation({
      testId: 'remote-owner-change-confirm',
      title: owner ? t.remoteKbPromoteOwner : t.remoteKbDemoteOwner,
      description: (owner ? t.remoteKbPromoteOwnerConfirm : t.remoteKbDemoteOwnerConfirm).replace('{name}', device.name),
      confirmLabel: owner ? t.remoteKbPromoteOwner : t.remoteKbDemoteOwner,
    });
    if (!confirmed) return;
    const updated = await run(`member-${device.id}`, () => invokeTauri('shared_kb_host_set_owner_device', { deviceId: device.id, owner }));
    if (updated) setOwnerDevices(current => current.map(item => (item.id === updated.id ? updated : item)));
  }

  async function downloadOwnerModel() {
    const status = await run('owner-model-download', () => invokeTauri('remote_kb_download_model', { serverId: selectedServerId }));
    if (status) setOwnerModelStatus(status);
  }

  async function removeHost(deleteData) {
    const prompt = deleteData ? t.remoteKbDeleteHostConfirm : t.remoteKbRemoveHostConfirm;
    const confirmed = await requestConfirmation({
      testId: deleteData ? 'shared-kb-delete-host-confirm' : 'shared-kb-remove-host-confirm',
      title: deleteData ? t.remoteKbDeleteHost : t.remoteKbRemoveHost,
      description: prompt,
      confirmLabel: deleteData ? t.remoteKbDeleteHost : t.remoteKbRemoveHost,
      dangerous: deleteData,
    });
    if (!confirmed) return;
    const done = await run(deleteData ? 'delete-host' : 'remove-host', () => invokeTauri('shared_kb_host_remove', {
      serverId: selectedServerId,
      deleteData,
    }));
    if (done === undefined) return;
    closeOwnerPanel();
    await Promise.all([loadHostStatus(), loadConnections()]);
    setNotice({ type: 'success', text: deleteData ? t.remoteKbHostDeleted : t.remoteKbHostRemoved });
  }

  async function backupHost() {
    const destination = await saveTauriDialog({
      defaultPath: `pinvou-shared-knowledge-${new Date().toISOString().slice(0, 10)}.pinbak`,
      filters: [{ name: 'PINVOU Backup', extensions: ['pinbak'] }],
    });
    if (!destination) return;
    const result = await run('backup-host', () => invokeTauri('shared_kb_host_backup', { destination }));
    if (!result) return;
    setBackupRecoveryCode(result.recoveryCode);
    setRecoveryCopyFeedback(null);
    setShowRecoveryCode(true);
  }

  async function openRestoreDialog() {
    const source = await openTauriDialog({
      multiple: false,
      directory: false,
      filters: [{ name: 'PINVOU Backup', extensions: ['pinbak'] }],
    });
    if (!source) return;
    setRestoreSource(source);
    setRestoreCode('');
    setShowRestoreDialog(true);
  }

  async function restoreHost() {
    if (!restoreSource || !selectedServerId) return;
    const migration = Boolean(restoreCode.trim());
    const confirmed = await requestConfirmation({
      testId: 'shared-kb-restore-confirm',
      title: t.remoteKbRestoreTitle,
      description: migration ? t.remoteKbMigrateConfirm : t.remoteKbRestoreConfirm,
      confirmLabel: t.remoteKbRestoreAction,
      dangerous: true,
    });
    if (!confirmed) return;
    const restored = await run('restore-host', () => invokeTauri('shared_kb_host_restore', {
      serverId: selectedServerId,
      source: restoreSource,
      recoveryCode: migration ? restoreCode.trim() : null,
    }));
    if (!restored) return;
    setShowRestoreDialog(false);
    setRestoreSource('');
    setRestoreCode('');
    await Promise.all([loadConnections(), loadHostStatus()]);
    setNotice({ type: 'success', text: migration ? t.remoteKbMigrated : t.remoteKbRestored });
  }

  async function permanentlyDelete(kind, id, name) {
    const confirmed = await requestConfirmation({
      testId: 'remote-permanent-delete-confirm',
      title: t.remoteKbPermanentDelete,
      description: t.remoteKbPermanentDeleteConfirm.replace('{name}', name),
      confirmLabel: t.remoteKbPermanentDelete,
      dangerous: true,
    });
    if (!confirmed) return;
    const command = kind === 'collection' ? 'remote_kb_permanently_delete_collection' : 'remote_kb_permanently_delete_document';
    const done = await run(`permanent-${kind}-${id}`, () => invokeTauri(command, { serverId: selectedServerId, id }));
    if (done === undefined) return;
    await loadCollections(selectedServerId);
    if (kind === 'document' && selectedCollectionId) await loadDocuments(selectedServerId, selectedCollectionId, true);
  }

  async function removeConnection(serverId) {
    const confirmed = await requestConfirmation({
      testId: 'remote-disconnect-confirm',
      title: t.remoteKbDisconnect,
      description: t.remoteKbRemoveConfirm,
      confirmLabel: t.remoteKbDisconnect,
    });
    if (!confirmed) return;
    const result = await run('remove-server', () => invokeTauri('remote_kb_remove_connection', { serverId }));
    if (result === undefined) return;
    await loadConnections();
  }

  async function createCollection() {
    const name = newCollectionName.trim();
    if (!name || !selectedServerId || isBusy('create-collection')) return;
    const created = await run('create-collection', () => invokeTauri('remote_kb_create_collection', {
      serverId: selectedServerId,
      name,
      description: null,
    }), t.remoteKbCollectionCreated);
    if (!created) return;
    setNewCollectionName('');
    setShowCollectionCreator(false);
    await loadCollections(selectedServerId);
    if (selectedServerId === selectedServerRef.current) selectCollection(created.id);
  }

  async function openPublishDialog() {
    const local = await run('local-collections', () => invokeTauri('kb_collection_list'));
    if (!local) return;
    setLocalCollections(local);
    setPublishCollectionId(local[0]?.id ? String(local[0].id) : '');
    setShowPublishDialog(true);
  }

  async function preparePublish() {
    const localCollection = localCollections.find(item => String(item.id) === String(publishCollectionId));
    if (!localCollection || !selectedServerId) return;
    const prepared = await run('prepare-publish', async () => {
      const localDocuments = await invokeTauri('kb_documents', {
        collectionId: localCollection.id,
        limit: 0,
      });
      if (!localDocuments?.length) throw new Error(t.remoteKbPublishEmpty);
      return { localCollection, localDocuments };
    });
    if (!prepared) return;
    setPublishDraft({
      serverId: selectedServerId,
      name: prepared.localCollection.name,
      description: prepared.localCollection.description || null,
    });
    setUploadQueue(prepared.localDocuments.map(document => ({
      path: document.path,
      name: document.name,
      status: 'queued',
      error: '',
      pollTimedOut: false,
    })));
    setUploadDiscovery({ count: prepared.localDocuments.length, skipped: 0 });
    setShowPublishDialog(false);
    setShowUploadDialog(true);
  }

  async function changeCollectionTrash(collection) {
    if (isBusy('collection-trash')) return;
    const command = collection.deletedAt ? 'remote_kb_restore_collection' : 'remote_kb_delete_collection';
    if (!collection.deletedAt) {
      const confirmed = await requestConfirmation({
        testId: 'remote-collection-trash-confirm',
        title: t.remoteKbTrash,
        description: t.remoteKbTrashConfirm,
        confirmLabel: t.remoteKbTrash,
      });
      if (!confirmed) return;
    }
    const done = await run('collection-trash', () => invokeTauri(command, {
      serverId: selectedServerId,
      id: collection.id,
    }));
    if (done === undefined) return;
    await loadCollections(selectedServerId);
  }

  async function chooseUploadFiles() {
    if (uploadInProgress) return;
    setShowUploadSourceMenu(false);
    const paths = await openTauriDialog({ multiple: true, directory: false });
    const selected = Array.isArray(paths) ? paths : (paths ? [paths] : []);
    if (!selected.length) return;
    setPublishDraft(null);
    setUploadQueue(selected.map(path => ({
      path,
      name: String(path).split(/[\\/]/).pop() || String(path),
      status: 'queued',
      error: '',
      pollTimedOut: false,
    })));
    setUploadDiscovery(null);
    setShowUploadDialog(true);
  }

  async function chooseUploadFolders() {
    if (uploadInProgress) return;
    setShowUploadSourceMenu(false);
    const roots = await openTauriDialog({ multiple: true, directory: true });
    const selected = Array.isArray(roots) ? roots : (roots ? [roots] : []);
    if (!selected.length) return;
    const discovery = await run('discover-folders', () => invokeTauri('remote_kb_discover_folder_files', {
      paths: selected,
    }));
    if (!discovery) return;
    if (discovery.limitExceeded) {
      setNotice({ type: 'error', text: t.remoteKbFolderLimitExceeded });
      return;
    }
    if (!discovery.paths?.length) {
      setNotice({ type: 'error', text: t.remoteKbFolderEmpty });
      return;
    }
    setPublishDraft(null);
    setUploadQueue(discovery.paths.map(path => ({
      path,
      name: String(path).split(/[\\/]/).pop() || String(path),
      status: 'queued',
      error: '',
      pollTimedOut: false,
    })));
    setUploadDiscovery({ count: discovery.paths.length, skipped: discovery.skipped || 0 });
    setShowUploadDialog(true);
  }

  async function startUpload() {
    const retryable = item => item.status === 'queued' || item.status === 'failed';
    const pending = item => item.status === 'pending_index' || item.status === 'duplicate_pending';
    if (!uploadQueue.some(retryable) || uploadInProgress) return;
    const serverId = selectedServerId;
    let collectionId = selectedCollectionId;
    setUploadInProgress(true);
    setBusyCounts(current => ({ ...current, upload: (current.upload || 0) + 1 }));
    setNotice(null);
    const nextQueue = uploadQueue.map(item => ({ ...item }));
    if (publishDraft) {
      try {
        const created = await invokeTauri('remote_kb_create_collection', {
          serverId,
          name: publishDraft.name,
          description: publishDraft.description,
        });
        collectionId = created.id;
        setPublishDraft(null);
        selectCollection(collectionId);
        await loadCollections(serverId);
      } catch (error) {
        setBusyCounts(current => {
          const next = { ...current };
          delete next.upload;
          return next;
        });
        setUploadInProgress(false);
        setNotice({ type: 'error', text: String(error) });
        return;
      }
    }
    for (let index = 0; index < nextQueue.length; index += 1) {
      if (!retryable(nextQueue[index])) continue;
      nextQueue[index] = {
        ...nextQueue[index], status: 'uploading', error: '', pollTimedOut: false,
      };
      setUploadQueue(nextQueue.map(item => ({ ...item })));
      try {
        const uploaded = await invokeTauri('remote_kb_upload_files', {
          serverId,
          collectionId,
          paths: [nextQueue[index].path],
        });
        const document = uploaded?.[0];
        const duplicate = Boolean(document?.alreadyExists);
        nextQueue[index] = {
          ...nextQueue[index],
          documentId: document?.id,
          status: duplicate
            ? (document?.status === 'ready' ? 'duplicate' : (document?.status === 'failed' ? 'duplicate_failed' : 'duplicate_pending'))
            : (document?.status === 'ready' ? 'success' : (document?.status === 'failed' ? 'index_failed' : 'pending_index')),
          error: document?.status === 'failed' ? (document.error || t.remoteKbUploadIndexFailed) : '',
          pollTimedOut: false,
        };
      } catch (error) {
        nextQueue[index] = { ...nextQueue[index], status: 'failed', error: String(error) };
      }
      setUploadQueue(nextQueue.map(item => ({ ...item })));
    }

    // Uploading is complete at this point. Indexing continues on the server, so
    // keep live status updates without blocking the rest of the application.
    setBusyCounts(current => {
      const count = current.upload || 0;
      if (count > 1) return { ...current, upload: count - 1 };
      const next = { ...current };
      delete next.upload;
      return next;
    });
    let refreshError = '';
    const pollInterval = uploadPollSetting('__REMOTE_UPLOAD_POLL_INTERVAL_MS__', uploadIndexPollIntervalMs);
    const pollDeadline = Date.now()
      + uploadPollSetting('__REMOTE_UPLOAD_POLL_TIMEOUT_MS__', uploadIndexPollTimeoutMs);
    while (nextQueue.some(pending) && Date.now() < pollDeadline) {
      const pendingIds = nextQueue
        .filter(item => pending(item) && item.documentId)
        .map(item => item.documentId);
      const currentDocuments = await settleDocumentStatusBatches(
        pendingIds,
        pollDeadline,
        documentIds => invokeTauri('remote_kb_document_statuses', { serverId, documentIds }),
      );
      const byId = new Map(currentDocuments.map(document => [document.id, document]));
      nextQueue.forEach((item, index) => {
        if (!pending(item) || !item.documentId) return;
        const document = byId.get(item.documentId);
        if (document?.status === 'ready') {
          nextQueue[index] = { ...item, status: item.status === 'duplicate_pending' ? 'duplicate' : 'success', error: '' };
        } else if (document?.status === 'failed') {
          nextQueue[index] = {
            ...item,
            status: item.status === 'duplicate_pending' ? 'duplicate_failed' : 'index_failed',
            error: document.error || t.remoteKbUploadIndexFailed,
          };
        }
      });
      setUploadQueue(nextQueue.map(item => ({ ...item })));
      if (nextQueue.some(pending) && Date.now() < pollDeadline) {
        await wait(Math.min(pollInterval, Math.max(0, pollDeadline - Date.now())));
      }
    }
    nextQueue.forEach((item, index) => {
      if (pending(item)) nextQueue[index] = { ...item, pollTimedOut: true };
    });
    setUploadQueue(nextQueue.map(item => ({ ...item })));

    try {
      const [nextCollections, documentRefresh] = await Promise.all([
        invokeTauri('remote_kb_collections', { serverId, includeDeleted: includeTrash }),
        loadDocuments(serverId, collectionId, true),
      ]);
      if (serverId === selectedServerRef.current && collectionId === selectedCollectionRef.current) {
        setCollections(nextCollections);
      }
      if (documentRefresh?.error) refreshError = documentRefresh.error;
    } catch (error) {
      refreshError = String(error);
    }
    setUploadInProgress(false);
    const counts = nextQueue.reduce((output, item) => ({ ...output, [item.status]: (output[item.status] || 0) + 1 }), {});
    const completed = counts.success || 0;
    const existing = counts.duplicate || 0;
    const processing = (counts.pending_index || 0) + (counts.duplicate_pending || 0);
    const failed = (counts.index_failed || 0) + (counts.duplicate_failed || 0) + (counts.failed || 0);
    const summary = [
      completed ? t.remoteKbUploadSuccess.replace('{count}', String(completed)) : '',
      existing ? t.remoteKbUploadExistingSummary.replace('{count}', String(existing)) : '',
      processing ? t.remoteKbUploadProcessingSummary.replace('{count}', String(processing)) : '',
      failed ? t.remoteKbUploadFailedSummary.replace('{count}', String(failed)) : '',
    ].filter(Boolean).join(' · ');
    setNotice({
      type: failed || refreshError ? 'error' : 'success',
      text: refreshError ? `${summary} ${t.remoteKbUploadRefreshFailed.replace('{error}', refreshError)}` : summary,
    });
  }

  async function changeDocumentTrash(document) {
    const serverId = selectedServerId;
    const collectionId = selectedCollectionId;
    const deleting = !document.deletedAt;
    const command = document.deletedAt ? 'remote_kb_restore_document' : 'remote_kb_delete_document';
    const busyKey = `document-trash-${document.id}`;
    const previousDocuments = documentsRef.current;
    const optimisticDocument = { ...document, deletedAt: deleting ? Math.floor(Date.now() / 1000) : null };
    const optimisticDocuments = includeTrash
      ? previousDocuments.map(item => (item.id === document.id ? optimisticDocument : item))
      : previousDocuments.filter(item => item.id !== document.id);
    const countDelta = deleting ? -1 : 1;

    setDocumentToTrash(null);
    setNotice(null);
    documentsRef.current = optimisticDocuments;
    setDocuments(optimisticDocuments);
    setCollections(current => current.map(collection => (collection.id === document.collectionId ? {
      ...collection,
      docCount: Math.max(0, (collection.docCount || 0) + countDelta),
      chunkCount: Math.max(0, (collection.chunkCount || 0) + countDelta * (document.nChunks || 0)),
      totalBytes: Math.max(0, (collection.totalBytes || 0) + countDelta * (document.size || 0)),
    } : collection)));
    setBusyCounts(current => ({ ...current, [busyKey]: (current[busyKey] || 0) + 1 }));
    try {
      await invokeTauri(command, {
        serverId,
        id: document.id,
      });
      setNotice({ type: 'success', text: deleting ? t.remoteKbDocumentTrashed : t.remoteKbDocumentRestored });
    } catch (error) {
      // 失败不恢复入口快照：busy key 按文档隔离，并发操作交错时旧快照会把
      // 其他文档已生效的乐观改动一并回滚。改为重新拉取服务端权威状态
      //（loadCollections/loadDocuments 自带过期请求与选中态防护）。
      await Promise.all([
        loadCollections(serverId),
        loadDocuments(serverId, collectionId, true),
      ]);
      setNotice({ type: 'error', text: String(error) });
      return;
    } finally {
      setBusyCounts(current => {
        const count = current[busyKey] || 0;
        if (count > 1) return { ...current, [busyKey]: count - 1 };
        const next = { ...current };
        delete next[busyKey];
        return next;
      });
    }
    await Promise.all([
      loadCollections(serverId),
      loadDocuments(serverId, collectionId, true),
    ]);
  }

  function requestDocumentTrash(document) {
    if (document.deletedAt) {
      changeDocumentTrash(document);
      return;
    }
    setDocumentToTrash(document);
  }

  async function replaceDocument(document) {
    const path = await openTauriDialog({ multiple: false, directory: false });
    if (!path) return;
    const updated = await run('replace', () => invokeTauri('remote_kb_replace_document', {
      serverId: selectedServerId,
      documentId: document.id,
      path,
    }), t.remoteKbReplaced);
    if (updated) await Promise.all([
      loadCollections(selectedServerId),
      loadDocuments(selectedServerId, selectedCollectionId, true),
    ]);
  }

  async function downloadDocument(document) {
    const destination = await openTauriDialog({ directory: true, multiple: false });
    if (!destination) return;
    await run('download', () => invokeTauri('remote_kb_download_document', {
      serverId: selectedServerId,
      id: document.id,
      destination,
    }), t.remoteKbDownloaded);
  }

  async function search() {
    if (!query.trim() || !selectedCollectionId) return;
    const requestId = ++searchRequestRef.current;
    const serverId = selectedServerId;
    const collectionId = selectedCollectionId;
    const next = await run('search', () => invokeTauri('remote_kb_search', {
      serverId,
      collectionIds: [collectionId],
      query: query.trim(),
      limit: 8,
    }));
    if (next
      && requestId === searchRequestRef.current
      && serverId === selectedServerRef.current
      && collectionId === selectedCollectionRef.current) setResults(next);
  }

  function changeTrashVisibility(checked) {
    collectionsRequestRef.current += 1;
    documentsRequestRef.current += 1;
    searchRequestRef.current += 1;
    setIncludeTrash(checked);
    setCollections([]);
    resetDocumentPaging();
    setResults([]);
  }

  if (!isTauriAvailable()) {
    return <div className={embedded ? `mx-auto max-w-[1400px] py-8 ${muted}` : `p-8 ${muted}`}>{t.remoteKbDesktopOnly}</div>;
  }

  return (
    <div
      className={embedded
        ? `w-full ${ink}`
        : `h-full overflow-y-auto bg-[#f7f8fa] p-5 dark:bg-[#111113] md:p-8 ${ink}`}
      data-testid="remote-knowledge-panel"
      data-embedded={embedded ? 'true' : 'false'}
    >
      <div className="mx-auto max-w-[1400px] space-y-5">
        <header
          data-testid="remote-knowledge-hero"
          className="relative flex min-h-[168px] items-center gap-8 overflow-hidden rounded-3xl bg-gradient-to-br from-[#e9f7f4] via-[#e5f4f8] to-[#e5edfb] p-7 dark:bg-gradient-to-br dark:from-[#132b2d] dark:via-[#142a32] dark:to-[#18263a]"
        >
          <div
            className="pointer-events-none absolute right-0 top-0 hidden h-full overflow-hidden xl:block"
            style={{ aspectRatio: '1240 / 453' }}
            aria-hidden="true"
          >
            <img
              src={remoteKnowledgeHeroImage}
              alt=""
              data-testid="remote-knowledge-hero-art"
              className="h-full w-auto max-w-none object-contain opacity-95 dark:opacity-25"
              style={{ WebkitMaskImage: 'linear-gradient(to right, transparent 0%, #000 34%, #000 100%)', maskImage: 'linear-gradient(to right, transparent 0%, #000 34%, #000 100%)' }}
            />
            <div
              data-testid="remote-knowledge-brand"
              className="absolute grid h-[82px] w-[82px] -translate-x-1/2 -translate-y-1/2 place-items-center rounded-full bg-white/30 backdrop-blur-[1px] dark:bg-[#132b2d]/45"
              style={{ left: '72.8%', top: '49.7%' }}
            >
              <PinvouLogo className="h-[62px] w-[62px] drop-shadow-[0_8px_14px_rgba(11,87,208,0.20)] dark:brightness-125" />
            </div>
          </div>
          <div className="relative z-10 min-w-0 flex-1 xl:max-w-[58%]">
            <h1 className="text-[21px] font-bold tracking-[-0.01em] text-[#163c40] dark:text-[#E3E3E3]">{t.remoteKbHeroTitle}</h1>
            <p className="mt-1.5 max-w-[680px] text-[13px] leading-5 text-[#456267] dark:text-[#b9cacc]">{t.remoteKbDesc}</p>
            <div className="mt-4 flex flex-wrap items-center gap-2">
              {hostStatus?.supported && !hostStatus.installed && (
                <button
                  data-testid="shared-kb-create-host"
                  className="inline-flex h-10 items-center justify-center gap-1.5 rounded-xl bg-[#0B57D0] px-5 text-[14px] font-bold text-white transition-all hover:-translate-y-0.5 hover:bg-[#0848ad] disabled:opacity-45"
                  onClick={installHost}
                  disabled={isBusy('install-host')}
                >
                  {isBusy('install-host') ? <RefreshCw size={15} className="animate-spin" /> : <Server size={15} />}
                  {t.remoteKbCreateHost}
                </button>
              )}
              {connectionsLoaded && hostStatus?.supported && hostStatus.installed && !hasLocalHostOwner && (
                <button
                  data-testid="shared-kb-reconnect-host"
                  className="inline-flex h-10 items-center justify-center gap-1.5 rounded-xl bg-[#0B57D0] px-5 text-[14px] font-bold text-white transition-all hover:-translate-y-0.5 hover:bg-[#0848ad] disabled:opacity-45"
                  onClick={reconnectHost}
                  disabled={isBusy('reconnect-host')}
                >
                  {isBusy('reconnect-host') ? <RefreshCw size={15} className="animate-spin" /> : <Server size={15} />}
                  {t.remoteKbReconnectHost}
                </button>
              )}
              {hostStatus?.supported && hostStatus.upgradeAvailable && !hostStatus.clientOutdated && (
                <button data-testid="shared-kb-upgrade-host" className={soft} onClick={upgradeHost} disabled={isBusy('upgrade-host')}>
                  {isBusy('upgrade-host') && <RefreshCw size={14} className="animate-spin" />}
                  {t.remoteKbUpgradeHost}
                </button>
              )}
              <button
                data-testid="remote-add-server"
                className="inline-flex h-10 items-center justify-center gap-1.5 rounded-xl bg-[#087e8b] px-5 text-[14px] font-bold text-white transition-all hover:-translate-y-0.5 hover:bg-[#066b75] disabled:opacity-45"
                onClick={openConnector}
              >
                <Plus size={15} />
                {t.remoteKbAddServer}
              </button>
              <button
                className="inline-flex h-10 items-center justify-center gap-1.5 rounded-xl bg-white/70 px-4 text-[13px] font-semibold text-[#456267] transition-colors hover:bg-white disabled:opacity-45 dark:bg-white/10 dark:text-[#C4C7C5] dark:hover:bg-white/15"
                onClick={refreshRemoteKnowledge}
                disabled={anyBusy}
                data-testid="remote-refresh-connections"
                title={t.remoteKbRefresh}
              >
                <RefreshCw size={14} className={isBusy('remote-refresh') || isBusy('connections') ? 'animate-spin' : ''} />
                <span className="sr-only">{t.remoteKbRefresh}</span>
              </button>
            </div>
          </div>
        </header>

        {hostStatus?.supported && hostStatus.clientOutdated && (
          <div
            data-testid="shared-kb-client-outdated"
            role="status"
            aria-live="polite"
            className="flex items-start gap-3 rounded-2xl border border-[#e6a23c]/25 bg-[#fff8e8] px-4 py-3.5 text-[#805719] dark:border-[#e6a23c]/25 dark:bg-[#3a2b15] dark:text-[#f2c879]"
          >
            <AlertTriangle size={18} className="mt-0.5 shrink-0" />
            <div className="min-w-0">
              <p className="text-[13px] font-semibold">{t.remoteKbClientOutdatedTitle}</p>
              <p className="mt-0.5 text-[12px] leading-5 opacity-85">
                {t.remoteKbClientOutdatedDesc
                  .replace('{appVersion}', hostStatus.appVersion || '—')
                  .replace('{serviceVersion}', hostStatus.serviceVersion || '—')}
              </p>
            </div>
          </div>
        )}

        {notice && (
          <div role="status" className={`flex items-start gap-2 rounded-xl border px-3.5 py-3 text-[13px] ${notice.type === 'error' ? 'border-[#d63a3a]/20 bg-[#d63a3a]/8 text-[#b72f2f]' : 'border-[#18a957]/20 bg-[#18a957]/8 text-[#16894a]'}`}>
            {notice.type === 'error' ? <AlertTriangle size={16} /> : <CheckCircle2 size={16} />}
            <span className="min-w-0 flex-1 break-all">{notice.text}</span>
            <button className="opacity-70 hover:opacity-100" onClick={() => setNotice(null)}><X size={15} /></button>
          </div>
        )}

        {!!pendingJoins.length && (
          <section data-testid="remote-pending-joins" className={`rounded-2xl p-4 animate-in fade-in slide-in-from-top-2 duration-200 motion-reduce:animate-none ${panel} ${panelShadow}`}>
            <div className="flex items-center justify-between gap-3">
              <div>
                <h2 className={`text-[14px] font-bold ${ink}`}>{t.remoteKbPendingTitle}</h2>
                <p className={`mt-0.5 text-[12px] ${muted}`}>{t.remoteKbPendingDesc}</p>
              </div>
              <button className={iconButton} onClick={refreshPendingJoins} title={t.remoteKbRefresh}><RefreshCw size={14} /></button>
            </div>
            <div className="mt-3 grid gap-2 sm:grid-cols-2">
              {pendingJoins.map(item => (
                <div key={item.requestId} className="flex items-center gap-3 rounded-xl bg-[#F7F9FC] px-3.5 py-3 dark:bg-white/[0.04]">
                  <div className="grid h-9 w-9 place-items-center rounded-xl bg-[#fff2dd] text-[#a76518] dark:bg-[#412d13] dark:text-[#eab66f]"><RefreshCw size={15} /></div>
                  <div className="min-w-0 flex-1">
                    <p className={`truncate text-[13px] font-semibold ${ink}`}>{item.serverName}</p>
                    <p className={`truncate text-[11px] ${muted}`}>{item.deviceName}</p>
                  </div>
                  <button data-testid="remote-cancel-pending-join" className={quiet} onClick={() => cancelPendingJoin(item.requestId)} disabled={isBusy(`cancel-join-${item.requestId}`)}>{t.remoteKbCancelRequest}</button>
                </div>
              ))}
            </div>
          </section>
        )}

        {connections.length > 1 && (
          <nav data-testid="remote-server-switcher" className="flex items-center gap-2 overflow-x-auto pb-1">
            <span className={`mr-1 shrink-0 text-[12px] font-semibold ${muted}`}>{t.remoteKbServers}</span>
            {connections.map(connection => {
              const selected = selectedServerId === connection.serverId;
              const dot = connection.online ? (connection.ready ? 'bg-[#18a957]' : 'bg-[#d6873e]') : 'bg-[#d63a3a]';
              return (
                <button key={connection.serverId} onClick={() => selectServer(connection.serverId)}
                  className={`flex h-10 shrink-0 items-center gap-2 rounded-xl border px-3 text-left transition-colors ${selected ? 'border-[#0B57D0]/30 bg-[#eaf2ff] text-[#0B57D0] dark:border-[#A8C7FA]/30 dark:bg-[#172b49] dark:text-[#A8C7FA]' : 'border-[#ececf1] bg-white text-[#444746] hover:bg-[#F0F4F9] dark:border-white/10 dark:bg-[#1E1F20] dark:text-[#C4C7C5] dark:hover:bg-[#2A2B2D]'}`}>
                  <span className={`h-2 w-2 rounded-full ${dot}`} />
                  <span className="max-w-[220px] truncate text-[13px] font-semibold">{connection.name}</span>
                  <span className="text-[11px] opacity-70">{connection.scope === 'owner' ? t.remoteKbOwner : connection.scope === 'manage' ? t.remoteKbManage : t.remoteKbReadOnly}</span>
                </button>
              );
            })}
          </nav>
        )}

        {connectionsLoaded && !connections.length && !showConnector && (
          <div className={`rounded-2xl border border-dashed border-[#d4d8e2] py-16 text-center dark:border-white/15 ${muted}`}>
            <div className="mx-auto grid h-14 w-14 place-items-center rounded-2xl bg-[#F0F4F9] text-[#0B57D0] dark:bg-[#2A2B2D] dark:text-[#A8C7FA]"><Server size={24} /></div>
            <p className="mt-4 text-[14px] font-semibold">{t.remoteKbNoServers}</p>
            <button className={`${soft} mt-4`} onClick={openConnector}><Plus size={14} />{t.remoteKbAddServer}</button>
          </div>
        )}

        {selectedConnection && (
          <main className="space-y-7">
            <section data-testid="remote-server-summary" className={`rounded-2xl p-4 ${panel} ${panelShadow}`}>
              <div className="flex flex-wrap items-center gap-4">
                <div className="grid h-12 w-12 shrink-0 place-items-center rounded-2xl bg-[#edf8f9] text-[#087e8b] dark:bg-white/10 dark:text-[#75d5dd]">
                  <Database size={22} />
                </div>
                <div className="min-w-[220px] flex-1">
                  <div className="flex flex-wrap items-center gap-2">
                    <h2 className={`truncate text-[16px] font-bold ${ink}`}>{selectedConnection.name}</h2>
                    <span className={`inline-flex items-center gap-1.5 rounded-full px-2.5 py-1 text-[11px] font-semibold ${selectedConnection.online && selectedConnection.ready ? 'bg-[#e2f6e9] text-[#16894a] dark:bg-[#13361f] dark:text-[#7DD3A8]' : 'bg-[#fff2dd] text-[#a76518] dark:bg-[#412d13] dark:text-[#eab66f]'}`}>
                      <span className={`h-1.5 w-1.5 rounded-full ${selectedConnection.online && selectedConnection.ready ? 'bg-[#18a957]' : 'bg-[#d6873e]'}`} />
                      {selectedConnection.online ? (selectedConnection.ready ? t.remoteKbReady : t.remoteKbNotReady) : t.remoteKbOffline}
                    </span>
                    <span className="rounded-full bg-white/80 px-2.5 py-1 text-[11px] font-semibold text-[#54545f] dark:bg-white/10 dark:text-[#C4C7C5]">
                      {isOwner ? t.remoteKbOwner : canManage ? t.remoteKbManage : t.remoteKbReadOnly}
                    </span>
                  </div>
                </div>
                <div className="flex items-center gap-5 text-right">
                  <div><div className={`text-[17px] font-bold ${ink}`}>{collections.length}</div><div className={`text-[11px] ${muted}`}>{t.remoteKbCollections}</div></div>
                  <div><div className={`text-[17px] font-bold ${ink}`}>{totalDocuments}</div><div className={`text-[11px] ${muted}`}>{t.remoteKbFiles}</div></div>
                  <div><div className={`text-[17px] font-bold ${ink}`}>{totalChunks}</div><div className={`text-[11px] ${muted}`}>{t.remoteKbChunks}</div></div>
                </div>
                <div className="flex items-center gap-1">
                  {isOwner && (
                    <button ref={ownerPanelTriggerRef} data-testid="remote-govern" className={soft} onClick={openOwnerPanel}>
                      <Users size={14} />{t.remoteKbGovern}
                      {!!pendingOwnerJoinRequests.length && (
                        <span data-testid="remote-govern-pending-count" className="grid h-5 min-w-5 place-items-center rounded-full bg-[#0B57D0] px-1 text-[10px] font-bold text-white animate-in zoom-in-75 duration-150 dark:bg-[#A8C7FA] dark:text-[#062E6F]">
                          {pendingOwnerJoinRequests.length}
                        </span>
                      )}
                    </button>
                  )}
                  {canManage && (
                    <label className={`${quiet} cursor-pointer px-2.5`} title={t.remoteKbShowTrash}>
                      <input data-testid="remote-trash-toggle" className="peer sr-only" type="checkbox" checked={includeTrash} onChange={event => changeTrashVisibility(event.target.checked)} />
                      <span className="relative h-5 w-8 rounded-full bg-[#d8dce3] transition-colors after:absolute after:left-0.5 after:top-0.5 after:h-4 after:w-4 after:rounded-full after:bg-white after:shadow-sm after:transition-transform peer-checked:bg-[#0B57D0] peer-checked:after:translate-x-3" />
                      {t.remoteKbShowTrash}
                    </label>
                  )}
                  {!isLocalHostOwner && (
                    <button data-testid="remote-disconnect" className={danger} onClick={() => removeConnection(selectedServerId)}><Trash2 size={14} />{t.remoteKbDisconnect}</button>
                  )}
                </div>
              </div>
              {selectedConnection.error && <p className="mt-3 rounded-xl bg-[#d63a3a]/8 px-3 py-2 text-[12px] text-[#d63a3a]">{selectedConnection.error}</p>}
            </section>

            <section>
              <div className="mb-3 flex flex-wrap items-end justify-between gap-3">
                <div>
                  <h2 className={`text-[15px] font-bold ${ink}`}>{t.remoteKbCollections}</h2>
                  <p className={`mt-0.5 text-[12px] ${muted}`}>{collections.length} {t.remoteKbCollections} · {totalDocuments} {t.remoteKbDocuments} · {totalChunks} {t.remoteKbChunks}</p>
                </div>
                {canManage && (
                  <div className="flex flex-wrap items-center gap-2">
                    <button data-testid="remote-publish-local" className={quiet} onClick={openPublishDialog} disabled={isBusy('local-collections')}><FolderPlus size={14} />{t.remoteKbPublishLocal}</button>
                    {!showCollectionCreator && <button className={soft} onClick={() => setShowCollectionCreator(true)}><Plus size={14} />{t.remoteKbNewCollection}</button>}
                  </div>
                )}
              </div>

              {!collections.length ? (
                <div className={`rounded-2xl border border-dashed border-[#d4d8e2] py-12 text-center text-[13px] dark:border-white/15 ${muted}`}>{t.remoteKbNoCollections}</div>
              ) : (
                <div data-testid="remote-collections-grid" className="grid gap-4 sm:grid-cols-2 xl:grid-cols-3">
                  {collections.map(collection => {
                    const color = stableColor(collection.name);
                    const selected = selectedCollectionId === collection.id;
                    return (
                      <article
                        key={collection.id}
                        data-selected={selected ? 'true' : 'false'}
                        className={`relative rounded-2xl p-4 transition-all ${panel} ${panelShadow} ${selected ? 'ring-2 ring-[#0B57D0]/25 dark:ring-[#A8C7FA]/25' : 'hover:border-[#dfe4f5] dark:hover:border-white/20'}`}
                      >
                        <button
                          className="w-full text-left"
                          aria-current={selected ? 'true' : undefined}
                          onClick={() => { if (!selected) selectCollection(collection.id); }}
                        >
                          <div className="flex items-start gap-3">
                            <div className="grid h-11 w-11 shrink-0 place-items-center rounded-xl" style={{ background: `${color}1f`, color }}><BookOpen size={20} /></div>
                            <div className="min-w-0 flex-1">
                              <h3 className={`truncate text-[14px] font-bold ${ink}`}>{collection.name}</h3>
                              <p className={`mt-0.5 text-[11.5px] ${muted}`}>{collection.docCount} {t.remoteKbDocuments} · {collection.chunkCount} {t.remoteKbChunks}</p>
                            </div>
                            {selected && <Check size={17} className="shrink-0" style={{ color }} />}
                          </div>
                          {collection.description && <p className={`mt-3 line-clamp-2 text-[12px] leading-5 ${muted}`}>{collection.description}</p>}
                        </button>
                        <div className="mt-4 flex items-center justify-between border-t border-gray-400/15 pt-3">
                          <span className={`inline-flex items-center gap-1.5 text-[11.5px] ${collection.status === 'ready' ? 'text-[#16894a] dark:text-[#7DD3A8]' : muted}`}>
                            <span className={`h-1.5 w-1.5 rounded-full ${collection.status === 'ready' ? 'bg-[#18a957]' : 'bg-[#d6873e]'}`} />
                            {collection.status === 'ready' ? t.remoteKbReady : t.remoteKbNotReady}
                          </span>
                          {canManage && (
                            <div className="flex items-center gap-1">
                              <button className={collection.deletedAt ? quiet : 'text-[11.5px] text-[#d63a3a] hover:underline disabled:opacity-50'} disabled={isBusy('collection-trash')} onClick={() => changeCollectionTrash(collection)}>
                                {collection.deletedAt ? t.remoteKbRestore : t.remoteKbTrash}
                              </button>
                              {isOwner && collection.deletedAt && <button className={danger} onClick={() => permanentlyDelete('collection', collection.id, collection.name)} disabled={isBusy(`permanent-collection-${collection.id}`)} title={t.remoteKbPermanentDelete}><Trash2 size={13} /></button>}
                            </div>
                          )}
                        </div>
                      </article>
                    );
                  })}
                </div>
              )}
            </section>

            {selectedCollectionId && (
              <section>
                <div className="mb-3 flex flex-wrap items-center justify-between gap-3">
                  <div className="min-w-0">
                    <div className="flex items-center gap-2">
                      <h2 className={`text-[15px] font-bold ${ink}`}>{t.remoteKbFiles}</h2>
                      {selectedCollection && <span className={`truncate text-[13px] ${muted}`}>· {selectedCollection.name}</span>}
                    </div>
                    <p data-testid="remote-documents-summary" className={`mt-0.5 text-[12px] ${muted}`}>{selectedCollection?.docCount || 0} {t.remoteKbDocuments} · {selectedCollection?.chunkCount || 0} {t.remoteKbChunks}</p>
                  </div>
                  {canManage && (uploadInProgress ? (
                    <button className={soft} onClick={() => setShowUploadDialog(true)}>
                      <RefreshCw size={14} className="animate-spin" />{t.remoteKbUploading}
                    </button>
                  ) : (
                    <div ref={uploadSourceMenuRef} className="relative">
                      <button
                        data-testid="remote-upload-menu-toggle"
                        className={soft}
                        aria-haspopup="menu"
                        aria-expanded={showUploadSourceMenu}
                        onClick={() => setShowUploadSourceMenu(open => !open)}
                      >
                        <Plus size={14} />{t.remoteKbAddContent}<ChevronDown size={14} />
                      </button>
                      {showUploadSourceMenu && (
                        <div
                          data-testid="remote-upload-menu"
                          role="menu"
                          className="absolute right-0 top-11 z-30 w-40 overflow-hidden rounded-xl border border-black/10 bg-white py-1 shadow-xl dark:border-white/10 dark:bg-[#202124]"
                        >
                          <button data-testid="remote-upload-files" role="menuitem" className="flex h-9 w-full items-center gap-2 px-3 text-left text-[13px] hover:bg-[#F1F3F4] dark:hover:bg-[#303134]" onClick={chooseUploadFiles}>
                            <Upload size={15} />{t.remoteKbUpload}
                          </button>
                          <button data-testid="remote-upload-folder" role="menuitem" className="flex h-9 w-full items-center gap-2 px-3 text-left text-[13px] hover:bg-[#F1F3F4] dark:hover:bg-[#303134]" onClick={chooseUploadFolders}>
                            <FolderPlus size={15} />{t.remoteKbUploadFolder}
                          </button>
                        </div>
                      )}
                    </div>
                  ))}
                </div>
                <div data-testid="remote-documents-table" className={`overflow-hidden rounded-2xl ${panel} ${panelShadow}`}>
                  <div className={`hidden items-center gap-3 border-b border-gray-400/15 bg-[#fbfbfd] px-5 py-3 text-[11.5px] font-semibold dark:bg-white/5 md:flex ${muted}`}>
                    <span className="min-w-0 flex-1">{t.remoteKbFileName}</span>
                    <span className="w-28">{t.remoteKbStatus}</span>
                    <span className="w-20 text-right">{t.remoteKbChunks}</span>
                    <span className="w-20 text-right">{t.remoteKbSize}</span>
                    <span className="w-28" />
                  </div>
                  {!documents.length && <div className={`py-12 text-center text-[13px] ${muted}`}>{documentsLoadingPage ? t.remoteKbLoadingMoreDocuments : t.remoteKbNoDocuments}</div>}
                  {documents.map(document => (
                    <div key={document.id} data-testid="remote-document-row" className="group flex flex-wrap items-center gap-3 border-b border-gray-400/10 px-4 py-3 last:border-0 hover:bg-[#F0F4F9] dark:hover:bg-[#2A2B2D] md:flex-nowrap md:px-5">
                      <div className="flex min-w-0 flex-1 items-center gap-3">
                        <div className="grid h-9 w-9 shrink-0 place-items-center rounded-xl bg-[#eef2fb] text-[#4b68bf] dark:bg-[#252b3b] dark:text-[#A8C7FA]">
                          <FileTypeIcon name={document.name} className="h-5 w-5" />
                        </div>
                        <div className="min-w-0">
                          <p className={`truncate text-[13px] font-medium ${ink}`} title={document.name}>{document.name}</p>
                          {document.error && <p className="mt-0.5 truncate text-[11px] text-[#d63a3a]" title={document.error}>{document.error}</p>}
                        </div>
                      </div>
                      <span className={`flex w-auto items-center gap-1.5 text-[11.5px] md:w-28 ${document.status === 'ready' ? 'text-[#16894a] dark:text-[#7DD3A8]' : document.status === 'failed' ? 'text-[#d63a3a]' : muted}`}>
                        <span className={`h-1.5 w-1.5 rounded-full ${document.status === 'ready' ? 'bg-[#18a957]' : document.status === 'failed' ? 'bg-[#d63a3a]' : 'bg-[#d6873e]'}`} />
                        {documentStatusLabel(document.status)}
                      </span>
                      <span className={`w-auto text-right text-[11.5px] md:w-20 ${muted}`}>{document.nChunks}</span>
                      <span className={`w-auto text-right text-[11.5px] md:w-20 ${muted}`}>{formatBytes(document.size)}</span>
                      <div className="ml-auto flex w-auto items-center justify-end gap-0.5 md:w-28">
                        {!document.deletedAt && <button className={iconButton} onClick={() => downloadDocument(document)} title={t.remoteKbDownload}><Download size={14} /></button>}
                        {canManage && !document.deletedAt && <button className={iconButton} onClick={() => replaceDocument(document)} title={t.remoteKbReplace}><RefreshCw size={14} /></button>}
                        {canManage && <button data-testid="remote-document-trash" className={`${iconButton} ${document.deletedAt ? '' : 'text-[#d63a3a]'}`} onClick={() => requestDocumentTrash(document)} disabled={isBusy(`document-trash-${document.id}`)} title={document.deletedAt ? t.remoteKbRestore : t.remoteKbTrash}>{isBusy(`document-trash-${document.id}`) ? <RefreshCw size={14} className="animate-spin" /> : document.deletedAt ? <RefreshCw size={14} /> : <Trash2 size={14} />}</button>}
                        {isOwner && document.deletedAt && <button className={`${iconButton} text-[#d63a3a]`} onClick={() => permanentlyDelete('document', document.id, document.name)} disabled={isBusy(`permanent-document-${document.id}`)} title={t.remoteKbPermanentDelete}><Trash2 size={14} /></button>}
                      </div>
                    </div>
                  ))}
                  {documentsHasMore && (
                    <div className="flex justify-center border-t border-gray-400/10 px-4 py-3">
                      <button
                        data-testid="remote-documents-load-more"
                        className={quiet}
                        onClick={() => loadDocuments(selectedServerId, selectedCollectionId, false)}
                        disabled={documentsLoadingPage}
                      >
                        {documentsLoadingPage && <RefreshCw size={14} className="animate-spin" />}
                        {documentsLoadingPage ? t.remoteKbLoadingMoreDocuments : t.remoteKbLoadMoreDocuments}
                      </button>
                    </div>
                  )}
                </div>
              </section>
            )}

            {selectedCollectionId && (
              <section data-testid="remote-search-panel" className={`rounded-2xl p-4 ${panel} ${panelShadow}`}>
                <div className="flex items-center gap-3">
                  <div className="grid h-9 w-9 shrink-0 place-items-center rounded-xl bg-[#F0F4F9] text-[#0B57D0] dark:bg-[#2A2B2D] dark:text-[#A8C7FA]"><Search size={17} /></div>
                  <div className="min-w-0 flex-1"><h2 className={`text-[14px] font-bold ${ink}`}>{t.remoteKbTestSearch}</h2></div>
                </div>
                <div className="mt-3 flex gap-2">
                  <input className={field} value={query} onChange={event => setQuery(event.target.value)} onKeyDown={event => { if (event.key === 'Enter') search(); }} placeholder={t.remoteKbSearchPlaceholder} />
                  <button className={primary} onClick={search} disabled={!query.trim() || isBusy('search')}>{isBusy('search') ? <RefreshCw size={14} className="animate-spin" /> : <Search size={14} />}{t.remoteKbSearch}</button>
                </div>
                {!!results.length && <div className="mt-4 grid gap-2">{results.map((result, index) => <article key={`${result.documentId}:${result.ord}:${index}`} className="rounded-xl border border-[#ececf1] bg-[#fbfbfd] p-3 dark:border-white/10 dark:bg-white/[0.04]"><div className="flex items-center gap-2"><BookOpen size={13} className="text-[#0B57D0] dark:text-[#A8C7FA]" /><p className={`truncate text-[12px] font-semibold ${ink}`}>{result.documentName}</p></div><p className={`mt-2 whitespace-pre-wrap text-[12px] leading-5 ${muted}`}>{result.text}</p></article>)}</div>}
              </section>
            )}
          </main>
        )}

        {hostProgress && (
          <OverlayDialog
            testId="shared-kb-host-progress"
            title={hostProgress.operation === 'upgrade'
              ? t.remoteKbHostProgressUpgradeTitle
              : (hostProgress.operation === 'reconnect' ? t.remoteKbHostProgressReconnectTitle : t.remoteKbHostProgressCreateTitle)}
            description={hostProgressLabel(hostProgress.phase)}
            icon={Server}
            onClose={() => setHostProgress(null)}
            closeLabel={t.remoteKbClose}
          >
            <div className="space-y-4">
              <div
                role="progressbar"
                aria-label={hostProgressLabel(hostProgress.phase)}
                aria-valuemin="0"
                aria-valuemax="100"
                aria-valuenow={hostProgress.percent}
                className="h-2 overflow-hidden rounded-full bg-[#e8ebf0] dark:bg-white/10"
              >
                <div
                  className={`h-full rounded-full transition-[width] duration-500 ${hostProgress.phase === 'failed' ? 'bg-[#d63a3a]' : 'bg-[#0B57D0] dark:bg-[#A8C7FA]'} ${!['complete', 'failed'].includes(hostProgress.phase) ? 'animate-pulse' : ''}`}
                  style={{ width: `${Math.max(5, Math.min(100, hostProgress.percent || 0))}%` }}
                />
              </div>
              {!['complete', 'failed'].includes(hostProgress.phase) && (
                <p className={`text-[12px] leading-5 ${muted}`}>{t.remoteKbHostProgressHint}</p>
              )}
              {hostProgress.phase === 'failed' && (
                <div data-testid="shared-kb-host-progress-error" className="flex items-start gap-2 rounded-xl bg-[#d63a3a]/8 px-3.5 py-3 text-[12px] leading-5 text-[#b72f2f]">
                  <AlertTriangle size={16} className="mt-0.5 shrink-0" />
                  <span className="min-w-0 break-words">{hostProgress.error}</span>
                </div>
              )}
              {hostProgress.phase === 'complete' && (
                <div className="flex items-center gap-2 text-[12px] font-semibold text-[#16894a] dark:text-[#7DD3A8]">
                  <CheckCircle2 size={16} />
                  {t.remoteKbHostProgressComplete}
                </div>
              )}
              {hostProgress.phase === 'failed' && (
                <div className="flex justify-end gap-2">
                  <button className={quiet} onClick={() => setHostProgress(null)}>{t.remoteKbClose}</button>
                  <button className={primary} onClick={hostProgress.operation === 'upgrade'
                    ? upgradeHost
                    : (hostProgress.operation === 'reconnect' ? reconnectHost : installHost)}>
                    <RefreshCw size={14} />
                    {t.remoteKbHostProgressRetry}
                  </button>
                </div>
              )}
            </div>
          </OverlayDialog>
        )}

        {showConnector && (
          <OverlayDialog
            testId="remote-connect-panel"
            title={t.remoteKbConnectTitle}
            icon={Server}
            onClose={closeConnector}
            closeLabel={t.remoteKbClose}
            closeDisabled={connecting || joinFeedback?.status === 'approved'}
          >
            {joinFeedback ? (
              <div
                data-testid="remote-join-feedback"
                data-status={joinFeedback.status}
                aria-live="polite"
                className="py-5 text-center animate-in fade-in zoom-in-95 duration-200 motion-reduce:animate-none"
              >
                <div className={`mx-auto grid h-14 w-14 place-items-center rounded-2xl transition-colors duration-200 ${joinFeedback.status === 'approved' ? 'bg-[#e2f6e9] text-[#16894a] dark:bg-[#13361f] dark:text-[#7DD3A8]' : 'bg-[#eaf2ff] text-[#0B57D0] dark:bg-[#172b49] dark:text-[#A8C7FA]'}`}>
                  <CheckCircle2 size={25} />
                </div>
                <h3 className={`mt-4 text-[15px] font-bold ${ink}`}>
                  {joinFeedback.status === 'approved' ? t.remoteKbConnected : t.remoteKbJoinRequested}
                </h3>
                {joinFeedback.serverName && <p className={`mt-1 text-[13px] font-semibold ${muted}`}>{joinFeedback.serverName}</p>}
                {joinFeedback.status === 'pending' && <p className={`mt-2 text-[12px] ${muted}`}>{t.remoteKbPendingDesc}</p>}
                {joinFeedback.status === 'pending' && (
                  <button
                    data-testid="remote-join-feedback-close"
                    className={`${primary} mt-5`}
                    onClick={closeConnector}
                  >
                    {t.remoteKbDone}
                  </button>
                )}
              </div>
            ) : (
              <>
                {identityProbe ? (
                  <div data-testid="remote-identity-confirmation" className="space-y-4 animate-in fade-in slide-in-from-right-1 duration-150 motion-reduce:animate-none">
                    <div className="rounded-2xl border border-[#0B57D0]/20 bg-[#0B57D0]/[0.035] p-4 dark:border-[#A8C7FA]/20 dark:bg-[#A8C7FA]/[0.05]">
                      <div className="flex items-start gap-3">
                        <div className="grid h-10 w-10 shrink-0 place-items-center rounded-xl bg-[#EAF2FF] text-[#0B57D0] dark:bg-[#172B49] dark:text-[#A8C7FA]"><Server size={18} /></div>
                        <div className="min-w-0 flex-1">
                          <h3 className={`truncate text-[14px] font-bold ${ink}`}>{identityProbe.serverName}</h3>
                          <p className={`mt-1 break-all text-[11.5px] ${muted}`}>{identityProbe.endpoint}</p>
                        </div>
                        <span className="rounded-full bg-[#EAF2FF] px-2 py-1 text-[10.5px] font-semibold text-[#0B57D0] dark:bg-[#172B49] dark:text-[#A8C7FA]">
                          {identityProbe.networkKind === 'tailscale' ? t.remoteKbTailnet : t.remoteKbLan}
                        </span>
                      </div>
                      <div className="mt-4 rounded-xl bg-white px-3.5 py-3 text-center dark:bg-[#171719]">
                        <p className={`text-[11px] font-medium ${muted}`}>{t.remoteKbIdentityCode}</p>
                        <p data-testid="remote-identity-code" className={`mt-1 select-all font-mono text-[18px] font-bold tracking-[0.08em] ${ink}`}>{identityProbe.identityCode}</p>
                      </div>
                    </div>
                    <p className={`text-[12px] leading-5 ${muted}`}>{t.remoteKbVerifyIdentityDesc}</p>
                    <input data-testid="remote-device-name" className={field} value={deviceName} onChange={event => setDeviceName(event.target.value)} onKeyDown={event => { if (event.key === 'Enter') connectServer(); }} placeholder={t.remoteKbDeviceName} aria-label={t.remoteKbDeviceName} autoComplete="name" disabled={connecting} />
                  </div>
                ) : (
                  <div className="space-y-4 animate-in fade-in duration-150 motion-reduce:animate-none">
                    <section aria-labelledby="remote-nearby-title">
                      <div className="flex items-center justify-between gap-3">
                        <h3 id="remote-nearby-title" className={`text-[13px] font-bold ${ink}`}>{t.remoteKbNearby}</h3>
                        <button type="button" className={iconButton} onClick={discoverNearby} disabled={discoveryStatus === 'discovering'} aria-label={t.remoteKbRefresh} title={t.remoteKbRefresh}><RefreshCw size={14} className={discoveryStatus === 'discovering' ? 'animate-spin' : ''} /></button>
                      </div>
                      {discoveryStatus === 'discovering' ? (
                        <div data-testid="remote-nearby-discovering" role="status" className={`mt-2 flex items-center gap-2 rounded-xl bg-[#F7F9FC] px-3.5 py-3 text-[12px] dark:bg-white/[0.04] ${muted}`}>
                          <RefreshCw size={14} className="animate-spin text-[#0B57D0] dark:text-[#A8C7FA]" />
                          {t.remoteKbDiscovering}
                        </div>
                      ) : visibleNearbyHosts.length ? (
                        <div data-testid="remote-nearby-list" className="mt-2 space-y-2">
                          {visibleNearbyHosts.map(probe => (
                            <button key={`${probe.serverId}:${probe.endpoint}`} type="button" className="flex w-full items-center gap-3 rounded-xl border border-[#e3e7ee] px-3.5 py-3 text-left transition-colors hover:border-[#0B57D0]/35 hover:bg-[#F7F9FC] dark:border-white/10 dark:hover:border-[#A8C7FA]/35 dark:hover:bg-white/[0.04]" onClick={() => chooseNearbyHost(probe)}>
                              <Server size={16} className="shrink-0 text-[#0B57D0] dark:text-[#A8C7FA]" />
                              <span className="min-w-0 flex-1"><span className={`block truncate text-[13px] font-semibold ${ink}`}>{probe.serverName}</span><span className={`mt-0.5 block truncate text-[11px] ${muted}`}>{probe.endpoint}</span></span>
                              <span className={`text-[11px] ${muted}`}>{t.remoteKbVerify}</span>
                            </button>
                          ))}
                        </div>
                      ) : discoveryStatus !== 'idle' && (
                        <p data-testid="remote-nearby-empty" className={`mt-2 rounded-xl bg-[#F7F9FC] px-3.5 py-3 text-[12px] dark:bg-white/[0.04] ${muted}`}>{t.remoteKbNearbyEmpty}</p>
                      )}
                    </section>
                    <div className="flex items-center gap-3" aria-hidden="true"><span className="h-px flex-1 bg-[#e3e7ee] dark:bg-white/10" /><span className={`text-[11px] ${muted}`}>{t.remoteKbManualConnect}</span><span className="h-px flex-1 bg-[#e3e7ee] dark:bg-white/10" /></div>
                    <input
                      autoFocus
                      data-testid="remote-invitation"
                      className={field}
                      value={invitation}
                      onChange={event => {
                        setInvitation(event.target.value);
                        setIdentityProbe(null);
                        setConnectorError('');
                      }}
                      onKeyDown={event => { if (event.key === 'Enter') connectServer(); }}
                      placeholder={t.remoteKbJoinSourcePlaceholder}
                      aria-label={t.remoteKbJoinSource}
                      aria-describedby="remote-join-source-help"
                      spellCheck={false}
                      disabled={connecting}
                    />
                    <input data-testid="remote-device-name" className={field} value={deviceName} onChange={event => setDeviceName(event.target.value)} onKeyDown={event => { if (event.key === 'Enter') connectServer(); }} placeholder={t.remoteKbDeviceName} aria-label={t.remoteKbDeviceName} autoComplete="name" disabled={connecting} />
                    <p id="remote-join-source-help" data-testid="remote-join-source-help" className={`text-[12px] leading-5 ${muted}`}>{t.remoteKbJoinHint}</p>
                  </div>
                )}
                {connectorError && <p role="alert" className="mt-3 rounded-xl bg-[#d63a3a]/8 px-3.5 py-3 text-[12px] leading-5 text-[#b72f2f]">{connectorError}</p>}
                <div className="mt-5 flex justify-end gap-2">
                  <button className={quiet} onClick={() => {
                    if (identityProbe) setIdentityProbe(null);
                    else closeConnector();
                  }} disabled={connecting}>{identityProbe ? t.remoteKbBack : t.remoteKbCancel}</button>
                  <button data-testid="remote-connect-submit" className={primary} disabled={connecting || !connectionDetailsReady} onClick={connectServer}>
                    {connecting && <RefreshCw size={14} className="animate-spin" />}{identityProbe ? t.remoteKbConfirmIdentity : (invitationIsShareLink ? t.remoteKbConnect : t.remoteKbVerify)}
                  </button>
                </div>
              </>
            )}
          </OverlayDialog>
        )}

        {showOwnerPanel && isOwner && !showRecoveryCode && !showRestoreDialog && !confirmation && (
          <OverlayDialog
            testId="remote-owner-panel"
            title={t.remoteKbGovernTitle}
            description={selectedConnection?.name}
            icon={Users}
            onClose={closeOwnerPanel}
            closeLabel={t.remoteKbClose}
            widthClassName="max-w-[720px]"
            scrollBody
          >
            <div className="space-y-4">
              <div role="tablist" aria-label={t.remoteKbGovernTitle} className="grid grid-cols-2 gap-1 rounded-xl bg-[#F0F4F9] p-1 dark:bg-[#171719]">
                <button
                  ref={ownerPeopleTabRef}
                  id="remote-owner-people-tab"
                  type="button"
                  role="tab"
                  aria-selected={ownerPanelTab === 'people'}
                  aria-controls="remote-owner-people-panel"
                  tabIndex={ownerPanelTab === 'people' ? 0 : -1}
                  data-testid="remote-owner-people-tab"
                  className={`${ownerTab} ${ownerPanelTab === 'people' ? 'bg-white text-[#0B57D0] shadow-sm dark:bg-[#2A2B2D] dark:text-[#A8C7FA]' : muted}`}
                  onClick={() => setOwnerPanelTab('people')}
                  onKeyDown={moveOwnerPanelTab}
                >
                  <Users size={15} />{t.remoteKbPeopleTab}
                </button>
                <button
                  ref={ownerHostTabRef}
                  id="remote-owner-host-tab"
                  type="button"
                  role="tab"
                  aria-selected={ownerPanelTab === 'host'}
                  aria-controls="remote-owner-host-panel"
                  tabIndex={ownerPanelTab === 'host' ? 0 : -1}
                  data-testid="remote-owner-host-tab"
                  className={`${ownerTab} ${ownerPanelTab === 'host' ? 'bg-white text-[#0B57D0] shadow-sm dark:bg-[#2A2B2D] dark:text-[#A8C7FA]' : muted}`}
                  onClick={() => setOwnerPanelTab('host')}
                  onKeyDown={moveOwnerPanelTab}
                >
                  <Server size={15} />{t.remoteKbServiceTab}
                </button>
              </div>

              {notice && (
                <div role={notice.type === 'error' ? 'alert' : 'status'} className={`flex items-start gap-2 rounded-xl border px-3.5 py-3 text-[13px] ${notice.type === 'error' ? 'border-[#d63a3a]/20 bg-[#d63a3a]/8 text-[#b72f2f]' : 'border-[#18a957]/20 bg-[#18a957]/8 text-[#16894a]'}`}>
                  {notice.type === 'error' ? <AlertTriangle size={16} /> : <CheckCircle2 size={16} />}
                  <span className="min-w-0 flex-1 break-words">{notice.text}</span>
                  <button type="button" className={iconButton} onClick={() => setNotice(null)} aria-label={t.remoteKbClose}><X size={15} /></button>
                </div>
              )}

              {ownerPanelTab === 'people' ? (
                <div id="remote-owner-people-panel" role="tabpanel" aria-labelledby="remote-owner-people-tab" className="space-y-4">
                  <section className={ownerSection}>
                    <div className="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
                      <div className="min-w-0">
                        <h3 className={`text-[15px] font-bold ${ink}`}>{t.remoteKbShareTitle}</h3>
                        <p className={`mt-1 text-[13px] leading-5 ${muted}`}>{t.remoteKbShareDesc}</p>
                      </div>
                      <button data-testid="remote-create-share" className={`${soft} w-full sm:w-auto`} onClick={createShare} disabled={isBusy('create-share')}><Link size={14} />{t.remoteKbCreateShare}</button>
                    </div>
                    <label className={`mt-4 flex cursor-pointer items-start gap-2.5 text-[13px] leading-5 ${muted}`}>
                      <input className="mt-0.5 h-4 w-4 shrink-0" type="checkbox" checked={autoApproveRead} onChange={event => setAutoApproveRead(event.target.checked)} />
                      {t.remoteKbAutoApproveRead}
                    </label>
                    <details className="mt-3">
                      <summary className={`cursor-pointer text-[13px] font-semibold ${muted}`}>{t.remoteKbOtherNetwork}</summary>
                      <input data-testid="remote-share-other-endpoint" className={`${field} mt-2`} value={shareEndpoint} onChange={event => setShareEndpoint(event.target.value)} placeholder={t.remoteKbOtherNetworkPlaceholder} spellCheck={false} />
                    </details>
                    {shareLink && (
                      <div className="mt-3 flex items-center gap-2 rounded-xl bg-[#F0F4F9] p-2 dark:bg-[#2A2B2D]">
                        <input readOnly className={`${field} border-0 bg-transparent`} value={shareLink} />
                        <button data-testid="remote-copy-share" type="button" className={iconButton} title={t.remoteKbCopy} aria-label={t.remoteKbCopy} onClick={() => copyWithFeedback(shareLink, t.remoteKbLinkCopied)}><Copy size={15} /></button>
                      </div>
                    )}
                    {!!ownerShares.filter(item => !item.stoppedAt && item.expiresAt > Date.now() / 1000).length && (
                      <div className="mt-3 space-y-2">
                        {ownerShares.filter(item => !item.stoppedAt && item.expiresAt > Date.now() / 1000).map(item => (
                          <div key={item.id} className="flex flex-wrap items-center gap-2 rounded-xl bg-[#F7F9FC] px-3 py-2.5 dark:bg-white/[0.04]">
                            <Link size={14} className="text-[#0B57D0] dark:text-[#A8C7FA]" />
                            <span className={`min-w-[180px] flex-1 truncate text-[12.5px] ${muted}`}>{new Date(item.expiresAt * 1000).toLocaleString()}</span>
                            {item.autoApproveRead && <span className="text-[12px] text-[#16894a] dark:text-[#7DD3A8]">{t.remoteKbAutoReadShort}</span>}
                            <button className={quiet} onClick={() => stopShare(item.id)} disabled={isBusy(`stop-share-${item.id}`)}>{t.remoteKbStopShare}</button>
                          </div>
                        ))}
                      </div>
                    )}
                  </section>

                  <section className={ownerSection}>
                    <div className="flex items-center justify-between gap-3">
                      <div className="flex items-center gap-2">
                        <h3 className={`text-[15px] font-bold ${ink}`}>{t.remoteKbRequestsTitle}</h3>
                        {!!pendingOwnerJoinRequests.length && <span className="rounded-full bg-[#EAF2FF] px-2 py-0.5 text-[11px] font-bold text-[#0B57D0] dark:bg-[#172B49] dark:text-[#A8C7FA]">{pendingOwnerJoinRequests.length}</span>}
                      </div>
                      <button type="button" className={iconButton} onClick={openOwnerPanel} aria-label={t.remoteKbRefresh} title={t.remoteKbRefresh}><RefreshCw size={14} /></button>
                    </div>
                    <div className="mt-3 space-y-2">
                      {!pendingOwnerJoinRequests.length && <p className={`py-5 text-center text-[13px] ${muted}`}>{t.remoteKbNoRequests}</p>}
                      {pendingOwnerJoinRequests.map(item => {
                        const requestBusy = isBusy(`owner-request-${item.id}`);
                        return (
                          <div key={item.id} data-testid="remote-owner-join-request" className="flex flex-col gap-2 rounded-xl bg-[#F7F9FC] px-3 py-3 animate-in fade-in slide-in-from-top-1 duration-150 dark:bg-white/[0.04] sm:flex-row sm:items-center">
                            <div className="min-w-0 flex-1"><p className={`truncate text-[13.5px] font-semibold ${ink}`}>{item.deviceName}</p></div>
                            <div className="flex flex-wrap items-center gap-1.5">
                              <button className={quiet} disabled={requestBusy} onClick={() => resolveJoinRequest(item.id, 'read')}>{t.remoteKbApproveRead}</button>
                              <button className={quiet} disabled={requestBusy} onClick={() => resolveJoinRequest(item.id, 'manage')}>{t.remoteKbApproveManage}</button>
                              <button className={danger} disabled={requestBusy} onClick={() => resolveJoinRequest(item.id, null)}>{t.remoteKbReject}</button>
                            </div>
                          </div>
                        );
                      })}
                    </div>
                  </section>

                  <section className={ownerSection}>
                    <div className="flex items-center justify-between gap-3">
                      <h3 className={`text-[15px] font-bold ${ink}`}>{t.remoteKbMembersTitle}</h3>
                      <span className={`rounded-full bg-[#F0F4F9] px-2 py-0.5 text-[12px] font-semibold dark:bg-[#2A2B2D] ${muted}`}>{ownerDevices.length}</span>
                    </div>
                    <div className="mt-3 space-y-2">
                      {ownerDevices.map(device => {
                        const current = device.id === selectedConnection?.deviceId;
                        const busy = isBusy(`member-${device.id}`);
                        return (
                          <div key={device.id} className="flex flex-col gap-2 rounded-xl bg-[#F7F9FC] px-3 py-3 dark:bg-white/[0.04] sm:flex-row sm:items-center">
                            <div className="min-w-[150px] flex-1">
                              <p className={`truncate text-[13.5px] font-semibold ${ink}`}>{device.name}{current ? ` · ${t.remoteKbThisDevice}` : ''}</p>
                              <p className={`mt-1 text-[12px] ${device.revoked ? 'text-[#d63a3a]' : muted}`}>{device.revoked ? t.remoteKbRevoked : device.scope === 'owner' ? t.remoteKbOwner : device.scope === 'manage' ? t.remoteKbManage : t.remoteKbReadOnly}</p>
                            </div>
                            <div className="flex flex-wrap items-center gap-1.5">
                              {!device.scope?.includes('owner') && !device.revoked && (
                                <select aria-label={`${device.name} · ${t.remoteKbMemberAccess}`} className="h-9 rounded-lg border border-[#dfe3ea] bg-white px-2.5 text-[12.5px] dark:border-white/10 dark:bg-[#171719]" value={device.scope} disabled={busy} onChange={event => updateMember(device, { scope: event.target.value })}>
                                  <option value="read">{t.remoteKbReadOnly}</option>
                                  <option value="manage">{t.remoteKbManage}</option>
                                </select>
                              )}
                              {!device.scope?.includes('owner') && <button className={quiet} disabled={busy} onClick={() => updateMember(device, { revoked: !device.revoked })}>{device.revoked ? t.remoteKbRestoreAccess : t.remoteKbRevokeAccess}</button>}
                              {isLocalHostOwner && !current && !device.revoked && <button className={quiet} disabled={busy} onClick={() => changeOwner(device, device.scope !== 'owner')}>{device.scope === 'owner' ? t.remoteKbDemoteOwner : t.remoteKbPromoteOwner}</button>}
                              {!current && device.scope !== 'owner' && <button type="button" className={danger} disabled={busy} onClick={() => removeMember(device)} aria-label={t.remoteKbRemoveMember.replace('{name}', device.name)} title={t.remoteKbRemoveMember.replace('{name}', device.name)}><Trash2 size={13} /></button>}
                            </div>
                          </div>
                        );
                      })}
                    </div>
                  </section>
                </div>
              ) : (
                <div id="remote-owner-host-panel" role="tabpanel" aria-labelledby="remote-owner-host-tab" className="space-y-4">
                  {ownerIdentity && (
                    <section data-testid="remote-owner-identity" className={ownerSection}>
                      <div className="flex items-start gap-3">
                        <div className="grid h-10 w-10 shrink-0 place-items-center rounded-xl bg-[#EAF2FF] text-[#0B57D0] dark:bg-[#172B49] dark:text-[#A8C7FA]"><CheckCircle2 size={18} /></div>
                        <div className="min-w-0 flex-1">
                          <h3 className={`text-[15px] font-bold ${ink}`}>{t.remoteKbHostIdentity}</h3>
                          <p className={`mt-1 text-[12px] leading-5 ${muted}`}>{t.remoteKbHostIdentityDesc}</p>
                          <p className={`mt-3 select-all font-mono text-[18px] font-bold tracking-[0.08em] ${ink}`}>{ownerIdentity.identityCode}</p>
                        </div>
                      </div>
                    </section>
                  )}
                  <section className={ownerSection}>
                    <div className="flex flex-col gap-3 sm:flex-row sm:items-center">
                      <div className="grid h-10 w-10 shrink-0 place-items-center rounded-xl bg-[#EAF2FF] text-[#0B57D0] dark:bg-[#172B49] dark:text-[#A8C7FA]"><Database size={18} /></div>
                      <div className="min-w-0 flex-1">
                        <h3 className={`text-[15px] font-bold ${ink}`}>{t.remoteKbModelTitle}</h3>
                        <p className={`mt-1 text-[13px] leading-5 ${ownerModelStatus?.error ? 'text-[#d63a3a]' : muted}`}>{ownerModelStatus?.error || (ownerModelStatus?.ready ? t.remoteKbModelReady : ownerModelStatus?.downloading ? t.remoteKbModelDownloading : t.remoteKbModelMissing)}</p>
                      </div>
                      {isLocalHostOwner && !ownerModelStatus?.ready && <button className={`${soft} w-full sm:w-auto`} onClick={downloadOwnerModel} disabled={ownerModelStatus?.downloading || isBusy('owner-model-download')}>{ownerModelStatus?.downloading ? <RefreshCw size={14} className="animate-spin" /> : <Download size={14} />}{ownerModelStatus?.downloading ? t.remoteKbModelDownloadingAction : t.remoteKbDownloadModel}</button>}
                    </div>
                  </section>

                  {isLocalHostOwner && (
                    <>
                      <section className={ownerSection}>
                        <h3 className={`text-[15px] font-bold ${ink}`}>{t.remoteKbHostSettings}</h3>
                        <p className={`mt-1 text-[13px] leading-5 ${muted}`}>{t.remoteKbHostSettingsDesc}</p>
                        <div className="mt-4 grid gap-2 sm:grid-cols-2">
                          <button data-testid="shared-kb-backup" className={`${soft} w-full`} onClick={backupHost} disabled={isBusy('backup-host')}><Download size={14} />{t.remoteKbBackup}</button>
                          <button data-testid="shared-kb-restore" className={`${soft} w-full`} onClick={openRestoreDialog} disabled={isBusy('restore-host')}><Upload size={14} />{t.remoteKbRestoreBackup}</button>
                        </div>
                      </section>
                      <section className="rounded-2xl border border-[#d63a3a]/20 bg-[#d63a3a]/[0.035] p-4 dark:bg-[#d63a3a]/[0.06] sm:p-5">
                        <h3 className="text-[15px] font-bold text-[#b72f2f] dark:text-[#ff8a80]">{t.remoteKbDangerZone}</h3>
                        <p className={`mt-1 text-[13px] leading-5 ${muted}`}>{t.remoteKbDangerZoneDesc}</p>
                        <div className="mt-4 flex flex-col gap-2 sm:flex-row sm:justify-end">
                          <button data-testid="shared-kb-remove-host" className={`${quiet} w-full sm:w-auto`} onClick={() => removeHost(false)} disabled={isBusy('remove-host')}>{t.remoteKbRemoveHost}</button>
                          <button data-testid="shared-kb-delete-host" className={`${danger} w-full bg-white sm:w-auto dark:bg-[#1E1F20]`} onClick={() => removeHost(true)} disabled={isBusy('delete-host')}><Trash2 size={14} />{t.remoteKbDeleteHost}</button>
                        </div>
                      </section>
                    </>
                  )}
                </div>
              )}
            </div>
          </OverlayDialog>
        )}

        {showRecoveryCode && (
          <OverlayDialog
            testId="shared-kb-recovery-code"
            title={t.remoteKbRecoveryTitle}
            description={t.remoteKbRecoveryDesc}
            icon={Database}
            onClose={() => setShowRecoveryCode(false)}
            closeLabel={t.remoteKbClose}
          >
            <textarea className={`${field} h-24 resize-none py-3 font-mono text-[11px]`} readOnly value={backupRecoveryCode} />
            {recoveryCopyFeedback && (
              <p role={recoveryCopyFeedback.type === 'error' ? 'alert' : 'status'} className={`mt-3 text-[12px] ${recoveryCopyFeedback.type === 'error' ? 'text-[#b72f2f]' : 'text-[#16894a]'}`}>
                {recoveryCopyFeedback.text}
              </p>
            )}
            <div className="mt-5 flex justify-end gap-2">
              <button data-testid="shared-kb-copy-recovery" className={quiet} onClick={() => copyWithFeedback(backupRecoveryCode, t.remoteKbRecoveryCopied, true)}><Copy size={13} />{t.remoteKbCopyRecovery}</button>
              <button data-testid="shared-kb-recovery-done" className={primary} onClick={() => setShowRecoveryCode(false)}>{t.remoteKbDone}</button>
            </div>
          </OverlayDialog>
        )}

        {showRestoreDialog && !confirmation && (
          <OverlayDialog
            testId="shared-kb-restore-dialog"
            title={t.remoteKbRestoreTitle}
            description={t.remoteKbRestoreDesc}
            icon={Upload}
            onClose={() => setShowRestoreDialog(false)}
            closeLabel={t.remoteKbClose}
            closeDisabled={isBusy('restore-host')}
          >
            <p className={`truncate rounded-xl bg-[#F7F9FC] px-3.5 py-3 text-[12px] dark:bg-white/[0.04] ${muted}`} title={restoreSource}>{restoreSource}</p>
            <textarea autoFocus className={`${field} mt-3 h-24 resize-none py-3 font-mono text-[11px]`} value={restoreCode} onChange={event => setRestoreCode(event.target.value)} placeholder={t.remoteKbRecoveryPlaceholder} />
            <p className={`mt-2 text-[11px] leading-5 ${muted}`}>{restoreCode.trim() ? t.remoteKbMigrationMode : t.remoteKbSameHostMode}</p>
            <div className="mt-5 flex justify-end gap-2">
              <button className={quiet} onClick={() => setShowRestoreDialog(false)} disabled={isBusy('restore-host')}>{t.remoteKbCancel}</button>
              <button data-testid="shared-kb-restore-submit" className={primary} onClick={restoreHost} disabled={isBusy('restore-host')}>
                {isBusy('restore-host') && <RefreshCw size={14} className="animate-spin" />}{t.remoteKbRestoreAction}
              </button>
            </div>
          </OverlayDialog>
        )}

        {showCollectionCreator && canManage && (
          <OverlayDialog
            testId="remote-create-collection-dialog"
            title={t.remoteKbNewCollection}
            icon={BookOpen}
            onClose={() => setShowCollectionCreator(false)}
            closeLabel={t.remoteKbClose}
            closeDisabled={isBusy('create-collection')}
          >
            <input autoFocus className={field} value={newCollectionName}
              onChange={event => setNewCollectionName(event.target.value)}
              onKeyDown={event => { if (event.key === 'Enter') createCollection(); }}
              placeholder={t.remoteKbCollectionName} />
            <div className="mt-5 flex justify-end gap-2">
              <button className={quiet} onClick={() => setShowCollectionCreator(false)} disabled={isBusy('create-collection')}>{t.remoteKbCancel}</button>
              <button className={primary} onClick={createCollection} disabled={!newCollectionName.trim() || isBusy('create-collection')}>
                {isBusy('create-collection') && <RefreshCw size={14} className="animate-spin" />}{t.remoteKbCreate}
              </button>
            </div>
          </OverlayDialog>
        )}

        {showPublishDialog && canManage && (
          <OverlayDialog
            testId="remote-publish-dialog"
            title={t.remoteKbPublishLocal}
            description={t.remoteKbPublishDesc}
            icon={FolderPlus}
            onClose={() => setShowPublishDialog(false)}
            closeLabel={t.remoteKbClose}
            closeDisabled={isBusy('prepare-publish')}
          >
            {localCollections.length ? (
              <select
                autoFocus
                className={field}
                value={publishCollectionId}
                onChange={event => setPublishCollectionId(event.target.value)}
                aria-label={t.remoteKbPublishChoose}
              >
                {localCollections.map(collection => (
                  <option key={collection.id} value={collection.id}>
                    {collection.name} · {collection.docCount} {t.remoteKbDocuments}
                  </option>
                ))}
              </select>
            ) : (
              <p className={`rounded-xl bg-[#F7F9FC] px-3.5 py-4 text-[12px] ${muted}`}>{t.remoteKbNoLocalCollections}</p>
            )}
            <div className="mt-5 flex justify-end gap-2">
              <button className={quiet} onClick={() => setShowPublishDialog(false)} disabled={isBusy('prepare-publish')}>{t.remoteKbCancel}</button>
              <button data-testid="remote-publish-continue" className={primary} onClick={preparePublish} disabled={!publishCollectionId || isBusy('prepare-publish')}>
                {isBusy('prepare-publish') && <RefreshCw size={14} className="animate-spin" />}{t.remoteKbPublishContinue}
              </button>
            </div>
          </OverlayDialog>
        )}

        {showUploadDialog && (
          <OverlayDialog
            testId="remote-upload-dialog"
            title={t.remoteKbUploadTitle}
            icon={Upload}
            onClose={() => {
              if (!uploadHasStarted) setPublishDraft(null);
              setShowUploadDialog(false);
            }}
            closeLabel={t.remoteKbClose}
            closeDisabled={isBusy('upload')}
          >
            {uploadDiscovery && (
              <p data-testid="remote-folder-discovery-summary" className={`mb-3 text-[12px] ${muted}`}>
                {t.remoteKbFolderSummary.replace('{count}', String(uploadDiscovery.count))}
                {uploadDiscovery.skipped ? ` · ${t.remoteKbFolderSkipped.replace('{count}', String(uploadDiscovery.skipped))}` : ''}
              </p>
            )}
            <div className="max-h-[360px] overflow-y-auto rounded-2xl border border-[#ececf1] dark:border-white/10">
              {uploadQueue.map((item, index) => (
                <div key={`${item.path}:${index}`} data-testid="remote-upload-row" data-status={item.status} className="flex items-center gap-3 border-b border-gray-400/10 px-3.5 py-3 last:border-0">
                  <div className="grid h-9 w-9 shrink-0 place-items-center rounded-xl bg-[#eef2fb] text-[#4b68bf] dark:bg-[#252b3b] dark:text-[#A8C7FA]">
                    <FileTypeIcon name={item.name} className="h-5 w-5" />
                  </div>
                  <div className="min-w-0 flex-1">
                    <p className={`truncate text-[12.5px] font-semibold ${ink}`} title={item.path}>{item.name}</p>
                    {item.error && <p className="mt-0.5 truncate text-[11px] text-[#d63a3a]" title={item.error}>{item.error}</p>}
                  </div>
                  {item.status !== 'queued' && (
                      <span className={`inline-flex shrink-0 items-center gap-1.5 text-[11.5px] ${item.status === 'success' || item.status === 'duplicate' ? 'text-[#16894a] dark:text-[#7DD3A8]' : item.status === 'failed' || item.status === 'index_failed' || item.status === 'duplicate_failed' ? 'text-[#d63a3a]' : item.status === 'pending_index' || item.status === 'duplicate_pending' ? 'text-[#a76518] dark:text-[#eab66f]' : muted}`}>
                      {item.status === 'uploading' && <RefreshCw size={13} className="animate-spin" />}
                      {(item.status === 'success' || item.status === 'duplicate') && <CheckCircle2 size={13} />}
                      {(item.status === 'failed' || item.status === 'index_failed' || item.status === 'duplicate_failed') && <AlertTriangle size={13} />}
                      {(item.status === 'pending_index' || item.status === 'duplicate_pending') && !item.pollTimedOut && <RefreshCw size={13} className="animate-spin" />}
                      {(item.status === 'pending_index' || item.status === 'duplicate_pending') && item.pollTimedOut
                        ? (item.status === 'duplicate_pending' ? t.remoteKbUploadExistingStillIndexing : t.remoteKbUploadStillIndexing)
                        : { uploading: t.remoteKbUploadingFile, success: t.remoteKbUploadDone, duplicate: t.remoteKbUploadExisting, pending_index: t.remoteKbUploadPendingIndex, duplicate_pending: t.remoteKbUploadExistingIndexing, index_failed: t.remoteKbUploadIndexFailed, duplicate_failed: t.remoteKbUploadExistingFailed, failed: t.remoteKbUploadFailed }[item.status]}
                    </span>
                  )}
                </div>
              ))}
            </div>
            {uploadHasStarted && (
              <div className="mt-4 h-1.5 overflow-hidden rounded-full bg-[#e8ebf0] dark:bg-white/10">
                <div className="h-full rounded-full bg-[#0B57D0] transition-[width] duration-300 dark:bg-[#A8C7FA]" style={{ width: `${uploadQueue.length ? (uploadQueue.filter(item => ['success', 'duplicate', 'index_failed', 'duplicate_failed', 'failed'].includes(item.status) || item.pollTimedOut).length / uploadQueue.length) * 100 : 0}%` }} />
              </div>
            )}
            <div className="mt-5 flex justify-end gap-2">
              <button className={quiet} onClick={() => {
                if (!uploadHasStarted) setPublishDraft(null);
                setShowUploadDialog(false);
              }} disabled={isBusy('upload')}>{uploadCloseLabel}</button>
              {uploadQueue.some(item => item.status === 'queued' || item.status === 'failed') && (
                <button className={primary} onClick={startUpload} disabled={uploadInProgress}>
                  {isBusy('upload') && <RefreshCw size={14} className="animate-spin" />}
                  {uploadQueue.some(item => item.status === 'failed') ? t.remoteKbRetryFailed : t.remoteKbStartUpload}
                </button>
              )}
            </div>
          </OverlayDialog>
        )}

        {confirmation && (
          <OverlayDialog
            testId={confirmation.testId || 'remote-action-confirm'}
            title={confirmation.title}
            description={confirmation.description}
            icon={AlertTriangle}
            onClose={() => finishConfirmation(false)}
            closeLabel={t.remoteKbClose}
          >
            <div className="flex flex-col-reverse gap-2 sm:flex-row sm:justify-end">
              <button
                autoFocus
                type="button"
                data-testid={`${confirmation.testId || 'remote-action-confirm'}-cancel`}
                className={`${quiet} w-full sm:w-auto`}
                onClick={() => finishConfirmation(false)}
              >
                {t.remoteKbCancel}
              </button>
              <button
                type="button"
                data-testid={`${confirmation.testId || 'remote-action-confirm'}-submit`}
                className={`${confirmation.dangerous ? `${danger} border border-[#d63a3a]/25 bg-[#d63a3a]/[0.06]` : primary} w-full sm:w-auto`}
                onClick={() => finishConfirmation(true)}
              >
                {confirmation.confirmLabel}
              </button>
            </div>
          </OverlayDialog>
        )}

        {documentToTrash && !confirmation && (
          <OverlayDialog
            testId="remote-document-trash-confirm"
            title={t.remoteKbDocumentTrashConfirm.replace('{name}', documentToTrash.name)}
            description={t.remoteKbDocumentTrashHint}
            icon={Trash2}
            onClose={() => setDocumentToTrash(null)}
            closeLabel={t.remoteKbClose}
          >
            <div className="flex justify-end gap-2">
              <button className={quiet} onClick={() => setDocumentToTrash(null)}>{t.remoteKbCancel}</button>
              <button className={danger} onClick={() => changeDocumentTrash(documentToTrash)}>{t.remoteKbTrash}</button>
            </div>
          </OverlayDialog>
        )}
      </div>
    </div>
  );
}

export { RemoteKnowledgeView };
