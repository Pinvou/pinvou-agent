import React, { useCallback, useEffect, useRef, useState } from 'react';
import { AlertTriangle, AppWindow, Archive, BookOpen, Check, Database, Download, Edit2, ExternalLink, FileText, FolderOpen, GridIcon, IconList, ImageIcon, Package, Plus, PresentationIcon, RefreshCw, Search, TableIcon, Trash2, X } from '../../components/icons.jsx';
import { bridge, useBridgeState } from '../../hooks/useBridge.js';
import { OFFICE_HTML_STYLE } from '../artifacts/ArtifactsPanel.jsx';
import { FilePreviewModal } from '../workflow/WorkflowView.jsx';
import { invokeTauri } from '../../platform/tauri/client.js';
import { can, isWeb } from '../../shared/platform.js';

let kbCache = { scan: null, stats: null, types: [], loaded: false, colls: [], allDocs: [], embedInfo: null, model: null, outputs: [], outputsLoaded: false };

    const KnowledgeView = ({ theme, t }) => {
      const isDark = theme === 'dark';
      const bs = useBridgeState(['knowledge', 'chat']); // 取知识模型进度和当前产物
      const inv = invokeTauri;
      const canDownloadArtifacts = !isWeb || can('artifactDownload');
      const canPickHostFiles = !isWeb || can('hostFilePicker');
      const canOpenSystemFiles = !isWeb && can('externalSystemOpen');
      const canInstallKbModel = can('localModelSetup') && can('dependencyInstall');

      const [sub, setSub] = useState('output'); // 'output' | 'files' | 'kb'

      // ---------- 共用 ----------
      const openFile = (p) => canOpenSystemFiles ? inv('open_in_system', { path: p }).catch(() => {}) : Promise.resolve(false);
      const openFolder = (p) => canOpenSystemFiles ? inv('open_containing_folder', { path: p }).catch(() => {}) : Promise.resolve(false);
      const fmtSize = (b) => {
        if (b == null) return '';
        if (b < 1024) return b + ' B';
        if (b < 1048576) return (b / 1024).toFixed(1) + ' KB';
        if (b < 1073741824) return (b / 1048576).toFixed(1) + ' MB';
        return (b / 1073741824).toFixed(2) + ' GB';
      };
      const fmtDate = (s) => {
        if (!s) return '';
        const d = new Date(s * 1000), p = (n) => String(n).padStart(2, '0');
        return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}`;
      };
      const fmtOutputDate = (s) => {
        if (!s) return '';
        const d = new Date(s * 1000), now = new Date(), p = (n) => String(n).padStart(2, '0');
        const sameDay = d.getFullYear() === now.getFullYear() && d.getMonth() === now.getMonth() && d.getDate() === now.getDate();
        if (sameDay) return `${t.kbOutTodayPrefix} ${p(d.getHours())}:${p(d.getMinutes())}`;
        const age = now.getTime() - d.getTime();
        if (age >= 0 && age < 7 * 86400000) return `${t.kbOutWeekdays[d.getDay()]} ${p(d.getHours())}:${p(d.getMinutes())}`;
        return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}`;
      };
      const muted = isDark ? 'text-[#C4C7C5]' : 'text-[#444746]';
      const card = isDark ? 'bg-[#1E1F20]' : 'bg-[#F0F4F9]';
      const cardHover = isDark ? 'hover:bg-[#1E1F20]' : 'hover:bg-[#F0F4F9]';
      const iconHover = isDark ? 'hover:bg-[#333537]' : 'hover:bg-[#E1E5EA]';
      const accent = isDark ? 'bg-[#A8C7FA] text-[#062E6F]' : 'bg-[#0B57D0] text-white';
      const soft = isDark ? 'bg-[#1E1F20] hover:bg-[#333537] text-[#A8C7FA]' : 'bg-[#F0F4F9] hover:bg-[#E1E5EA] text-[#0B57D0]';
      const ink = isDark ? 'text-[#E3E3E3]' : 'text-[#1F1F1F]';
      // 设计稿白卡:白底 + 细边框 + 柔阴影(本页文件/知识库卡片专用,区别于全局灰底 card)
      const panel = isDark ? 'bg-[#1E1F20] border border-white/10' : 'bg-white border border-[#ececf1]';
      const panelHover = isDark ? 'hover:border-white/20' : 'hover:border-[#dfe4f5]';
      const panelShadow = isDark ? {} : { boxShadow: '0 1px 2px rgba(24,24,40,.04), 0 8px 24px rgba(24,24,40,.04)' };
      // 文件类型 → 配色(对齐设计稿的彩色 ext 方块/类型卡)
      const EXT_COLOR = { doc:'#2f6beb',docx:'#2f6beb',md:'#5a6acf',txt:'#8a8a9a',rtf:'#2f6beb',odt:'#2f6beb',wps:'#2f6beb',html:'#2f6beb',htm:'#2f6beb',mhtml:'#2f6beb',mht:'#2f6beb',
        xls:'#18a957',xlsx:'#18a957',csv:'#18a957',ods:'#18a957',et:'#18a957',
        ppt:'#e0773a',pptx:'#e0773a',odp:'#e0773a',dps:'#e0773a',pdf:'#d63a3a',
        png:'#d6589a',jpg:'#d6589a',jpeg:'#d6589a',gif:'#d6589a',webp:'#d6589a',bmp:'#d6589a',svg:'#d6589a',heic:'#d6589a',fig:'#d6589a',
        zip:'#8a6ad6',rar:'#8a6ad6','7z':'#8a6ad6',tar:'#8a6ad6',gz:'#8a6ad6' };
      const extOf = (f) => (f.ext || (f.name && f.name.includes('.') ? f.name.split('.').pop() : '') || '').toLowerCase();
      const extColor = (e) => EXT_COLOR[e] || '#8a8a9a';
      const extLabel = (e) => (e || '?').toUpperCase().slice(0, 4);
      const CAT_COLOR = { all:'#6a6a78', doc:'#2f6beb', sheet:'#18a957', ppt:'#e0773a', pdf:'#d63a3a', img:'#d6589a', zip:'#8a6ad6' };
      // 每个类型卡的独立图标(对齐设计稿,不再全用 FileText)
      const CAT_ICON = { all:GridIcon, doc:FileText, sheet:TableIcon, ppt:PresentationIcon, pdf:FileText, img:ImageIcon, zip:Archive };
      // 知识库 → 按分类/名稳定配色(对齐设计稿彩色卡片图标)
      const COLL_PALETTE = ['#3f7bf0','#7b5fe6','#1aa07a','#d6873e','#d6589a','#4b7bd6','#e0903a','#2b9d7a','#7d6ae6'];
      const collColor = (c) => COLL_PALETTE[Math.abs(String((c && (c.category || c.name)) || '').split('').reduce((a, ch) => a + ch.charCodeAt(0), 0)) % COLL_PALETTE.length];

      // ================= 产出物 =================
      const [outputs, setOutputs] = useState(kbCache.outputs);
      const [outputsLoaded, setOutputsLoaded] = useState(kbCache.outputsLoaded);
      const [outCat, setOutCat] = useState('all');
      const [outQuery, setOutQuery] = useState('');
      const [outView, setOutView] = useState('grid');
      const [outputPreview, setOutputPreview] = useState(null);
      const outPreviewCache = useRef({});
      const outPreviewQueue = useRef({ active: 0, jobs: [] });
      const runQueuedPreview = useCallback((job) => new Promise((resolve, reject) => {
        const q = outPreviewQueue.current;
        const pump = () => {
          while (q.active < 2 && q.jobs.length > 0) {
            const item = q.jobs.shift();
            q.active += 1;
            Promise.resolve()
              .then(item.job)
              .then(item.resolve, item.reject)
              .finally(() => {
                q.active -= 1;
                setTimeout(pump, 60);
              });
          }
        };
        q.jobs.push({ job, resolve, reject });
        pump();
      }), []);
      const outputListSig = (list) => (list || []).map((o) => `${o.path || ''}|${o.mtime || 0}|${o.size || 0}|${o.sessionId || ''}|${o.source || ''}|${o.name || ''}`).join('\n');
      const outputsSigRef = useRef(outputListSig(kbCache.outputs));
      const OUTPUT_CATS = [
        { key: 'all', label: t.kbOutCatAll, color: '#6a6a78', icon: GridIcon },
        { key: 'web', label: t.kbOutCatWeb, color: '#2f6beb', icon: AppWindow },
        { key: 'doc', label: t.kbOutCatDoc, color: '#2b9d7a', icon: FileText },
        { key: 'img', label: t.kbOutCatImg, color: '#d6589a', icon: ImageIcon },
        { key: 'ppt', label: t.kbOutCatPpt, color: '#e0773a', icon: PresentationIcon },
      ];
      const outCatMeta = (k) => OUTPUT_CATS.find((c) => c.key === k) || OUTPUT_CATS[0];
      const refreshOutputs = useCallback(async () => {
        const list = bridge && bridge.artifacts.listDeliverableIndex
          ? await bridge.artifacts.listDeliverableIndex().catch(() => [])
          : await inv('list_deliverable_index').catch(() => []);
        const nextList = list || [];
        const nextSig = outputListSig(nextList);
        if (nextSig !== outputsSigRef.current) {
          outputsSigRef.current = nextSig;
          setOutputs(nextList);
          kbCache.outputs = nextList;
        }
        setOutputsLoaded(true);
        kbCache.outputsLoaded = true;
      }, []);
      useEffect(() => { if (sub === 'output') refreshOutputs(); }, [sub, refreshOutputs]);
      useEffect(() => {
        if (sub !== 'output') return;
        const onFocus = () => refreshOutputs();
        window.addEventListener('focus', onFocus);
        return () => window.removeEventListener('focus', onFocus);
      }, [sub, refreshOutputs]);
      const outputArtifactKey = ((bs && bs.artifacts) || []).map((a) => `${a.path || ''}:${a.basename || ''}`).join('|');
      useEffect(() => {
        if (sub === 'output') refreshOutputs();
      }, [sub, outputArtifactKey, refreshOutputs]);
      const filteredOutputs = React.useMemo(() => {
        const q = outQuery.trim().toLowerCase();
        return outputs.filter((o) => {
          const catOk = outCat === 'all' || o.category === outCat;
          const qOk = !q || String(o.name || '').toLowerCase().includes(q) || String(o.source || '').toLowerCase().includes(q);
          return catOk && qOk;
        });
      }, [outputs, outCat, outQuery]);
      const queryOutputs = React.useMemo(() => {
        const q = outQuery.trim().toLowerCase();
        return outputs.filter((o) => !q || String(o.name || '').toLowerCase().includes(q) || String(o.source || '').toLowerCase().includes(q));
      }, [outputs, outQuery]);
      const outputCount = (k) => k === 'all' ? outputs.length : outputs.filter((o) => o.category === k).length;
      const outputDesc = (o) => `${fmtSize(o.size)} · ${o.source || t.kbSubOutput}`;
      const groupOutputs = (list) => {
        const now = new Date();
        const startToday = new Date(now.getFullYear(), now.getMonth(), now.getDate()).getTime() / 1000;
        const startWeek = startToday - ((now.getDay() + 6) % 7) * 86400;
        const groups = [
          { key: 'today', label: t.kbOutGroupToday, ts: startToday, rows: [] },
          { key: 'week', label: t.kbOutGroupWeek, ts: startWeek, rows: [] },
        ];
        const byMonth = new Map();
        list.forEach((o) => {
          const mtime = o.mtime || 0;
          if (mtime >= startToday) { groups[0].rows.push(o); return; }
          if (mtime >= startWeek) { groups[1].rows.push(o); return; }
          const d = o.mtime ? new Date(o.mtime * 1000) : null;
          const key = d ? `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}` : 'unknown';
          if (!byMonth.has(key)) {
            byMonth.set(key, {
              key,
              label: d ? t.kbOutMonthLabel(d.getFullYear(), d.getMonth() + 1) : t.kbOutGroupUnknown,
              ts: d ? new Date(d.getFullYear(), d.getMonth(), 1).getTime() : 0,
              rows: [],
            });
          }
          byMonth.get(key).rows.push(o);
        });
        byMonth.forEach((g) => groups.push(g));
        return groups.filter((g) => g.rows.length > 0);
      };
      const openOutputChat = async (o) => {
        if (bridge && bridge.sessions.switchToSession && o.sessionId) {
          await bridge.sessions.switchToSession(o.sessionId);
        }
      };
      const continueOutput = async (o) => {
        await openOutputChat(o);
        if (bridge && bridge.chat.prefillComposer) {
          bridge.chat.prefillComposer(`${t.kbOutContinuePrefill(o.name)}\n\n文件路径：${o.path}\n\n${t.kbOutRequirementLabel}`);
        }
      };
      const newOutputProject = async (o) => {
        if (bridge && bridge.sessions.createNewSession) await bridge.sessions.createNewSession();
        if (bridge && bridge.chat.prefillComposer) {
          bridge.chat.prefillComposer(`${t.kbOutContinuePrefill(o.name)}\n\n文件路径：${o.path}\n\n${t.kbOutRequirementLabel}`);
        }
      };
      const OutputLivePreview = ({ o, onOpen }) => {
        const ext = String(o.ext || '').toLowerCase();
        const outputSessionId = o.sessionId || o.session_id || null;
        const cacheKey = `${outputSessionId || ''}|${o.path}|${o.mtime || 0}`;
        const boxRef = useRef(null);
        const [visible, setVisible] = useState(false);
        const [pv, setPv] = useState(() => outPreviewCache.current[cacheKey] || { idle: true });
        const title = o.name.replace(/\.[^.]+$/, '');
        const [frameReady, setFrameReady] = useState(false);
        useEffect(() => {
          const node = boxRef.current;
          if (!node) return;
          if (!('IntersectionObserver' in window)) { setVisible(true); return; }
          const io = new IntersectionObserver((entries) => {
            if (entries.some((e) => e.isIntersecting)) {
              setVisible(true);
              io.disconnect();
            }
          }, { rootMargin: '0px', threshold: 0.08 });
          io.observe(node);
          return () => io.disconnect();
        }, [cacheKey]);
        useEffect(() => {
          let alive = true;
          const hit = outPreviewCache.current[cacheKey];
          if (hit) { setPv(hit); return () => { alive = false; }; }
          if (!visible) { setPv({ idle: true }); return () => { alive = false; }; }
          setPv({ loading: true });
          setFrameReady(false);
          const timer = setTimeout(() => {
          runQueuedPreview(async () => {
            const freshHit = outPreviewCache.current[cacheKey];
            if (freshHit) return freshHit;
            try {
              let next = null;
              if (o.category === 'img' && bridge.artifacts.readArtifactImageB64) {
                next = { kind: 'image', url: await bridge.artifacts.readArtifactImageB64(o.path) };
              } else if (ext === 'pptx' && bridge.artifacts.readArtifactThumbnail) {
                const thumb = await bridge.artifacts.readArtifactThumbnail(o.path);
                next = thumb ? { kind: 'image', url: thumb } : null;
              }
              if (!next && (o.category === 'web' || ext === 'html' || ext === 'htm') && bridge.artifacts.readArtifactText) {
                next = { kind: 'html', html: await bridge.artifacts.readArtifactText(o.path) };
              }
              if (!next && ['docx', 'doc', 'odt', 'rtf'].includes(ext) && bridge.artifacts.renderArtifactVisual) {
                const visual = await bridge.artifacts.renderArtifactVisual(o.path);
                if (visual && visual.mode === 'html' && visual.html) {
                  next = { kind: 'officeHtml', html: visual.html + OFFICE_HTML_STYLE };
                }
              }
              if (!next && ['md', 'markdown', 'txt', 'csv', 'json', 'log'].includes(ext) && bridge.artifacts.readArtifactText) {
                const text = await bridge.artifacts.readArtifactText(o.path);
                next = { kind: 'text', text: text.slice(0, 1600) };
              }
              if (!next) next = { kind: 'fallback' };
              outPreviewCache.current[cacheKey] = next;
              return next;
            } catch (e) {
              const next = { kind: 'fallback', error: String(e) };
              outPreviewCache.current[cacheKey] = next;
              return next;
            }
          }).then((next) => { if (alive) setPv(next); });
          }, 80);
          return () => { alive = false; clearTimeout(timer); };
        }, [cacheKey, visible, o.path, o.category, ext, outputSessionId, runQueuedPreview]);

        const htmlPreviewDoc = (html) => '<style>*{animation-duration:.001s!important;}</style>' + (html || '');
        const officePreviewDoc = (html) => '<style>html,body{background:#fff!important;margin:0;color:#111!important;}*{animation-duration:.001s!important;}</style>' + (html || '');
        const shell = (children) => (
          <div ref={boxRef} onClick={onOpen} role={onOpen ? 'button' : undefined} tabIndex={onOpen ? 0 : undefined}
            onKeyDown={(e) => { if (onOpen && (e.key === 'Enter' || e.key === ' ')) { e.preventDefault(); onOpen(); } }}
            className={`h-[164px] m-2.5 rounded-[13px] overflow-hidden relative border border-white/[0.06] bg-[#15171a] ${onOpen ? 'cursor-pointer' : ''}`}>
            {children}
          </div>
        );
        if (pv.idle || pv.loading) return shell(
          <div className="absolute inset-0 p-6">
            <div className="h-[13px] w-[68%] rounded-full bg-white/15 animate-pulse mb-4"></div>
            <div className="h-2 w-[88%] rounded-full bg-white/10 animate-pulse mb-2.5"></div>
            <div className="h-2 w-[76%] rounded-full bg-white/10 animate-pulse mb-2.5"></div>
            <div className="h-2 w-[54%] rounded-full bg-white/10 animate-pulse"></div>
          </div>
        );
        if (pv.kind === 'image') return shell(<img src={pv.url} alt="" className="w-full h-full object-cover" />);
        if (pv.kind === 'html') return shell(
          <>
            {!frameReady && <div className="absolute inset-0 bg-[#15171a]"></div>}
            <iframe title={o.name} sandbox="allow-same-origin" srcDoc={htmlPreviewDoc(pv.html)} onLoad={() => setTimeout(() => setFrameReady(true), 80)}
              className={`absolute inset-0 w-[200%] h-[200%] origin-top-left scale-50 bg-[#15171a] pointer-events-none border-0 transition-opacity duration-300 ${frameReady ? 'opacity-100' : 'opacity-0'}`}
              style={{ colorScheme: 'dark' }} />
          </>
        );
        if (pv.kind === 'officeHtml') return shell(
          <>
            {!frameReady && <div className="absolute inset-0 bg-white"></div>}
            <iframe title={o.name} sandbox="allow-same-origin" srcDoc={officePreviewDoc(pv.html)} onLoad={() => setTimeout(() => setFrameReady(true), 80)}
              className={`absolute inset-0 w-[200%] h-[200%] origin-top-left scale-50 bg-white pointer-events-none border-0 transition-opacity duration-300 ${frameReady ? 'opacity-100' : 'opacity-0'}`}
              style={{ colorScheme: 'light' }} />
          </>
        );
        if (pv.kind === 'text') {
          const lines = String(pv.text || '').split(/\r?\n/).filter(Boolean).slice(0, 8);
          return shell(
            <div className="absolute inset-0 p-5 font-mono text-[11px] leading-relaxed text-[#9aa2ad] overflow-hidden">
              <b className="block text-[#e7eaf0] text-[14px] mb-3 truncate"># {title}</b>
              {lines.map((line, i) => <p key={i} className={`m-0 mb-1.5 truncate ${i % 2 ? 'text-[#6e747e]' : ''}`}>{line}</p>)}
            </div>
          );
        }
        const meta = outCatMeta(o.category);
        const Icon = meta.icon || FileText;
        return shell(
          <div className="absolute inset-0 grid place-items-center" style={{ color: meta.color }}>
            <div className="w-16 h-16 rounded-2xl grid place-items-center" style={{ background: meta.color + '24' }}><Icon size={30} /></div>
          </div>
        );
      };

      const LocalFilePreview = ({ f, onOpen }) => {
        const ext = extOf(f);
        const cacheKey = `local|${f.path}|${f.mtime || 0}|${f.size || 0}`;
        const boxRef = useRef(null);
        const [visible, setVisible] = useState(false);
        const [pv, setPv] = useState(() => outPreviewCache.current[cacheKey] || { idle: true });
        const [frameReady, setFrameReady] = useState(false);
        useEffect(() => {
          const node = boxRef.current;
          if (!node) return;
          if (!('IntersectionObserver' in window)) { setVisible(true); return; }
          const io = new IntersectionObserver((entries) => {
            if (entries.some((e) => e.isIntersecting)) {
              setVisible(true);
              io.disconnect();
            }
          }, { rootMargin: '0px', threshold: 0.08 });
          io.observe(node);
          return () => io.disconnect();
        }, [cacheKey]);
        useEffect(() => {
          let alive = true;
          const hit = outPreviewCache.current[cacheKey];
          if (hit) { setPv(hit); return () => { alive = false; }; }
          if (!visible) { setPv({ idle: true }); return () => { alive = false; }; }
          // 本机知识文件不是 Session 产物；Web 端不读取或下载任意主机路径。
          if (isWeb) { setPv({ kind: 'fallback' }); return () => { alive = false; }; }
          setPv({ loading: true });
          setFrameReady(false);
          const timer = setTimeout(() => {
            runQueuedPreview(async () => {
              const freshHit = outPreviewCache.current[cacheKey];
              if (freshHit) return freshHit;
              try {
                let next = null;
                if (['png','jpg','jpeg','gif','webp','bmp','svg'].includes(ext) && bridge.artifacts.readArtifactImageB64) {
                  next = { kind: 'image', url: await bridge.artifacts.readArtifactImageB64(f.path) };
                }
                if (!next && ['html','htm'].includes(ext) && bridge.artifacts.readArtifactText) {
                  const html = await bridge.artifacts.readArtifactText(f.path);
                  let bodyText = '';
                  try {
                    const doc = new DOMParser().parseFromString(String(html || ''), 'text/html');
                    doc.querySelectorAll('script,style,noscript').forEach((n) => n.remove());
                    bodyText = ((doc.body && doc.body.innerText) || '').trim();
                  } catch (_) {}
                  next = (bodyText.length < 24 && /<script[\s>]/i.test(html))
                    ? { kind: 'text', text: html.slice(0, 1200) }
                    : { kind: 'html', html };
                }
                if (!next && ['docx','doc','odt','rtf','xlsx','xls','pptx','ppt','pdf'].includes(ext) && bridge.artifacts.renderArtifactVisual) {
                  const visual = await bridge.artifacts.renderArtifactVisual(f.path);
                  if (visual && visual.mode === 'html' && visual.html) next = { kind: 'officeHtml', html: visual.html + OFFICE_HTML_STYLE };
                  else if (visual && visual.mode === 'images' && visual.images && visual.images.length) next = { kind: 'image', url: visual.images[0] };
                }
                if (!next && ['md','markdown','txt','csv','json','log'].includes(ext) && bridge.artifacts.readArtifactText) {
                  const text = await bridge.artifacts.readArtifactText(f.path);
                  next = { kind: 'text', text: text.slice(0, 1200) };
                }
                if (!next) next = { kind: 'fallback' };
                outPreviewCache.current[cacheKey] = next;
                return next;
              } catch (e) {
                const next = { kind: 'fallback', error: String(e) };
                outPreviewCache.current[cacheKey] = next;
                return next;
              }
            }).then((next) => { if (alive) setPv(next); });
          }, 80);
          return () => { alive = false; clearTimeout(timer); };
        }, [cacheKey, visible, f.path, ext, runQueuedPreview]);

        const col = extColor(ext);
        const htmlPreviewDoc = (html) => '<style>*{animation-duration:.001s!important;}</style>' + (html || '');
        const officePreviewDoc = (html) => '<style>html,body{background:#fff!important;margin:0;color:#111!important;}*{animation-duration:.001s!important;}</style>' + (html || '');
        const shell = (children) => (
          <div ref={boxRef} onClick={onOpen} role={onOpen ? 'button' : undefined} tabIndex={onOpen ? 0 : undefined}
            onKeyDown={(e) => { if (onOpen && (e.key === 'Enter' || e.key === ' ')) { e.preventDefault(); onOpen(); } }}
            className={`h-[126px] rounded-[14px] overflow-hidden relative border border-white/[0.06] bg-[#15171a] mb-3 ${onOpen ? 'cursor-pointer' : ''}`}>
            {children}
          </div>
        );
        if (pv.idle || pv.loading) return shell(
          <div className="absolute inset-0 p-5">
            <div className="h-[12px] w-[70%] rounded-full bg-white/15 animate-pulse mb-3"></div>
            <div className="h-2 w-[88%] rounded-full bg-white/10 animate-pulse mb-2"></div>
            <div className="h-2 w-[62%] rounded-full bg-white/10 animate-pulse"></div>
          </div>
        );
        if (pv.kind === 'image') return shell(<img src={pv.url} alt="" className="w-full h-full object-cover" />);
        if (pv.kind === 'html') return shell(
          <>
            {!frameReady && <div className="absolute inset-0 bg-[#15171a]"></div>}
            <iframe title={f.name} sandbox="allow-same-origin" srcDoc={htmlPreviewDoc(pv.html)} onLoad={() => setTimeout(() => setFrameReady(true), 80)}
              className={`absolute inset-0 w-[200%] h-[200%] origin-top-left scale-50 bg-[#15171a] pointer-events-none border-0 transition-opacity duration-300 ${frameReady ? 'opacity-100' : 'opacity-0'}`}
              style={{ colorScheme: 'dark' }} />
          </>
        );
        if (pv.kind === 'officeHtml') return shell(
          <>
            {!frameReady && <div className="absolute inset-0 bg-white"></div>}
            <iframe title={f.name} sandbox="allow-same-origin" srcDoc={officePreviewDoc(pv.html)} onLoad={() => setTimeout(() => setFrameReady(true), 80)}
              className={`absolute inset-0 w-[200%] h-[200%] origin-top-left scale-50 bg-white pointer-events-none border-0 transition-opacity duration-300 ${frameReady ? 'opacity-100' : 'opacity-0'}`}
              style={{ colorScheme: 'light' }} />
          </>
        );
        if (pv.kind === 'text') {
          const lines = String(pv.text || '').split(/\r?\n/).filter(Boolean).slice(0, 7);
          return shell(
            <div className="absolute inset-0 p-4 font-mono text-[10px] leading-relaxed text-[#9aa2ad] overflow-hidden">
              <b className="block text-[#e7eaf0] text-[12px] mb-2 truncate"># {f.name}</b>
              {lines.map((line, i) => <p key={i} className={`m-0 mb-1 truncate ${i % 2 ? 'text-[#6e747e]' : ''}`}>{line}</p>)}
            </div>
          );
        }
        return shell(
          <div className="absolute inset-0 grid place-items-center" style={{ color: col }}>
            <div className="w-14 h-14 rounded-2xl grid place-items-center text-[12px] font-black" style={{ background: col + '24' }}>{extLabel(ext)}</div>
          </div>
        );
      };

      // ================= 文件管理 (L0) =================
      const [scan, setScan] = useState(kbCache.scan);
      const [stats, setStats] = useState(kbCache.stats);
      const [types, setTypes] = useState(kbCache.types);
      const [loaded, setLoaded] = useState(kbCache.loaded); // L0 首次拉完才 true,之前不判空状态
      const [cat, setCat] = useState('all');
      const [query, setQuery] = useState('');
      const [results, setResults] = useState([]);
      const [searched, setSearched] = useState(false);
      const [addToKb, setAddToKb] = useState(null);

      const CATS = [
        { key: 'all', label: t.kbCatAll, exts: null },
        { key: 'doc', label: t.kbCatDoc, exts: ['doc','docx','md','txt','rtf','odt','wps','html','htm','mhtml','mht'] },
        { key: 'sheet', label: t.kbCatSheet, exts: ['xls','xlsx','csv','ods','et'] },
        { key: 'ppt', label: t.kbCatPpt, exts: ['ppt','pptx','odp','dps'] },
        { key: 'pdf', label: t.kbCatPdf, exts: ['pdf'] },
        { key: 'img', label: t.kbCatImg, exts: ['png','jpg','jpeg','gif','webp','bmp','svg','heic'] },
        { key: 'zip', label: t.kbCatZip, exts: ['zip','rar','7z','tar','gz'] },
      ];
      const extCountMap = React.useMemo(() => { const m = {}; types.forEach((x) => { m[x.ext] = x.count; }); return m; }, [types]);
      const catCount = (c) => {
        if (!c.exts) return stats ? stats.totalFiles : 0;
        return c.exts.reduce((s, e) => s + (extCountMap[e] || 0), 0);
      };

      const refreshL0 = useCallback(async () => {
        // 三个查询并行(原顺序 await 累加延迟);拉完更新缓存 + loaded,供 remount 秒显。
        const [s, st, ty] = await Promise.all([
          inv('kb_scan_status').catch(() => null),
          inv('kb_stats').catch(() => null),
          inv('kb_type_counts').catch(() => []),
        ]);
        if (s) setScan(s);
        if (st) setStats(st);
        setTypes(ty || []);
        kbCache.scan = s || kbCache.scan;
        kbCache.stats = st || kbCache.stats;
        kbCache.types = ty || kbCache.types;
        kbCache.loaded = true;
        setLoaded(true);
      }, []);
      useEffect(() => { refreshL0(); }, []);
      useEffect(() => {
        if (!scan || !scan.running) return;
        // 扫描中:既刷统计(类型卡数字增长),也增量重查文件表——文件随扫描逐渐冒出来,
        // 不再"顶部扫描中却下面说没有文件"。cat/query 进依赖,让闭包取当前筛选/搜索值。
        const id = setInterval(() => { refreshL0(); runSearch(cat, query); }, 1500);
        return () => clearInterval(id);
      }, [scan ? scan.running : false, cat, query]);

      const runSearch = async (catKey, text) => {
        const c = CATS.find((x) => x.key === catKey) || CATS[0];
        const q = { text: text || null, limit: 200 };
        if (c.exts) q.exts = c.exts;
        try { setResults(await inv('kb_search', { query: q }) || []); }
        catch (e) { setResults([]); }
        setSearched(true);
      };
      useEffect(() => { if (sub === 'files') runSearch(cat, query); }, [cat, sub]);

      const startScan = async () => { try { setScan(await inv('kb_start_scan', { roots: null })); } catch (e) {} };
      useEffect(() => {
        if (scan && !scan.running && scan.phase === 'done') { refreshL0(); runSearch(cat, query); }
      }, [scan ? scan.running : false]);

      const scanning = !!(scan && scan.running);
      const total = stats ? stats.totalFiles : 0;
      // 加 loaded:首次拉完前不判"还没建立索引"(避免把"加载中"误显示成空状态)。
      const neverScanned = loaded && !scanning && total === 0;

      // 懒触发增量扫:进入文件管理页时,库非空且距上次扫描超冷却期才扫一次(先用缓存秒显、
      // 扫完刷新)。不进页=零扫描;库空(新用户)走空状态手动首扫,不在这里自动全盘扫。
      // 全盘索引可能覆盖数十万文件，5 分钟冷却会让用户频繁切页时反复触发重 I/O。
      // 自动刷新降为 6 小时一次；需要立即同步时仍可点页面里的手动扫描。
      const AUTOSCAN_COOLDOWN = 6 * 60 * 60;
      useEffect(() => {
        if (sub !== 'files' || !loaded || scanning || total === 0) return;
        const last = scan && scan.finishedAt ? scan.finishedAt : 0;
        if (Math.floor(Date.now() / 1000) - last > AUTOSCAN_COOLDOWN) startScan();
      }, [loaded, sub]);

      // ================= 知识库 (L1) =================
      const [colls, setColls] = useState(kbCache.colls);
      const [activeColl, setActiveColl] = useState(null);
      const [docs, setDocs] = useState([]);
      const [allDocs, setAllDocs] = useState(kbCache.allDocs);
      const [idx, setIdx] = useState(null);
      const [newColl, setNewColl] = useState(null);
      const [delColl, setDelColl] = useState(null); // 待删除知识集(二次确认),null=无
      const [confirmDoc, setConfirmDoc] = useState(null); // 行内二次确认中的文档 id,null=无
      const [embedInfo, setEmbedInfo] = useState(kbCache.embedInfo);
      const [kbModel, setKbModel] = useState(kbCache.model); // embedding 模型部署状态(null=未知)
      const [kbCat, setKbCat] = useState('all'); // 知识库分类筛选 tab

      const loadDocs = async (cid) => { try { setDocs(await inv('kb_documents', { collectionId: cid, limit: 0 }) || []); } catch (e) {} };
      const loadColls = useCallback(async () => {
        try { const c = await inv('kb_collection_list') || []; setColls(c); kbCache.colls = c; } catch (e) {}
        try { const d = await inv('kb_documents', { collectionId: 0, limit: 0 }) || []; setAllDocs(d); kbCache.allDocs = d; } catch (e) {}
        try { const ei = await inv('kb_embed_info'); setEmbedInfo(ei); kbCache.embedInfo = ei; } catch (e) {}
        try { const m = await inv('kb_model_status'); setKbModel(m); kbCache.model = m; } catch (e) {}
      }, []);
      useEffect(() => { loadColls(); }, []); // 挂载即加载,文件管理「加入知识库」浮层也要用
      useEffect(() => { if (sub === 'kb') loadColls(); }, [sub]);

      // ── embedding 模型 gate(未装则知识库页显下载引导,装好热加载免重启)──
      const modelInstalled = kbModel == null ? true : !!kbModel.installed; // 未知时不闪 gate(mock/旧后端)
      const kbm = (bs && bs.kbModelSetup) || {};
      const dlProg = kbm.progress || null;
      const downloading = !!kbm.downloading;
      const mb = (n) => Math.round((n || 0) / 1048576);
      // 进度百分比:download 阶段用真实 downloaded/total(占 0~95%),校验/解压/完成递进到 100。
      const dlPct = (() => {
        if (!dlProg) return 0;
        if (dlProg.stage === 'download') return dlProg.total > 0 ? Math.min(95, Math.floor(dlProg.downloaded / dlProg.total * 95)) : 0;
        if (dlProg.stage === 'verify') return 96;
        if (dlProg.stage === 'extract') return 98;
        if (dlProg.stage === 'done') return 100;
        return 0;
      })();
      const dlStageLabel = !dlProg ? t.kbModelStageDownload
        : dlProg.stage === 'verify' ? t.kbModelStageVerify
        : dlProg.stage === 'extract' ? t.kbModelStageExtract
        : dlProg.stage === 'done' ? t.kbModelStageDone
        : t.kbModelStageDownload;
      const startModelDownload = async () => {
        if (!canInstallKbModel) return;
        try {
          const st = await bridge.knowledge.downloadKbModel();
          if (st) { setKbModel(st); kbCache.model = st; }
          loadColls(); // 模型就绪后刷新语义徽标/列表
        } catch (e) {}
      };
      // 用户恰好在首帧后台加载期间进入知识库时，模型就绪后刷新语义状态徽标。
      useEffect(() => {
        if (kbm.startupReady) loadColls();
      }, [kbm.startupReady, loadColls]);

      const indexing = !!(idx && idx.running);
      useEffect(() => {
        if (!indexing) return;
        const id = setInterval(async () => {
          try {
            const s = await inv('kb_index_status'); setIdx(s);
            if (!s.running) { loadColls(); if (activeColl) loadDocs(activeColl.id); }
          } catch (e) {}
        }, 1000);
        return () => clearInterval(id);
      }, [indexing]);

      // newColl 带 id=编辑(改名/改分类),否则新建。编辑时透传原 description(后端 UPDATE 会覆盖该列)。
      const createColl = async () => {
        if (!newColl || !newColl.name.trim()) return;
        const name = newColl.name.trim(), category = (newColl.category || '').trim() || null;
        try {
          if (newColl.id) {
            await inv('kb_collection_update', { id: newColl.id, name, category, description: newColl.description ?? null });
            if (activeColl && activeColl.id === newColl.id) setActiveColl({ ...activeColl, name, category });
          } else {
            await inv('kb_collection_create', { name, category, description: null });
          }
        } catch (e) {}
        setNewColl(null); loadColls();
      };
      const deleteColl = async (id) => {
        try { await inv('kb_collection_delete', { id }); } catch (e) {}
        if (activeColl && activeColl.id === id) setActiveColl(null);
        loadColls();
      };
      // 点知识库卡片=就地聚焦该集(再点同卡/「全部」取消),下方文件表随之切换。不再跳二级详情页。
      const openColl = (c) => { if (activeColl && activeColl.id === c.id) setActiveColl(null); else { setActiveColl(c); loadDocs(c.id); } };
      const addSources = async (cid) => {
        let paths = [];
        try { paths = (bridge && bridge.files.pickFiles) ? await bridge.files.pickFiles() : []; } catch (e) { paths = []; }
        if (!paths || !paths.length) return;
        try { setIdx(await inv('kb_collection_add_sources', { collectionId: cid, paths })); } catch (e) {}
      };
      // 知识库页底部入口：选文件 → 单知识集直接加；多个/无则走「加入知识库」浮层选择。
      const dzPick = async () => {
        let paths = [];
        try { paths = (bridge && bridge.files.pickFiles) ? await bridge.files.pickFiles() : []; } catch (e) { paths = []; }
        if (!paths || !paths.length) return;
        if (colls.length === 1) { try { setIdx(await inv('kb_collection_add_sources', { collectionId: colls[0].id, paths })); } catch (e) {} }
        else { setAddToKb(paths); }
      };
      const StatusPill = ({ s }) => {
        const map = { ready: ['●', t.kbStReady, isDark ? 'text-[#7DD3A8]' : 'text-[#18a957]'], indexing: ['◐', t.kbStIndexing, isDark ? 'text-[#A8C7FA]' : 'text-[#0B57D0]'], pending: ['○', t.kbStPending, isDark ? 'text-[#E8C468]' : 'text-[#c98a00]'] };
        const v = map[s] || map.ready;
        return <span className={`text-[12px] font-medium ${v[2]}`}>{v[0]} {v[1]}</span>;
      };
      const docStatusLabel = (d) => d.parseStatus === 'parsed' ? `${d.nChunks} ${t.kbBlocks}` : (d.parseStatus === 'skipped' ? t.kbSkipped : (d.parseStatus === 'pending' ? t.kbStPending : d.parseStatus));

      const SubTab = ({ k, label, count }) => (
        <button onClick={() => setSub(k)}
          className={`flex items-center gap-2 px-1 pb-3 text-[15px] font-bold border-b-2 -mb-[1px] transition-colors ${sub === k ? (isDark ? 'border-[#A8C7FA] ' + ink : 'border-[#0B57D0] ' + ink) : `border-transparent ${muted}`}`}>
          {label}{count != null && <span className={`text-[11px] px-2 py-0.5 rounded-md ${sub === k ? accent : card}`}>{count}</span>}
        </button>
      );

      return (
        <div className="flex-1 flex flex-col w-full h-full relative z-10 animate-in fade-in duration-300">
          {/* Header */}
          <div className="w-full max-w-7xl mx-auto px-4 sm:px-6 md:px-10 pt-12 pb-3">
            <h1 className="text-[32px] font-normal tracking-tight mb-1.5">{t.kbPageTitle}</h1>
            <p className={`text-[14px] ${muted}`}>{sub === 'files' ? t.kbFilesSub : sub === 'kb' ? t.kbKbSub : t.kbOutSub}</p>
          </div>
          {/* Sub-tabs */}
          <div className="w-full max-w-7xl mx-auto px-4 sm:px-6 md:px-10 flex items-center gap-7 border-b border-gray-400/15">
            <SubTab k="output" label={t.kbSubOutput} count={outputs.length || null} />
            <SubTab k="files" label={t.kbSubFiles} count={total ? total.toLocaleString() : null} />
            <SubTab k="kb" label={t.kbSubKb} count={modelInstalled ? (colls.length || null) : null} />
          </div>

          <div className="flex-1 overflow-y-auto custom-scrollbar py-6">

            {/* ============ 文件管理 ============ */}
            {sub === 'files' && (
              <div className="max-w-7xl mx-auto px-4 sm:px-6 md:px-10">
                {!loaded ? (
                  // 加载骨架:页面壳即时呈现(搜索栏+真实类型卡,数字/文件用灰条占位),数据 async 填,
                  // 避免整页空白死等 refreshL0(大库冷读时尤其明显)。loaded 后切真实数据,结构一致很平滑。
                  <div>
                    <div className="flex items-center gap-3 mb-5">
                      <div className={`flex-1 flex items-center gap-3 px-5 py-3 rounded-full ${card}`}>
                        <Search size={18} className={muted} />
                        <span className={`text-[15px] ${isDark ? 'text-[#C4C7C5]' : 'text-[#444746]'}`}>{t.kbSearchPlaceholder}</span>
                      </div>
                    </div>
                    <div className={`text-[15px] font-bold mb-3 ${ink}`}>{t.kbBrowseByType}</div>
                    <div className="grid grid-cols-4 lg:grid-cols-7 gap-3 mb-7">
                      {CATS.map((c) => { const col = CAT_COLOR[c.key] || '#8a8a9a'; const CatI = CAT_ICON[c.key] || FileText; return (
                        <div key={c.key} className={`flex items-center gap-3 p-3 rounded-xl ${panel}`} style={panelShadow}>
                          <div className="w-9 h-9 rounded-xl grid place-items-center shrink-0" style={{ background: col + (isDark ? '33' : '1f'), color: col }}><CatI size={17} /></div>
                          <div className="min-w-0">
                            <div className={`text-[13px] font-bold truncate ${ink}`}>{c.label}</div>
                            <div className={`h-3 w-10 rounded mt-1.5 animate-pulse ${isDark ? 'bg-white/10' : 'bg-black/[0.07]'}`} />
                          </div>
                        </div>
                      );})}
                    </div>
                    <div className={`text-[15px] font-bold mb-3 ${ink}`}>{t.kbAllFiles}</div>
                    <div className={`rounded-2xl overflow-hidden ${panel}`} style={panelShadow}>
                      {Array.from({ length: 6 }).map((_, i) => (
                        <div key={i} className="flex items-center gap-3 px-5 py-3 border-b border-gray-400/10 last:border-0">
                          <div className={`w-7 h-7 rounded-lg shrink-0 animate-pulse ${isDark ? 'bg-white/10' : 'bg-black/[0.07]'}`} />
                          <div className={`flex-1 h-3 rounded animate-pulse ${isDark ? 'bg-white/10' : 'bg-black/[0.07]'}`} style={{ maxWidth: `${60 - i * 6}%` }} />
                        </div>
                      ))}
                    </div>
                  </div>
                ) : neverScanned ? (
                  <div className={`text-center py-20 ${muted}`}>
                    <p className="text-[15px] mb-4">{t.kbEmptyHint}</p>
                    <button onClick={startScan} className={`px-5 py-2.5 rounded-full text-[14px] font-medium ${accent}`}>{t.kbScanNow}</button>
                  </div>
                ) : (
                  <div>
                    <div className="flex items-center gap-3 mb-5">
                      <div className={`flex-1 flex items-center gap-3 px-5 py-3 rounded-full ${card}`}>
                        <Search size={18} className={muted} />
                        <input type="text" value={query} placeholder={t.kbSearchPlaceholder}
                          onChange={(e) => setQuery(e.target.value)} onKeyDown={(e) => { if (e.key === 'Enter') runSearch(cat, query); }}
                          className={`flex-1 bg-transparent border-none outline-none text-[15px] ${isDark ? 'placeholder:text-[#C4C7C5]' : 'placeholder:text-[#444746]'}`} />
                      </div>
                      {/* 自动维护(档3)已让库自保最新,重扫退化成兜底图标钮:闲时干净贴设计稿,
                          扫描中自动展开显示进度。 */}
                      <button onClick={startScan} disabled={scanning} title={t.kbRescan}
                        className={`shrink-0 flex items-center gap-2 rounded-full transition-colors ${scanning ? 'px-4 py-2.5 cursor-default' : 'p-2.5'} ${soft}`}>
                        <RefreshCw size={16} className={scanning ? 'animate-spin' : ''} />
                        {scanning && <span className="text-[12px] font-medium">{t.kbScanning} {(scan.scanned || 0).toLocaleString()}</span>}
                      </button>
                    </div>

                    <div>
                        {/* 按类型浏览 — 彩色类型卡(每类独立图标 + 白卡) */}
                        <div className="flex items-baseline justify-between mb-3">
                          <div className={`text-[15px] font-bold ${ink}`}>{t.kbBrowseByType}</div>
                          <div className={`text-[12px] ${muted}`}>{t.kbMonitored}{scan && scan.roots && scan.roots.length ? ` · ${t.kbMonitoredDirs.replace('{n}', scan.roots.length)}` : ''}</div>
                        </div>
                        <div className="grid grid-cols-4 lg:grid-cols-7 gap-3 mb-7">
                          {CATS.map((c) => {
                            const col = CAT_COLOR[c.key] || '#8a8a9a';
                            const on = cat === c.key;
                            const CatI = CAT_ICON[c.key] || FileText;
                            return (
                            <button key={c.key} onClick={() => setCat(c.key)}
                              className={`flex items-center gap-3 p-3 rounded-xl text-left transition-all ${panel} ${panelHover}`}
                              style={on ? { borderColor: col, boxShadow: `${isDark ? '' : '0 1px 2px rgba(24,24,40,.04), '}0 0 0 3px ${col}1f` } : panelShadow}>
                              <div className="w-9 h-9 rounded-xl grid place-items-center shrink-0" style={{ background: col + (isDark ? '33' : '1f'), color: col }}><CatI size={17} /></div>
                              <div className="min-w-0">
                                <div className={`text-[13px] font-bold truncate ${ink}`}>{c.label}</div>
                                <div className={`text-[11px] ${muted}`}>{catCount(c).toLocaleString()}{t.kbItemUnit}</div>
                              </div>
                            </button>
                          );})}
                        </div>

                        {/* 最近文件 — 卡片(L0 无打开次数,用最近修改的前 4 个) */}
                        {!query && results.length > 0 && (
                          <div className="mb-7">
                            <div className={`text-[15px] font-bold mb-3 ${ink}`}>{t.kbRecentFiles}</div>
                            <div className="grid grid-cols-2 lg:grid-cols-4 gap-3">
                              {results.slice(0, 4).map((f) => { const e = extOf(f); const col = extColor(e); return (
                                <div key={f.path} className={`p-3 rounded-2xl ${panel} ${panelHover} transition-all`} style={panelShadow}>
                                  <LocalFilePreview f={f} onOpen={isWeb ? null : () => setOutputPreview({ path: f.path, sessionId: null })} />
                                  <div className={`text-[13px] font-bold truncate ${ink}`} title={f.name}>{f.name}</div>
                                  <div className={`text-[11px] truncate mt-1 ${muted}`}>{f.path}</div>
                                  <div className="flex items-center justify-between mt-3 pt-3 border-t border-gray-400/10">
                                    <span className={`text-[11px] ${muted}`}>{fmtDate(f.mtime)}</span>
                                    {canOpenSystemFiles && <button onClick={() => openFile(f.path)} className={`px-2.5 py-1 rounded-lg text-[11px] font-bold ${soft}`}>{t.kbOpen}</button>}
                                  </div>
                                </div>
                              );})}
                            </div>
                          </div>
                        )}

                        {/* 全部文件 — 标准表格 */}
                        <div className="flex items-baseline justify-between mb-3">
                          <div className={`text-[15px] font-bold ${ink}`}>{query ? `${t.kbResults} · ${results.length}` : t.kbAllFiles}</div>
                          <div className={`text-[12px] ${muted}`}>{t.kbSortByModified}</div>
                        </div>
                        {searched && results.length === 0 ? (
                          <div className={`text-center py-16 ${muted} text-[14px]`}>{scanning ? `${t.kbScanningHint} ${(scan.scanned || 0).toLocaleString()}` : t.kbNoResults}</div>
                        ) : (
                          <div className={`rounded-2xl overflow-hidden ${panel}`} style={panelShadow}>
                            <div className={`flex items-center gap-3 px-5 py-3 text-[11.5px] font-semibold ${muted} border-b border-gray-400/15 ${isDark ? 'bg-white/5' : 'bg-[#fbfbfd]'}`}>
                              <span className="flex-1 min-w-0">{t.kbColName}</span>
                              <span className="w-[26%] hidden lg:block">{t.kbColLoc}</span>
                              <span className="w-20 text-right">{t.kbColSize}</span>
                              <span className="w-28 text-right">{t.kbColTime}</span>
                              <span className="w-24"></span>
                            </div>
                            {results.map((f) => { const e = extOf(f); const col = extColor(e); return (
                              <div key={f.path} className={`group flex items-center gap-3 px-5 py-2.5 border-b border-gray-400/10 last:border-0 ${cardHover} transition-colors`}>
                                <div className="flex-1 min-w-0 flex items-center gap-3">
                                  <span className="w-7 h-7 rounded-lg grid place-items-center text-[8.5px] font-extrabold text-white shrink-0" style={{ background: col }}>{extLabel(e)}</span>
                                  <span className={`text-[13px] truncate ${ink}`} title={f.name}>{f.name}</span>
                                </div>
                                <span className={`w-[26%] truncate text-[12px] hidden lg:block ${muted}`} title={f.path}>{f.path}</span>
                                <span className={`w-20 text-right text-[12px] ${muted}`}>{fmtSize(f.size)}</span>
                                <span className={`w-28 text-right text-[12px] ${muted}`}>{fmtDate(f.mtime)}</span>
                                <div className="w-24 flex items-center justify-end gap-1 opacity-0 group-hover:opacity-100 transition-opacity">
                                  <button title={t.kbAddToKb} onClick={() => setAddToKb(f.path)} className={`p-1.5 rounded-full ${iconHover}`}><Plus size={15} /></button>
                                  {canOpenSystemFiles && <button title={t.kbOpen} onClick={() => openFile(f.path)} className={`p-1.5 rounded-full ${iconHover}`}><ExternalLink size={15} /></button>}
                                  {canOpenSystemFiles && <button title={t.kbOpenFolder} onClick={() => openFolder(f.path)} className={`p-1.5 rounded-full ${iconHover}`}><BookOpen size={15} /></button>}
                                </div>
                              </div>
                            );})}
                          </div>
                        )}
                      </div>
                  </div>
                )}
              </div>
            )}

            {/* ============ 产出物 ============ */}
            {sub === 'output' && (
              <div className="max-w-7xl mx-auto px-4 sm:px-6 md:px-10">
                <div className="flex flex-col lg:flex-row lg:items-center gap-3 mb-5">
                  <div className={`flex-1 flex items-center gap-3 px-5 py-3 rounded-[16px] ${isDark ? 'bg-[#1f2124] border border-white/[0.06]' : 'bg-white border border-[#ececf1]'}`} style={panelShadow}>
                    <Search size={18} className={muted} />
                    <input type="text" value={outQuery} placeholder={t.kbOutSearchList}
                      onChange={(e) => setOutQuery(e.target.value)}
                      className={`flex-1 bg-transparent border-none outline-none text-[15px] ${isDark ? 'placeholder:text-[#C4C7C5]' : 'placeholder:text-[#444746]'}`} />
                  </div>
                  <div className={`h-[52px] inline-flex items-center gap-1 p-1 rounded-[16px] border ${isDark ? 'bg-[#1f2124] border-white/[0.06]' : 'bg-white border-[#ececf1]'}`} style={panelShadow}>
                    <button title={t.kbOutGallery} onClick={() => setOutView('grid')}
                      className={`w-10 h-10 rounded-xl grid place-items-center transition-colors ${outView === 'grid' ? (isDark ? 'bg-[#2a3952] text-[#79aaff]' : 'bg-[#e9f1ff] text-[#2f6beb]') : muted}`}>
                      <GridIcon size={18} />
                    </button>
                    <button title={t.kbOutList} onClick={() => setOutView('list')}
                      className={`w-10 h-10 rounded-xl grid place-items-center transition-colors ${outView === 'list' ? (isDark ? 'bg-[#2a3952] text-[#79aaff]' : 'bg-[#e9f1ff] text-[#2f6beb]') : muted}`}>
                      <IconList size={18} />
                    </button>
                  </div>
                </div>

                {!outputsLoaded ? (
                  <div className={`rounded-2xl overflow-hidden ${panel}`} style={panelShadow}>
                    {Array.from({ length: 6 }).map((_, i) => (
                      <div key={i} className="flex items-center gap-3 px-5 py-3 border-b border-gray-400/10 last:border-0">
                        <div className={`w-8 h-8 rounded-lg shrink-0 animate-pulse ${isDark ? 'bg-white/10' : 'bg-black/[0.07]'}`} />
                        <div className={`flex-1 h-3 rounded animate-pulse ${isDark ? 'bg-white/10' : 'bg-black/[0.07]'}`} style={{ maxWidth: `${70 - i * 7}%` }} />
                      </div>
                    ))}
                  </div>
                ) : outputs.length === 0 ? (
                  <div className={`text-center py-20 ${muted}`}>
                    <div className={`w-14 h-14 mx-auto rounded-2xl grid place-items-center mb-4 ${card}`}><Archive size={24} /></div>
                    <p className={`text-[15px] font-bold mb-1 ${ink}`}>{t.kbOutEmpty}</p>
                    <p className="text-[13px]">{t.kbOutEmptyHint}</p>
                  </div>
                ) : (() => {
                  const activeOutputs = outView === 'list' ? filteredOutputs : queryOutputs;
                  if (outView === 'grid' && activeOutputs.length === 0) return (
                    <div className={`text-center py-20 ${muted}`}>
                      <div className={`w-14 h-14 mx-auto rounded-2xl grid place-items-center mb-4 ${card}`}><Archive size={24} /></div>
                      <p className={`text-[15px] font-bold mb-1 ${ink}`}>{t.kbOutEmpty}</p>
                      <p className="text-[13px]">{t.kbOutEmptyHint}</p>
                    </div>
                  );
                  const sections = groupOutputs(activeOutputs).filter((x) => x.rows.length > 0);
                  const OutputActions = ({ o, compact }) => (
                    <div className={`flex items-center gap-2 ${compact ? 'justify-end' : 'mt-4'}`}>
                      <button onClick={() => continueOutput(o)} className={`h-8 px-3 rounded-full text-[12.5px] font-bold ${isDark ? 'bg-[#283650] text-[#8db7ff]' : 'bg-[#e9f1ff] text-[#2f6beb]'}`}>{t.kbOutContinue}</button>
                      <button onClick={() => newOutputProject(o)} className={`h-8 px-3 rounded-full text-[12.5px] font-bold ${isDark ? 'bg-white/[0.075] text-[#dfe3e9]' : 'bg-[#f2f4f8] text-[#444746]'}`}>{t.kbOutNewProject}</button>
                      {canOpenSystemFiles && <button title={t.kbOutOpenFolder} onClick={() => openFolder(o.path)} className={`w-8 h-8 rounded-lg grid place-items-center ${iconHover}`}><FolderOpen size={16} /></button>}
                      {isWeb && canDownloadArtifacts && <button title="下载产物" onClick={() => bridge.artifacts.downloadArtifact(o.path, o.sessionId || o.session_id)} className={`w-8 h-8 rounded-lg grid place-items-center ${iconHover}`}><Download size={16} /></button>}
                    </div>
                  );
                  return (
                    <div>
                      <div className="flex items-baseline justify-between mb-3">
                        <div className={`text-[15px] font-bold ${ink}`}>{outView === 'grid' ? t.kbOutSort : t.kbOutCount(activeOutputs.length)} · {t.kbOutCount(activeOutputs.length)}</div>
                        <div className={`text-[12px] ${muted}`}>{t.kbOutSort}</div>
                      </div>

                      {outView === 'list' && (
                        <div className="grid grid-cols-2 md:grid-cols-3 xl:grid-cols-5 gap-3 mb-5">
                          {OUTPUT_CATS.map((c) => {
                            const on = outCat === c.key;
                            const CIcon = c.icon || FileText;
                            return (
                              <button key={c.key} onClick={() => setOutCat(c.key)}
                                className={`relative h-[72px] flex items-center gap-3 rounded-[14px] px-4 text-left border overflow-hidden transition-all ${isDark ? 'bg-[#1f2124] border-white/[0.09] hover:border-white/[0.16]' : 'bg-white border-[#ececf1] hover:border-[#dfe4f5]'}`}
                                style={on ? { borderColor: c.color + 'aa', boxShadow: `0 0 0 2px ${c.color}22` } : panelShadow}>
                                <span className="w-[38px] h-[38px] rounded-[13px] grid place-items-center shrink-0" style={{ color: c.color, background: c.color + (isDark ? '2b' : '1a') }}><CIcon size={18} /></span>
                                <span className="min-w-0">
                                  <b className={`block text-[14px] font-extrabold truncate ${ink}`}>{c.label}</b>
                                  <em className={`not-italic block text-[12px] font-semibold mt-1 ${muted}`}>{outputCount(c.key)} 个</em>
                                </span>
                              </button>
                            );
                          })}
                        </div>
                      )}

                      {outView === 'grid' ? (
                        <div className="space-y-8">
                          {sections.map(({ key, label, rows }) => (
                            <div key={key}>
                              <div className="flex items-center gap-3 mb-3">
                                <span className={`text-[20px] font-extrabold ${ink}`}>{label}</span>
                                <small className={`text-[13px] font-semibold ${muted}`}>{t.kbOutGroupCount(rows.length)}</small>
                                <span className="h-px flex-1 bg-gradient-to-r from-gray-400/20 to-transparent" />
                              </div>
                              <div className="grid grid-cols-1 lg:grid-cols-2 xl:grid-cols-3 gap-[18px]">
                                {rows.map((o) => {
                                  const meta = outCatMeta(o.category);
                                  return (
                                    <article key={o.path} className={`min-h-[288px] rounded-[18px] overflow-hidden border transition-all ${isDark ? 'bg-[linear-gradient(180deg,rgba(36,38,42,.92),rgba(28,30,33,.92))] border-white/[0.08] hover:border-[#79aaff]/30' : 'bg-white border-[#ececf1] hover:border-[#b9cdf6]'}`} style={panelShadow}>
                                      <OutputLivePreview o={o} onOpen={() => setOutputPreview({ path: o.path, sessionId: o.sessionId || o.session_id || null })} />
                                      <div className="px-5 pb-[18px]">
                                        <div className="flex items-start gap-2 mt-1">
                                          <div className={`text-[17px] leading-snug font-extrabold flex-1 min-w-0 truncate ${ink}`} title={o.name}>{o.name}</div>
                                          <span className="h-6 px-2.5 rounded-full inline-flex items-center text-[11.5px] font-extrabold" style={{ color: meta.color, background: meta.color + (isDark ? '22' : '16') }}>{String(o.ext || '').toUpperCase().slice(0, 4)}</span>
                                        </div>
                                        <div className={`flex items-center gap-2 text-[13px] mt-2 ${muted}`}><span>{fmtOutputDate(o.mtime)}</span><i className="w-1 h-1 rounded-full bg-current opacity-40"></i><span className="truncate">{o.source || t.kbSubOutput}</span></div>
                                        <OutputActions o={o} />
                                      </div>
                                    </article>
                                  );
                                })}
                              </div>
                            </div>
                          ))}
                        </div>
                      ) : (
                        <div className={`rounded-[18px] overflow-hidden border ${isDark ? 'bg-[#1d1f22]/80 border-white/[0.075]' : 'bg-white border-[#ececf1]'}`} style={panelShadow}>
                          <div className={`hidden lg:grid grid-cols-[minmax(0,1.9fr)_150px_150px_260px] gap-4 px-5 h-12 items-center text-[12px] font-extrabold ${muted} border-b border-gray-400/15 ${isDark ? 'bg-white/[0.015]' : 'bg-[#fbfbfd]'}`}>
                            <span>{t.kbOutColName}</span><span>{t.kbOutColTime}</span><span>{t.kbOutColSource}</span><span className="text-right">{t.kbOutColActions}</span>
                          </div>
                          {activeOutputs.length === 0 && (
                            <div className={`text-center py-14 ${muted}`}>
                              <div className={`w-12 h-12 mx-auto rounded-2xl grid place-items-center mb-3 ${card}`}><Archive size={20} /></div>
                              <p className={`text-[14px] font-bold mb-3 ${ink}`}>{t.kbNoResults || t.kbOutEmpty}</p>
                              <button onClick={() => { setOutCat('all'); setOutQuery(''); }} className={`px-4 py-2 rounded-full text-[13px] font-bold ${soft}`}>{t.kbOutCatAll}</button>
                            </div>
                          )}
                          {activeOutputs.map((o) => {
                            const meta = outCatMeta(o.category);
                            return (
                              <div key={o.path} className={`grid grid-cols-1 lg:grid-cols-[minmax(0,1.9fr)_150px_150px_260px] gap-3 lg:gap-4 px-5 py-4 lg:min-h-[76px] items-center border-b border-gray-400/10 last:border-0 ${cardHover}`}>
                                <div className="flex items-center gap-3 min-w-0">
                                  <span className="w-10 h-10 rounded-[13px] grid place-items-center shrink-0 text-[10.5px] font-black border border-white/[0.06]" style={{ color: meta.color, background: meta.color + (isDark ? '24' : '16') }}>{String(o.ext || '').toUpperCase().slice(0, 4) || meta.label}</span>
                                  <div className="min-w-0">
                                    <strong className={`block text-[15px] truncate ${ink}`} title={o.name}>{o.name}</strong>
                                    <span className={`block text-[12px] truncate mt-1 ${muted}`} title={o.path}>{outputDesc(o)}</span>
                                  </div>
                                </div>
                                <span className={`text-[12.5px] ${muted}`}>{fmtOutputDate(o.mtime)}</span>
                                <span className={`text-[12.5px] truncate ${muted}`} title={o.source || ''}>{o.source || '—'}</span>
                                <OutputActions o={o} compact />
                              </div>
                            );
                          })}
                        </div>
                      )}
                    </div>
                  );
                })()}
              </div>
            )}
            {outputPreview && <FilePreviewModal path={outputPreview.path} sessionId={outputPreview.sessionId} theme={theme} onClose={() => setOutputPreview(null)} />}

            {/* ============ 知识库 · 未装 embedding 模型 → gate ============ */}
            {sub === 'kb' && !modelInstalled && (
              <div className="max-w-[560px] mx-auto text-center pt-8 pb-2">
                <div className="w-[84px] h-[84px] mx-auto rounded-[24px] grid place-items-center relative"
                  style={{ background: isDark ? 'linear-gradient(135deg,#2A2440,#1E2438)' : 'linear-gradient(135deg,#efeafe,#e3ecfb)' }}>
                  <Database size={40} className="text-[#6f5cf0]" />
                  <span className="absolute -right-1.5 -bottom-1.5 w-[30px] h-[30px] rounded-full grid place-items-center"
                    style={{ background: 'linear-gradient(135deg,#6f5cf0,#5b6cf2)', border: `3px solid ${isDark ? '#131314' : '#fff'}`, boxShadow: '0 4px 10px rgba(108,92,231,.35)' }}>
                    <Download size={14} className="text-white" />
                  </span>
                </div>
                <h2 className={`mt-5 text-[20px] font-extrabold ${ink}`}>{t.kbModelTitle}</h2>
                <p className={`mt-2.5 mx-auto max-w-[450px] text-[13.5px] leading-relaxed ${muted}`}>{t.kbModelDesc}</p>

                <div className={`mt-5 mx-auto max-w-[480px] text-left rounded-2xl p-[18px] ${panel}`} style={panelShadow}>
                  <div className="flex items-center gap-3">
                    <div className="w-[46px] h-[46px] rounded-xl grid place-items-center shrink-0"
                      style={{ background: isDark ? '#2A2440' : '#f0eefb', color: '#6f5cf0' }}><Package size={23} /></div>
                    <div className="flex-1 min-w-0">
                      <div className={`text-[14.5px] font-extrabold ${ink}`}>{t.kbModelPkgName}</div>
                      <div className={`text-[12px] mt-0.5 ${muted}`}>{t.kbModelPkgSub}</div>
                    </div>
                    <span className="text-[11.5px] font-bold px-2.5 py-1 rounded-lg shrink-0"
                      style={{ color: '#6f5cf0', background: isDark ? '#2A2440' : '#efeafe' }}>{(kbModel && kbModel.version) || 'bge-m3'}</span>
                  </div>
                  <div className="flex flex-wrap gap-2 mt-3.5">
                    {[
                      t.kbModelChipDownload.replace('{n}', mb(kbModel && kbModel.sizeBytes) || 545),
                      t.kbModelChipInstalled.replace('{n}', mb(kbModel && kbModel.installedBytes) || 560),
                      t.kbModelChipOffline, t.kbModelChipLang,
                    ].map((c, i) => (
                      <span key={i} className={`text-[11.5px] px-2.5 py-1 rounded-lg ${isDark ? 'bg-white/5 text-[#C4C7C5]' : 'bg-[#f4f5f8] text-[#5a5a66]'}`}>{c}</span>
                    ))}
                  </div>
                  <div className="mt-3.5 pt-3.5 border-t border-gray-400/15 flex flex-col gap-2.5">
                    {[t.kbModelItem1, t.kbModelItem2, t.kbModelItem3].map((it, i) => (
                      <div key={i} className={`flex items-center gap-2.5 text-[12.5px] ${isDark ? 'text-[#C4C7C5]' : 'text-[#56565f]'}`}>
                        <Check size={15} className="text-[#18a957] shrink-0" />{it}
                      </div>
                    ))}
                  </div>
                </div>

                {!canInstallKbModel ? (
                  <div className={`mt-5 rounded-xl px-4 py-3 text-[13px] leading-relaxed ${isDark ? 'bg-[#1E2B3A] text-[#A8C7FA]' : 'bg-[#E8F0FE] text-[#174EA6]'}`}>
                    知识库模型尚未安装，请先在桌面端完成安装；安装后刷新此页面即可使用。
                  </div>
                ) : !downloading ? (
                  <div className="mt-5">
                    <button onClick={startModelDownload}
                      className="px-5 py-2.5 rounded-xl text-[14px] font-bold text-white"
                      style={{ background: 'linear-gradient(135deg,#6f5cf0,#5b6cf2)', boxShadow: '0 6px 16px rgba(108,92,231,.32)' }}>
                      {t.kbModelDownloadBtn} →
                    </button>
                    <div className={`mt-3 text-[12px] ${muted}`}>{t.kbModelFoot}</div>
                    {kbm.error && <div className="mt-2 text-[12px] text-[#d63a3a]">{kbm.error}</div>}
                  </div>
                ) : (
                  <div className="mt-5 max-w-[480px] mx-auto">
                    <div className={`h-2 rounded-full overflow-hidden ${isDark ? 'bg-white/10' : 'bg-[#edf0fa]'}`}>
                      <div className="h-full rounded-full transition-all" style={{ width: dlPct + '%', background: 'linear-gradient(90deg,#5b6cf2,#2f8bff)' }} />
                    </div>
                    <div className="flex justify-between mt-2.5 text-[12.5px]">
                      <span className={`font-semibold ${isDark ? 'text-[#A8C7FA]' : 'text-[#2f6beb]'}`}>{dlStageLabel}</span>
                      <span className="font-bold text-[#2f6beb]">{dlPct}%</span>
                    </div>
                  </div>
                )}
              </div>
            )}

            {/* ============ 知识库 列表（模型已就绪）============ */}
            {sub === 'kb' && modelInstalled && (
              <div className="max-w-7xl mx-auto px-4 sm:px-6 md:px-10">
                <div className={`rounded-3xl p-7 mb-6 flex items-center gap-6 ${isDark ? 'bg-gradient-to-br from-[#2A2440] to-[#1E2438]' : 'bg-gradient-to-br from-[#ece8fc] to-[#dcebfb]'}`}>
                  <div className="flex-1 min-w-0">
                    <h2 className={`text-[20px] font-bold mb-3 ${isDark ? 'text-[#E3E3E3]' : 'text-[#211f33]'}`}>{t.kbBannerTitle}</h2>
                    <button onClick={() => setNewColl({ name: '', category: '' })} className="px-5 py-2.5 rounded-xl text-[14px] font-bold text-white" style={{ background: 'linear-gradient(135deg,#6f5cf0,#5b6cf2)' }}>{t.kbNewColl} →</button>
                    <div className="flex gap-2 mt-4 flex-wrap">
                      {[t.kbStep1, t.kbStep2, t.kbStep3].map((s, i) => (
                        <span key={i} className={`text-[12px] px-3 py-1.5 rounded-full ${isDark ? 'bg-white/10 text-[#C4C7C5]' : 'bg-white/70 text-[#54506b]'}`}><b className="text-[#6c5ce7]">{i + 1}</b> {s}</span>
                      ))}
                    </div>
                  </div>
                  <div className="hidden xl:flex gap-3 shrink-0">
                    {['#3f7bf0', '#7b5fe6', '#1aa07a', '#d6873e', '#d6589a'].map((c, i) => (
                      <div key={i} className={`w-16 h-20 rounded-2xl grid place-items-center shadow-sm ${isDark ? 'bg-white/10' : 'bg-white/70'}`}>
                        <div className="w-9 h-9 rounded-xl grid place-items-center" style={{ background: c + '22', color: c }}><FileText size={18} /></div>
                      </div>
                    ))}
                  </div>
                </div>

                {/* 语义检索状态 */}
                <div className="flex items-center gap-2 mb-5 text-[12px]">
                  <span className={`px-3 py-1 rounded-full ${embedInfo && embedInfo.enabled ? (isDark ? 'bg-[#13361f] text-[#7DD3A8]' : 'bg-[#e6f6ec] text-[#18a957]') : `${card} ${muted}`}`}>
                    {embedInfo && embedInfo.enabled ? `${t.kbEmbedOn}（${embedInfo.model}）` : t.kbEmbedOff}
                  </span>
                </div>

                {colls.length === 0 ? (
                  <div className={`text-center py-16 ${muted} text-[14px]`}>{t.kbNoColls}</div>
                ) : (() => {
                  const cats = ['all', ...Array.from(new Set(colls.map((c) => c.category).filter(Boolean)))];
                  const shown = colls.filter((c) => kbCat === 'all' || c.category === kbCat);
                  return (
                  <div className="mb-8">
                    {cats.length > 1 && (
                      <div className="flex items-center gap-2 flex-wrap mb-4">
                        {cats.map((ct) => (
                          <button key={ct} onClick={() => setKbCat(ct)}
                            className={`px-4 py-1.5 rounded-full text-[13px] font-medium transition-colors ${kbCat === ct ? accent : `${card} ${muted}`}`}>
                            {ct === 'all' ? t.kbCatAll : ct}
                          </button>
                        ))}
                      </div>
                    )}
                    <div className="flex items-baseline justify-between mb-3">
                      <div className={`text-[15px] font-bold ${ink}`}>{t.kbMyColls}</div>
                      <div className={`text-[12px] ${muted}`}>{colls.length} {t.kbCollUnit} · {colls.reduce((s, c) => s + (c.docCount || 0), 0)} {t.kbDocs} · {fmtSize(colls.reduce((s, c) => s + (c.totalBytes || 0), 0))}</div>
                    </div>
                    <div className="grid grid-cols-1 gap-4 sm:grid-cols-2 xl:grid-cols-3">
                      {shown.map((c) => {
                        const prog = (idx && idx.running && idx.collectionId === c.id && idx.total > 0) ? Math.round((idx.done / idx.total) * 100) : null;
                        const isIdx = c.status === 'indexing' || prog != null;
                        return (
                        <div key={c.id} onClick={() => openColl(c)} className={`p-4 rounded-2xl cursor-pointer transition-all ${panel} ${panelHover}`}
                          style={activeColl && activeColl.id === c.id ? { borderColor: collColor(c), boxShadow: `${isDark ? '' : '0 1px 2px rgba(24,24,40,.04), '}0 0 0 2px ${collColor(c)}55` } : panelShadow}>
                          <div className="flex items-start gap-3">
                            <div className="w-11 h-11 rounded-xl grid place-items-center shrink-0" style={{ background: collColor(c) + (isDark ? '33' : '1f'), color: collColor(c) }}><BookOpen size={20} /></div>
                            <div className="flex-1 min-w-0">
                              <div className={`text-[15px] font-bold truncate ${ink}`}>{c.name}</div>
                              <div className={`text-[12px] ${muted}`}>{c.category || t.kbUncat}</div>
                            </div>
                            {activeColl && activeColl.id === c.id && <Check size={18} style={{ color: collColor(c) }} className="shrink-0" />}
                          </div>
                          {c.description && <div className={`text-[12px] mt-3 line-clamp-2 ${muted}`}>{c.description}</div>}
                          {isIdx && (
                            <div className="mt-3">
                              <div className="h-1.5 rounded-full overflow-hidden" style={{ background: isDark ? '#2A2B2D' : '#edf0fa' }}>
                                <div className="h-full rounded-full transition-all" style={{ width: (prog != null ? prog : 40) + '%', background: 'linear-gradient(90deg,#5b6cf2,#2f8bff)' }} />
                              </div>
                              {prog != null && <div className="text-[11px] mt-1" style={{ color: isDark ? '#A8C7FA' : '#0B57D0' }}>{t.kbIndexing} {prog}%</div>}
                            </div>
                          )}
                          <div className="flex items-center justify-between mt-4 pt-3 border-t border-gray-400/15">
                            <span className={`text-[12px] ${muted}`}><b className={isDark ? 'text-[#C4C7C5]' : 'text-[#54545f]'}>{c.docCount}</b> {t.kbDocs} · {fmtSize(c.totalBytes)}</span>
                            <StatusPill s={c.status} />
                          </div>
                        </div>
                      );})}
                    </div>
                  </div>
                  );
                })()}

                {colls.length > 0 && (
                  <div>
                    {/* 知识库内文件:未选库=跨库总表(带所属列);点卡片聚焦某库=该库文件 + 加文件/删库。 */}
                    <div className="flex items-center justify-between gap-3 mb-3 min-h-[36px]">
                      <div className="flex items-center gap-2 min-w-0">
                        <div className={`text-[15px] font-bold ${ink}`}>{t.kbCollFiles}</div>
                        {activeColl && <>
                          <span className={`text-[14px] truncate ${muted}`}>· {activeColl.name}</span>
                          <button onClick={() => setActiveColl(null)} className={`shrink-0 px-2.5 py-0.5 rounded-full text-[12px] ${card} ${muted}`}>{t.kbAllColls}</button>
                        </>}
                      </div>
                      {activeColl && <div className="flex items-center gap-2 shrink-0">
                        {canPickHostFiles && <button onClick={() => addSources(activeColl.id)} disabled={indexing} className={`flex items-center gap-2 px-4 py-2 rounded-full text-[13px] font-medium ${indexing ? 'opacity-60 cursor-default' : ''} ${soft}`}>
                          {indexing ? <RefreshCw size={14} className="animate-spin" /> : <Plus size={14} />}
                          {indexing ? `${t.kbIndexing} ${idx.done}/${idx.total}` : t.kbAddFiles}
                        </button>}
                        <button title={t.kbEditColl} onClick={() => setNewColl({ id: activeColl.id, name: activeColl.name, category: activeColl.category || '', description: activeColl.description ?? null })} className={`p-2 rounded-full ${iconHover}`}><Edit2 size={15} /></button>
                        <button title={t.kbDeleteColl} onClick={() => setDelColl(activeColl)} className={`p-2 rounded-full ${iconHover}`}><Trash2 size={15} /></button>
                      </div>}
                    </div>
                    {(() => {
                      const rows = activeColl ? docs : allDocs;
                      if (rows.length === 0) return <div className={`text-center py-12 ${muted} text-[14px]`}>{activeColl ? t.kbCollEmpty : t.kbNoCollFiles}</div>;
                      return (
                      <div className={`rounded-2xl overflow-hidden ${panel}`} style={panelShadow}>
                        <div className={`flex items-center gap-3 px-5 py-3 text-[11.5px] font-semibold ${muted} border-b border-gray-400/15 ${isDark ? 'bg-white/5' : 'bg-[#fbfbfd]'}`}>
                          <span className="flex-1 min-w-0">{t.kbColName}</span>
                          {!activeColl && <span className="w-[24%]">{t.kbColColl}</span>}
                          <span className="w-24 text-right">{t.kbStatus}</span>
                          <span className="w-16"></span>
                        </div>
                        {rows.map((d) => { const e = extOf(d); const col = extColor(e); return (
                          <div key={d.id} className={`group flex items-center gap-3 px-5 py-2.5 border-b border-gray-400/10 last:border-0 ${cardHover}`}>
                            <div className="flex-1 min-w-0 flex items-center gap-3">
                              <span className="w-7 h-7 rounded-lg grid place-items-center text-[8.5px] font-extrabold text-white shrink-0" style={{ background: col }}>{extLabel(e)}</span>
                              <span className={`text-[13px] truncate ${ink}`} title={d.name}>{d.name}</span>
                            </div>
                            {!activeColl && <span className={`w-[24%] min-w-0 flex items-center gap-2 text-[12px] ${muted}`}>
                              <span className="w-2 h-2 rounded-full shrink-0" style={{ background: collColor({ category: d.collName, name: d.collName }) }}></span>
                              <span className="truncate">{d.collName}</span>
                            </span>}
                            <span className={`w-24 text-right text-[12px] ${muted}`}>{docStatusLabel(d)}</span>
                            {confirmDoc === d.id ? (
                              <div className="flex items-center justify-end gap-1.5 shrink-0" onClick={(e) => e.stopPropagation()}>
                                <span className="text-[12px] font-medium" style={{ color: '#d63a3a' }}>{t.kbRemoveQ}</span>
                                <button title={t.kbRemove} onClick={async () => { setConfirmDoc(null); await inv('kb_remove_document', { docId: d.id }); if (activeColl) loadDocs(activeColl.id); loadColls(); }} className={`p-1 rounded-full ${iconHover}`} style={{ color: '#d63a3a' }}><Check size={15} /></button>
                                <button title={t.kbCancel} onClick={() => setConfirmDoc(null)} className={`p-1 rounded-full ${iconHover} ${muted}`}><X size={15} /></button>
                              </div>
                            ) : (
                              <div className="w-16 flex items-center justify-end gap-1 opacity-0 group-hover:opacity-100 transition-opacity">
                                {canOpenSystemFiles && <button title={t.kbOpen} onClick={() => openFile(d.path)} className={`p-1.5 rounded-full ${iconHover}`}><ExternalLink size={14} /></button>}
                                <button title={t.kbRemove} onClick={() => setConfirmDoc(d.id)} className={`p-1.5 rounded-full ${iconHover}`}><Trash2 size={14} /></button>
                              </div>
                            )}
                          </div>
                        );})}
                      </div>
                      );
                    })()}
                  </div>
                )}

                {/* 加入知识库入口：点击选文件(单知识集直接加，多个弹选择) */}
                {canPickHostFiles && <div onClick={dzPick}
                  className={`mt-5 flex items-center justify-center gap-2 px-4 py-5 rounded-2xl border border-dashed cursor-pointer transition-colors ${isDark ? 'border-[#444746] hover:border-[#A8C7FA] text-[#C4C7C5]' : 'border-[#d4d8e2] hover:border-[#0B57D0] text-[#444746]'}`}>
                  <Plus size={16} className={isDark ? 'text-[#A8C7FA]' : 'text-[#0B57D0]'} />
                  <span className="text-[13px]">{t.kbAddToKb}</span>
                </div>}
              </div>
            )}

          </div>

          {/* 删除知识集 二次确认(删库连同所有文档+索引,不可恢复) */}
          {delColl && (
            <div className="absolute inset-0 z-50 flex items-center justify-center bg-black/40 p-3 sm:p-4" onClick={() => setDelColl(null)}>
              <div onClick={(e) => e.stopPropagation()} className={`w-full max-w-[400px] max-h-full overflow-y-auto rounded-2xl p-5 sm:p-6 ${isDark ? 'bg-[#1E1F20]' : 'bg-white'}`}>
                <div className={`flex items-center gap-2 text-[16px] font-bold mb-2 ${ink}`}>
                  <AlertTriangle size={18} style={{ color: '#d63a3a' }} />
                  {t.kbDelCollConfirm.replace('{n}', delColl.name)}
                </div>
                <div className={`text-[13px] leading-relaxed mb-5 ${muted}`}>{t.kbDelCollWarn.replace('{c}', delColl.docCount || 0)}</div>
                <div className="flex justify-end gap-2">
                  <button onClick={() => setDelColl(null)} className={`px-4 py-2 rounded-full text-[13px] ${card} ${muted}`}>{t.kbCancel}</button>
                  <button onClick={() => { deleteColl(delColl.id); setDelColl(null); }} className="px-4 py-2 rounded-full text-[13px] font-medium text-white" style={{ background: '#d63a3a' }}>{t.kbDelete}</button>
                </div>
              </div>
            </div>
          )}

          {/* 新建知识集 modal */}
          {newColl && (
            <div className="absolute inset-0 z-50 flex items-center justify-center bg-black/40 p-3 sm:p-4" onClick={() => setNewColl(null)}>
              <div onClick={(e) => e.stopPropagation()} className={`w-full max-w-[400px] max-h-full overflow-y-auto rounded-2xl p-5 sm:p-6 ${isDark ? 'bg-[#1E1F20]' : 'bg-white'}`}>
                <div className={`text-[17px] font-bold mb-4 ${ink}`}>{newColl.id ? t.kbEditColl : t.kbNewColl}</div>
                <input autoFocus value={newColl.name} placeholder={t.kbCollNamePh} onChange={(e) => setNewColl({ ...newColl, name: e.target.value })} onKeyDown={(e) => { if (e.key === 'Enter') createColl(); }}
                  className={`w-full px-4 py-2.5 rounded-xl mb-3 text-[14px] outline-none ${isDark ? 'bg-[#2A2B2D] text-[#E3E3E3]' : 'bg-[#F0F4F9] text-[#1F1F1F]'}`} />
                <input value={newColl.category} placeholder={t.kbCollCatPh} onChange={(e) => setNewColl({ ...newColl, category: e.target.value })}
                  className={`w-full px-4 py-2.5 rounded-xl mb-4 text-[14px] outline-none ${isDark ? 'bg-[#2A2B2D] text-[#E3E3E3]' : 'bg-[#F0F4F9] text-[#1F1F1F]'}`} />
                <div className="flex justify-end gap-2">
                  <button onClick={() => setNewColl(null)} className={`px-4 py-2 rounded-full text-[13px] ${card} ${muted}`}>{t.kbCancel}</button>
                  <button onClick={createColl} className={`px-4 py-2 rounded-full text-[13px] font-medium ${accent}`}>{newColl.id ? t.kbSave : t.kbCreate}</button>
                </div>
              </div>
            </div>
          )}

          {/* 加入知识库 浮层 */}
          {addToKb && (
            <div className="absolute inset-0 z-50 flex items-center justify-center bg-black/40 p-3 sm:p-4" onClick={() => setAddToKb(null)}>
              <div onClick={(e) => e.stopPropagation()} className={`w-full max-w-[380px] max-h-full overflow-y-auto rounded-2xl p-5 sm:p-6 ${isDark ? 'bg-[#1E1F20]' : 'bg-white'}`}>
                <div className={`text-[16px] font-bold mb-1 ${ink}`}>{t.kbAddToKb}</div>
                <div className={`text-[12px] mb-4 truncate ${muted}`}>{Array.isArray(addToKb) ? `${addToKb.length} ${t.kbDocs}` : addToKb}</div>
                {colls.length === 0 ? (
                  <div className={`text-[13px] mb-4 ${muted}`}>{t.kbNoCollsShort}</div>
                ) : (
                  <div className="flex flex-col gap-1 mb-4 max-h-[240px] overflow-y-auto">
                    {colls.map((c) => (
                      <button key={c.id} onClick={async () => { try { setIdx(await inv('kb_collection_add_sources', { collectionId: c.id, paths: Array.isArray(addToKb) ? addToKb : [addToKb] })); } catch (e) {} setAddToKb(null); setSub('kb'); }}
                        className={`text-left px-4 py-2.5 rounded-xl text-[14px] ${card} ${iconHover} ${ink}`}>{c.name}</button>
                    ))}
                  </div>
                )}
                <button onClick={() => { const p = addToKb; setAddToKb(null); setSub('kb'); setNewColl({ name: '', category: '' }); }} className={`w-full px-4 py-2.5 rounded-xl text-[13px] font-medium ${soft}`}>+ {t.kbNewColl}</button>
              </div>
            </div>
          )}
        </div>
      );
    };


    // ==========================================
    // Monitor View (Material 3 Style)
    // ==========================================
    // 长按确认清除按钮（hold-to-confirm，防误触）：按住 850ms 进度填满才执行，
    // 松手 / 移开 / 失焦即取消；执行时图标转一圈、变绿「已清除」，900ms 后复位。
    // 鼠标 / 触摸 / 键盘(空格·回车)均支持。数字归零动画由父级 onClear 负责。

export { kbCache, KnowledgeView };
