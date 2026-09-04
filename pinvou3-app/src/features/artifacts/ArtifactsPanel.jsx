import { useEffect, useRef, useState } from 'react';
import { FileTypeIcon } from '../../components/files/FileTypeIcon.jsx';
import { Edit2, ExternalLink, FolderOpen, Maximize2, Minimize2, XCircle } from '../../components/icons.jsx';
import { bridge } from '../../hooks/useBridge.js';
import { can, isWeb } from '../../shared/platform.js';
import { ScaledHtmlPreview } from '../settings/composer-shared.jsx';
import { cardBtnCls } from '../tools/tool-renderers.jsx';
import { DESIGN_MESSAGE_TYPES, buildDesignRuntimeScript } from './design-runtime.js';
import { DesignInspectorPanel } from './DesignInspectorPanel.jsx';
import { EditableMarkdownPreview } from './EditableMarkdownPreview.jsx';

const ArtifactTileIcon = ({ name, tileCls = 'w-9 h-9 rounded-[10px]', glyphCls = 'w-5 h-5' }) => {
      return (
        <span className={`shrink-0 inline-flex items-center justify-center bg-black/[0.04] dark:bg-white/[0.08] ${tileCls}`}>
          <FileTypeIcon name={name} className={glyphCls} />
        </span>
      );
    };
    const apPad2 = (x) => String(x).padStart(2, '0');
    const apFormatBytes = (n) => {
      if (n == null) return '';
      if (n < 1024) return n + ' B';
      if (n < 1024 * 1024) return (n / 1024).toFixed(1) + ' KB';
      return (n / (1024 * 1024)).toFixed(1) + ' MB';
    };
    const apFormatMtime = (sec) => {
      if (!sec) return '';
      const d = new Date(sec * 1000);
      return d.getFullYear() + '-' + apPad2(d.getMonth() + 1) + '-' + apPad2(d.getDate()) +
        ' ' + apPad2(d.getHours()) + ':' + apPad2(d.getMinutes());
    };
    // kind(后端分类) → 人话类型名;不在表里则回退扩展名大写。
    const apKindLabel = (t, kind, name) => {
      const m = t.apKinds && t.apKinds[kind];
      if (m) return m;
      const ext = ((name || '').split('.').pop() || '').toUpperCase();
      return ext || (kind || 'FILE');
    };
    const normalizeArtifactPath = (path) => String(path || '').replaceAll('\\', '/').toLowerCase();
    const sameArtifactPath = (left, right) => {
      const a = normalizeArtifactPath(left);
      const b = normalizeArtifactPath(right);
      return !!a && !!b && a === b;
    };
    const changeMatchesSession = (change, bs) => {
      if (!change || !change.sessionId || !bs?.activeSessionId) return true;
      return change.sessionId === bs.activeSessionId;
    };
    const HTML_ZOOM_OPTIONS = [
      { key: 'fit', labelKey: 'zoomFit' },
      { key: 'actual', labelKey: 'zoomActual' },
    ];
    const clampHtmlScale = (value) => Math.max(0.1, Math.min(3, Number(value) || 1));
    // 注入到 office→HTML 预览 iframe 末尾:LibreOffice 导出的表格 border=0、字号 x-small,
    // 这里补网格线/字号/单元格换行,让 xlsx 读起来像表格。放在文档后 → 同特异性下后定义胜出。
    const OFFICE_HTML_STYLE = '<style>'
      + 'body{margin:14px;background:#fff;color:#1f1f1f;font-family:system-ui,-apple-system,"Segoe UI",sans-serif;}'
      + 'table{border-collapse:collapse;width:auto;max-width:100%;}'
      + 'td,th{border:1px solid #d4d7dc;padding:5px 9px;font-size:13px!important;vertical-align:top;max-width:460px;overflow-wrap:anywhere;}'
      + 'tr:first-child td{background:#eef2f8;font-weight:600;}'
      + 'img{max-width:100%;height:auto;}'
      + '</style>';

    // Stable empty-array default: an inline [] is a new reference on every render, which makes memoized children re-render repeatedly.
    const EMPTY_DESIGN_CHANGES = [];

    // eslint-disable-next-line sonarjs/cognitive-complexity -- unified preview/design workbench panel: every state-machine branch maps to a preview kind or a design runtime event; splitting would sever the pv/sel linkage
    const ArtifactsPanel = ({ bs, t, onClose, isWide, onGotoSettings, isFullscreen = false, onToggleFullscreen, preferredArtifactPath, onPreviewArtifact, designCommand, selectedDesignElement, designChanges = EMPTY_DESIGN_CHANGES, onDesignElementSelected, onDesignChangeApplied, onDesignMutation, onDesignApplyChange, onDesignClearChanges, onDesignAiSubmit, designAiState, onDesignAiStateChange }) => {
      const uiA = t.uiArtifacts;
      // Visual editing (design workbench) is now a manual in-panel toggle:
      // no longer triggered by the session lane — any HTML artifact can
      // enter "edit mode" once fullscreen.
      const [designEditMode, setDesignEditMode] = useState(false);
      const canOpenContainingFolder = can('externalSystemOpen');
      const canDownloadArtifacts = can('artifactDownload');
      const artifacts = (bs && bs.artifacts) || [];
      const activeSessionId = bs && bs.activeSessionId;
      const initialPreviewArtifact = preferredArtifactPath
        ? artifacts.find((a) => sameArtifactPath(a.path, preferredArtifactPath))
        : artifacts[artifacts.length - 1] || null;
      const initialSelectedArtifact = initialPreviewArtifact || artifacts[artifacts.length - 1] || null;
      const [tab, setTab] = useState(initialSelectedArtifact ? 'preview' : 'list');     // 'list' | 'preview'
      const [sel, setSel] = useState(initialSelectedArtifact ? { ...initialSelectedArtifact, sessionId: initialSelectedArtifact.sessionId || activeSessionId } : null);        // 选中的 artifact { path, basename }
      const [pv, setPv] = useState(initialSelectedArtifact ? { loading: true } : {});            // 预览态
      const [infos, setInfos] = useState({});      // path → { size, kind, modified }(列表行元信息)
      const [externalUpdateBlocked, setExternalUpdateBlocked] = useState(/** @type {false|'removed'|'modified'} */ (false));
      const [htmlZoomMode, setHtmlZoomMode] = useState('fit');
      const [htmlScale, setHtmlScale] = useState(1);
      const [htmlCustomScale, setHtmlCustomScale] = useState(1);
      const [htmlZoomMenuOpen, setHtmlZoomMenuOpen] = useState(false);
      const [artifactMenuOpen, setArtifactMenuOpen] = useState(false);
      const [localDesignAiState, setLocalDesignAiState] = useState({ text: '', status: 'idle', lastPrompt: '', pendingPath: '', startedAt: 0 });
      const [designAiNow, setDesignAiNow] = useState(() => Date.now());
      const showDesignWorkbench = isFullscreen && designEditMode && pv.kind === 'html';
      const mdPreviewRef = useRef(null);
      const designFrameRef = useRef(null);
      const designRuntimeScriptRef = useRef(null);
      const designAiTimerRef = useRef(null);
      const designChangesRef = useRef(designChanges);
      // eslint-disable-next-line react-hooks/refs -- latest-ref sync: read only in callbacks/events (publishing design changes); render output does not depend on it
      designChangesRef.current = designChanges;

      const currentZoomOption = HTML_ZOOM_OPTIONS.find((option) => option.key === htmlZoomMode) || HTML_ZOOM_OPTIONS[0];
      const setPresetZoomMode = (mode) => {
        setHtmlZoomMode(mode);
        setHtmlZoomMenuOpen(false);
      };
      const adjustHtmlCustomScale = (delta) => {
        const next = clampHtmlScale((htmlZoomMode === 'custom' ? htmlCustomScale : htmlScale) + delta);
        setHtmlCustomScale(next);
        setHtmlZoomMode('custom');
      };
      const handleHtmlCustomScaleChange = (scale) => {
        setHtmlCustomScale(clampHtmlScale(scale));
        setHtmlZoomMode('custom');
      };
      const currentDesignAiState = designAiState || localDesignAiState;
      const designAiText = currentDesignAiState.text || '';
      const designAiStatus = currentDesignAiState.status || 'idle';
      const designAiLastPrompt = currentDesignAiState.lastPrompt || '';
      const designAiPendingPath = currentDesignAiState.pendingPath || '';
      const designAiStartedAt = Number(currentDesignAiState.startedAt || 0);
      const designAiElapsedSec = designAiStartedAt > 0 ? Math.max(0, Math.round((designAiNow - designAiStartedAt) / 1000)) : 0;
      const setDesignAiStatePatch = (patchOrUpdater) => {
        const apply = (prev) => {
          const base = prev || { text: '', status: 'idle', lastPrompt: '', pendingPath: '', startedAt: 0 };
          const patch = typeof patchOrUpdater === 'function' ? patchOrUpdater(base) : patchOrUpdater;
          return { ...base, ...patch };
        };
        if (onDesignAiStateChange) onDesignAiStateChange(apply);
        else setLocalDesignAiState(apply);
      };
      const describeDesignAiActivity = () => {
        if (designAiStatus === 'updated') return uiA.aiRefreshed;
        if (designAiStatus === 'no-update') return uiA.aiCanContinue;
        if (designAiStatus === 'cancelled') return '';
        if (bs?.thinking?.active && bs.thinking.phase === 'tool' && bs.thinking.toolName) {
          return uiA.aiCallingTool(bs.thinking.toolName);
        }
        if (bs?.thinking?.active) return uiA.aiThinking;
        if (bs?.busy) return uiA.aiWaitingModel;
        if (designAiStatus === 'sending') return uiA.aiSent;
        return '';
      };
      const designAiStatusTitle = (() => {
        const suffix = (designAiStatus === 'sending' || designAiStatus === 'running') && designAiStartedAt ? ` · ${designAiElapsedSec}s` : '';
        if (designAiStatus === 'updated') return uiA.aiUpdated;
        if (designAiStatus === 'no-update') return uiA.aiNoUpdate;
        if (designAiStatus === 'cancelled') return uiA.aiStopped;
        return uiA.aiAdjusting(suffix);
      })();
      const designAiActivity = describeDesignAiActivity();
      const designAiStatusDetail = [designAiLastPrompt, designAiActivity].filter(Boolean).join(' · ');
      const submitDesignAiText = (event) => {
        event.preventDefault();
        const text = designAiText.trim();
        if (!text || !onDesignAiSubmit) return;
        onDesignAiSubmit(text);
        setDesignAiStatePatch({
          text: '',
          lastPrompt: text,
          pendingPath: sel && sel.path || '',
          status: bs && bs.busy ? 'running' : 'sending',
          startedAt: Date.now(),
        });
      };
      const resetDesignAiStatusSoon = (delay = 2200) => {
        if (designAiTimerRef.current) window.clearTimeout(designAiTimerRef.current);
        designAiTimerRef.current = window.setTimeout(() => {
          setDesignAiStatePatch({ status: 'idle', lastPrompt: '', pendingPath: '', startedAt: 0 });
        }, delay);
      };
      const cancelDesignAi = () => {
        if (bridge.chat && bridge.chat.cancelGeneration) bridge.chat.cancelGeneration().catch(() => {});
        setDesignAiStatePatch({ status: 'cancelled', startedAt: 0 });
        resetDesignAiStatusSoon(1400);
      };

      useEffect(() => {
        if (designAiStatus !== 'sending' && designAiStatus !== 'running') return;
        // eslint-disable-next-line react-hooks/set-state-in-effect -- set the elapsed-time baseline immediately, then let the timer advance it every second
        setDesignAiNow(Date.now());
        const timer = window.setInterval(() => setDesignAiNow(Date.now()), 1000);
        return () => window.clearInterval(timer);
      }, [designAiStatus]);

      function destroyDesignRuntime() {
        const frame = designFrameRef.current;
        if (!frame || !frame.contentWindow) return;
        try {
          frame.contentWindow.postMessage({ type: DESIGN_MESSAGE_TYPES.DESTROY }, '*');
        } catch {
          // ignore
        }
      }

      function postDesignCommand(message) {
        const frame = designFrameRef.current;
        if (!frame || !frame.contentWindow || !message) return false;
        try {
          frame.contentWindow.postMessage(message, '*');
          return true;
        } catch {
          return false;
        }
      }

      function replayDesignChanges() {
        designChangesRef.current
          .filter((change) => change && change.status !== 'failed' && change.selector)
          .forEach((change) => {
            postDesignCommand({
              type: DESIGN_MESSAGE_TYPES.APPLY_CHANGE,
              payload: {
                selector: change.selector,
                changeId: change.id,
                changeType: change.type,
                property: change.property,
                oldValue: change.oldValue,
                value: change.newValue,
              },
            });
          });
      }

      function injectDesignRuntime(frame) {
        designFrameRef.current = frame || null;
        if (!showDesignWorkbench || !frame || !frame.contentWindow) return;
        try {
          if (!designRuntimeScriptRef.current) designRuntimeScriptRef.current = buildDesignRuntimeScript();
          const script = designRuntimeScriptRef.current;
          frame.contentWindow.eval(script);
        } catch (error) {
          console.warn('[pinvou3][artifacts] design runtime inject failed', error);
        }
      }

      const handlePreviewFrameLoad = (frame) => {
        injectDesignRuntime(frame);
      };

      // Exit-edit-mode boundary: destroy the iframe runtime and reset the
      // AI-adjustment state and the selected element (the main session's
      // generation is not cancelled — AI file edits may continue, just
      // without visual editing). If the selected element is not cleared, the
      // composer placeholder stays stuck on "adjust selected element".
      function exitDesignEditMode() {
        destroyDesignRuntime();
        setDesignEditMode(false);
        setDesignAiStatePatch({ status: 'idle', lastPrompt: '', pendingPath: '', startedAt: 0 });
        if (onDesignElementSelected) onDesignElementSelected(null);
      }

      useEffect(() => {
        if (!showDesignWorkbench) {
          destroyDesignRuntime();
          return;
        }
        injectDesignRuntime(designFrameRef.current);
        // eslint-disable-next-line react-hooks/exhaustive-deps -- re-inject only when the preview document identity changes; depending on the inject function itself would break that trigger timing
      }, [showDesignWorkbench, tab, pv.kind, pv.text, pv.visual && pv.visual.html]);

      // Auto-exit edit mode on artifact switch / tab switch / leaving
      // fullscreen: record the scope on entering edit mode, and exit as soon
      // as the scope changes (including a different artifact).
      const designEditScopeKey = sel && sel.path ? `${tab}:${sel.path}:${isFullscreen}` : null;
      const designEditEnteredScopeRef = useRef(null);
      useEffect(() => {
        if (!designEditMode) return;
        if (!isFullscreen || tab !== 'preview' || designEditEnteredScopeRef.current !== designEditScopeKey) {
          exitDesignEditMode();
        }
        // eslint-disable-next-line react-hooks/exhaustive-deps -- reset only on scope switches; designEditMode itself is set by user action
      }, [designEditScopeKey]);

      useEffect(() => {
        if (!showDesignWorkbench || !designCommand || !designCommand.seq) return;
        if (designCommand.kind === 'apply') {
          const ok = postDesignCommand({
            type: DESIGN_MESSAGE_TYPES.APPLY_CHANGE,
            payload: designCommand.payload,
          });
          if (!ok && onDesignChangeApplied) {
            onDesignChangeApplied({ changeId: designCommand.payload && designCommand.payload.changeId, ok: false, error: 'design runtime is not ready' });
          }
        } else if (designCommand.kind === 'clear') {
          postDesignCommand({ type: DESIGN_MESSAGE_TYPES.CLEAR_CHANGES });
        }
        // eslint-disable-next-line react-hooks/exhaustive-deps -- trigger once per command sequence number; the designCommand object/callback is a new reference on every render
      }, [showDesignWorkbench, designCommand && designCommand.seq]);

      useEffect(() => {
        const onMessage = (event) => {
          const data = event && event.data;
          if (!data || data.source !== 'pinvou-design-runtime') return;
          if (designFrameRef.current && event.source !== designFrameRef.current.contentWindow) return;
          if (data.type === DESIGN_MESSAGE_TYPES.READY) {
            replayDesignChanges();
          } else if (data.type === DESIGN_MESSAGE_TYPES.ELEMENT_SELECTED) {
            const element = data.payload && data.payload.element;
            if (onDesignElementSelected) onDesignElementSelected(element || null);
          } else if (data.type === DESIGN_MESSAGE_TYPES.ERROR) {
            console.warn('[pinvou3][artifacts] design runtime error', data.payload && data.payload.error);
          } else if (data.type === DESIGN_MESSAGE_TYPES.CHANGE_APPLIED) {
            if (onDesignChangeApplied) onDesignChangeApplied(data.payload || {});
          } else if (data.type === DESIGN_MESSAGE_TYPES.ELEMENT_MUTATED && onDesignMutation) onDesignMutation(data.payload || {});
        };
        window.addEventListener('message', onMessage);
        return () => {
          window.removeEventListener('message', onMessage);
          destroyDesignRuntime();
        };
        // eslint-disable-next-line react-hooks/exhaustive-deps -- the event listener only needs to mount once; callbacks are forwarded via ref/params, and adding them as deps would repeatedly rebind and replay design changes
      }, [onDesignElementSelected, onDesignMutation]);

      async function flushMarkdownPreview() {
        if (tab !== 'preview' || pv.kind !== 'md' || !mdPreviewRef.current) return true;
        return mdPreviewRef.current.flush();
      }

      function hasDirtyMarkdownPreview() {
        return tab === 'preview' && pv.kind === 'md' && !!mdPreviewRef.current?.hasDirty();
      }

      // 进面板 / artifacts 变化 → 批量拉元信息(给列表行的「最后修改」+ 类型)
      const pathsKey = artifacts.map((a) => a.path).join('|');
      useEffect(() => {
        let cancelled = false;
        (async () => {
          const entries = await Promise.all(artifacts.map(async (a) => {
            try { return [a.path, await bridge.artifacts.artifactInfo(a.path)]; }
            catch { return [a.path, null]; }
          }));
          if (cancelled) return;
          const m = {};
          entries.forEach(([p, i]) => { if (i) m[p] = i; });
          setInfos(m);
        })();
        return () => { cancelled = true; };
        // eslint-disable-next-line react-hooks/exhaustive-deps -- pathsKey is a stable digest of artifacts; depending on the artifacts array directly would refetch on every reference change
      }, [pathsKey, activeSessionId]);

      // 切 session(artifacts 整批换了)→ 选中文件已不在新列表 → 清预览、退回列表。
      // 路径含 session id,故「不在列表」可靠区分换 session vs 同 session 内新增文件。
      useEffect(() => {
        if (sel && artifacts.every((a) => a.path !== sel.path)) {
          const change = bs && bs.artifactChange;
          if (
            changeMatchesSession(change, bs) &&
            change?.event === 'removed' &&
            sameArtifactPath(change.path, sel.path)
          ) {
            if (hasDirtyMarkdownPreview()) {
              // eslint-disable-next-line react-hooks/set-state-in-effect -- deleted externally while local unsaved edits exist: synchronously flag the interception so dirty data is not overwritten
              setExternalUpdateBlocked('removed');
              setTab('preview');
              return;
            }
            setPv({ missing: true, info: null });
            setTab('preview');
            setExternalUpdateBlocked(false);
            return;
          }
          let cancelled = false;
          (async () => {
            const ok = await flushMarkdownPreview();
            if (cancelled || !ok) return;
            setSel(null); setPv({}); setTab('list');
          })();
          return () => { cancelled = true; };
        }
        // eslint-disable-next-line react-hooks/exhaustive-deps -- trigger on the pathsKey digest; depending on closure functions like flush/hasDirty would change the trigger frequency
      }, [pathsKey]);

      async function preview(a, options = {}) {
        const ok = options.skipFlush ? true : await flushMarkdownPreview();
        if (!ok) return;
        const selected = { ...a, sessionId: a.sessionId || activeSessionId };
        setSel(selected);
        onPreviewArtifact?.(selected);
        setTab('preview');
        setPv({ loading: true });
        setExternalUpdateBlocked(false);
      }

      useEffect(() => {
        if (!sel || !sel.path || !pv.loading) return;
        let cancelled = false;
        (async () => {
          try {
            const info = await bridge.artifacts.artifactInfo(sel.path);
            if (cancelled) return;
            if (!info || !info.exists) { setPv({ missing: true, info }); return; }
            if (['md', 'html', 'text'].includes(info.kind)) {
              const text = await bridge.artifacts.readArtifactText(sel.path);
              if (!cancelled) setPv({ kind: info.kind, text, info });
            } else {
              const visual = await bridge.artifacts.renderArtifactVisual(sel.path);
              if (!cancelled) setPv({ kind: info.kind, visual, info });
            }
          } catch (e) {
            if (!cancelled) setPv({ error: String(e) });
          }
        })();
        return () => { cancelled = true; };
        // eslint-disable-next-line react-hooks/exhaustive-deps -- load once on the selected-path + loading edge only; unrelated deps like artifacts would trigger duplicate fetches
      }, [sel && sel.path, pv.loading]);

      useEffect(() => {
        if (pv.loading || artifacts.length === 0) return;
        const preferred = preferredArtifactPath
          ? artifacts.find((a) => sameArtifactPath(a.path, preferredArtifactPath))
          : null;
        const fallback = artifacts[artifacts.length - 1] || null;
        const target = preferred || (sel ? null : fallback);
        if (!target) return;
        if (sel && sameArtifactPath(sel.path, target.path)) return;
        // eslint-disable-next-line react-hooks/set-state-in-effect -- auto-select the newest artifact when entering the panel; a one-off sync of the external list into local selection state
        preview(target, { skipFlush: !sel });
        // eslint-disable-next-line react-hooks/exhaustive-deps -- auto-select only needs evaluating on candidate-set/selected-path change edges; depending on the preview function would retrigger it repeatedly
      }, [preferredArtifactPath, pathsKey, activeSessionId, sel && sel.path, pv.loading]);

      const selectedArtifactPath = sel && sel.path;
      useEffect(() => {
        // eslint-disable-next-line react-hooks/set-state-in-effect -- synchronously collapse the menu on selection/tab switch; no cascading render risk
        setArtifactMenuOpen(false);
      }, [selectedArtifactPath, tab]);

      async function handleTabSelect(key) {
        if (key === tab) return;
        const ok = await flushMarkdownPreview();
        if (!ok) return;
        setTab(key);
      }

      async function handleClose() {
        const ok = await flushMarkdownPreview();
        if (ok) onClose?.();
      }

      function updateMarkdownPreview(text, info) {
        setPv((prev) => ({ ...prev, text, info: info || prev.info }));
        setExternalUpdateBlocked(false);
        if (sel?.path && info) {
          setInfos((prev) => ({ ...prev, [sel.path]: info }));
        }
      }

      useEffect(() => {
        const change = bs && bs.artifactChange;
        if (!change?.seq || !sel || tab !== 'preview') return;
        if (!changeMatchesSession(change, bs)) return;
        if (!sameArtifactPath(change.path, sel.path)) return;
        if (['sending', 'running', 'refreshing'].includes(designAiStatus)) {
          // eslint-disable-next-line react-hooks/set-state-in-effect -- synchronously flip the AI state when a disk change arrives, avoiding a stale status indicator
          setDesignAiStatePatch({ status: 'updated', startedAt: 0 });
          resetDesignAiStatusSoon();
        }
        if (change.event === 'removed') {
          if (hasDirtyMarkdownPreview()) {
            setExternalUpdateBlocked('removed');
            return;
          }
          setPv({ missing: true, info: null });
          setExternalUpdateBlocked(false);
          return;
        }
        let cancelled = false;
        (async () => {
          if (pv.kind === 'md' && mdPreviewRef.current) {
            const ok = await mdPreviewRef.current.reloadFromDisk({ force: false });
            if (!cancelled) setExternalUpdateBlocked(ok ? false : 'modified');
            return;
          }
          if (sel) await preview(sel);
        })();
        return () => { cancelled = true; };
        // eslint-disable-next-line react-hooks/exhaustive-deps -- trigger only on the artifactChange.seq edge; other deps would re-evaluate the preview refresh on every render
      }, [bs?.artifactChange?.seq]);

      useEffect(() => {
        if (designAiStatus === 'sending' && bs && bs.busy) {
          // eslint-disable-next-line react-hooks/set-state-in-effect -- synchronously advance the AI state machine sending→running on the busy edge
          setDesignAiStatePatch((current) => ({ status: 'running', startedAt: current.startedAt || Date.now() }));
        }
        if ((designAiStatus === 'sending' || designAiStatus === 'running') && bs && !bs.busy) {
          if (designAiTimerRef.current) window.clearTimeout(designAiTimerRef.current);
          designAiTimerRef.current = window.setTimeout(() => {
            setDesignAiStatePatch((current) => {
              if (current.status !== 'sending' && current.status !== 'running') return {};
              return { status: designAiPendingPath ? 'no-update' : 'idle', startedAt: 0 };
            });
            if (designAiPendingPath) resetDesignAiStatusSoon(2600);
          }, 3500);
        }
        // eslint-disable-next-line react-hooks/exhaustive-deps -- the state machine is driven by busy/status edges; adding function deps would rebuild the timer repeatedly
      }, [bs && bs.busy, designAiStatus, designAiPendingPath]);

      useEffect(() => () => {
        if (designAiTimerRef.current) window.clearTimeout(designAiTimerRef.current);
      }, []);

      const muted = 'text-[#757575] dark:text-[#8E8E8E]';
      const needsDependencyCheck = (message) => /LibreOffice/i.test(String(message || ''));
      const dependencyCheckButton = (message) => (
        needsDependencyCheck(message) && onGotoSettings
          ? <button type="button" onClick={onGotoSettings} className={`px-2 py-1 rounded-full font-medium bg-black/5 hover:bg-black/10 text-[#1F1F1F] dark:bg-white/10 dark:hover:bg-white/20 dark:text-[#E3E3E3]`}>{t.depGoInstall || t.depInstallBtn}</button>
          : null
      );
      const tabBtn = (key, label) => {
        const active = tab === key;
        const disabled = key === 'preview' && !sel;
        return (
          <button type="button" key={key} disabled={disabled}
            onClick={() => !disabled && handleTabSelect(key)}
            className={`px-4 py-1.5 rounded-full text-[13px] font-medium transition-colors
              ${active ? 'bg-[#E8EDF2] text-[#1F1F1F] dark:bg-[#333537] dark:text-[#E3E3E3]'
                : disabled ? 'text-[#BDC1C6] dark:text-[#5F6368] cursor-not-allowed'
                : 'text-[#444746] hover:bg-[#F0F4F9] dark:text-[#C4C7C5] dark:hover:bg-[#282A2C]'}`}>
            {label}
          </button>
        );
      };
      const renderArtifactSwitcher = () => {
        if (tab !== 'preview' || !sel) {
          return (
            <div className={`flex items-center gap-1 rounded-full p-0.5 bg-[#F0F4F9] dark:bg-[#141517]`}>
              {tabBtn('list', t.apTabList)}
              {tabBtn('preview', t.apTabPreview)}
            </div>
          );
        }
        const info = infos[sel.path];
        return (
          <div className="relative min-w-0">
            <button
              type="button"
              data-testid="artifact-switcher-button"
              onClick={() => setArtifactMenuOpen((open) => !open)}
              className={`flex h-9 max-w-[360px] items-center gap-2 rounded-full border px-3 text-left text-[13px] font-semibold shadow-sm transition-colors border-black/[0.06] bg-white text-[#1D1D1F] hover:bg-[#F5F5F7] dark:border-white/10 dark:bg-[#2C2C2E] dark:text-[#F5F5F7] dark:hover:bg-[#3A3A3C]`}
              aria-haspopup="menu"
              aria-expanded={artifactMenuOpen ? 'true' : 'false'}
              title={sel.basename}
            >
              <ArtifactTileIcon name={sel.basename} tileCls="w-6 h-6 rounded-[8px]" glyphCls="w-3.5 h-3.5" />
              <span className="min-w-0 truncate">{sel.basename}</span>
              {artifacts.length > 1 && <span className={`shrink-0 rounded-full px-1.5 py-0.5 text-[11px] bg-[#F2F2F7] text-[#6E6E73] dark:bg-white/10 dark:text-[#D1D1D6]`}>{artifacts.length}</span>}
              <span className={`shrink-0 text-[10px] text-[#8E8E93] dark:text-[#A1A1AA]`}>▼</span>
            </button>
            {artifactMenuOpen && (
              <div
                data-testid="artifact-switcher-menu"
                role="menu"
                className={`absolute left-0 top-11 z-40 w-[320px] max-w-[calc(100vw-48px)] rounded-[18px] border p-1.5 shadow-2xl backdrop-blur-2xl border-black/10 bg-white/95 text-[#1D1D1F] dark:border-white/10 dark:bg-[#2C2C2E]/95 dark:text-[#F5F5F7]`}
              >
                <div className={`px-3 pb-1 pt-1 text-[11px] font-medium text-[#8E8E93] dark:text-[#A1A1AA]`}>
                  {uiA.switchArtifact}
                </div>
                {artifacts.map((a) => {
                  const itemInfo = infos[a.path];
                  const active = sel && sameArtifactPath(sel.path, a.path);
                  return (
                    // biome-ignore lint/a11y/useFocusableInteractive: menu-item container; the keyboard path is handled by the inner real button (artifact-switcher-item)
                    <div
                      key={a.path}
                      role="menuitem"
                      className={`group flex w-full items-center gap-1 rounded-[12px] transition-colors ${
                        active
                          ? 'bg-[#E5F0FF] dark:bg-[#0A84FF]/24'
                          : 'hover:bg-black/[0.05] dark:hover:bg-white/10'
                      }`}
                      title={a.path}
                    >
                      <button
                        type="button"
                        data-testid="artifact-switcher-item"
                        onClick={() => { setArtifactMenuOpen(false); preview(a); }}
                        className="flex min-w-0 flex-1 items-center gap-2 rounded-[12px] px-2.5 py-2 text-left"
                      >
                        <ArtifactTileIcon name={a.basename} tileCls="w-8 h-8 rounded-[10px]" glyphCls="w-4 h-4" />
                        <span className="min-w-0 flex-1">
                          <span className={`block truncate text-[13px] font-medium text-[#1D1D1F] dark:text-[#F5F5F7]`}>{a.basename}</span>
                          <span className={`block truncate text-[11px] text-[#8E8E93] dark:text-[#A1A1AA]`}>
                            {itemInfo ? apFormatMtime(itemInfo.modified) : '—'}
                          </span>
                        </span>
                        {active && <span className="shrink-0 text-[12px] text-[#007AFF]">✓</span>}
                      </button>
                      {canOpenContainingFolder && (
                        <button
                          type="button"
                          title={t.apBtnLocate}
                          onClick={(event) => { event.preventDefault(); event.stopPropagation(); bridge.artifacts.openContainingFolder(a.path); }}
                          className={`mr-1 flex h-7 w-7 shrink-0 items-center justify-center rounded-full opacity-0 transition-opacity group-hover:opacity-100 hover:bg-white text-[#6E6E73] dark:hover:bg-white/10 dark:text-[#D1D1D6]`}
                        >
                          <FolderOpen size={14} />
                        </button>
                      )}
                    </div>
                  );
                })}
                {info && (
                  <div className={`mt-1 border-t px-3 pt-2 text-[11px] border-black/10 text-[#8E8E93] dark:border-white/10 dark:text-[#A1A1AA]`}>
                    {uiA.currentMtime(apFormatMtime(info.modified))}
                  </div>
                )}
              </div>
            )}
          </div>
        );
      };

      // ── 预览内容区(按 kind / visual.mode 渲染)──
      const renderContent = () => {
        if (pv.loading) return <div className={`text-[13px] ${muted}`}>{t.apConverting}</div>;
        if (pv.missing) return <div className={`text-[13px] ${muted}`}>{t.apMissing}</div>;
        if (pv.error) return <div className={`text-[13px] text-[#C5221F] dark:text-[#F28B82]`}>{t.apReadFail(pv.error)}</div>;
        if (pv.kind === 'md') {
          return (
            <div className="flex flex-col gap-2">
              {externalUpdateBlocked && (
                <div className={`rounded-lg px-3 py-2 text-[12px] bg-[#FFF7E0] text-[#8A5A00] dark:bg-[#3A2F16] dark:text-[#FDD663]`}>
                  {externalUpdateBlocked === 'removed'
                    ? t.apMdExternalRemovalBlocked
                    : t.apMdExternalUpdateBlocked}
                </div>
              )}
              <EditableMarkdownPreview
                ref={mdPreviewRef}
                artifact={sel}
                initialText={pv.text || ''}
                initialInfo={pv.info}
                t={t}
                onSaved={updateMarkdownPreview}
                onReloaded={updateMarkdownPreview}
              />
            </div>
          );
        }
        if (pv.kind === 'html') {
          // 方角 + 不裁剪:WebKitGTK 对「会内部滚动的 iframe」做任何 border-radius 裁剪
          // (含外层 overflow-hidden)都会在边缘留黑色梳齿残影。去掉圆角是唯一彻底解。
          return (
            <ScaledHtmlPreview
              html={pv.text || ''}
              title={(sel && sel.path) || t.apTabPreview}
              onFrameLoad={handlePreviewFrameLoad}
              onOpenExternal={(url) => bridge.artifacts.openUserExternalUrl(url)}
              zoomMode={showDesignWorkbench ? htmlZoomMode : 'auto-width'}
              customScale={htmlCustomScale}
              onScaleChange={setHtmlScale}
              onCustomScaleChange={handleHtmlCustomScaleChange}
            />
          );
        }
        if (pv.kind === 'text') {
          return <pre className={`text-[12px] whitespace-pre-wrap break-words font-mono text-[#444746] dark:text-[#C4C7C5]`}>{pv.text}</pre>;
        }
        // 可视化结果
        const vis = pv.visual;
        if (vis && vis.mode === 'html') {
          return (
            <div className="flex flex-col gap-2 h-full">
              {vis.warning && <div className={`flex items-center gap-2 text-[12px] text-[#E37400] dark:text-[#FDD663]`}><span>⚠️ {vis.warning}</span>{dependencyCheckButton(vis.warning)}</div>}
              <iframe sandbox="allow-same-origin allow-scripts" className="w-full flex-1 min-h-[480px] border-0 block bg-white"
                title={(sel && sel.path) || t.apTabPreview}
                data-testid="artifact-html-preview-frame"
                onLoad={(e) => handlePreviewFrameLoad(e.currentTarget)}
                srcDoc={(vis.html || '') + OFFICE_HTML_STYLE} />
            </div>
          );
        }
        if (vis && vis.mode === 'images') {
          return (
            <div className="flex flex-col items-center gap-3">
              {vis.warning && <div className={`self-start flex items-center gap-2 text-[12px] text-[#E37400] dark:text-[#FDD663]`}><span>⚠️ {vis.warning}</span>{dependencyCheckButton(vis.warning)}</div>}
              {(vis.images || []).map((src, i) => (
                <img key={i} src={src} className="max-w-full h-auto rounded-lg shadow-sm" alt={`page-${i + 1}`} />
              ))}
            </div>
          );
        }
        // 统一兜底卡(unsupported / 转换失败 / binary)
        return (
          <div className={`flex flex-col items-center justify-center text-center gap-3 py-10 ${muted}`}>
            {sel
              ? <ArtifactTileIcon name={sel.basename} tileCls="w-14 h-14 rounded-[16px]" glyphCls="w-7 h-7" />
              : <FileTypeIcon kind="other" className="h-11 w-11" />}
            <span className={`text-[14px] font-medium text-[#1F1F1F] dark:text-[#E3E3E3]`}>{sel && sel.basename}</span>
            <p className="text-[13px] max-w-[360px]">{(vis && vis.warning) || t.apUnsupported}</p>
            {vis && dependencyCheckButton(vis.warning)}
            {(!isWeb || canDownloadArtifacts) && (
              <button type="button" onClick={() => sel && bridge.artifacts.openArtifactExternal(sel.path)} className={cardBtnCls('primary')}>
                {t.apBtnOpen}
              </button>
            )}
          </div>
        );
      };

      return (
        <div className={isWide ? "relative w-full h-full" : "absolute inset-0 z-30 flex justify-end pointer-events-auto"}>
          {/* biome-ignore lint/a11y/useKeyWithClickEvents: backdrop click-to-close layer; the keyboard path is handled by the title-bar close button (artifact-close) */}
          {/* biome-ignore lint/a11y/noStaticElementInteractions: backdrop click-to-close layer, a non-interactive container */}
          {!isWide && <div className="absolute inset-0 bg-black/40" onClick={handleClose}></div>}
          <div className={`relative h-full flex flex-col bg-white dark:bg-[#1E1F20] ${isWide ? 'w-full border-l border-black/10 dark:border-white/10' : 'w-[680px] max-w-[88vw] shadow-2xl animate-in slide-in-from-right duration-200'}`}>
            {/* header + tabs */}
            <div className={`flex items-center justify-between px-3 py-2.5 border-b border-black/10 dark:border-white/10`}>
              {renderArtifactSwitcher()}
              <div className="flex items-center gap-1.5">
                {isFullscreen && tab === 'preview' && pv.kind === 'html' && (
                  <button type="button"
                    onClick={() => {
                      if (designEditMode) {
                        exitDesignEditMode();
                      } else {
                        designEditEnteredScopeRef.current = designEditScopeKey;
                        setDesignEditMode(true);
                      }
                    }}
                    data-testid="artifact-edit-mode-toggle"
                    aria-pressed={designEditMode}
                    aria-label={designEditMode ? uiA.fsExitEdit : uiA.fsEnterEdit}
                    title={designEditMode ? uiA.fsExitEdit : uiA.fsEnterEdit}
                    className={`h-8 rounded-full inline-flex items-center gap-1.5 px-3 text-[13px] font-semibold transition-colors shadow-sm ${
                      designEditMode
                        ? 'bg-[#007AFF] text-white hover:bg-[#0066D6]'
                        : 'bg-[#F2F2F7] text-[#1D1D1F] hover:bg-[#E5E5EA] ring-1 ring-black/[0.04] dark:bg-white/10 dark:text-[#F5F5F7] dark:hover:bg-white/15 dark:ring-white/10'
                    }`}>
                    <Edit2 size={15} />
                    <span>{designEditMode ? uiA.fsExitEditShort : uiA.fsEditMode}</span>
                  </button>
                )}
                {onToggleFullscreen && (
                  <button type="button"
                    onClick={onToggleFullscreen}
                    data-testid="artifact-fullscreen-toggle"
                    aria-label={isFullscreen ? uiA.fsExitFullBack : uiA.fsEnterPreview}
                    title={isFullscreen ? uiA.fsExit : uiA.fsEnter}
                    className={`w-8 h-8 rounded-full flex items-center justify-center hover:bg-[#F0F4F9] text-[#444746] dark:hover:bg-[#333537] dark:text-[#C4C7C5]`}>
                    {isFullscreen ? <Minimize2 size={17} /> : <Maximize2 size={17} />}
                  </button>
                )}
                <button type="button"
                  onClick={handleClose}
                  data-testid="artifact-close"
                  aria-label={uiA.closePreviewAria}
                  title={uiA.closePreviewTitle}
                  className={`w-8 h-8 rounded-full flex items-center justify-center hover:bg-[#F0F4F9] text-[#444746] dark:hover:bg-[#333537] dark:text-[#C4C7C5]`}>
                  <XCircle size={18} />
                </button>
              </div>
            </div>

            {/* body */}
            <div className="flex-1 min-h-0 flex flex-col">
              {tab === 'list' ? (
                <div className="flex-1 overflow-y-auto custom-scrollbar p-2">
                  {artifacts.length === 0 ? (
                    <div className={`p-4 text-[13px] ${muted}`}>{t.apEmpty}</div>
                  ) : artifacts.map((a) => {
                    const info = infos[a.path];
                    return (
                      // biome-ignore lint/a11y/useSemanticElements: the list row has multiple nested layouts and inline buttons; a button would break existing styles
                      <div key={a.path} role="button" tabIndex={0} onClick={() => preview(a)}
                        onKeyDown={(e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); preview(a); } }}
                        className={`group flex items-center gap-3 px-3 py-2.5 rounded-xl cursor-pointer
                          ${sel && sel.path === a.path ? 'bg-[#E8EDF2] dark:bg-[#333537]' : 'hover:bg-[#F0F4F9] dark:hover:bg-[#282A2C]'}`}>
                        <ArtifactTileIcon name={a.basename} />
                        <div className="flex-1 min-w-0">
                          <div className={`text-[14px] truncate text-[#1F1F1F] dark:text-[#E3E3E3]`} title={a.path}>{a.basename}</div>
                          <div className={`text-[12px] truncate ${muted}`}>
                            {t.apLastMod} {info ? apFormatMtime(info.modified) : '—'}
                          </div>
                        </div>
                        {canOpenContainingFolder && <button type="button" title={t.apBtnLocate} onClick={(e) => { e.stopPropagation(); bridge.artifacts.openContainingFolder(a.path); }}
                          className={`opacity-0 group-hover:opacity-100 w-8 h-8 rounded-full flex items-center justify-center hover:bg-white text-[#444746] dark:hover:bg-[#1E1F20] dark:text-[#C4C7C5]`}><FolderOpen size={16} /></button>
                        }
                      </div>
                    );
                  })}
                </div>
              ) : sel ? (
                <>
                  {/* preview content */}
                  <div className={`flex-1 min-h-0 flex min-w-0 ${showDesignWorkbench ? 'flex-row' : 'flex-col xl:flex-row'}`}>
                    <div className="relative flex-1 overflow-y-auto custom-scrollbar p-4 min-w-0" data-testid="artifact-preview-content">
                      {renderContent()}
                      {showDesignWorkbench && pv.kind === 'html' && (
                        <div data-testid="artifact-html-zoom-controls" className={`absolute bottom-4 right-4 z-20 flex items-center gap-1 rounded-full border p-1 text-[12px] shadow-lg backdrop-blur-2xl border-black/10 bg-white/95 text-[#1F1F1F] dark:border-white/10 dark:bg-[#1E1F20]/95 dark:text-[#E3E3E3]`}>
                          <div className="relative">
                            <button
                              type="button"
                              data-testid="artifact-html-zoom-mode"
                              onClick={() => setHtmlZoomMenuOpen((open) => !open)}
                              className={`h-8 rounded-full px-3 font-semibold transition-colors bg-[#007AFF] text-white hover:bg-[#006EE6] dark:bg-[#0A84FF] dark:hover:bg-[#409CFF]`}
                              aria-haspopup="menu"
                              aria-expanded={htmlZoomMenuOpen ? 'true' : 'false'}
                            >
                              {htmlZoomMode === 'custom' ? uiA.zoomCustom : uiA[currentZoomOption.labelKey]}
                              <span className="ml-1 text-[10px] opacity-80">▼</span>
                            </button>
                            {htmlZoomMenuOpen && (
                              <div
                                data-testid="artifact-html-zoom-menu"
                                className={`absolute bottom-10 left-0 min-w-[128px] rounded-[14px] border p-1.5 text-[13px] shadow-xl border-black/10 bg-white text-[#1D1D1F] dark:border-white/10 dark:bg-[#2C2C2E] dark:text-[#F5F5F7]`}
                              >
                                {HTML_ZOOM_OPTIONS.map((option) => (
                                  <button
                                    key={option.key}
                                    type="button"
                                    data-testid={`artifact-html-zoom-${option.key}`}
                                    onClick={() => setPresetZoomMode(option.key)}
                                    className={`flex h-9 w-full items-center justify-between rounded-[10px] px-3 text-left font-medium transition-colors ${
                                      htmlZoomMode === option.key
                                        ? 'bg-[#E5F0FF] text-[#0057D9] dark:bg-[#0A84FF]/25 dark:text-[#F5F5F7]'
                                        : 'hover:bg-black/[0.05] dark:hover:bg-white/10'
                                    }`}
                                  >
                                    <span>{uiA[option.labelKey]}</span>
                                    {htmlZoomMode === option.key && <span className="text-[12px]">✓</span>}
                                  </button>
                                ))}
                              </div>
                            )}
                          </div>
                          <span data-testid="artifact-html-zoom-scale" className={`min-w-[42px] px-2 text-center font-medium text-[#1D1D1F] dark:text-[#F5F5F7]`}>{Math.round(htmlScale * 100)}%</span>
                          <button
                            type="button"
                            data-testid="artifact-html-zoom-out"
                            onClick={() => adjustHtmlCustomScale(-0.1)}
                            className={`h-8 w-8 rounded-full text-[17px] font-semibold transition-colors hover:bg-black/5 dark:hover:bg-white/10`}
                            aria-label={uiA.zoomOut}
                            title={uiA.zoomOut}
                          >
                            -
                          </button>
                          <button
                            type="button"
                            data-testid="artifact-html-zoom-in"
                            onClick={() => adjustHtmlCustomScale(0.1)}
                            className={`h-8 w-8 rounded-full text-[17px] font-semibold transition-colors hover:bg-black/5 dark:hover:bg-white/10`}
                            aria-label={uiA.zoomIn}
                            title={uiA.zoomIn}
                          >
                            +
                          </button>
                        </div>
                      )}
                      {showDesignWorkbench && (
                        <div
                          aria-hidden="true"
                          className="pointer-events-none absolute inset-x-0 bottom-0 z-20 h-32 bg-gradient-to-t from-black/45 via-black/16 to-transparent"
                        />
                      )}
                      {showDesignWorkbench && (
                        <form
                          data-testid="artifact-design-ai-composer"
                          onSubmit={submitDesignAiText}
                          className={`absolute bottom-16 left-1/2 z-30 flex min-h-11 -translate-x-1/2 items-center gap-2 rounded-[22px] border px-2.5 py-1.5 shadow-[0_14px_38px_rgba(0,0,0,.24)] backdrop-blur-2xl border-white/80 bg-white/[0.96] text-[#1D1D1F] dark:border-white/15 dark:bg-[#1C1C1E]/95 dark:text-[#F5F5F7]`}
                          style={{ width: 'min(520px, calc(100% - 260px))' }}
                        >
                          {designAiStatus === 'idle' ? (
                            <>
                              <input
                                value={designAiText}
                                onChange={(event) => setDesignAiStatePatch({ text: event.target.value })}
                                data-testid="artifact-design-ai-input"
                                placeholder={selectedDesignElement ? uiA.aiPhElement : uiA.aiPhDesign}
                                className={`min-w-0 flex-1 bg-transparent text-[13px] outline-none placeholder:text-[#6E6E73] dark:placeholder:text-[#C7C7CC]`}
                              />
                              <button
                                type="submit"
                                data-testid="artifact-design-ai-send"
                                disabled={!designAiText.trim()}
                                className={`h-8 rounded-full px-3 text-[12px] font-semibold transition-colors ${
                                  designAiText.trim()
                                    ? 'bg-[#007AFF] text-white shadow-sm hover:bg-[#006EE6]'
                                    : 'bg-[#E5E5EA] text-[#8E8E93] dark:bg-white/14 dark:text-[#C7C7CC]'
                                }`}
                              >
                                {uiA.send}
                              </button>
                            </>
                          ) : (
                            <>
                              <div className="flex min-w-0 flex-1 items-center gap-2" data-testid="artifact-design-ai-status">
                                <div className="flex min-w-0 flex-1 flex-col justify-center">
                                  <div className="flex items-center gap-2 text-[13px] font-semibold leading-5">
                                  {(designAiStatus === 'sending' || designAiStatus === 'running') && <span className="h-2 w-2 animate-pulse rounded-full bg-[#007AFF]" />}
                                  <span>{designAiStatusTitle}</span>
                                  </div>
                                  {designAiStatusDetail && (
                                    <div className={`truncate text-[11px] leading-4 text-[#6E6E73] dark:text-[#C7C7CC]`}>
                                      {designAiStatusDetail}
                                    </div>
                                  )}
                                </div>
                                {(designAiStatus === 'updated' || designAiStatus === 'no-update') && (
                                  <div className={`shrink-0 rounded-full px-2 py-1 text-[11px] font-medium bg-[#F2F2F7] text-[#6E6E73] dark:bg-white/10 dark:text-[#D1D1D6]`}>
                                    {designAiStatus === 'updated' ? uiA.previewRefreshed : uiA.canContinue}
                                  </div>
                                )}
                              </div>
                              {(designAiStatus === 'sending' || designAiStatus === 'running') && (
                                <button
                                  type="button"
                                  data-testid="artifact-design-ai-stop"
                                  onClick={cancelDesignAi}
                                  className={`h-7 shrink-0 rounded-full px-2.5 text-[12px] font-semibold transition-colors bg-[#F2F2F7] text-[#1D1D1F] hover:bg-[#E5E5EA] dark:bg-white dark:text-[#1D1D1F] dark:hover:bg-[#F2F2F7]`}
                                >
                                  {uiA.stop}
                                </button>
                              )}
                            </>
                          )}
                        </form>
                      )}
                    </div>
                    {showDesignWorkbench && (
                      <div
                        className={`w-[300px] shrink-0 overflow-hidden border-l border-black/10 bg-white dark:border-white/10 dark:bg-[#1E1F20]`}
                        data-testid="artifact-design-inspector-host">
                        <DesignInspectorPanel
                          t={t}
                          selectedElement={selectedDesignElement}
                          changes={designChanges}
                          onApplyChange={onDesignApplyChange}
                          onClearChanges={onDesignClearChanges}
                          docked
                        />
                      </div>
                    )}
                  </div>
                  {/* meta footer */}
                  {!isFullscreen && <div className={`shrink-0 border-t px-4 py-3 border-black/10 bg-[#F8FAFD] dark:border-white/10 dark:bg-[#1A1B1D]`} data-testid="artifact-meta-footer">
                    <div className={`text-[14px] font-medium truncate text-[#1F1F1F] dark:text-[#E3E3E3]`}>{sel.basename}</div>
                    <div className={`mt-0.5 text-[12px] ${muted}`}>
                      {apKindLabel(t, pv.info && pv.info.kind, sel.basename)}
                      {pv.info && pv.info.size ? ' · ' + apFormatBytes(pv.info.size) : ''}
                    </div>
                    <div className={`mt-1.5 text-[12px] flex gap-2 ${muted}`}>
                      <span className="shrink-0">{t.apLocLabel}</span>
                      <span className="break-all">{sel.path}</span>
                    </div>
                    {pv.info && pv.info.modified ? (
                      <div className={`mt-0.5 text-[12px] flex gap-2 ${muted}`}>
                        <span className="shrink-0">{t.apMtimeLabel}</span>
                        <span>{apFormatMtime(pv.info.modified)}</span>
                      </div>
                    ) : null}
                    <div className="mt-3 flex items-center gap-2">
                      {(!isWeb || canDownloadArtifacts) && (
                        <button type="button" onClick={() => bridge.artifacts.openArtifactExternal(sel.path)}
                          className={`flex-1 flex items-center justify-center gap-1.5 ${cardBtnCls('primary')}`}>
                          <ExternalLink size={15} /> {t.apBtnOpen}
                        </button>
                      )}
                      {canOpenContainingFolder && (
                        <button type="button" onClick={() => bridge.artifacts.openContainingFolder(sel.path)}
                          className={`flex-1 flex items-center justify-center gap-1.5 ${cardBtnCls()}`}>
                          <FolderOpen size={15} /> {t.apBtnLocate}
                        </button>
                      )}
                    </div>
                  </div>}
                </>
              ) : (
                <div className={`flex-1 flex items-center justify-center text-[13px] ${muted}`}>{t.apPreviewHint}</div>
              )}
            </div>
          </div>
        </div>
      );
    };

    // ==========================================
    // 卡片池 (Persona / AgentPool)
    // ==========================================
    // Side B: agency-agents-zh 按"部门"组织(无档位/评分), 派生稳定的部门配色。

export { ArtifactTileIcon, apPad2, apFormatBytes, apFormatMtime, apKindLabel, OFFICE_HTML_STYLE, ArtifactsPanel };
