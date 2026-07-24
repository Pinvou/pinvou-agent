import React, { useEffect, useRef, useState } from 'react';
import { Download, ExternalLink, FolderOpen, XCircle } from '../../components/icons.jsx';
import { bridge } from '../../hooks/useBridge.js';
import { can, isWeb } from '../../shared/platform.js';
import { _ARTIFACT_FMT, _artifactKind } from '../../shared/artifact-utils.js';
import { ScaledHtmlPreview } from '../settings/SettingsView.jsx';
import { AcFmtIcon } from '../tools/tool-common.jsx';
import { cardBtnCls } from '../tools/tool-renderers.jsx';
import { EditableMarkdownPreview } from './EditableMarkdownPreview.jsx';

const ArtifactTileIcon = ({ name, tileCls = 'w-9 h-9 rounded-[10px]', glyphCls = 'w-5 h-5' }) => {
      const kind = _artifactKind(name);
      const fmt = _ARTIFACT_FMT[kind] || _ARTIFACT_FMT.other;
      return (
        <span className={`shrink-0 inline-flex items-center justify-center ${tileCls}`} style={{ background: fmt.color }}>
          <AcFmtIcon kind={kind} className={`${glyphCls} text-white`} />
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
    const normalizeArtifactPath = (path) => String(path || '').replace(/\\/g, '/').toLowerCase();
    const sameArtifactPath = (left, right) => {
      const a = normalizeArtifactPath(left);
      const b = normalizeArtifactPath(right);
      return !!a && !!b && a === b;
    };
    const changeMatchesSession = (change, bs) => {
      if (!change || !change.sessionId || !bs?.activeSessionId) return true;
      return change.sessionId === bs.activeSessionId;
    };
    // 注入到 office→HTML 预览 iframe 末尾:LibreOffice 导出的表格 border=0、字号 x-small,
    // 这里补网格线/字号/单元格换行,让 xlsx 读起来像表格。放在文档后 → 同特异性下后定义胜出。
    const OFFICE_HTML_STYLE = '<style>'
      + 'body{margin:14px;background:#fff;color:#1f1f1f;font-family:system-ui,-apple-system,"Segoe UI",sans-serif;}'
      + 'table{border-collapse:collapse;width:auto;max-width:100%;}'
      + 'td,th{border:1px solid #d4d7dc;padding:5px 9px;font-size:13px!important;vertical-align:top;max-width:460px;overflow-wrap:anywhere;}'
      + 'tr:first-child td{background:#eef2f8;font-weight:600;}'
      + 'img{max-width:100%;height:auto;}'
      + '</style>';

    const ArtifactsPanel = ({ bs, theme, t, onClose, isWide, onGotoSettings }) => {
      const isDark = theme === 'dark';
      const canOpenContainingFolder = can('externalSystemOpen');
      const canDownloadArtifacts = can('artifactDownload');
      const artifacts = (bs && bs.artifacts) || [];
      const activeSessionId = bs && bs.activeSessionId;
      const [tab, setTab] = useState('list');     // 'list' | 'preview'
      const [sel, setSel] = useState(null);        // 选中的 artifact { path, basename }
      const [pv, setPv] = useState({});            // 预览态
      const [infos, setInfos] = useState({});      // path → { size, kind, modified }(列表行元信息)
      const [externalUpdateBlocked, setExternalUpdateBlocked] = useState(false);
      const mdPreviewRef = useRef(null);

      async function flushMarkdownPreview() {
        if (tab !== 'preview' || pv.kind !== 'md' || !mdPreviewRef.current) return true;
        return await mdPreviewRef.current.flush();
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
            catch (_) { return [a.path, null]; }
          }));
          if (cancelled) return;
          const m = {};
          entries.forEach(([p, i]) => { if (i) m[p] = i; });
          setInfos(m);
        })();
        return () => { cancelled = true; };
      }, [pathsKey, activeSessionId]);

      // 切 session(artifacts 整批换了)→ 选中文件已不在新列表 → 清预览、退回列表。
      // 路径含 session id,故「不在列表」可靠区分换 session vs 同 session 内新增文件。
      useEffect(() => {
        if (sel && !artifacts.some((a) => a.path === sel.path)) {
          const change = bs && bs.artifactChange;
          if (
            changeMatchesSession(change, bs) &&
            change?.event === 'removed' &&
            sameArtifactPath(change.path, sel.path)
          ) {
            if (hasDirtyMarkdownPreview()) {
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
      }, [pathsKey]);

      async function preview(a) {
        const ok = await flushMarkdownPreview();
        if (!ok) return;
        const selected = { ...a, sessionId: a.sessionId || activeSessionId };
        setSel(selected);
        setTab('preview');
        setPv({ loading: true });
        setExternalUpdateBlocked(false);
        try {
          const info = await bridge.artifacts.artifactInfo(a.path);
          if (!info || !info.exists) { setPv({ missing: true, info }); return; }
          if (info.kind === 'md' || info.kind === 'html' || info.kind === 'text') {
            const text = await bridge.artifacts.readArtifactText(a.path);
            setPv({ kind: info.kind, text, info });
          } else {
            // image / pdf / docx / xlsx / legacy_office / binary → 后端可视化转换
            const visual = await bridge.artifacts.renderArtifactVisual(a.path);
            setPv({ kind: info.kind, visual, info });
          }
        } catch (e) { setPv({ error: String(e) }); }
      }

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
      }, [bs?.artifactChange?.seq]);

      const muted = isDark ? 'text-[#8E8E8E]' : 'text-[#757575]';
      const needsDependencyCheck = (message) => /LibreOffice/i.test(String(message || ''));
      const dependencyCheckButton = (message) => (
        needsDependencyCheck(message) && onGotoSettings
          ? <button onClick={onGotoSettings} className={`px-2 py-1 rounded-full font-medium ${isDark ? 'bg-white/10 hover:bg-white/20 text-[#E3E3E3]' : 'bg-black/5 hover:bg-black/10 text-[#1F1F1F]'}`}>{t.depGoInstall || t.depInstallBtn}</button>
          : null
      );
      const tabBtn = (key, label) => {
        const active = tab === key;
        const disabled = key === 'preview' && !sel;
        return (
          <button key={key} disabled={disabled}
            onClick={() => !disabled && handleTabSelect(key)}
            className={`px-4 py-1.5 rounded-full text-[13px] font-medium transition-colors
              ${active ? (isDark ? 'bg-[#333537] text-[#E3E3E3]' : 'bg-[#E8EDF2] text-[#1F1F1F]')
                : disabled ? (isDark ? 'text-[#5F6368]' : 'text-[#BDC1C6]') + ' cursor-not-allowed'
                : (isDark ? 'text-[#C4C7C5] hover:bg-[#282A2C]' : 'text-[#444746] hover:bg-[#F0F4F9]')}`}>
            {label}
          </button>
        );
      };

      // ── 预览内容区(按 kind / visual.mode 渲染)──
      const renderContent = () => {
        if (pv.loading) return <div className={`text-[13px] ${muted}`}>{t.apConverting}</div>;
        if (pv.missing) return <div className={`text-[13px] ${muted}`}>{t.apMissing}</div>;
        if (pv.error) return <div className={`text-[13px] ${isDark ? 'text-[#F28B82]' : 'text-[#C5221F]'}`}>{t.apReadFail(pv.error)}</div>;
        if (pv.kind === 'md') {
          return (
            <div className="flex flex-col gap-2">
              {externalUpdateBlocked && (
                <div className={`rounded-lg px-3 py-2 text-[12px] ${isDark ? 'bg-[#3A2F16] text-[#FDD663]' : 'bg-[#FFF7E0] text-[#8A5A00]'}`}>
                  {externalUpdateBlocked === 'removed'
                    ? (t.apMdExternalRemovalBlocked || '文件已在外部删除。未保存编辑已保留在当前编辑器中，请先复制内容或恢复原文件。')
                    : (t.apMdExternalUpdateBlocked || '文件已在外部更新。当前有未保存编辑，已暂不自动覆盖。')}
                </div>
              )}
              <EditableMarkdownPreview
                ref={mdPreviewRef}
                artifact={sel}
                initialText={pv.text || ''}
                initialInfo={pv.info}
                isDark={isDark}
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
          return <ScaledHtmlPreview html={pv.text || ''} />;
        }
        if (pv.kind === 'text') {
          return <pre className={`text-[12px] whitespace-pre-wrap break-words font-mono ${isDark ? 'text-[#C4C7C5]' : 'text-[#444746]'}`}>{pv.text}</pre>;
        }
        // 可视化结果
        const vis = pv.visual;
        if (vis && vis.mode === 'html') {
          return (
            <div className="flex flex-col gap-2 h-full">
              {vis.warning && <div className={`flex items-center gap-2 text-[12px] ${isDark ? 'text-[#FDD663]' : 'text-[#E37400]'}`}><span>⚠️ {vis.warning}</span>{dependencyCheckButton(vis.warning)}</div>}
              <iframe sandbox="allow-same-origin" className="w-full flex-1 min-h-[480px] border-0 block bg-white"
                srcDoc={(vis.html || '') + OFFICE_HTML_STYLE} />
            </div>
          );
        }
        if (vis && vis.mode === 'images') {
          return (
            <div className="flex flex-col items-center gap-3">
              {vis.warning && <div className={`self-start flex items-center gap-2 text-[12px] ${isDark ? 'text-[#FDD663]' : 'text-[#E37400]'}`}><span>⚠️ {vis.warning}</span>{dependencyCheckButton(vis.warning)}</div>}
              {(vis.images || []).map((src, i) => (
                <img key={i} src={src} className="max-w-full h-auto rounded-lg shadow-sm" alt={`page-${i + 1}`} />
              ))}
            </div>
          );
        }
        // 统一兜底卡(unsupported / 转换失败 / binary)
        return (
          <div className={`flex flex-col items-center justify-center text-center gap-3 py-10 ${muted}`}>
            {sel ? <ArtifactTileIcon name={sel.basename} tileCls="w-14 h-14 rounded-[16px]" glyphCls="w-7 h-7" /> : <span className="text-[44px]">📎</span>}
            <span className={`text-[14px] font-medium ${isDark ? 'text-[#E3E3E3]' : 'text-[#1F1F1F]'}`}>{sel && sel.basename}</span>
            <p className="text-[13px] max-w-[360px]">{(vis && vis.warning) || t.apUnsupported}</p>
            {vis && dependencyCheckButton(vis.warning)}
            {(!isWeb || canDownloadArtifacts) && (
              <button onClick={() => sel && bridge.artifacts.openArtifactExternal(sel.path)} className={cardBtnCls(isDark, 'primary')}>
                {t.apBtnOpen}
              </button>
            )}
          </div>
        );
      };

      return (
        <div className={isWide ? "relative w-full h-full" : "absolute inset-0 z-30 flex justify-end pointer-events-auto"}>
          {!isWide && <div className="absolute inset-0 bg-black/40" onClick={handleClose}></div>}
          <div className={`relative h-full flex flex-col ${isDark ? 'bg-[#1E1F20]' : 'bg-white'} ${isWide ? 'w-full border-l ' + (isDark ? 'border-white/10' : 'border-black/10') : 'w-[680px] max-w-[88vw] shadow-2xl animate-in slide-in-from-right duration-200'}`}>
            {/* header + tabs */}
            <div className={`flex items-center justify-between px-3 py-2.5 border-b ${isDark ? 'border-white/10' : 'border-black/10'}`}>
              <div className={`flex items-center gap-1 rounded-full p-0.5 ${isDark ? 'bg-[#141517]' : 'bg-[#F0F4F9]'}`}>
                {tabBtn('list', t.apTabList)}
                {tabBtn('preview', t.apTabPreview)}
              </div>
              <button onClick={handleClose} className={`w-8 h-8 rounded-full flex items-center justify-center ${isDark ? 'hover:bg-[#333537] text-[#C4C7C5]' : 'hover:bg-[#F0F4F9] text-[#444746]'}`}><XCircle size={18} /></button>
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
                      <div key={a.path} onClick={() => preview(a)}
                        className={`group flex items-center gap-3 px-3 py-2.5 rounded-xl cursor-pointer
                          ${sel && sel.path === a.path ? (isDark ? 'bg-[#333537]' : 'bg-[#E8EDF2]') : (isDark ? 'hover:bg-[#282A2C]' : 'hover:bg-[#F0F4F9]')}`}>
                        <ArtifactTileIcon name={a.basename} />
                        <div className="flex-1 min-w-0">
                          <div className={`text-[14px] truncate ${isDark ? 'text-[#E3E3E3]' : 'text-[#1F1F1F]'}`} title={a.path}>{a.basename}</div>
                          <div className={`text-[12px] truncate ${muted}`}>
                            {t.apLastMod} {info ? apFormatMtime(info.modified) : '—'}
                          </div>
                        </div>
                        {canOpenContainingFolder && <button title={t.apBtnLocate} onClick={(e) => { e.stopPropagation(); bridge.artifacts.openContainingFolder(a.path); }}
                          className={`opacity-0 group-hover:opacity-100 w-8 h-8 rounded-full flex items-center justify-center ${isDark ? 'hover:bg-[#1E1F20] text-[#C4C7C5]' : 'hover:bg-white text-[#444746]'}`}><FolderOpen size={16} /></button>
                        }
                      </div>
                    );
                  })}
                </div>
              ) : !sel ? (
                <div className={`flex-1 flex items-center justify-center text-[13px] ${muted}`}>{t.apPreviewHint}</div>
              ) : (
                <>
                  {/* preview content */}
                  <div className="flex-1 overflow-y-auto custom-scrollbar p-4 min-w-0">{renderContent()}</div>
                  {/* meta footer */}
                  <div className={`shrink-0 border-t px-4 py-3 ${isDark ? 'border-white/10 bg-[#1A1B1D]' : 'border-black/10 bg-[#F8FAFD]'}`}>
                    <div className={`text-[14px] font-medium truncate ${isDark ? 'text-[#E3E3E3]' : 'text-[#1F1F1F]'}`}>{sel.basename}</div>
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
                        <button onClick={() => bridge.artifacts.openArtifactExternal(sel.path)}
                          className={`flex-1 flex items-center justify-center gap-1.5 ${cardBtnCls(isDark, 'primary')}`}>
                          <ExternalLink size={15} /> {t.apBtnOpen}
                        </button>
                      )}
                      {canOpenContainingFolder && (
                        <button onClick={() => bridge.artifacts.openContainingFolder(sel.path)}
                          className={`flex-1 flex items-center justify-center gap-1.5 ${cardBtnCls(isDark)}`}>
                          <FolderOpen size={15} /> {t.apBtnLocate}
                        </button>
                      )}
                    </div>
                  </div>
                </>
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
