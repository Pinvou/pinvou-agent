import React, { forwardRef, useCallback, useEffect, useImperativeHandle, useMemo, useRef, useState } from 'react';
import TurndownService from 'turndown';
import { gfm } from 'turndown-plugin-gfm';
import { Check, Sparkles, X } from '../../components/icons.jsx';
import { bridge } from '../../hooks/useBridge.js';

const createTurndown = () => {
  const turndown = new TurndownService({
    headingStyle: 'atx',
    bulletListMarker: '-',
    codeBlockStyle: 'fenced',
  });
  turndown.use(gfm);
  turndown.keep(['kbd']);
  return turndown;
};

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
  isDark,
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
  const turndown = useMemo(createTurndown, []);

  const [draft, setDraft] = useState(initialText || '');
  const [saveState, setSaveState] = useState('idle');
  const [errorText, setErrorText] = useState('');
  const [selectionUi, setSelectionUi] = useState(null);
  const [aiInputOpen, setAiInputOpen] = useState(false);
  const [aiInstruction, setAiInstruction] = useState('');

  const applyMarkdownToDom = useCallback((markdown) => {
    const el = editableRef.current;
    if (!el) return;
    applyingHtmlRef.current = true;
    el.innerHTML = bridge.rendering.renderMarkdown(markdown || '');
    requestAnimationFrame(() => {
      applyingHtmlRef.current = false;
    });
  }, []);

  useEffect(() => {
    const text = initialText || '';
    latestDraftRef.current = text;
    lastSavedRef.current = text;
    setDraft(text);
    setSaveState('idle');
    setErrorText('');
    setSelectionUi(null);
    setAiInputOpen(false);
    setAiInstruction('');
    applyMarkdownToDom(text);
  }, [artifact?.path, applyMarkdownToDom]);

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

    const content = latestDraftRef.current;
    if (content === lastSavedRef.current) return true;

    setSaveState('saving');
    setErrorText('');
    const promise = (async () => {
      await bridge.artifacts.writeArtifactText(artifact.path, content);
      let info = initialInfo || null;
      try { info = await bridge.artifacts.artifactInfo(artifact.path); } catch (_) {}
      lastSavedRef.current = content;
      setSaveState('saved');
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
  }, [artifact?.path, initialInfo, onSaved]);

  useEffect(() => {
    saveNowRef.current = saveNow;
  }, [saveNow]);

  const reloadFromDisk = useCallback(async ({ force = false } = {}) => {
    if (!artifact?.path || !bridge.artifacts.readArtifactText) return false;
    if (!force && latestDraftRef.current !== lastSavedRef.current) return false;
    try {
      const text = await bridge.artifacts.readArtifactText(artifact.path);
      let info = initialInfo || null;
      try { info = await bridge.artifacts.artifactInfo(artifact.path); } catch (_) {}
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
    hasDirty: () => latestDraftRef.current !== lastSavedRef.current,
    reloadFromDisk,
  }), [saveNow, reloadFromDisk]);

  useEffect(() => {
    if (draft === lastSavedRef.current) return;
    setSaveState('dirty');
    const timer = setTimeout(() => { saveNow(); }, 1000);
    return () => clearTimeout(timer);
  }, [draft, saveNow]);

  useEffect(() => () => {
    if (latestDraftRef.current !== lastSavedRef.current) {
      saveNowRef.current?.();
    }
  }, []);

  const markdownFromDom = useCallback(() => {
    const el = editableRef.current;
    if (!el) return '';
    return turndown.turndown(el.innerHTML).trim();
  }, [turndown]);

  const handleInput = useCallback(() => {
    if (applyingHtmlRef.current) return;
    const next = markdownFromDom();
    latestDraftRef.current = next;
    setDraft(next);
    if (!aiInputOpen) setSelectionUi(null);
  }, [aiInputOpen, markdownFromDom]);

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
    if (!aiInputOpen) return undefined;
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
      const input = document.getElementById('md-selection-ai-input');
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
  const showStatus = saveState === 'dirty' || saveState === 'saving' || saveState === 'error';

  return (
    <div ref={rootRef} className="relative min-h-[420px]">
      {showStatus ? (
        <div className={`absolute right-2 top-2 z-20 rounded-full border px-3 py-1 text-[12px] shadow-sm ${saveState === 'error'
          ? (isDark ? 'border-[#5F2120] bg-[#2B1716] text-[#F28B82]' : 'border-[#F4C7C3] bg-[#FCE8E6] text-[#C5221F]')
          : (isDark ? 'border-white/10 bg-[#202124] text-[#C4C7C5]' : 'border-black/10 bg-white text-[#5F6368]')}`}>
          {status}{errorText ? `: ${errorText}` : ''}
        </div>
      ) : null}

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
        className={`msg-md min-h-[420px] rounded-lg px-1 py-1 text-[14px] leading-relaxed outline-none focus:ring-2 focus:ring-[#A8C7FA]/60 ${isDark ? 'dark-code text-[#E3E3E3]' : 'light-code text-[#1F1F1F]'}`}
      />

      {selectionUi ? (
        <div
          ref={overlayRef}
          className="fixed z-[90]"
          style={{ left: aiInputOpen ? selectionUi.inputX : selectionUi.buttonX, top: selectionUi.y }}
        >
          {!aiInputOpen ? (
            <button type="button" onMouseDown={openAiInput}
              className={`h-9 rounded-full border px-3 shadow-[0_8px_24px_rgba(0,0,0,0.16)] inline-flex items-center gap-1.5 text-[13px] font-medium transition-transform duration-150 ease-out hover:scale-[1.02] active:scale-[0.98] ${isDark ? 'border-white/10 bg-[#1C1C1E] text-[#F2F2F7]' : 'border-black/10 bg-white text-[#1C1C1E]'}`}>
              <Sparkles size={15} /> {t.apMdAiEdit}
            </button>
          ) : (
            <div className={`w-[316px] max-w-[calc(100vw-24px)] h-11 rounded-[22px] border shadow-[0_12px_32px_rgba(0,0,0,0.18)] flex items-center gap-1.5 px-2.5 ${isDark ? 'border-white/10 bg-[#1C1C1E]' : 'border-black/10 bg-white'}`}>
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
                  if (e.key === 'Enter') submitAiEdit();
                  if (e.key === 'Escape') clearAiUi();
                }}
                placeholder={t.apMdAiInstructionPlaceholder}
                className={`min-w-0 flex-1 bg-transparent outline-none text-[13px] leading-5 ${isDark ? 'text-[#F2F2F7] placeholder:text-[#8E8E93]' : 'text-[#1C1C1E] placeholder:text-[#8E8E93]'}`}
              />
              <button type="button" onClick={submitAiEdit}
                className={`shrink-0 w-8 h-8 rounded-full inline-flex items-center justify-center transition-colors ${isDark ? 'bg-[#0A84FF] text-white hover:bg-[#409CFF]' : 'bg-[#007AFF] text-white hover:bg-[#006EE6]'}`}>
                <Check size={17} />
              </button>
              <button type="button" onClick={clearAiUi}
                className={`shrink-0 w-7 h-7 rounded-full inline-flex items-center justify-center transition-colors ${isDark ? 'text-[#D1D1D6] hover:bg-white/10' : 'text-[#6E6E73] hover:bg-black/5'}`}>
                <X size={14} />
              </button>
            </div>
          )}
        </div>
      ) : null}
    </div>
  );
});

export { EditableMarkdownPreview };
