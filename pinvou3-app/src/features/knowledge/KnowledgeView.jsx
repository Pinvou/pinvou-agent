import React, { useCallback, useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { AlertTriangle, AppWindow, Archive, BookOpen, Check, ChevronDown, Database, Download, Edit2, ExternalLink, FileText, FolderOpen, GridIcon, IconList, ImageIcon, Package, Plus, PresentationIcon, RefreshCw, TableIcon, Trash2, X } from '../../components/icons.jsx';
import { IosSearchField, IosSegmentedControl } from '../../components/IosControls.jsx';
import { bridge, useBridgeState } from '../../hooks/useBridge.js';
import { OFFICE_HTML_STYLE } from '../artifacts/ArtifactsPanel.jsx';
import { FilePreviewModal } from '../artifacts/FilePreviewModal.jsx';
import { RemoteKnowledgeView } from '../remote-knowledge/RemoteKnowledgeView.jsx';
import { invokeTauri } from '../../platform/tauri/client.js';
import { resolveAppAssetUrl } from '../../shared/asset-url.mjs';
import { can, isWeb } from '../../shared/platform.js';
import { isImeComposing } from '../../shared/ime-guard.mjs';

let kbCache = { scan: null, stats: null, types: [], loaded: false, colls: [], allDocs: [], embedInfo: null, model: null, outputs: [], outputsLoaded: false };

const MODEL_PROGRESS_RADIUS = 31;
const MODEL_PROGRESS_CIRCUMFERENCE = 2 * Math.PI * MODEL_PROGRESS_RADIUS;

function ModelProgressIndicator({ downloading, percent, label }) {
  if (!downloading) {
    return (
      <div className="flex flex-col items-center gap-2.5" role="status" aria-live="polite">
        <div className="relative h-[76px] w-[76px]" aria-hidden="true">
          <svg className="h-full w-full animate-spin motion-reduce:animate-none" viewBox="0 0 76 76">
            <circle cx="38" cy="38" r={MODEL_PROGRESS_RADIUS} fill="none" stroke="currentColor" strokeWidth="6" className="text-[#edf0fa] dark:text-white/10" />
            <circle cx="38" cy="38" r={MODEL_PROGRESS_RADIUS} fill="none" stroke="currentColor" strokeWidth="6" strokeLinecap="round" strokeDasharray="48 147" className="text-[#5b6cf2]" />
          </svg>
        </div>
        <span className="text-[12.5px] font-semibold text-[#2f6beb] dark:text-[#A8C7FA]">{label}</span>
      </div>
    );
  }

  const progressOffset = MODEL_PROGRESS_CIRCUMFERENCE * (1 - percent / 100);
  return (
    <div className="flex flex-col items-center gap-2.5" role="status" aria-live="polite">
      <div
        className="relative h-[76px] w-[76px]"
        role="progressbar"
        aria-label={label}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={percent}
        aria-valuetext={`${percent}%`}
      >
        <svg className="h-full w-full -rotate-90" viewBox="0 0 76 76" aria-hidden="true">
          <circle cx="38" cy="38" r={MODEL_PROGRESS_RADIUS} fill="none" stroke="currentColor" strokeWidth="6" className="text-[#edf0fa] dark:text-white/10" />
          <circle
            cx="38"
            cy="38"
            r={MODEL_PROGRESS_RADIUS}
            fill="none"
            stroke="currentColor"
            strokeWidth="6"
            strokeLinecap="round"
            strokeDasharray={MODEL_PROGRESS_CIRCUMFERENCE}
            strokeDashoffset={progressOffset}
            className="text-[#5b6cf2] transition-[stroke-dashoffset] duration-300 motion-reduce:transition-none"
          />
        </svg>
        <span className="absolute inset-0 flex items-center justify-center text-[17px] font-extrabold tabular-nums text-[#2f6beb] dark:text-[#A8C7FA]">
          {percent}<span className="ml-0.5 text-[10px] font-bold">%</span>
        </span>
      </div>
      <span className="text-[12.5px] font-semibold text-[#2f6beb] dark:text-[#A8C7FA]">{label}</span>
    </div>
  );
}

    const KnowledgeView = ({ theme, t, mode }) => {
      // mode='outputs' 时作为一级「产出物」视图独立渲染:固定 output 段,隐藏段切换,显示自己的标题。
      const outputsOnly = mode === 'outputs';
      const isDark = theme === 'dark';
      const bs = useBridgeState(['knowledge', 'chat']); // 取知识模型进度和当前产物
      const inv = invokeTauri;
      const canDownloadArtifacts = !isWeb || can('artifactDownload');
      const canPickHostFiles = !isWeb || can('hostFilePicker');
      const canOpenSystemFiles = !isWeb && can('externalSystemOpen');
      const canInstallKbModel = can('localModelSetup') && can('dependencyInstall');

      const [sub, setSub] = useState(outputsOnly ? 'output' : 'kb'); // 'output' | 'files' | 'kb' | 'remote'；统一知识库入口默认落本地知识库

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
      const muted = 'text-[#444746] dark:text-[#C4C7C5]';
      const card = 'bg-[#F0F4F9] dark:bg-[#1E1F20]';
      const cardHover = 'hover:bg-[#F0F4F9] dark:hover:bg-[#1E1F20]';
      const iconHover = 'hover:bg-[#E1E5EA] dark:hover:bg-[#333537]';
      const accent = 'bg-[#0B57D0] text-white dark:bg-[#A8C7FA] dark:text-[#062E6F]';
      const soft = 'bg-[#F0F4F9] hover:bg-[#E1E5EA] text-[#0B57D0] dark:bg-[#1E1F20] dark:hover:bg-[#333537] dark:text-[#A8C7FA]';
      const ink = 'text-[#1F1F1F] dark:text-[#E3E3E3]';
      // 设计稿白卡:白底 + 细边框 + 柔阴影(本页文件/知识库卡片专用,区别于全局灰底 card)
      const panel = 'bg-white border border-[#ececf1] dark:bg-[#1E1F20] dark:border-white/10';
      const panelHover = 'hover:border-[#dfe4f5] dark:hover:border-white/20';
      // P2 残留:inline-style boxShadow(boxShadow 含 rgba 非纯色,规则 C/D 留 P2);仍依赖 isDark。
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
      const [outView, setOutView] = useState('list');
      const [outSortDir, setOutSortDir] = useState('desc');
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
      const FILE_ICON_BY_EXT = {
        html: '/file-icons/html.svg', htm: '/file-icons/html.svg', mhtml: '/file-icons/html.svg', mht: '/file-icons/html.svg',
        xml: '/file-icons/xml.svg', json: '/file-icons/code.svg', js: '/file-icons/code.svg', jsx: '/file-icons/code.svg', ts: '/file-icons/code.svg', tsx: '/file-icons/code.svg', css: '/file-icons/code.svg',
        xls: '/file-icons/xlsx.svg', xlsx: '/file-icons/xlsx.svg', csv: '/file-icons/csv.svg', ods: '/file-icons/xlsx.svg', et: '/file-icons/xlsx.svg',
        ppt: '/file-icons/pptx.svg', pptx: '/file-icons/pptx.svg', odp: '/file-icons/pptx.svg', dps: '/file-icons/pptx.svg',
        doc: '/file-icons/docx.svg', docx: '/file-icons/docx.svg', md: '/file-icons/txt.svg', txt: '/file-icons/txt.svg', rtf: '/file-icons/docx.svg', odt: '/file-icons/docx.svg', wps: '/file-icons/docx.svg',
        pdf: '/file-icons/pdf.svg',
        png: '/file-icons/photo.svg', jpg: '/file-icons/photo.svg', jpeg: '/file-icons/photo.svg', gif: '/file-icons/photo.svg', webp: '/file-icons/photo.svg', bmp: '/file-icons/photo.svg', svg: '/file-icons/photo.svg', heic: '/file-icons/photo.svg', fig: '/file-icons/photo.svg',
        zip: '/file-icons/zip.svg', rar: '/file-icons/zip.svg', '7z': '/file-icons/zip.svg', tar: '/file-icons/zip.svg', gz: '/file-icons/zip.svg',
      };
      const FILE_ICON_BY_CAT = { web: '/file-icons/html.svg', doc: '/file-icons/docx.svg', img: '/file-icons/photo.svg', ppt: '/file-icons/pptx.svg' };
      const fileIconSrc = (ext, category) => resolveAppAssetUrl(
        FILE_ICON_BY_EXT[String(ext || '').toLowerCase()]
          || FILE_ICON_BY_CAT[category]
          || '/file-icons/genericfile.svg',
      );
      const OutputFileIcon = ({ meta, ext, category }) => {
        const lowerExt = String(ext || '').toLowerCase();
        const code = lowerExt.toUpperCase().slice(0, 4) || meta.label;
        const isImageIcon = category === 'img' || ['png', 'jpg', 'jpeg', 'gif', 'webp', 'bmp', 'svg', 'heic', 'fig'].includes(lowerExt);
        return (
          <span
            className="grid h-12 w-12 shrink-0 place-items-center overflow-hidden text-[10px] font-semibold tracking-[0.02em]"
            style={{ color: meta.color }}
          >
            <img src={fileIconSrc(ext, category)} alt="" className={`${isImageIcon ? 'h-9 w-9' : 'h-10 w-10'} object-contain`} draggable={false} />
            <span className="sr-only">{code}</span>
          </span>
        );
      };
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
      const sortedFilteredOutputs = React.useMemo(() => {
        const dir = outSortDir === 'asc' ? 1 : -1;
        return [...filteredOutputs].sort((a, b) => {
          const byTime = ((a.mtime || 0) - (b.mtime || 0)) * dir;
          if (byTime) return byTime;
          return String(a.name || '').localeCompare(String(b.name || ''));
        });
      }, [filteredOutputs, outSortDir]);
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
          bridge.chat.prefillComposer(`${t.kbOutContinuePrefill(o.name)}\n\n${t.uiKnowledge.filePathLabel}${o.path}\n\n${t.kbOutRequirementLabel}`);
        }
      };
      const newOutputProject = async (o) => {
        if (bridge && bridge.sessions.createNewSession) await bridge.sessions.createNewSession();
        if (bridge && bridge.chat.prefillComposer) {
          bridge.chat.prefillComposer(`${t.kbOutContinuePrefill(o.name)}\n\n${t.uiKnowledge.filePathLabel}${o.path}\n\n${t.kbOutRequirementLabel}`);
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
                next = { kind: 'image', url: await bridge.artifacts.readArtifactImageB64(o.path, outputSessionId) };
              } else if (ext === 'pptx' && bridge.artifacts.readArtifactThumbnail) {
                const thumb = await bridge.artifacts.readArtifactThumbnail(o.path, outputSessionId);
                next = thumb ? { kind: 'image', url: thumb } : null;
              }
              if (!next && (o.category === 'web' || ext === 'html' || ext === 'htm') && bridge.artifacts.readArtifactText) {
                next = { kind: 'html', html: await bridge.artifacts.readArtifactText(o.path, outputSessionId) };
              }
              if (!next && ['docx', 'doc', 'odt', 'rtf'].includes(ext) && bridge.artifacts.renderArtifactVisual) {
                const visual = await bridge.artifacts.renderArtifactVisual(o.path, outputSessionId);
                if (visual && visual.mode === 'html' && visual.html) {
                  next = { kind: 'officeHtml', html: visual.html + OFFICE_HTML_STYLE };
                }
              }
              if (!next && ['md', 'markdown', 'txt', 'csv', 'json', 'log'].includes(ext) && bridge.artifacts.readArtifactText) {
                const text = await bridge.artifacts.readArtifactText(o.path, outputSessionId);
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

        const htmlPreviewDoc = (html) => '<style>html,body{overflow:hidden!important;}*{animation-duration:.001s!important;scrollbar-width:none!important;}*::-webkit-scrollbar{display:none!important;}</style>' + (html || '');
        const officePreviewDoc = (html) => '<style>html,body{background:#fff!important;margin:0;color:#111!important;overflow:hidden!important;}*{animation-duration:.001s!important;scrollbar-width:none!important;}*::-webkit-scrollbar{display:none!important;}</style>' + (html || '');
        const shell = (children) => (
          <div ref={boxRef} onClick={onOpen} role={onOpen ? 'button' : undefined} tabIndex={onOpen ? 0 : undefined}
            onKeyDown={(e) => { if (onOpen && (e.key === 'Enter' || e.key === ' ')) { e.preventDefault(); onOpen(); } }}
            className={`h-[164px] m-2 rounded-[15px] overflow-hidden relative bg-[#111216] ring-1 ring-white/[0.045] ${onOpen ? 'cursor-pointer' : ''}`}>
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
            <iframe title={o.name} sandbox="allow-same-origin" scrolling="no" srcDoc={htmlPreviewDoc(pv.html)} onLoad={() => setTimeout(() => setFrameReady(true), 80)}
              className={`absolute inset-0 w-[200%] h-[200%] origin-top-left scale-50 bg-[#15171a] pointer-events-none border-0 transition-opacity duration-300 ${frameReady ? 'opacity-100' : 'opacity-0'}`}
              style={{ colorScheme: 'dark' }} />
          </>
        );
        if (pv.kind === 'officeHtml') return shell(
          <>
            {!frameReady && <div className="absolute inset-0 bg-white"></div>}
            <iframe title={o.name} sandbox="allow-same-origin" scrolling="no" srcDoc={officePreviewDoc(pv.html)} onLoad={() => setTimeout(() => setFrameReady(true), 80)}
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
          // 本机知识文件不是 Session 产物；Web 端不读取任意主机路径。
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
          <div ref={boxRef} onClick={onOpen} role="button" tabIndex={0}
            onKeyDown={(e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); onOpen(); } }}
            className="h-[126px] rounded-[14px] overflow-hidden relative border border-white/[0.06] bg-[#15171a] cursor-pointer mb-3">
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
      const [fileSortDir, setFileSortDir] = useState('desc');
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
      const sortedResults = React.useMemo(() => {
        const dir = fileSortDir === 'asc' ? 1 : -1;
        return [...results].sort((a, b) => {
          const byTime = ((a.mtime || 0) - (b.mtime || 0)) * dir;
          if (byTime) return byTime;
          return String(a.name || '').localeCompare(String(b.name || ''));
        });
      }, [results, fileSortDir]);

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
      useEffect(() => {
        if (!outputsOnly) refreshL0();
      }, [outputsOnly, refreshL0]);
      useEffect(() => {
        if (outputsOnly || !scan || !scan.running) return;
        // 扫描中:既刷统计(类型卡数字增长),也增量重查文件表——文件随扫描逐渐冒出来,
        // 不再"顶部扫描中却下面说没有文件"。cat/query 进依赖,让闭包取当前筛选/搜索值。
        const id = setInterval(() => { refreshL0(); runSearch(cat, query); }, 1500);
        return () => clearInterval(id);
      }, [outputsOnly, scan ? scan.running : false, cat, query]);

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
        if (!outputsOnly && scan && !scan.running && scan.phase === 'done') { refreshL0(); runSearch(cat, query); }
      }, [outputsOnly, scan ? scan.running : false]);

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
      const activeCollRef = useRef(activeColl);
      useEffect(() => { activeCollRef.current = activeColl; }, [activeColl]);
      const [docs, setDocs] = useState([]);
      const [allDocs, setAllDocs] = useState(kbCache.allDocs);
      const [idx, setIdx] = useState(null);
      const [importError, setImportError] = useState('');
      const [failedFilesLoading, setFailedFilesLoading] = useState(false);
      const failedPaginationRef = useRef({ jobId: null, generation: 0, initialized: false, nextOffset: 0 });
      const replaceIndexState = useCallback((next) => {
        const generation = failedPaginationRef.current.generation + 1;
        failedPaginationRef.current = {
          jobId: next && next.jobId ? next.jobId : null,
          generation,
          initialized: false,
          nextOffset: 0,
        };
        setFailedFilesLoading(false);
        setIdx(next);
      }, []);
      const invalidateFailedPagination = useCallback(() => {
        const current = failedPaginationRef.current;
        failedPaginationRef.current = {
          jobId: current.jobId,
          generation: current.generation + 1,
          initialized: false,
          nextOffset: 0,
        };
        setFailedFilesLoading(false);
      }, []);
      const [newColl, setNewColl] = useState(null);
      const [delColl, setDelColl] = useState(null); // 待删除知识集(二次确认),null=无
      const [confirmDoc, setConfirmDoc] = useState(null); // 待从知识库移除的文档,null=无
      const [removeDocError, setRemoveDocError] = useState('');
      const [embedInfo, setEmbedInfo] = useState(kbCache.embedInfo);
      const [kbModel, setKbModel] = useState(kbCache.model); // embedding 模型部署状态(null=未知)
      const [kbCat, setKbCat] = useState('all'); // 知识库分类筛选 tab

      const loadDocs = async (cid) => { try { setDocs(await inv('kb_documents', { collectionId: cid, limit: 0 }) || []); } catch (e) {} };
      const loadColls = useCallback(async () => {
        try {
          const c = await inv('kb_collection_list') || [];
          setColls(c);
          setActiveColl((current) => (current ? c.find((item) => item.id === current.id) || null : null));
          kbCache.colls = c;
        } catch (e) {}
        try { const d = await inv('kb_documents', { collectionId: 0, limit: 0 }) || []; setAllDocs(d); kbCache.allDocs = d; } catch (e) {}
        try { const ei = await inv('kb_embed_info'); setEmbedInfo(ei); kbCache.embedInfo = ei; } catch (e) {}
        try { const m = await inv('kb_model_status'); setKbModel(m); kbCache.model = m; } catch (e) {}
        try { replaceIndexState(await inv('kb_index_status')); } catch (e) {}
      }, [replaceIndexState]);
      // 本地文件与本地知识库两个分区依赖本机知识集数据；远程分区自行加载服务器数据。
      // 一级「产出物」只读产出物索引，不应触发任何知识库查询。
      useEffect(() => {
        if (!outputsOnly && (sub === 'files' || sub === 'kb')) loadColls();
      }, [outputsOnly, sub, loadColls]);

      const kbm = (bs && bs.kbModelSetup) || {};
      // ── embedding 模型 gate：区分磁盘文件与当前进程真实可用状态。旧后端未返回
      // ready 时保持兼容；新后端加载失败则给出重试加载和原子修复入口。
      const effectiveKbModel = kbm.status || kbModel;
      const modelInstalled = effectiveKbModel == null ? true : !!effectiveKbModel.installed;
      const modelReadyKnown = effectiveKbModel && typeof effectiveKbModel.ready === 'boolean';
      const modelLoading = !!(kbm.startupLoading || (effectiveKbModel && effectiveKbModel.loading));
      const modelReady = !modelReadyKnown || effectiveKbModel.ready === true || kbm.startupReady === true;
      const modelFailed = modelInstalled && modelReadyKnown && !modelReady && !modelLoading;
      const modelUsable = modelInstalled && modelReady;
      const dlProg = kbm.progress || null;
      const downloading = !!kbm.downloading;
      const mb = (n) => Math.round((n || 0) / 1048576);
      // 进度百分比:download 阶段用真实累计字节(占 0~95%),校验/准备/完成递进到 100。
      const dlPct = (() => {
        if (!dlProg) return 0;
        if (dlProg.stage === 'download' || dlProg.stage === 'verify') return dlProg.total > 0 ? Math.min(95, Math.floor(dlProg.downloaded / dlProg.total * 95)) : 0;
        if (dlProg.stage === 'prepare') return 98;
        if (dlProg.stage === 'done') return 100;
        return 0;
      })();
      const dlStageLabel = !dlProg ? t.kbModelStageDownload
        : dlProg.stage === 'verify' ? t.kbModelStageVerify
        : dlProg.stage === 'prepare' ? t.kbModelStagePrepare
        : dlProg.stage === 'done' ? t.kbModelStageDone
        : t.kbModelStageDownload;
      const startModelDownload = async (repair = false) => {
        if (!canInstallKbModel) return;
        try {
          const st = await bridge.knowledge.downloadKbModel(repair);
          if (st) { setKbModel(st); kbCache.model = st; }
          loadColls(); // 模型就绪后刷新语义徽标/列表
        } catch (e) {}
      };
      // 用户恰好在首帧后台加载期间进入知识库时，模型就绪后刷新语义状态徽标。
      useEffect(() => {
        if (!outputsOnly && kbm.startupReady) loadColls();
      }, [outputsOnly, kbm.startupReady, loadColls]);

      const indexing = !!(idx && idx.running);
      useEffect(() => {
        if (!indexing) return;
        const id = setInterval(async () => {
          try {
            const s = await inv('kb_index_status'); replaceIndexState(s);
            if (!s.running) { loadColls(); if (activeColl) loadDocs(activeColl.id); }
          } catch (e) {}
        }, 1000);
        return () => clearInterval(id);
      }, [indexing, replaceIndexState]);

      const resumeImport = async () => {
        if (!idx || !idx.jobId || failedFilesLoading) return;
        invalidateFailedPagination();
        setImportError('');
        try {
          replaceIndexState(await inv('kb_index_resume', { jobId: idx.jobId }));
        } catch (e) {
          setImportError(`${t.kbResumeImportFailed}: ${String((e && e.message) || e)}`);
          try { replaceIndexState(await inv('kb_index_status')); } catch (_) {}
        }
      };
      const cancelImport = async () => {
        if (failedFilesLoading) return;
        invalidateFailedPagination();
        setImportError('');
        try {
          await inv('kb_index_cancel');
          replaceIndexState(await inv('kb_index_status'));
          loadColls();
        } catch (e) {
          setImportError(`${t.kbCancelImportFailed}: ${String((e && e.message) || e)}`);
          try { replaceIndexState(await inv('kb_index_status')); } catch (_) {}
        }
      };
      const retryImportFile = async (itemId) => {
        if (!idx || !idx.jobId || failedFilesLoading) return;
        invalidateFailedPagination();
        setImportError('');
        try {
          replaceIndexState(await inv('kb_index_retry_file', { jobId: idx.jobId, itemId }));
        } catch (e) {
          setImportError(`${t.kbRetryImportFailed}: ${String((e && e.message) || e)}`);
          try { replaceIndexState(await inv('kb_index_status')); } catch (_) {}
        }
      };
      const loadMoreFailedFiles = async () => {
        if (!idx || !idx.jobId || failedFilesLoading) return;
        setFailedFilesLoading(true);
        setImportError('');
        const pagination = failedPaginationRef.current;
        const request = {
          jobId: idx.jobId,
          generation: pagination.generation,
          initialized: pagination.initialized,
          offset: pagination.initialized ? pagination.nextOffset : 0,
        };
        if (request.jobId !== pagination.jobId || request.offset == null) {
          setFailedFilesLoading(false);
          return;
        }
        try {
          const page = await inv('kb_index_failed_files', { jobId: request.jobId, offset: request.offset, limit: 50 });
          const currentPage = failedPaginationRef.current;
          if (currentPage.jobId !== request.jobId || currentPage.generation !== request.generation) return;
          failedPaginationRef.current = {
            ...currentPage,
            initialized: true,
            nextOffset: page.nextOffset == null ? null : page.nextOffset,
          };
          setIdx((current) => {
            if (!current || current.jobId !== request.jobId) return current;
            if (!request.initialized) return { ...current, failedFiles: page.files || [] };
            const known = new Set((current.failedFiles || []).map((file) => file.itemId));
            const added = (page.files || []).filter((file) => !known.has(file.itemId));
            return { ...current, failedFiles: [...(current.failedFiles || []), ...added] };
          });
        } catch (e) {
          const currentPage = failedPaginationRef.current;
          if (currentPage.jobId === request.jobId && currentPage.generation === request.generation) {
            setImportError(`${t.kbLoadFailedFilesFailed}: ${String((e && e.message) || e)}`);
          }
        } finally {
          const currentPage = failedPaginationRef.current;
          if (currentPage.jobId === request.jobId && currentPage.generation === request.generation) {
            setFailedFilesLoading(false);
          }
        }
      };

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
      const removeDocument = async (document) => {
        setConfirmDoc(null);
        setRemoveDocError('');
        setDocs((current) => current.filter((item) => item.id !== document.id));
        setAllDocs((current) => {
          const next = current.filter((item) => item.id !== document.id);
          kbCache.allDocs = next;
          return next;
        });
        setColls((current) => current.map((collection) => (collection.id === document.collectionId ? {
          ...collection,
          docCount: Math.max(0, (collection.docCount || 0) - 1),
          chunkCount: Math.max(0, (collection.chunkCount || 0) - (document.nChunks || 0)),
          totalBytes: Math.max(0, (collection.totalBytes || 0) - (document.size || 0)),
        } : collection)));
        try {
          await inv('kb_remove_document', { docId: document.id });
        } catch (error) {
          setRemoveDocError(`${t.kbRemoveFailed}: ${String((error && error.message) || error)}`);
          const currentCollectionId = activeCollRef.current?.id;
          await Promise.all([
            loadColls(),
            currentCollectionId ? loadDocs(currentCollectionId) : Promise.resolve(),
          ]);
          return;
        }
        if (activeCollRef.current) await loadDocs(activeCollRef.current.id);
        await loadColls();
      };
      // 点知识库卡片=就地聚焦该集(再点同卡/「全部」取消),下方文件表随之切换。不再跳二级详情页。
      const openColl = (c) => { if (activeColl && activeColl.id === c.id) setActiveColl(null); else { setActiveColl(c); loadDocs(c.id); } };
      // kind='files' 走文件多选；kind='folders' 走目录选择，后端 WalkDir 递归展开。
      const doAdd = async (cid, kind) => {
        if (!canPickHostFiles || indexing) return;
        const picker = bridge && bridge.files && (kind === 'folders' ? bridge.files.pickFolders : bridge.files.pickFiles);
        if (!picker) return;
        let paths = [];
        try { paths = await picker(); } catch (e) { paths = []; }
        if (!paths || !paths.length) return;
        try { replaceIndexState(await inv('kb_collection_add_sources', { collectionId: cid, paths })); } catch (e) {}
      };
      // 知识库页底部入口：选文件/文件夹 → 单知识集直接加；多个/无则走「加入知识库」浮层选择。
      const dzPick = async (kind) => {
        if (!canPickHostFiles || indexing) return;
        const picker = bridge && bridge.files && (kind === 'folders' ? bridge.files.pickFolders : bridge.files.pickFiles);
        if (!picker) return;
        let paths = [];
        try { paths = await picker(); } catch (e) { paths = []; }
        if (!paths || !paths.length) return;
        if (colls.length === 1) { try { replaceIndexState(await inv('kb_collection_add_sources', { collectionId: colls[0].id, paths })); } catch (e) {} }
        else { setAddToKb(paths); }
      };
      // 「+ 添加 ▾」下拉菜单：文件 / 文件夹。portal 到 body 以免被 overflow-y-auto 裁剪。
      const [addMenu, setAddMenu] = useState(null); // null | {left,top,width,src}
      const openAddMenu = (src, el) => {
        const r = el.getBoundingClientRect(); const w = 188, h = 96;
        const left = Math.max(8, Math.min(r.right - w, window.innerWidth - w - 8));
        const top = (r.bottom + 6 + h > window.innerHeight) ? Math.max(8, r.top - h - 6) : Math.max(8, r.bottom + 6);
        setAddMenu({ left, top, width: w, src });
      };
      useEffect(() => {
        if (!addMenu) return;
        const close = () => setAddMenu(null);
        const esc = (e) => { if (e.key === 'Escape') { e.preventDefault(); close(); } };
        document.addEventListener('pointerdown', close);
        window.addEventListener('keydown', esc);
        window.addEventListener('resize', close);
        window.addEventListener('scroll', close, true);
        return () => { document.removeEventListener('pointerdown', close); window.removeEventListener('keydown', esc); window.removeEventListener('resize', close); window.removeEventListener('scroll', close, true); };
      }, [addMenu]);
      const chooseAdd = (kind) => { const src = addMenu && addMenu.src; setAddMenu(null); if (src === 'coll') doAdd(activeColl && activeColl.id, kind); else dzPick(kind); };
      const folderPickerAvailable = !!(bridge && bridge.files && bridge.files.pickFolders);
      const StatusPill = ({ s }) => {
        const map = { ready: ['●', t.kbStReady, 'text-[#18a957] dark:text-[#7DD3A8]'], indexing: ['◐', t.kbStIndexing, 'text-[#0B57D0] dark:text-[#A8C7FA]'], pending: ['○', t.kbStPending, 'text-[#c98a00] dark:text-[#E8C468]'] };
        const v = map[s] || map.ready;
        return <span className={`text-[12px] font-medium ${v[2]}`}>{v[0]} {v[1]}</span>;
      };
      const docStatusLabel = (d) => d.parseStatus === 'parsed' ? `${d.nChunks} ${t.kbBlocks}` : (d.parseStatus === 'skipped' ? t.kbSkipped : (d.parseStatus === 'pending' ? t.kbStPending : d.parseStatus));

      return (
        <div className="flex-1 flex flex-col w-full h-full relative z-10 overflow-y-auto p-4 custom-scrollbar animate-in fade-in duration-300 sm:p-6 lg:p-10">
          {/* Header */}
          <div className="w-full max-w-[1400px] mx-auto border-b border-slate-200/50 dark:border-white/10">
            <div className="flex flex-col gap-3 pb-6 lg:flex-row lg:items-center lg:justify-between">
              <div className="shrink-0">
                {outputsOnly ? (
                  <div>
                    <h1 className={`text-[20px] font-bold tracking-tight ${ink}`}>{t.outputs}</h1>
                    <p className={`text-[13px] mt-0.5 ${muted}`}>{t.kbOutSub}</p>
                  </div>
                ) : (
                  <IosSegmentedControl
                    value={sub}
                    onChange={setSub}
                    isDark={isDark}
                    segments={[
                      { key: 'files', label: t.kbSubFiles, count: total ? total.toLocaleString() : null },
                      { key: 'kb', label: t.kbSubKb, count: modelInstalled ? (colls.length || null) : null },
                      { key: 'remote', label: t.kbSubRemote },
                    ]}
                  />
                )}
              </div>
              <div className="flex min-w-0 flex-col gap-3 overflow-hidden lg:ml-8 lg:flex-1 lg:flex-row lg:items-center lg:justify-end">
                {sub === 'output' ? (
                  <>
                    <IosSearchField
                      value={outQuery}
                      onChange={(e) => setOutQuery(e.target.value)}
                      placeholder={t.kbOutSearchList}
                      isDark={isDark}
                      compact
                      className="w-full min-w-0 lg:max-w-[360px] lg:flex-1"
                    />
                    <IosSegmentedControl
                      value={outView}
                      onChange={setOutView}
                      isDark={isDark}
                      compact
                      segments={[
                        { key: 'grid', label: t.kbOutGallery, Icon: GridIcon },
                        { key: 'list', label: t.kbOutList, Icon: IconList },
                      ]}
                    />
                  </>
                ) : null}
                {sub === 'files' && !neverScanned ? (
                  <>
                    <IosSearchField
                      value={loaded ? query : ''}
                      placeholder={t.kbSearchPlaceholder}
                      onChange={loaded ? (e) => setQuery(e.target.value) : () => {}}
                      onKeyDown={(e) => { if (loaded && e.key === 'Enter' && !isImeComposing(e)) runSearch(cat, query); }}
                      isDark={isDark}
                      compact
                      disabled={!loaded}
                      className="w-full min-w-0 lg:max-w-[360px] lg:flex-1"
                    />
                    <button onClick={startScan} disabled={scanning || !loaded} title={t.kbRescan}
                      className={`inline-flex h-9 shrink-0 items-center rounded-full px-4 text-[13px] font-semibold shadow-sm transition-colors whitespace-nowrap ${scanning ? 'cursor-default' : ''} bg-[#E9E9EB] text-[#1D1D1F] hover:bg-[#DADADD] disabled:hover:bg-[#E9E9EB] dark:bg-[#2C2C2E] dark:text-white dark:hover:bg-[#3A3A3C] dark:disabled:hover:bg-[#2C2C2E]`}>
                      <RefreshCw size={14} className={`mr-2 opacity-70 ${scanning ? 'animate-spin' : ''}`} />
                      {scanning ? `${t.kbScanning} ${(scan.scanned || 0).toLocaleString()}` : t.kbRescan}
                    </button>
                  </>
                ) : null}
              </div>
            </div>
          </div>

          <div className="flex-1 py-6">

            {/* ============ 文件管理 ============ */}
            {sub === 'files' && (
              <div className="max-w-[1400px] mx-auto">
                {!loaded ? (
                  // 加载骨架:页面壳即时呈现(搜索栏+真实类型卡,数字/文件用灰条占位),数据 async 填,
                  // 避免整页空白死等 refreshL0(大库冷读时尤其明显)。loaded 后切真实数据,结构一致很平滑。
                  <div>
                    <div className={`text-[15px] font-bold mb-3 ${ink}`}>{t.kbBrowseByType}</div>
                    <div className="grid grid-cols-4 lg:grid-cols-7 gap-3 mb-7">
                      {CATS.map((c) => { const col = CAT_COLOR[c.key] || '#8a8a9a'; const CatI = CAT_ICON[c.key] || FileText; return (
                        <div key={c.key} className={`flex items-center gap-3 p-3 rounded-xl ${panel}`} style={panelShadow}>
                          <div className="w-9 h-9 rounded-xl grid place-items-center shrink-0" style={{ background: col + (isDark ? '33' : '1f'), color: col }}><CatI size={17} /></div> {/* isDark dynamic-value: 保留 (background 依赖运行时 col) */}
                          <div className="min-w-0">
                            <div className={`text-[13px] font-bold truncate ${ink}`}>{c.label}</div>
                            <div className={`h-3 w-10 rounded mt-1.5 animate-pulse bg-black/[0.07] dark:bg-white/10`} />
                          </div>
                        </div>
                      );})}
                    </div>
                    <div className={`text-[15px] font-bold mb-3 ${ink}`}>{t.kbAllFiles}</div>
                    <div className={`rounded-2xl overflow-hidden ${panel}`} style={panelShadow}>
                      {Array.from({ length: 6 }).map((_, i) => (
                        <div key={i} className="flex items-center gap-3 px-5 py-3 border-b border-gray-400/10 last:border-0">
                          <div className={`w-7 h-7 rounded-lg shrink-0 animate-pulse bg-black/[0.07] dark:bg-white/10`} />
                          <div className={`flex-1 h-3 rounded animate-pulse bg-black/[0.07] dark:bg-white/10`} style={{ maxWidth: `${60 - i * 6}%` }} />
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
                    <div>
                        <div className="mb-5 flex flex-col gap-3 md:flex-row md:items-center md:justify-between">
                          <div className="flex overflow-x-auto gap-2 no-scrollbar scroll-smooth">
                            {CATS.map((c) => {
                              const on = cat === c.key;
                              return (
                                <button key={c.key} onClick={() => setCat(c.key)}
                                  className={"h-7 whitespace-nowrap shrink-0 text-[13px] px-3 rounded-full font-semibold transition-colors " + (on ? 'bg-[#3A3A3C] dark:bg-[#fff] text-[#fff] dark:text-[#000]' : 'bg-[#F2F2F7] dark:bg-[#2C2C2E] text-[#000] dark:text-[#fff]')}>
                                  {c.label}
                                  <span className="ml-1.5 opacity-70">{catCount(c).toLocaleString()}</span>
                                </button>
                              );
                            })}
                          </div>
                        </div>

                        {searched && results.length === 0 ? (
                          <div className={`text-center py-16 ${muted} text-[14px]`}>{scanning ? `${t.kbScanningHint} ${(scan.scanned || 0).toLocaleString()}` : t.kbNoResults}</div>
                        ) : (
                          <div className="grid grid-cols-1">
                            <div
                              className="hidden md:grid grid-cols-[minmax(0,1fr)_100px_132px_132px] items-center gap-4 border-b pb-2 text-[12px] font-medium border-b-[rgba(198,198,200,.5)] dark:border-b-[#38383A] text-[rgba(60,60,67,.55)] dark:text-[rgba(235,235,245,.5)]"
                            >
                              <span className="text-left">{t.kbColName}</span>
                              <span className="text-right">{t.kbColSize}</span>
                              <button
                                type="button"
                                onClick={() => setFileSortDir((v) => v === 'desc' ? 'asc' : 'desc')}
                                className={`inline-flex w-fit items-center gap-1 rounded-[8px] py-1 transition-colors hover:text-[#1D1D1F] dark:hover:text-[#E5E5EA]`}
                              >
                                <span>{t.kbColTime}</span>
                                <ChevronDown size={13} className={`transition-transform ${fileSortDir === 'asc' ? 'rotate-180' : ''}`} />
                              </button>
                              <span className="text-center">{t.kbOutColActions}</span>
                            </div>
                            {sortedResults.map((f) => { const e = extOf(f); return (
                              <div key={f.path} onClick={() => setOutputPreview({ path: f.path, sessionId: null })}
                                className="group py-4 border-b cursor-pointer border-b-[rgba(198,198,200,.5)] dark:border-b-[#38383A]">
                                  <div className="grid grid-cols-[minmax(0,1fr)_auto] md:grid-cols-[minmax(0,1fr)_100px_132px_132px] items-center gap-4">
                                  <div className="flex min-w-0 items-center gap-4">
                                    <OutputFileIcon meta={{ color: extColor(e), label: extLabel(e) }} ext={e} />
                                    <div className="min-w-0">
                                      <h2 className={`text-[14px] font-semibold tracking-tight truncate mb-0.5 ${ink}`} title={f.name}>{f.name}</h2>
                                      <p className="text-[12px] truncate text-[rgba(60,60,67,.6)] dark:text-[rgba(235,235,245,.6)]">
                                        {f.path}
                                      </p>
                                    </div>
                                  </div>
                                  <span className={`hidden md:block text-right text-[12px] ${muted}`}>{fmtSize(f.size)}</span>
                                  <span className={`hidden md:block text-[12px] font-medium tabular-nums ${muted}`}>{fmtDate(f.mtime)}</span>
                                  <div className="flex shrink-0 items-center justify-end gap-1">
                                    <button title={t.kbAddToKb} onClick={(e2) => { e2.stopPropagation(); setAddToKb(f.path); if (outputsOnly) loadColls(); }}
                                      className={`grid h-8 w-8 place-items-center rounded-[9px] transition-colors active:opacity-70 text-[#3A3A3C] hover:bg-[#F2F2F7] dark:text-[#C7C7CC] dark:hover:bg-white/[0.08]`}>
                                      <Plus size={15} />
                                    </button>
                                      {canOpenSystemFiles && (
                                        <>
                                          <button onClick={(e2) => { e2.stopPropagation(); openFile(f.path); }}
                                            className={`h-8 rounded-[9px] px-2.5 text-[12px] font-medium whitespace-nowrap transition-colors active:opacity-70 text-[#007AFF] hover:bg-[#007AFF]/10 dark:text-[#0A84FF] dark:hover:bg-[#0A84FF]/10`}>
                                            {t.kbOpen}
                                          </button>
                                          <button title={t.kbOpenFolder} onClick={(e2) => { e2.stopPropagation(); openFolder(f.path); }}
                                            className={`grid h-8 w-8 place-items-center rounded-[9px] transition-colors active:opacity-70 text-[#3A3A3C] hover:bg-[#F2F2F7] dark:text-[#C7C7CC] dark:hover:bg-white/[0.08]`}>
                                            <FolderOpen size={15} />
                                          </button>
                                        </>
                                      )}
                                  </div>
                                  <div className="col-span-2 text-[12px] md:hidden text-[rgba(60,60,67,.55)] dark:text-[rgba(235,235,245,.5)]">
                                    {fmtSize(f.size)} · {fmtDate(f.mtime)}
                                  </div>
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
              <div className="max-w-[1400px] mx-auto">
                {!outputsLoaded ? (
                  <div className={`rounded-2xl overflow-hidden ${panel}`} style={panelShadow}>
                    {Array.from({ length: 6 }).map((_, i) => (
                      <div key={i} className="flex items-center gap-3 px-5 py-3 border-b border-gray-400/10 last:border-0">
                        <div className={`w-8 h-8 rounded-lg shrink-0 animate-pulse bg-black/[0.07] dark:bg-white/10`} />
                        <div className={`flex-1 h-3 rounded animate-pulse bg-black/[0.07] dark:bg-white/10`} style={{ maxWidth: `${70 - i * 7}%` }} />
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
                  const activeOutputs = outView === 'list' ? sortedFilteredOutputs : queryOutputs;
                  if (outView === 'grid' && activeOutputs.length === 0) return (
                    <div className={`text-center py-20 ${muted}`}>
                      <div className={`w-14 h-14 mx-auto rounded-2xl grid place-items-center mb-4 ${card}`}><Archive size={24} /></div>
                      <p className={`text-[15px] font-bold mb-1 ${ink}`}>{t.kbOutEmpty}</p>
                      <p className="text-[13px]">{t.kbOutEmptyHint}</p>
                    </div>
                  );
                  const sections = groupOutputs(activeOutputs).filter((x) => x.rows.length > 0);
                  const OutputActions = ({ o, compact }) => (
                    <div className={`flex items-center gap-1 ${compact ? 'justify-end' : 'mt-3'}`}>
                      <button onClick={() => continueOutput(o)}
                        className={`h-8 px-2.5 rounded-[9px] text-[13px] font-medium transition-colors active:opacity-70 text-[#007AFF] hover:bg-[#007AFF]/10 dark:text-[#0A84FF] dark:hover:bg-[#0A84FF]/10`}>
                        {t.kbOutContinue}
                      </button>
                      <button onClick={() => newOutputProject(o)}
                        className={`h-8 px-2.5 rounded-[9px] text-[13px] font-medium transition-colors active:opacity-70 text-[#3A3A3C] hover:bg-[#F2F2F7] dark:text-[#D1D1D6] dark:hover:bg-white/[0.08]`}>
                        {t.kbOutNewProject}
                      </button>
                      {canOpenSystemFiles && (
                        <button title={t.kbOutOpenFolder} onClick={() => openFolder(o.path)}
                          className={`grid h-8 w-8 place-items-center rounded-[9px] transition-colors active:opacity-70 text-[#3A3A3C] hover:bg-[#F2F2F7] dark:text-[#C7C7CC] dark:hover:bg-white/[0.08]`}>
                          <FolderOpen size={15} />
                        </button>
                      )}
                      {isWeb && canDownloadArtifacts && bridge.artifacts.downloadArtifact && (
                        <button title={t.uiKnowledge.downloadOutput} onClick={() => bridge.artifacts.downloadArtifact(o.path, o.sessionId || o.session_id)}
                          className={`grid h-8 w-8 place-items-center rounded-[9px] transition-colors active:opacity-70 text-[#3A3A3C] hover:bg-[#F2F2F7] dark:text-[#C7C7CC] dark:hover:bg-white/[0.08]`}>
                          <Download size={15} />
                        </button>
                      )}
                    </div>
                  );
                  return (
                    <div>
                      {outView === 'list' && (
                        <div className="relative mb-5">
                          <div className="flex overflow-x-auto gap-2 no-scrollbar scroll-smooth">
                          {OUTPUT_CATS.map((c) => {
                            const on = outCat === c.key;
                            return (
                              <button key={c.key} onClick={() => setOutCat(c.key)}
                                className={"h-7 whitespace-nowrap shrink-0 text-[13px] px-3 rounded-full font-semibold transition-colors " + (on ? 'bg-[#3A3A3C] dark:bg-[#fff] text-[#fff] dark:text-[#000]' : 'bg-[#F2F2F7] dark:bg-[#2C2C2E] text-[#000] dark:text-[#fff]')}>
                                {c.label}
                                <span className="ml-1.5 opacity-70">{outputCount(c.key)}</span>
                              </button>
                            );
                          })}
                          </div>
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
                                    <article key={o.path} className={`group min-h-[286px] rounded-[22px] overflow-hidden border transition-all duration-200 bg-white border-black/[0.045] hover:border-black/[0.075] dark:bg-[#1C1C1E] dark:border-white/[0.055] dark:hover:bg-[#202124] dark:hover:border-white/[0.09]`}
                                      style={isDark ? { boxShadow: '0 14px 36px rgba(0,0,0,.24)' } : { boxShadow: '0 1px 2px rgba(0,0,0,.035), 0 10px 24px rgba(0,0,0,.05)' }}>{/* isDark dynamic-value: 保留 (multi-stop boxShadow) */}
                                      <OutputLivePreview o={o} onOpen={() => setOutputPreview({ path: o.path, sessionId: o.sessionId || o.session_id || null })} />
                                      <div className="px-5 pb-4">
                                        <div className="flex items-start gap-3 pt-1">
                                          <div className={`text-[17px] leading-[23px] font-semibold flex-1 min-w-0 truncate ${ink}`} title={o.name}>{o.name}</div>
                                          <span className="h-6 min-w-[48px] px-2.5 rounded-full inline-flex items-center justify-center text-[11px] font-normal tracking-[0.02em] shrink-0 text-[#0066CC] dark:text-[#8DB7FF] bg-[rgba(0,122,255,.08)] dark:bg-[rgba(10,132,255,.10)]">{String(o.ext || '').toUpperCase().slice(0, 4)}</span>
                                        </div>
                                        <div className={`flex items-center gap-2 text-[13px] mt-2 text-[#6E6E73] dark:text-[#AEAEB2]`}><span>{fmtOutputDate(o.mtime)}</span><i className="w-1 h-1 rounded-full bg-current opacity-40"></i><span className="truncate">{o.source || t.kbSubOutput}</span></div>
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
                        <div>
                          {activeOutputs.length === 0 && (
                            <div className={`text-center py-14 ${muted}`}>
                              <div className={`w-12 h-12 mx-auto rounded-2xl grid place-items-center mb-3 ${card}`}><Archive size={20} /></div>
                              <p className={`text-[14px] font-bold mb-3 ${ink}`}>{t.kbNoResults || t.kbOutEmpty}</p>
                              <button onClick={() => { setOutCat('all'); setOutQuery(''); }} className={`px-4 py-2 rounded-full text-[13px] font-bold ${soft}`}>{t.kbOutCatAll}</button>
                            </div>
                          )}
                          {activeOutputs.length > 0 && (
                            <div className="grid grid-cols-1">
                              <div
                                className="hidden md:grid grid-cols-[minmax(0,1fr)_132px_176px] items-center gap-4 border-b pb-2 text-[12px] font-medium border-b-[rgba(198,198,200,.5)] dark:border-b-[#38383A] text-[rgba(60,60,67,.55)] dark:text-[rgba(235,235,245,.5)]"
                              >
                                <span className="text-left">{t.kbColName}</span>
                                <button
                                  type="button"
                                  onClick={() => setOutSortDir((v) => v === 'desc' ? 'asc' : 'desc')}
                                  className={`inline-flex w-fit justify-self-start items-center gap-1 rounded-[8px] py-1 transition-colors hover:text-[#1D1D1F] dark:hover:text-[#E5E5EA]`}
                                >
                                  <span>{t.kbColTime}</span>
                                  <ChevronDown size={13} className={`transition-transform ${outSortDir === 'asc' ? 'rotate-180' : ''}`} />
                                </button>
                                <div className="flex justify-end">
                                  <span className="w-[144px] text-center">{t.kbOutColActions}</span>
                                </div>
                              </div>
                              {activeOutputs.map((o) => {
                                const meta = outCatMeta(o.category);
                                return (
                                  <div key={o.path} onClick={() => setOutputPreview({ path: o.path, sessionId: o.sessionId || o.session_id || null })}
                                    className="group py-4 border-b cursor-pointer border-b-[rgba(198,198,200,.5)] dark:border-b-[#38383A]">
                                    <div className="grid grid-cols-[minmax(0,1fr)_auto] md:grid-cols-[minmax(0,1fr)_132px_176px] items-center gap-4">
                                      <div className="flex min-w-0 items-center gap-4">
                                        <OutputFileIcon meta={meta} ext={o.ext} category={o.category} />
                                        <div className="min-w-0">
                                          <h2 className={`text-[14px] font-semibold tracking-tight truncate mb-0.5 ${ink}`} title={o.name}>{o.name}</h2>
                                          <p className="text-[12px] truncate text-[rgba(60,60,67,.6)] dark:text-[rgba(235,235,245,.6)]">
                                            {o.source || t.kbSubOutput}
                                          </p>
                                        </div>
                                      </div>
                                      <div className="hidden md:block text-[12px] font-medium tabular-nums text-[rgba(60,60,67,.62)] dark:text-[rgba(235,235,245,.62)]">
                                        {fmtOutputDate(o.mtime)}
                                      </div>
                                      <div className="flex shrink-0 items-center justify-end gap-1">
                                        <button onClick={(e) => { e.stopPropagation(); continueOutput(o); }}
                                          className={`h-8 rounded-[9px] px-2.5 text-[12px] font-medium transition-colors active:opacity-70 text-[#007AFF] hover:bg-[#007AFF]/10 dark:text-[#0A84FF] dark:hover:bg-[#0A84FF]/10`}>
                                          {t.kbOutContinue}
                                        </button>
                                        <button onClick={(e) => { e.stopPropagation(); newOutputProject(o); }}
                                          className={`h-8 rounded-[9px] px-2.5 text-[12px] font-medium transition-colors active:opacity-70 text-[#3A3A3C] hover:bg-[#F2F2F7] dark:text-[#D1D1D6] dark:hover:bg-white/[0.08]`}>
                                          {t.kbOutNewProject}
                                        </button>
                                        {canOpenSystemFiles && (
                                          <button title={t.kbOutOpenFolder} onClick={(e) => { e.stopPropagation(); openFolder(o.path); }}
                                            className={`grid h-8 w-8 place-items-center rounded-[9px] transition-colors active:opacity-70 text-[#3A3A3C] hover:bg-[#F2F2F7] dark:text-[#C7C7CC] dark:hover:bg-white/[0.08]`}>
                                            <FolderOpen size={15} />
                                          </button>
                                        )}
                                        {isWeb && canDownloadArtifacts && bridge.artifacts.downloadArtifact && (
                                          <button title={t.uiKnowledge.downloadOutput} onClick={(e) => { e.stopPropagation(); bridge.artifacts.downloadArtifact(o.path, o.sessionId || o.session_id); }}
                                            className={`grid h-8 w-8 place-items-center rounded-[9px] transition-colors active:opacity-70 text-[#3A3A3C] hover:bg-[#F2F2F7] dark:text-[#C7C7CC] dark:hover:bg-white/[0.08]`}>
                                            <Download size={15} />
                                          </button>
                                        )}
                                      </div>
                                      <div className="col-span-2 text-[12px] md:hidden text-[rgba(60,60,67,.55)] dark:text-[rgba(235,235,245,.5)]">
                                        {fmtOutputDate(o.mtime)}
                                      </div>
                                    </div>
                                  </div>
                                );
                              })}
                            </div>
                          )}
                        </div>
                      )}
                    </div>
                  );
                })()}
              </div>
            )}
            {outputPreview && <FilePreviewModal path={outputPreview.path} sessionId={outputPreview.sessionId} theme={theme} t={t} onClose={() => setOutputPreview(null)} />}

            {sub === 'remote' && <RemoteKnowledgeView t={t} embedded />}

            {/* ============ 知识库 · embedding 模型未安装/加载中/加载失败 → gate ============ */}
            {sub === 'kb' && !modelUsable && (
              <div className="max-w-[560px] mx-auto text-center pt-8 pb-2">
                <div className="w-[84px] h-[84px] mx-auto rounded-[24px] grid place-items-center relative"
                  style={{ background: isDark ? 'linear-gradient(135deg,#2A2440,#1E2438)' : 'linear-gradient(135deg,#efeafe,#e3ecfb)' }}>{/* isDark dynamic-value: 保留 (linear-gradient) */}
                  <Database size={40} className="text-[#6f5cf0]" />
                  <span className="absolute -right-1.5 -bottom-1.5 w-[30px] h-[30px] rounded-full grid place-items-center"
                    style={{ background: 'linear-gradient(135deg,#6f5cf0,#5b6cf2)', border: `3px solid ${isDark ? '#131314' : '#fff'}`, boxShadow: '0 4px 10px rgba(108,92,231,.35)' }}>{/* isDark dynamic-value: 保留 (border 模板字符串拼色) */}
                    <Download size={14} className="text-white" />
                  </span>
                </div>
                <h2 className={`mt-5 text-[20px] font-extrabold ${ink}`}>{modelInstalled ? (modelLoading ? t.kbModelLoadingTitle : t.kbModelFailedTitle) : t.kbModelTitle}</h2>
                <p className={`mt-2.5 mx-auto max-w-[450px] text-[13.5px] leading-relaxed ${muted}`}>{modelInstalled ? (modelLoading ? t.kbModelLoadingDesc : t.kbModelFailedDesc) : t.kbModelDesc}</p>

                <div className={`mt-5 mx-auto max-w-[480px] text-left rounded-2xl p-[18px] ${panel}`} style={panelShadow}>
                  <div className="flex items-center gap-3">
                    <div className="w-[46px] h-[46px] rounded-xl grid place-items-center shrink-0 bg-[#f0eefb] dark:bg-[#2A2440]"
                      style={{ color: '#6f5cf0' }}><Package size={23} /></div>
                    <div className="flex-1 min-w-0">
                      <div className={`text-[14.5px] font-extrabold ${ink}`}>{t.kbModelPkgName}</div>
                      <div className={`text-[12px] mt-0.5 ${muted}`}>{t.kbModelPkgSub}</div>
                    </div>
                    <span className="text-[11.5px] font-bold px-2.5 py-1 rounded-lg shrink-0 text-[#6f5cf0] bg-[#efeafe] dark:bg-[#2A2440]">{(kbModel && kbModel.version) || 'bge-m3'}</span>
                  </div>
                  <div className="flex flex-wrap gap-2 mt-3.5">
                    {[
                      t.kbModelChipDownload.replace('{n}', mb(kbModel && kbModel.sizeBytes) || 545),
                      t.kbModelChipInstalled.replace('{n}', mb(kbModel && kbModel.installedBytes) || 560),
                      t.kbModelChipOffline, t.kbModelChipLang,
                    ].map((c, i) => (
                      <span key={i} className={`text-[11.5px] px-2.5 py-1 rounded-lg bg-[#f4f5f8] text-[#5a5a66] dark:bg-white/5 dark:text-[#C4C7C5]`}>{c}</span>
                    ))}
                  </div>
                  <div className="mt-3.5 pt-3.5 border-t border-gray-400/15 flex flex-col gap-2.5">
                    {[t.kbModelItem1, t.kbModelItem2, t.kbModelItem3].map((it, i) => (
                      <div key={i} className={`flex items-center gap-2.5 text-[12.5px] text-[#56565f] dark:text-[#C4C7C5]`}>
                        <Check size={15} className="text-[#18a957] shrink-0" />{it}
                      </div>
                    ))}
                  </div>
                </div>

                {!downloading && !modelLoading ? (
                  <div className="mt-5">
                    {modelFailed ? (
                      <div className="flex items-center justify-center gap-3">
                        <button onClick={() => startModelDownload(false)} className={`px-5 py-2.5 rounded-xl text-[14px] font-bold bg-[#eceef7] text-[#3f4250] dark:bg-white/10 dark:text-white`}>{t.kbModelRetryBtn}</button>
                        <button onClick={() => startModelDownload(true)} className="px-5 py-2.5 rounded-xl text-[14px] font-bold text-white"
                          style={{ background: 'linear-gradient(135deg,#6f5cf0,#5b6cf2)', boxShadow: '0 6px 16px rgba(108,92,231,.32)' }}>{t.kbModelRepairBtn} →</button>
                      </div>
                    ) : (
                      <button onClick={() => startModelDownload(false)}
                        className="px-5 py-2.5 rounded-xl text-[14px] font-bold text-white"
                        style={{ background: 'linear-gradient(135deg,#6f5cf0,#5b6cf2)', boxShadow: '0 6px 16px rgba(108,92,231,.32)' }}>
                        {t.kbModelDownloadBtn} →
                      </button>
                    )}
                    <div className={`mt-3 text-[12px] ${muted}`}>{t.kbModelFoot}</div>
                    {(kbm.error || (effectiveKbModel && effectiveKbModel.error)) && <div className="mt-2 text-[12px] text-[#d63a3a]">{kbm.error || effectiveKbModel.error}</div>}
                  </div>
                ) : (
                  <div className="mt-5 max-w-[480px] mx-auto">
                    <ModelProgressIndicator
                      downloading={downloading}
                      percent={dlPct}
                      label={modelLoading && !downloading ? t.kbModelLoading : dlStageLabel}
                    />
                  </div>
                )}
              </div>
            )}

            {/* ============ 知识库 列表（模型已就绪）============ */}
            {sub === 'kb' && modelUsable && (
              <div className="max-w-[1400px] mx-auto">
                <div className={`rounded-3xl p-7 mb-6 flex items-center gap-6 bg-gradient-to-br from-[#ece8fc] to-[#dcebfb] dark:bg-gradient-to-br dark:from-[#2A2440] dark:to-[#1E2438]`}>
                  <div className="flex-1 min-w-0">
                    <h2 className={`text-[20px] font-bold mb-3 text-[#211f33] dark:text-[#E3E3E3]`}>{t.kbBannerTitle}</h2>
                    <button onClick={() => setNewColl({ name: '', category: '' })} className="px-5 py-2.5 rounded-xl text-[14px] font-bold text-white" style={{ background: 'linear-gradient(135deg,#6f5cf0,#5b6cf2)' }}>{t.kbNewColl} →</button>
                    <div className="flex gap-2 mt-4 flex-wrap">
                      {[t.kbStep1, t.kbStep2, t.kbStep3].map((s, i) => (
                        <span key={i} className={`text-[12px] px-3 py-1.5 rounded-full bg-white/70 text-[#54506b] dark:bg-white/10 dark:text-[#C4C7C5]`}><b className="text-[#6c5ce7]">{i + 1}</b> {s}</span>
                      ))}
                    </div>
                  </div>
                  <div className="hidden xl:flex gap-3 shrink-0">
                    {['#3f7bf0', '#7b5fe6', '#1aa07a', '#d6873e', '#d6589a'].map((c, i) => (
                      <div key={i} className={`w-16 h-20 rounded-2xl grid place-items-center shadow-sm bg-white/70 dark:bg-white/10`}>
                        <div className="w-9 h-9 rounded-xl grid place-items-center" style={{ background: c + '22', color: c }}><FileText size={18} /></div>
                      </div>
                    ))}
                  </div>
                </div>

                {/* 语义检索状态 */}
                <div className="flex items-center gap-2 mb-5 text-[12px]">
                  <span className={`px-3 py-1 rounded-full ${embedInfo && embedInfo.enabled ? 'bg-[#e6f6ec] text-[#18a957] dark:bg-[#13361f] dark:text-[#7DD3A8]' : `${card} ${muted}`}`}>
                    {embedInfo && embedInfo.enabled ? `${t.kbEmbedOn}（${embedInfo.model}）` : t.kbEmbedOff}
                  </span>
                </div>

                {importError && (
                  <div data-testid="kb-import-error" role="alert" className="mb-3 rounded-xl border border-[#d63a3a]/30 bg-[#d63a3a]/10 px-4 py-3 text-[12px] text-[#d63a3a]">
                    {importError}
                  </div>
                )}
                {idx && (idx.running || idx.resumable || idx.failed > 0) && (
                  <div className={`mb-5 rounded-2xl border p-4 border-[#dfe3ee] bg-[#f8f9fd] dark:border-white/10 dark:bg-white/[0.04]`}>
                    <div className="flex flex-wrap items-start justify-between gap-3">
                      <div className="min-w-0">
                        <div className={`text-[14px] font-bold ${ink}`}>
                          {idx.resumable ? t.kbImportInterrupted : (idx.running ? t.kbIndexing : t.kbImportDoneWithErrors)}
                        </div>
                        {idx.resumable && <div className={`mt-1 text-[12px] ${muted}`}>{t.kbImportInterruptedHint}</div>}
                        <div className={`mt-2 text-[12px] ${muted}`}>
                          {t.kbImportProgress} {idx.done || 0}/{idx.total || 0}
                          {idx.failed > 0 ? ` · ${t.kbImportErrors} ${idx.failed}` : ''}
                        </div>
                        {idx.currentPath && (
                          <div className={`mt-1 max-w-[760px] truncate text-[12px] ${muted}`} title={idx.currentPath}>
                            {t.kbCurrentFile} {idx.currentPath}
                            {idx.currentChunksTotal > 0 ? ` · ${t.kbChunkProgress} ${idx.currentChunksDone}/${idx.currentChunksTotal}` : ''}
                          </div>
                        )}
                      </div>
                      <div className="flex shrink-0 items-center gap-2">
                        {idx.resumable && (
                          <button onClick={resumeImport} disabled={failedFilesLoading}
                            className={`rounded-full px-4 py-2 text-[13px] font-semibold ${accent} ${failedFilesLoading ? 'cursor-default opacity-50' : ''}`}>{t.kbResumeImport}</button>
                        )}
                        {(idx.running || idx.resumable) && (
                          <button onClick={cancelImport} disabled={failedFilesLoading}
                            className={`rounded-full px-4 py-2 text-[13px] ${card} ${muted} ${failedFilesLoading ? 'cursor-default opacity-50' : ''}`}>{t.kbCancelImport}</button>
                        )}
                      </div>
                    </div>
                    {idx.failedFiles && idx.failedFiles.length > 0 && (
                      <div className="mt-4 border-t border-gray-400/15 pt-3">
                        <div className={`mb-2 text-[12px] font-semibold ${ink}`}>{t.kbFailedFiles}</div>
                        <div className="flex flex-col gap-2">
                          {idx.failedFiles.map((file) => (
                            <div key={file.itemId} className="flex items-center gap-3 text-[12px]">
                              <div className="min-w-0 flex-1">
                                <div className={`truncate font-medium ${ink}`} title={file.path}>{file.name}</div>
                                <div className="truncate text-[#d63a3a]" title={file.error}>{file.error}</div>
                              </div>
                              <button onClick={() => retryImportFile(file.itemId)} disabled={indexing || failedFilesLoading}
                                className={`shrink-0 rounded-full px-3 py-1.5 font-medium ${soft} ${indexing || failedFilesLoading ? 'cursor-default opacity-50' : ''}`}>
                                {t.kbRetryFile}
                              </button>
                            </div>
                          ))}
                        </div>
                        {((failedPaginationRef.current.jobId === idx.jobId && failedPaginationRef.current.initialized)
                          ? failedPaginationRef.current.nextOffset != null
                          : idx.failedFiles.length < idx.failed) && (
                          <button onClick={loadMoreFailedFiles} disabled={failedFilesLoading}
                            className={`mt-3 rounded-full px-3 py-1.5 text-[12px] font-medium ${soft} ${failedFilesLoading ? 'cursor-default opacity-50' : ''}`}>
                            {failedFilesLoading ? t.kbLoadingFailedFiles : t.kbLoadMoreFailedFiles}
                          </button>
                        )}
                      </div>
                    )}
                  </div>
                )}

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
                    <div className="grid grid-cols-3 gap-4">
                      {shown.map((c) => {
                        const prog = (idx && idx.running && idx.collectionId === c.id && idx.total > 0) ? Math.round((idx.done / idx.total) * 100) : null;
                        const isIdx = c.status === 'indexing' || prog != null;
                        return (
                        <div key={c.id} onClick={() => openColl(c)} className={`p-4 rounded-2xl cursor-pointer transition-all ${panel} ${panelHover}`}
                          style={activeColl && activeColl.id === c.id ? { borderColor: collColor(c), boxShadow: `${isDark ? '' : '0 1px 2px rgba(24,24,40,.04), '}0 0 0 2px ${collColor(c)}55` } : panelShadow}>{/* isDark dynamic-value: 保留 (boxShadow 含运行时 collColor) */}
                          <div className="flex items-start gap-3">
                            <div className="w-11 h-11 rounded-xl grid place-items-center shrink-0" style={{ background: collColor(c) + (isDark ? '33' : '1f'), color: collColor(c) }}><BookOpen size={20} /></div> {/* isDark dynamic-value: 保留 (background 依赖运行时 collColor) */}
                            <div className="flex-1 min-w-0">
                              <div className={`text-[15px] font-bold truncate ${ink}`}>{c.name}</div>
                              <div className={`text-[12px] ${muted}`}>{c.category || t.kbUncat}</div>
                            </div>
                            {activeColl && activeColl.id === c.id && <Check size={18} style={{ color: collColor(c) }} className="shrink-0" />}
                          </div>
                          {c.description && <div className={`text-[12px] mt-3 line-clamp-2 ${muted}`}>{c.description}</div>}
                          {isIdx && (
                            <div className="mt-3">
                              <div className="h-1.5 rounded-full overflow-hidden bg-[#edf0fa] dark:bg-[#2A2B2D]">
                                <div className="h-full rounded-full transition-all" style={{ width: (prog != null ? prog : 40) + '%', background: 'linear-gradient(90deg,#5b6cf2,#2f8bff)' }} />
                              </div>
                              {prog != null && <div className="text-[11px] mt-1 text-[#0B57D0] dark:text-[#A8C7FA]">{t.kbIndexing} {prog}%</div>}
                            </div>
                          )}
                          <div className="flex items-center justify-between mt-4 pt-3 border-t border-gray-400/15">
                            <span className={`text-[12px] ${muted}`}><b className="text-[#54545f] dark:text-[#C4C7C5]">{c.docCount}</b> {t.kbDocs} · {fmtSize(c.totalBytes)}</span>
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
                            <button onClick={(e) => { e.stopPropagation(); openAddMenu('coll', e.currentTarget); }} disabled={indexing || !canPickHostFiles} className={`flex items-center gap-2 px-4 py-2 rounded-full text-[13px] font-medium ${(indexing || !canPickHostFiles) ? 'opacity-60 cursor-default' : ''} ${soft}`}>
                          {indexing ? <RefreshCw size={14} className="animate-spin" /> : <Plus size={14} />}
                          {!indexing && <ChevronDown size={13} className="opacity-70" />}
                          {indexing ? `${t.kbIndexing} ${idx.done}/${idx.total}` : t.kbAdd}
                        </button>
                        <button title={t.kbEditColl} onClick={() => setNewColl({ id: activeColl.id, name: activeColl.name, category: activeColl.category || '', description: activeColl.description ?? null })} className={`p-2 rounded-full ${iconHover}`}><Edit2 size={15} /></button>
                        <button title={t.kbDeleteColl} onClick={() => setDelColl(activeColl)} className={`p-2 rounded-full ${iconHover}`}><Trash2 size={15} /></button>
                      </div>}
                    </div>
                    {(() => {
                      const rows = activeColl ? docs : allDocs;
                      if (rows.length === 0) return <div className={`text-center py-12 ${muted} text-[14px]`}>{activeColl ? t.kbCollEmpty : t.kbNoCollFiles}</div>;
                      return (
                      <div className={`rounded-2xl overflow-hidden ${panel}`} style={panelShadow}>
                        <div className={`flex items-center gap-3 px-5 py-3 text-[11.5px] font-semibold ${muted} border-b border-gray-400/15 bg-[#fbfbfd] dark:bg-white/5`}>
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
                            <div className="w-16 flex items-center justify-end gap-1 opacity-0 group-hover:opacity-100 transition-opacity">
                              {canOpenSystemFiles && <button title={t.kbOpen} onClick={() => openFile(d.path)} className={`p-1.5 rounded-full ${iconHover}`}><ExternalLink size={14} /></button>}
                              <button data-testid="kb-remove-document" title={t.kbRemove} onClick={() => setConfirmDoc(d)} className={`p-1.5 rounded-full ${iconHover}`}><Trash2 size={14} /></button>
                            </div>
                          </div>
                        );})}
                      </div>
                      );
                    })()}
                    {removeDocError && <div role="alert" className="mt-3 rounded-xl bg-[#d63a3a]/8 px-3 py-2 text-[12px] text-[#d63a3a]">{removeDocError}</div>}
                  </div>
                )}

                {/* 加入知识库入口：点击选文件/文件夹(单知识集直接加，多个弹选择) */}
                <div onClick={(e) => { if (indexing || !canPickHostFiles) return; e.stopPropagation(); openAddMenu('dz', e.currentTarget); }}
                  className={`mt-5 flex items-center justify-center gap-2 px-4 py-5 rounded-2xl border border-dashed transition-colors ${(indexing || !canPickHostFiles) ? 'cursor-default opacity-60' : 'cursor-pointer'} border-[#d4d8e2] hover:border-[#0B57D0] text-[#444746] dark:border-[#444746] dark:hover:border-[#A8C7FA] dark:text-[#C4C7C5]`}>
                  <Plus size={16} className="text-[#0B57D0] dark:text-[#A8C7FA]" />
                  <span className="text-[13px]">{t.kbAddToKb}</span>
                  {!(indexing || !canPickHostFiles) && <ChevronDown size={13} className="opacity-60" />}
                </div>
              </div>
            )}

          </div>

          {/* 删除知识集 二次确认(删库连同所有文档+索引,不可恢复) */}
          {delColl && (
            <div className="absolute inset-0 z-50 flex items-center justify-center bg-black/40" onClick={() => setDelColl(null)}>
              <div onClick={(e) => e.stopPropagation()} className={`w-[400px] rounded-2xl p-6 bg-white dark:bg-[#1E1F20]`}>
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

          {/* 从本地知识库移除文档：只删除索引，不触碰磁盘原文件。 */}
          {confirmDoc && (
            <div data-testid="kb-remove-document-confirm" className="absolute inset-0 z-50 flex items-center justify-center bg-black/40" onClick={() => setConfirmDoc(null)}>
              <div role="dialog" aria-modal="true" onClick={(e) => e.stopPropagation()} className="w-[400px] rounded-2xl bg-white p-6 dark:bg-[#1E1F20]">
                <div className={`mb-2 flex items-center gap-2 text-[16px] font-bold ${ink}`}>
                  <AlertTriangle size={18} style={{ color: '#d63a3a' }} />
                  {t.kbRemoveDocConfirm.replace('{n}', confirmDoc.name)}
                </div>
                <div className={`mb-5 text-[13px] leading-relaxed ${muted}`}>{t.kbRemoveDocWarn}</div>
                <div className="flex justify-end gap-2">
                  <button onClick={() => setConfirmDoc(null)} className={`rounded-full px-4 py-2 text-[13px] ${card} ${muted}`}>{t.kbCancel}</button>
                  <button onClick={() => removeDocument(confirmDoc)} className="rounded-full bg-[#d63a3a] px-4 py-2 text-[13px] font-medium text-white">{t.kbRemove}</button>
                </div>
              </div>
            </div>
          )}

          {/* 新建知识集 modal */}
          {newColl && (
            <div className="absolute inset-0 z-50 flex items-center justify-center bg-black/40" onClick={() => setNewColl(null)}>
              <div onClick={(e) => e.stopPropagation()} className={`w-[400px] rounded-2xl p-6 bg-white dark:bg-[#1E1F20]`}>
                <div className={`text-[17px] font-bold mb-4 ${ink}`}>{newColl.id ? t.kbEditColl : t.kbNewColl}</div>
                <input autoFocus value={newColl.name} placeholder={t.kbCollNamePh} onChange={(e) => setNewColl({ ...newColl, name: e.target.value })} onKeyDown={(e) => { if (e.key === 'Enter' && !isImeComposing(e)) createColl(); }}
                  className={`w-full px-4 py-2.5 rounded-xl mb-3 text-[14px] outline-none bg-[#F0F4F9] text-[#1F1F1F] dark:bg-[#2A2B2D] dark:text-[#E3E3E3]`} />
                <input value={newColl.category} placeholder={t.kbCollCatPh} onChange={(e) => setNewColl({ ...newColl, category: e.target.value })}
                  className={`w-full px-4 py-2.5 rounded-xl mb-4 text-[14px] outline-none bg-[#F0F4F9] text-[#1F1F1F] dark:bg-[#2A2B2D] dark:text-[#E3E3E3]`} />
                <div className="flex justify-end gap-2">
                  <button onClick={() => setNewColl(null)} className={`px-4 py-2 rounded-full text-[13px] ${card} ${muted}`}>{t.kbCancel}</button>
                  <button onClick={createColl} className={`px-4 py-2 rounded-full text-[13px] font-medium ${accent}`}>{newColl.id ? t.kbSave : t.kbCreate}</button>
                </div>
              </div>
            </div>
          )}

          {/* 加入知识库 浮层 */}
          {addToKb && (
            <div className="absolute inset-0 z-50 flex items-center justify-center bg-black/40" onClick={() => setAddToKb(null)}>
              <div onClick={(e) => e.stopPropagation()} className={`w-[380px] rounded-2xl p-6 bg-white dark:bg-[#1E1F20]`}>
                <div className={`text-[16px] font-bold mb-1 ${ink}`}>{t.kbAddToKb}</div>
                <div className={`text-[12px] mb-4 truncate ${muted}`}>{Array.isArray(addToKb) ? `${addToKb.length} ${t.kbDocs}` : addToKb}</div>
                {colls.length === 0 ? (
                  <div className={`text-[13px] mb-4 ${muted}`}>{t.kbNoCollsShort}</div>
                ) : (
                  <div className="flex flex-col gap-1 mb-4 max-h-[240px] overflow-y-auto">
                    {colls.map((c) => (
                      <button key={c.id} onClick={async () => { try { replaceIndexState(await inv('kb_collection_add_sources', { collectionId: c.id, paths: Array.isArray(addToKb) ? addToKb : [addToKb] })); } catch (e) {} setAddToKb(null); if (!outputsOnly) setSub('kb'); }}
                        className={`text-left px-4 py-2.5 rounded-xl text-[14px] ${card} ${iconHover} ${ink}`}>{c.name}</button>
                    ))}
                  </div>
                )}
                <button onClick={() => { const p = addToKb; setAddToKb(null); if (!outputsOnly) setSub('kb'); setNewColl({ name: '', category: '' }); }} className={`w-full px-4 py-2.5 rounded-xl text-[13px] font-medium ${soft}`}>+ {t.kbNewColl}</button>
              </div>
            </div>
          )}

          {/* 「+ 添加 ▾」下拉菜单：文件 / 文件夹(后端 WalkDir 递归展开目录) */}
          {addMenu && typeof document !== 'undefined' && createPortal(
            <div onPointerDown={(e) => e.stopPropagation()} style={{ left: addMenu.left, top: addMenu.top, width: addMenu.width }}
              className={`fixed z-[1000] overflow-hidden rounded-xl py-1 shadow-xl ring-1 bg-white ring-black/10 dark:bg-[#202124] dark:ring-white/10`}>
              <button data-testid="kb-add-files" onClick={() => chooseAdd('files')} className={`w-full h-9 px-3 flex items-center gap-2 text-left text-[14px] text-[#1F1F1F] hover:bg-[#F1F3F4] dark:text-[#E3E3E3] dark:hover:bg-[#303134]`}><FileText size={15} /><span>{t.kbAddFiles}</span></button>
              <button data-testid="kb-add-folder" onClick={() => chooseAdd('folders')} disabled={!folderPickerAvailable} className={`w-full h-9 px-3 flex items-center gap-2 text-left text-[14px] ${folderPickerAvailable ? 'text-[#1F1F1F] hover:bg-[#F1F3F4] dark:text-[#E3E3E3] dark:hover:bg-[#303134]' : 'opacity-40 cursor-default'}`}><FolderOpen size={15} /><span>{t.kbAddFolder}</span></button>
            </div>, document.body)}
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
