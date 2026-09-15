import { forwardRef, useCallback, useEffect, useImperativeHandle, useRef, useState, useSyncExternalStore } from 'react';
import { Check, Sparkles, X } from '../../components/icons.jsx';
import { bridge } from '../../hooks/useBridge.js';
import { isImeComposing } from '../../shared/ime-guard.mjs';
import { createTurndownService } from '../../shared/turndown-factory.js';
import { getSyntaxHighlightVersion, subscribeSyntaxHighlight } from '../../shared/syntax-highlighter.js';

const markdownFenceFor = (text) => {
  const runs = String(text || '').match(/`{3,}/g) || [];
  const max = runs.reduce((n, run) => Math.max(n, run.length), 2);
  return '`'.repeat(max + 1);
};

const buildAiPrompt = ({ t, artifact, selectedText, instruction }) => {
  const fence = markdownFenceFor(selectedText);
  return t.apMdAiPrompt({
    path: artifact.path,
    title: artifact.basename || artifact.path,
    selectedText,
    instruction,
    fence,
  });
};

const statusText = (t, state) => {
  if (state === 'dirty') return t.apMdDirty || '';
  if (state === 'saving') return t.apMdSaving || '';
  if (state === 'error') return t.apMdSaveFailed || '';
  return t.apMdSaved || '';
};

const EditableMarkdownPreview = forwardRef(function EditableMarkdownPreview({
  artifact,
  initialText,
  initialInfo,
  t,
  onSaved,
  onReloaded,
}, ref) {
  const rootRef = useRef(null);
  const editableRef = useRef(null);
  const overlayRef = useRef(null);
  const latestDraftRef = useRef(initialText || '');
  const lastSavedRef = useRef(initialText || '');
  const savingPromiseRef = useRef(null);
  const saveNowRef = useRef(null);
  const pendingSaveRef = useRef(false);
  const applyingHtmlRef = useRef(false);
  // turndown 就绪前用户敲进 contentEditable 的编辑只存在于 DOM,尚未投影进
  // latestDraftRef(见 handleInput 早退分支)。此标志让懒语言重放 effect 跳过
  // (refs 相等不代表 DOM 无编辑,重放会抹掉这段输入),并让 dirty 状态/保存
  // 调度不至于把窗口内的编辑误判为「干净」。
  const pendingDomEditRef = useRef(false);
  const turndownRef = useRef(null);
  const turndownReadyRef = useRef(null);
  // applyMarkdownToDom 定义在下方;懒语言重放 effect 声明在其前,经 ref 取用。
  const applyMarkdownToDomRef = useRef(null);

  const [draft, setDraft] = useState(initialText || '');
  const [saveState, setSaveState] = useState('idle');
  const [errorText, setErrorText] = useState('');
  const [selectionUi, setSelectionUi] = useState(null);
  const [aiInputOpen, setAiInputOpen] = useState(false);
  const [aiInstruction, setAiInstruction] = useState('');
  // 懒语言注册完成后 bump 版本号:仅在没有未保存编辑时把当前草稿重新渲染进
  // DOM 恢复高亮;有编辑时不重放,避免覆盖 DOM 里尚未投影的修改。除 refs 相等
  // 外还须查 pendingDomEditRef:投影未就绪窗口内 refs 恒相等,但 DOM 里可能
  // 已有未投影的用户输入,此刻重放会把它抹掉。
  const syntaxVersion = useSyncExternalStore(subscribeSyntaxHighlight, getSyntaxHighlightVersion);
  useEffect(() => {
    if (!pendingDomEditRef.current && latestDraftRef.current === lastSavedRef.current) {
      applyMarkdownToDomRef.current?.(latestDraftRef.current);
    }
  }, [syntaxVersion]);

  // turndown 就绪前 handleInput 不做 DOM→Markdown 投影,这段编辑只留在 DOM 里;
  // 就绪回调补一次投影,避免 blur/unmount 保存时丢失。加载失败(如动态 import
  // 异常)清掉状态并吞掉错误,由下次输入触发重试,不产生 unhandled rejection。
  const loadTurndown = useCallback(() => {
    if (turndownRef.current || turndownReadyRef.current) return;
    turndownReadyRef.current = createTurndownService().then((instance) => {
      turndownRef.current = instance;
      const el = editableRef.current;
      if (el && !applyingHtmlRef.current) {
        const projected = instance.turndown(el.innerHTML).trim();
        if (projected !== latestDraftRef.current) {
          latestDraftRef.current = projected;
          setDraft(projected);
        }
      }
      pendingDomEditRef.current = false;
      return instance;
    }).catch(() => {
      turndownReadyRef.current = null;
      return null;
    });
  }, []);

  // 挂载即后台预热,编辑开始时通常已就绪;失败时由 handleInput 重试。
  useEffect(() => {
    loadTurndown();
    return () => { turndownReadyRef.current = null; };
  }, [loadTurndown]);

  const applyMarkdownToDom = useCallback((markdown) => {
    const el = editableRef.current;
    if (!el) return;
    applyingHtmlRef.current = true;
    el.innerHTML = bridge.rendering.renderMarkdown(markdown || '');
    requestAnimationFrame(() => {
      applyingHtmlRef.current = false;
    });
  }, []);
  applyMarkdownToDomRef.current = applyMarkdownToDom;

  useEffect(() => {
    const text = initialText || '';
    latestDraftRef.current = text;
    lastSavedRef.current = text;
    pendingDomEditRef.current = false;
    setDraft(text);
    setSaveState('idle');
    setErrorText('');
    setSelectionUi(null);
    setAiInputOpen(false);
    setAiInstruction('');
    applyMarkdownToDom(text);
    // eslint-disable-next-line react-hooks/exhaustive-deps -- reset wholesale only on artifact path edges; initialText changes under the same path must not wipe user edits
  }, [artifact?.path, applyMarkdownToDom]);

  // 输入是高频事件,turndown 未就绪时不做 DOM→Markdown 投影(编辑暂存于 DOM,
  // 由 loadTurndown 的就绪回调补投影),就绪后恢复逐键同步。声明在 saveNow 之前
  // (saveNow 的未投影编辑兜底也用它)。
  const markdownFromDom = useCallback(() => {
    const el = editableRef.current;
    const instance = turndownRef.current;
    if (!el || !instance) return '';
    return instance.turndown(el.innerHTML).trim();
  }, []);

  const saveNow = useCallback(async () => {
    if (!artifact?.path) return true;
    if (!bridge.artifacts.writeArtifactText) return false;

    if (savingPromiseRef.current) {
      pendingSaveRef.current = true;
      await savingPromiseRef.current;
      if (pendingSaveRef.current) {
        pendingSaveRef.current = false;
        return saveNow();
      }
      return latestDraftRef.current === lastSavedRef.current;
    }

    // 卸载保存/外部 flush 可能赶在 turndown 就绪投影之前:refs 相等但 DOM 里
    // 还有未投影编辑。turndown 尚未就绪时无法同步投影(动态 import),只能如实
    // 报告未保存,让调用方(ArtifactsPanel 关闭确认)走「保存失败」路径而不是
    // 误以为已保存;就绪回调的投影随后由调度 effect 正常收尾。
    const content = latestDraftRef.current;
    if (content === lastSavedRef.current) {
      if (!pendingDomEditRef.current) return true;
      if (!turndownRef.current) return false;
      const projected = markdownFromDom();
      if (projected === lastSavedRef.current) {
        pendingDomEditRef.current = false;
        return true;
      }
      latestDraftRef.current = projected;
    }

    setSaveState('saving');
    setErrorText('');
    const promise = (async () => {
      await bridge.artifacts.writeArtifactText(artifact.path, content);
      let info = initialInfo || null;
      try { info = await bridge.artifacts.artifactInfo(artifact.path); } catch { /* fall back to initialInfo if metadata fetch fails */ }
      lastSavedRef.current = content;
      setSaveState('saved');
      // 保存成功即草稿==已保存:立即补一次懒语言重放。刚保存的内容若含首次
      // 出现的懒语言围栏,注册可能在保存后才完成;syntaxVersion bump 的重放
      // effect 依赖 latestDraftRef==lastSavedRef,此刻两者恰好相等,直接投影
      // 幂等且与该 effect 同语义。
      applyMarkdownToDomRef.current?.(content);
      onSaved?.(content, info);
      return true;
    })().catch((e) => {
      setSaveState('error');
      setErrorText(String(e));
      return false;
    });

    savingPromiseRef.current = promise;
    const ok = await promise;
    savingPromiseRef.current = null;
    if (pendingSaveRef.current) {
      pendingSaveRef.current = false;
      return saveNow();
    }
    return ok;
  }, [artifact?.path, initialInfo, markdownFromDom, onSaved]);

  useEffect(() => {
    saveNowRef.current = saveNow;
  }, [saveNow]);

  const reloadFromDisk = useCallback(async ({ force = false } = {}) => {
    if (!artifact?.path || !bridge.artifacts.readArtifactText) return false;
    if (!force && latestDraftRef.current !== lastSavedRef.current) return false;
    try {
      const text = await bridge.artifacts.readArtifactText(artifact.path);
      let info = initialInfo || null;
      try { info = await bridge.artifacts.artifactInfo(artifact.path); } catch { /* fall back to initialInfo if metadata fetch fails */ }
      latestDraftRef.current = text || '';
      lastSavedRef.current = text || '';
      setDraft(text || '');
      setSaveState('saved');
      setErrorText('');
      applyMarkdownToDom(text || '');
      onReloaded?.(text || '', info);
      return true;
    } catch (e) {
      setSaveState('error');
      setErrorText(String(e));
      return false;
    }
  }, [artifact?.path, applyMarkdownToDom, initialInfo, onReloaded]);

  useImperativeHandle(ref, () => ({
    flush: saveNow,
    // 外部删除保护(ArtifactsPanel)也要看见未投影的 DOM 编辑。
    hasDirty: () => pendingDomEditRef.current || latestDraftRef.current !== lastSavedRef.current,
    reloadFromDisk,
  }), [saveNow, reloadFromDisk]);

  useEffect(() => {
    // pendingDomEditRef 置位时 draft 已被拨动(见 handleInput),与 refs 相等的
    // 情形互斥;真正的防抖保存仍以 draft 为准。
    if (draft === lastSavedRef.current) return;
    setSaveState('dirty');
    const timer = setTimeout(() => { saveNow(); }, 1000);
    return () => clearTimeout(timer);
  }, [draft, saveNow]);

  useEffect(() => () => {
    if (pendingDomEditRef.current || latestDraftRef.current !== lastSavedRef.current) {
      saveNowRef.current?.();
    }
  }, []);

  const handleInput = useCallback(() => {
    if (applyingHtmlRef.current) return;
    if (!turndownRef.current) {
      // 预热未完成或此前失败:触发(重)加载,就绪回调会把 DOM 里的编辑补进草稿。
      // 先记下「DOM 有未投影编辑」:该标志挡住懒语言重放 effect(防止重放抹掉
      // 这段输入),并驱动下方保存 effect 立即标 dirty——否则窗口内关面板时
      // flush 因 refs 相等短路,这段输入会被静默丢弃。
      pendingDomEditRef.current = true;
      setDraft((prev) => (prev === lastSavedRef.current ? `${prev}\u200B` : prev));
      loadTurndown();
      return;
    }
    const next = markdownFromDom();
    latestDraftRef.current = next;
    setDraft(next);
    if (!aiInputOpen) setSelectionUi(null);
  }, [aiInputOpen, loadTurndown, markdownFromDom]);

  const handlePaste = useCallback((e) => {
    const text = e.clipboardData?.getData('text/plain');
    if (text == null) return;
    e.preventDefault();
    const inserted = document.execCommand?.('insertText', false, text);
    if (!inserted) {
      const sel = window.getSelection?.();
      if (sel && sel.rangeCount > 0) {
        const range = sel.getRangeAt(0);
        range.deleteContents();
        const node = document.createTextNode(text);
        range.insertNode(node);
        range.setStartAfter(node);
        range.setEndAfter(node);
        sel.removeAllRanges();
        sel.addRange(range);
      }
    }
    handleInput();
  }, [handleInput]);

  const clearTextSelection = useCallback(() => {
    window.getSelection?.()?.removeAllRanges();
  }, []);

  const clearAiUi = useCallback(({ clearSelection = true } = {}) => {
    setSelectionUi(null);
    setAiInputOpen(false);
    setAiInstruction('');
    if (clearSelection) clearTextSelection();
  }, [clearTextSelection]);

  const updateSelection = useCallback(() => {
    if (aiInputOpen) return;
    const root = editableRef.current;
    const sel = window.getSelection?.();
    if (!root || !sel || sel.rangeCount === 0 || sel.isCollapsed) {
      setSelectionUi(null);
      return;
    }
    if (!root.contains(sel.anchorNode) || !root.contains(sel.focusNode)) {
      setSelectionUi(null);
      return;
    }
    const selectedText = sel.toString();
    if (!selectedText.trim()) {
      setSelectionUi(null);
      return;
    }
    const range = sel.getRangeAt(0);
    // WebKit's DOMRectList has no Symbol.iterator (no iterable<> in the IDL); spreading throws TypeError, so Array.from is required.
    // eslint-disable-next-line unicorn/prefer-spread -- DOMRectList is not iterable on any Safari/WKWebView version
    const rects = Array.from(range.getClientRects()).filter((r) => r.width || r.height);
    const rect = rects[rects.length - 1] || range.getBoundingClientRect();
    if (!rect || (!rect.width && !rect.height)) {
      setSelectionUi(null);
      return;
    }
    const panelRect = rootRef.current?.getBoundingClientRect();
    const bounds = panelRect && panelRect.width > 0
      ? panelRect
      : { left: 0, top: 0, right: window.innerWidth, bottom: window.innerHeight };
    const pad = 12;
    const buttonWidth = 132;
    const inputWidth = 316;
    const overlayHeight = 52;
    const clamp = (value, min, max) => Math.min(max, Math.max(min, value));
    const minX = bounds.left + pad;
    const maxButtonX = Math.max(minX, bounds.right - buttonWidth - pad);
    const maxInputX = Math.max(minX, bounds.right - inputWidth - pad);
    const minY = bounds.top + pad;
    const maxY = Math.max(minY, bounds.bottom - overlayHeight - pad);
    const buttonX = clamp(rect.right + 8, minX, maxButtonX);
    const inputX = clamp(rect.right - inputWidth, minX, maxInputX);
    const y = clamp(rect.bottom + 8, minY, maxY);
    setSelectionUi({ selectedText, buttonX, inputX, y });
  }, [aiInputOpen]);

  useEffect(() => {
    const handler = () => updateSelection();
    document.addEventListener('selectionchange', handler);
    return () => document.removeEventListener('selectionchange', handler);
  }, [updateSelection]);

  useEffect(() => {
    if (!aiInputOpen) return;
    const handler = (e) => {
      const overlay = overlayRef.current;
      if (overlay && overlay.contains(e.target)) return;
      clearAiUi();
    };
    document.addEventListener('pointerdown', handler, true);
    return () => document.removeEventListener('pointerdown', handler, true);
  }, [aiInputOpen, clearAiUi]);

  const openAiInput = useCallback((e) => {
    e.preventDefault();
    e.stopPropagation();
    setAiInputOpen(true);
    setTimeout(() => {
      const input = document.querySelector('#md-selection-ai-input');
      if (input) input.focus();
    }, 0);
  }, []);

  const submitAiEdit = useCallback(async () => {
    if (!selectionUi?.selectedText || !artifact?.path) return;
    const instruction = aiInstruction.trim();
    if (!instruction) return;
    const ok = await saveNow();
    if (!ok) return;
    const confirmText = t.apMdComposerReplaceConfirm || '';
    if (typeof window !== 'undefined' && !window.confirm(confirmText)) return;
    bridge.chat.prefillComposer?.(buildAiPrompt({
      t,
      artifact,
      selectedText: selectionUi.selectedText,
      instruction,
    }));
    clearAiUi();
  }, [aiInstruction, artifact, clearAiUi, saveNow, selectionUi, t]);

  const status = statusText(t, saveState);
  const showStatus = ['dirty', 'saving', 'error'].includes(saveState);

  return (
    <div ref={rootRef} className="relative min-h-[420px]">
      {showStatus ? (
        <div className={`absolute right-2 top-2 z-20 rounded-full border px-3 py-1 text-[12px] shadow-sm ${saveState === 'error'
          ? 'border-[#F4C7C3] bg-[#FCE8E6] text-[#C5221F] dark:border-[#5F2120] dark:bg-[#2B1716] dark:text-[#F28B82]'
          : 'border-black/10 bg-white text-[#5F6368] dark:border-white/10 dark:bg-[#202124] dark:text-[#C4C7C5]'}`}>
          {status}{errorText ? `: ${errorText}` : ''}
        </div>
      ) : null}

      {/* biome-ignore lint/a11y/noStaticElementInteractions: contentEditable rich-text editing area, natively focusable and typeable; events are editor-internal machinery */}
      <div
        ref={editableRef}
        contentEditable
        suppressContentEditableWarning
        spellCheck={false}
        onInput={handleInput}
        onPaste={handlePaste}
        onBlur={() => { saveNow(); }}
        onMouseUp={updateSelection}
        onKeyUp={updateSelection}
        className="msg-md light-code dark-code min-h-[420px] rounded-lg px-1 py-1 text-[14px] leading-relaxed outline-none focus:ring-2 focus:ring-[#A8C7FA]/60 text-[#1F1F1F] dark:text-[#E3E3E3]"
      />

      {selectionUi ? (
        <div
          ref={overlayRef}
          className="fixed z-[90]"
          style={{ left: aiInputOpen ? selectionUi.inputX : selectionUi.buttonX, top: selectionUi.y }}
        >
          {aiInputOpen ? (
            <div className="w-[316px] max-w-[calc(100vw-24px)] h-11 rounded-[22px] border shadow-[0_12px_32px_rgba(0,0,0,0.18)] flex items-center gap-1.5 px-2.5 border-black/10 bg-white dark:border-white/10 dark:bg-[#1C1C1E]">
              <input
                id="md-selection-ai-input"
                name="pinvou-md-ai-instruction"
                autoComplete="off"
                autoCorrect="off"
                autoCapitalize="off"
                spellCheck={false}
                value={aiInstruction}
                onChange={(e) => setAiInstruction(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === 'Enter' && !isImeComposing(e)) submitAiEdit();
                  if (e.key === 'Escape') clearAiUi();
                }}
                placeholder={t.apMdAiInstructionPlaceholder}
                className="min-w-0 flex-1 bg-transparent outline-none text-[13px] leading-5 text-[#1C1C1E] placeholder:text-[#8E8E93] dark:text-[#F2F2F7]"
              />
              <button type="button" onClick={submitAiEdit}
                className="shrink-0 w-8 h-8 rounded-full inline-flex items-center justify-center transition-colors bg-[#007AFF] text-white hover:bg-[#006EE6] dark:bg-[#0A84FF] dark:hover:bg-[#409CFF]">
                <Check size={17} />
              </button>
              <button type="button" onClick={clearAiUi}
                className="shrink-0 w-7 h-7 rounded-full inline-flex items-center justify-center transition-colors text-[#6E6E73] hover:bg-black/5 dark:text-[#D1D1D6] dark:hover:bg-white/10">
                <X size={14} />
              </button>
            </div>
          ) : (
            <button type="button" onMouseDown={openAiInput}
              className="h-9 rounded-full border px-3 shadow-[0_8px_24px_rgba(0,0,0,0.16)] inline-flex items-center gap-1.5 text-[13px] font-medium transition-transform duration-150 ease-out hover:scale-[1.02] active:scale-[0.98] border-black/10 bg-white text-[#1C1C1E] dark:border-white/10 dark:bg-[#1C1C1E] dark:text-[#F2F2F7]">
              <Sparkles size={15} /> {t.apMdAiEdit}
            </button>
          )}
        </div>
      ) : null}
    </div>
  );
});

export { EditableMarkdownPreview };
