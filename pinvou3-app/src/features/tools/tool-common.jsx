import React, { useEffect, useMemo, useRef, useState } from 'react';
import { FileTypeIcon } from '../../components/files/FileTypeIcon.jsx';
import { BookOpen, Building2, ChevronDown, ChevronLeft, ChevronRight, CloudSun, Code, Cpu, FileText, Globe, Hexagon, IconGrid, IconList, Layout, LineChart, Mail, MessageCircle, Navigation, Package, Palette, Presentation, Search, Send, Server, TrendingDown, TrendingUp, User, Video, Wrench, XIcon, Zap } from '../../components/icons.jsx';
import { bridge } from '../../hooks/useBridge.js';
import { _ARTIFACT_FMT, _artifactKind } from '../../shared/artifact-utils.js';
import { can, isWeb } from '../../shared/platform.js';
import { parseUnifiedDiff, diffStats } from './unified-diff-parser.js';
import { dict } from '../../shared/i18n.js';

// 调用方尚未下发 t 时回退中文词典（与现状一致），接入 t 后自动多语。
const tc = (t) => (t && t.uiToolCommon) || dict.zh.uiToolCommon;

const AcFmtIcon = FileTypeIcon;
    // 设计稿专用图标（逐字照搬 前端-产物卡片.txt，含其独有 strokeWidth）。
    const AcShieldCheck = ({ className }) => <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" className={className}><path d="M20 13c0 5-3.5 7.5-7.66 8.95a1 1 0 0 1-.67-.01C7.5 20.5 4 18 4 13V6a1 1 0 0 1 1-1c2 0 4.5-1.2 6.24-2.72a1.17 1.17 0 0 1 1.52 0C14.51 3.81 17 5 19 5a1 1 0 0 1 1 1z"/><path d="m9 12 2 2 4-4"/></svg>;
    const AcSparkles = ({ className }) => <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" className={className}><path d="m12 3-1.912 5.813a2 2 0 0 1-1.275 1.275L3 12l5.813 1.912a2 2 0 0 1 1.275 1.275L12 21l1.912-5.813a2 2 0 0 1 1.275-1.275L21 12l-5.813-1.912a2 2 0 0 1-1.275-1.275L12 3Z"/></svg>;
    const AcArrowUpRight = ({ className }) => <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round" className={className}><path d="M7 7h10v10"/><path d="M7 17 17 7"/></svg>;
    const AcFolder = ({ className }) => <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" className={className}><path d="M4 20h16a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.93a2 2 0 0 1-1.66-.9l-.82-1.2A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13c0 1.1.9 2 2 2Z"/></svg>;

    const ArtifactCard = ({ item, theme, t, isLatest }) => {
      const path = item.path || '';
      const canOpenArtifact = !isWeb || can('artifactDownload');
      const kind = _artifactKind(path);
      const fmt = _ARTIFACT_FMT[kind] || _ARTIFACT_FMT.other;
      const basename = (String(path).split(/[\\/]/).pop()) || '';
      const title = item.title || basename || t.artifactLabel;
      const open = () => { if (bridge.available && path) bridge.artifacts.openArtifactExternal(path, item.sessionId); };

      // 封面缩略图：仅 pptx 异步抽取（Rust read_artifact_thumbnail 读 docProps/thumbnail.jpeg → data URL）。
      // 拿不到则 hasCover=false，走紧凑态。本地数据、无外链。
      const [coverUrl, setCoverUrl] = useState(null);
      useEffect(() => {
        let alive = true;
        setCoverUrl(null);
        if (kind === 'pptx' && bridge.available && bridge.artifacts.readArtifactThumbnail && path) {
          bridge.artifacts.readArtifactThumbnail(path).then((u) => { if (alive && u) setCoverUrl(u); }).catch(() => {});
        }
        return () => { alive = false; };
      }, [path, kind, item.sessionId]);
      const hasCover = !!coverUrl;

      return (
        <div className="flex justify-start" style={{ fontFamily:'-apple-system, BlinkMacSystemFont, "SF Pro Text", "PingFang SC", "Microsoft YaHei", sans-serif' }}>
          <div className="w-full bg-white dark:bg-[#1E1E1E] rounded-[24px] shadow-[0_8px_30px_rgba(0,0,0,0.04)] dark:shadow-[0_8px_30px_rgba(0,0,0,0.2)] border border-black/[0.04] dark:border-white/[0.06] p-3 flex flex-col transition-all duration-300">

            {/* 封面区域 */}
            {hasCover ? (
              <div className={`relative group/cover rounded-[16px] overflow-hidden bg-gray-100 dark:bg-[#2C2C2E] border border-black/[0.02] dark:border-white/[0.02] ${canOpenArtifact ? 'cursor-pointer' : ''}`} onClick={canOpenArtifact ? open : undefined}>
                <div className="w-full aspect-[16/9] relative">
                  <img src={coverUrl} alt={tc(t).coverAlt} className="w-full h-full object-cover transition-transform duration-500 ease-out group-hover/cover:scale-[1.02]" />
                </div>
                <div className="absolute top-3 right-3 px-2.5 py-1 rounded-[6px] bg-white/95 dark:bg-[#2C2C2E]/95 text-[#111] dark:text-[#eee] text-[11px] font-bold uppercase tracking-widest shadow-sm">
                  {fmt.label}
                </div>
              </div>
            ) : (
              <div className="px-3 pt-3 pb-2 flex items-center space-x-3">
                <div className="w-10 h-10 rounded-[8px] flex items-center justify-center shrink-0 bg-black/[0.04] dark:bg-white/[0.08]">
                  <AcFmtIcon kind={kind} className="w-6 h-6" />
                </div>
                <span className="text-[13px] font-semibold text-[#888] dark:text-[#999] uppercase tracking-wider">
                  {fmt.label}
                </span>
              </div>
            )}

            {/* 标题与打开按钮区 */}
            <div onClick={canOpenArtifact ? open : undefined} className={`px-3 pt-4 pb-5 flex justify-between items-center gap-4 group/header ${canOpenArtifact ? 'cursor-pointer' : ''}`}>
              <h2 className="text-[20px] font-semibold tracking-tight text-[#111] dark:text-[#eee] leading-snug truncate group-hover/header:text-[#007AFF] transition-colors">
                {title}
              </h2>
              {canOpenArtifact && <button onClick={(e) => { e.stopPropagation(); open(); }} className="flex-shrink-0 flex items-center justify-center w-9 h-9 rounded-full bg-gray-100 dark:bg-white/5 group-hover/header:bg-gray-200 dark:group-hover/header:bg-white/10 text-[#007AFF] dark:text-[#0A84FF] transition-colors active:scale-95" aria-label={tc(t).open}>
                <AcArrowUpRight className="w-[18px] h-[18px]" />
              </button>}
            </div>

            {/* 智能操作区：品 / 悟，横排单行、无副标题；仅最新产物显示 */}
            {isLatest && (
              <div className="grid grid-cols-2 gap-3 mb-4 px-3">
                <button onClick={() => bridge.available && bridge.interaction.summonPinvou(path)} title={t.pvBtnPinTitle}
                  className="flex items-center justify-center min-w-0 py-3.5 px-3 rounded-[12px] bg-[#F9F9F9] dark:bg-white/5 hover:bg-[#F0F0F0] dark:hover:bg-white/10 transition-colors active:scale-[0.98] group/btn" aria-label={tc(t).pinAriaLabel}>
                  <AcShieldCheck className="w-[18px] h-[18px] text-[#FF9500] dark:text-[#FF9F0A] mr-2 shrink-0" />
                  <span className="text-[14px] font-medium text-[#111] dark:text-[#eee] truncate">{t.pvBtnPinLabel}</span>
                </button>

                <button onClick={() => bridge.available && bridge.interaction.inspectPinvou(path)} title={t.pvBtnWuTitle}
                  className="flex items-center justify-center min-w-0 py-3.5 px-3 rounded-[12px] bg-[#F9F9F9] dark:bg-white/5 hover:bg-[#F0F0F0] dark:hover:bg-white/10 transition-colors active:scale-[0.98] group/btn" aria-label={tc(t).wuAriaLabel}>
                  <AcSparkles className="w-[18px] h-[18px] text-[#5E5CE6] dark:text-[#5E5CE6] mr-2 shrink-0" />
                  <span className="text-[14px] font-medium text-[#111] dark:text-[#eee] truncate">{t.pvBtnWuLabel}</span>
                </button>
              </div>
            )}

            {/* 底部路径 */}
            <div className="mx-3 mt-1 mb-2 pt-3 border-t border-gray-100 dark:border-white/[0.05]">
              <div className="flex items-center gap-1.5 text-[12px] text-[#999] dark:text-[#777]">
                <AcFolder className="w-3.5 h-3.5 shrink-0" />
                <span className="truncate" title={path}>{path}</span>
              </div>
            </div>

          </div>
        </div>
      );
    };

    // ==========================================
    // Tool Card
    // ==========================================
    // 弱化类工具：渲染成单行灰条（看得见但不抢眼）；其余有产出类保留醒目卡片。
    // 含 ①只读探查 ②待办/清单的细碎进度操作（易刷屏、对用户价值低）。
    // 批量出方案的 checklist_write/todo_write/update_plan 不在此列（它们走方案卡）。
    const QUIET_TOOLS = new Set([
      'read_file', 'list_dir', 'grep_files', 'file_search', 'glob',
      'checklist_update', 'todo_update', 'checklist_add', 'todo_add', 'checklist_list', 'todo_list',
    ]);

    const isQuietTool = (item) => {
      if (!item) return false;
      if (QUIET_TOOLS.has(item.name)) return true;
      return item.name === 'File' && ['read', 'list', 'search_name', 'search_content'].includes(item.args?.action);
    };

    const toolBasename = (p) => {
      if (typeof p !== 'string' || !p) return '';
      const parts = p.replace(/\/+$/, '').split('/');
      return parts[parts.length - 1] || p;
    };

    // A 档摘要：只从结构化 args 提“动作对象”（文件名/命令/模式），稳且免费，不 parse output。
    const toolSummary = (name, args, t) => {
      if (!args || typeof args !== 'object') return '';
      switch (name) {
        case 'read_file': {
          const base = toolBasename(args.path);
          if (args.start_line || args.max_lines) {
            const s = Number(args.start_line) || 1;
            const max = Number(args.max_lines);
            return base + ' · ' + t.tsLine + ' ' + s + (max ? '-' + (s + max - 1) : '+');
          }
          return base;
        }
        case 'File': {
          const action = args.action;
          if (action === 'read') {
            const base = toolBasename(args.path);
            if (args.start_line || args.max_lines) {
              const s = Number(args.start_line) || 1;
              const max = Number(args.max_lines);
              return base + ' · ' + t.tsLine + ' ' + s + (max ? '-' + (s + max - 1) : '+');
            }
            return base;
          }
          if (action === 'list') return toolBasename(args.path || '.') || '.';
          if (action === 'search_content') return args.pattern ? '"' + args.pattern + '"' : '';
          if (action === 'search_name') return args.query ? '"' + args.query + '"' : '';
          if (action === 'patch') {
            const paths = [];
            const add = path => {
              const base = toolBasename(path);
              if (base && !paths.includes(base)) paths.push(base);
            };
            add(args.path);
            for (const key of ['replace', 'changes']) {
              if (Array.isArray(args[key])) args[key].forEach(change => add(change?.path));
            }
            String(args.patch || '').split(/\r?\n/).forEach(line => {
              const match = line.match(/^\*\*\* (?:Add|Update|Delete) File:\s*(.+?)\s*$/)
                || line.match(/^\+\+\+\s+(?:b\/)?(.+?)\s*$/);
              if (match && match[1] !== '/dev/null') add(match[1]);
            });
            return paths.join(', ');
          }
          return toolBasename(args.path);
        }
        case 'write_file':
        case 'edit_file':
          return toolBasename(args.path);
        case 'list_dir':
          return toolBasename(args.path || '.') || '.';
        case 'grep_files':
          return args.pattern ? '"' + args.pattern + '"' : '';
        case 'file_search':
          return args.query ? '"' + args.query + '"' : '';
        case 'exec_shell':
        case 'task_shell_start':
        case 'shell':
        case 'Bash':
          return typeof args.command === 'string' ? args.command.replace(/\s+/g, ' ').trim() : '';
        case 'checklist_update':
        case 'todo_update':
          return args.status === 'completed' ? t.tsDone
            : args.status === 'in_progress' ? t.tsInProgress
            : args.status === 'pending' ? t.tsPending : '';
        default:
          return '';
      }
    };

    // 底座对超 12K 的工具输出会压成 [TOOL_OUTPUT_RECEIPT]（content-addressed 溢出），
    // 直接展示一堆 sha/handle 是噪音 —— 翻译成人话 + 保留 preview 摘要。
    const isReceipt = (text) => typeof text === 'string' && text.trim().startsWith('[TOOL_OUTPUT_RECEIPT]');
    const parseReceipt = (text) => {
      const fields = {};
      String(text).split('\n').forEach((line) => {
        const m = line.match(/^\s*([a-z_]+):\s*(.*)$/);
        if (m) fields[m[1]] = m[2];
      });
      return fields;
    };
    const ReceiptBlock = ({ text, t }) => {
      const f = parseReceipt(text);
      const muted = 'text-[#757575] dark:text-[#8E8E8E]';
      const body = 'text-[#444746] dark:text-[#C4C7C5]';
      // 输出超大被底座存档、只回传 preview。存档/压缩机制对用户无意义 ——
      // 只展示内容开头 + 一句不带术语的诚实提示（避免误以为是完整输出）。
      const pv = (f.preview && f.preview !== '(none)') ? f.preview.replace(/\\n/g, '\n') : '';
      const pvIsDiff = pv && (/(^|\n)@@/.test(pv) || (pv.indexOf('@@') >= 0 && /(^|\n)[+-]/.test(pv)));
      const note = <div className={`mt-0.5 text-[11px] ${muted}`}>{t.receiptNote}</div>;
      if (pvIsDiff) return (<div><DiffView text={pv} t={t} />{note}</div>);
      return (
        <div>
          <div className={outBox()} style={{ whiteSpace: 'pre-wrap' }}>{pv || t.receiptEmpty}</div>
          {pv ? note : null}
        </div>
      );
    };

    // ── 每工具定制结果视图（仿 Claude Code）：解析失败一律 fallback 纯文本，永不崩 ──
    const tryParseJson = (text) => { try { return JSON.parse(text); } catch (_) { return null; } };
    // checklist/plan 输出是「摘要行\n{json}」，切首个换行后 parse
    const tryTailJson = (text) => {
      if (typeof text !== 'string') return null;
      const i = text.indexOf('\n');
      if (i < 0) return null;
      try { return JSON.parse(text.slice(i + 1)); } catch (_) { return null; }
    };
    const looksDiff = (text) => typeof text === 'string'
      && /(^|\n)--- /.test(text) && /(^|\n)\+\+\+ /.test(text);
    const outBox = () => 'tool-card-output custom-scrollbar rounded-lg p-2 text-[12px] bg-white text-[#444746] dark:bg-[#131314] dark:text-[#C4C7C5]';
    const TODO_SYM = { completed: '☑', in_progress: '◐', pending: '☐' };
    const TODO_TOOLS = ['checklist_write', 'checklist_update', 'checklist_add', 'checklist_list', 'todo_write', 'todo_update', 'todo_add', 'todo_list', 'update_plan'];

    const OutputPre = ({ text }) => (
      <pre className={outBox()} style={{ whiteSpace: 'pre-wrap' }}>
        {typeof text === 'string' ? text : JSON.stringify(text, null, 2)}
      </pre>
    );
    const OutputError = ({ text }) => (
      <pre className={`tool-card-output custom-scrollbar rounded-lg p-2 text-[12px] bg-white text-[#C5221F] dark:bg-[#131314] dark:text-[#F28B82]`} style={{ whiteSpace: 'pre-wrap' }}>
        {typeof text === 'string' ? text : JSON.stringify(text, null, 2)}
      </pre>
    );
    const ListDirView = ({ items, t }) => {
      const muted = 'text-[#757575] dark:text-[#8E8E8E]';
      const sorted = items.slice().sort((a, b) => ((b.is_dir ? 1 : 0) - (a.is_dir ? 1 : 0)) || String(a.name).localeCompare(String(b.name)));
      return (
        <div className={outBox()} style={{ fontFamily: 'monospace' }}>
          <div className={`mb-1 ${muted}`} style={{ fontFamily: 'inherit' }}>{t.listDirCount(items.length)}</div>
          {sorted.map((it, i) => <div key={i}>{it.is_dir ? '📁' : '📄'} {it.name}{it.is_dir ? '/' : ''}</div>)}
        </div>
      );
    };
    const GrepView = ({ data, t }) => {
      const muted = 'text-[#757575] dark:text-[#8E8E8E]';
      const matches = Array.isArray(data.matches) ? data.matches : [];
      return (
        <div className={outBox()}>
          <div className={`mb-1 ${muted}`}>
            {t.grepHits(data.total_matches != null ? data.total_matches : matches.length)}
            {data.files_searched != null ? t.grepFiles(data.files_searched) : ''}
            {data.truncated ? t.grepTruncated : ''}
          </div>
          {matches.map((m, i) => (
            <div key={i} className="mt-1">
              <span className={muted}>{m.file}:{m.line_number}</span>
              <pre style={{ whiteSpace: 'pre-wrap', margin: 0, fontFamily: 'monospace' }}>{m.line}</pre>
            </div>
          ))}
        </div>
      );
    };
    // IDE 风格 diff viewer:解析 unified diff → 行号 + 着色背景 + 文件头 + 摘要脚注。
    // 替换原纯文本按行着色版本(2026-07 升级,对齐 Cursor/Claude Code/Cline 行业标准)。
    // 解析失败或 receipt preview 截断时降级为单列文本,绝不崩。
    const DiffView = ({ text, t }) => {
      const parsed = useMemo(() => parseUnifiedDiff(text), [text]);
      // M4:diffStats 在 parsed 不变时不必重算,用 useMemo 避免每次渲染 O(n) 扫描。
      // 多文件场景每个 file 段各自算 stats;顶层 stats(向后兼容/全局胶囊)只走聚合。
      // 注意:Hook 必须无条件调用 —— 不能放在下方 !parsed.ok 的 early return 之后,
      // 否则同一组件从解析失败切到成功时 Hook 数量变化,React 报
      // "Rendered more hooks than during the previous render"(评审 M4)。
      const fileStatsList = useMemo(
        () => (parsed.ok && parsed.files ? parsed.files : []).map((f) => diffStats({ hunks: f.hunks })),
        [parsed],
      );
      // 展开/收起完整 diff(默认 200px 滚动,点文件头 chevron 展开全部)。
      const [expanded, setExpanded] = useState(false);
      // 解析失败(非 diff 文本 / 大文件 [diff omitted] / 截断 receipt preview):走文本兜底。
      if (!parsed.ok) {
        if (parsed.omitReason) {
          const muted = 'text-[#757575] dark:text-[#8E8E8E]';
          const body = 'text-[#444746] dark:text-[#C4C7C5]';
          // H5:omitReason 现在可能同时含 summary("Wrote N bytes")和
          // "[diff omitted] ..." 原因(summary 在前)。summary 走正常字色,
          // omitReason 整段保持灰字提示。
          return (
            <div className={outBox()} style={{ whiteSpace: 'pre-wrap', fontFamily: 'monospace' }}>
              {parsed.summary ? <div className={body + ' mb-1'}>{parsed.summary}</div> : null}
              <div className={muted}>{parsed.omitReason}</div>
            </div>
          );
        }
        const lines = String(text).split('\n');
        const add = 'text-[#137333] dark:text-[#93D5A6]';
        const del = 'text-[#C5221F] dark:text-[#F28B82]';
        const hunk = 'text-[#0B57D0] dark:text-[#A8C7FA]';
        const muted = 'text-[#757575] dark:text-[#8E8E8E]';
        const color = (l) => /^(\+\+\+|---)/.test(l) ? muted : l.startsWith('+') ? add : l.startsWith('-') ? del : l.startsWith('@@') ? hunk : '';
        return (
          <pre className={outBox()} style={{ whiteSpace: 'pre-wrap', fontFamily: 'monospace' }}>
            {lines.map((l, i) => <div key={i} className={color(l)}>{l || ' '}</div>)}
          </pre>
        );
      }
      // 配色:沿用原 DiffView 的 iOS 风格调色板,新增行级背景色提升可读性。
      const addText = 'text-[#137333] dark:text-[#93D5A6]';
      const delText = 'text-[#C5221F] dark:text-[#F28B82]';
      const ctxText = 'text-[#444746] dark:text-[#C4C7C5]';
      const mutedText = 'text-[#757575] dark:text-[#8E8E8E]';
      const addBg = 'bg-[#e6f4ea] dark:bg-[#0e1f0e]';
      const delBg = 'bg-[#fce8e6] dark:bg-[#2a0e0e]';
      const hunkBg = 'bg-[#e8f0fe] dark:bg-[#0d1a2e]';
      const headerBg = 'bg-[#f1f3f4] dark:bg-[#1b1b1d]';
      const metaText = 'text-[#5f6368] dark:text-[#8E8E8E]';

      const noStyle = { whiteSpace: 'pre-wrap', fontFamily: 'ui-monospace, SFMono-Regular, Menlo, Consolas, monospace' };

      // H1:多文件分段渲染 —— 每个文件段独立 header + 统计胶囊;单文件场景下
      // parsed.files 长度为 1,与原行为一致。
      const files = parsed.files && parsed.files.length > 0 ? parsed.files : [{ oldPath: parsed.oldPath, newPath: parsed.newPath, hunks: parsed.hunks }];
      const T = tc(t);

      return (
        // 根节点不用 overflow-hidden class:vendor/tailwind.js 运行时把生成的
        // .overflow-hidden 注入到 base.css 之后,会盖掉 .tool-card-output 的
        // overflow-y:auto,导致 diff 被 200px max-height 裁剪且无法滚动(e2e 实测)。
        // 内联样式优先级最高:显式 y 滚动 + x 裁剪(保圆角),expanded 时放开 max-height。
        <div
          data-testid="diff-view"
          className={`${outBox()} p-0`}
          style={{ overflowY: 'auto', overflowX: 'hidden', ...(expanded ? { maxHeight: 'none' } : {}) }}
        >
          {files.map((file, fi) => {
            const fst = fileStatsList[fi] || { add: 0, del: 0, ctx: 0 };
            return (
              <div key={fi} className={fi > 0 ? `border-t border-black/10 dark:border-white/10` : ''}>
                {/* 文件头:旧路径 → 新路径(同文件只显示一个)。add/del 统计胶囊 + 展开按钮。 */}
                <div data-testid="diff-file-header" className={`flex items-center justify-between gap-2 px-3 py-1.5 text-[11px] border-b border-black/5 dark:border-white/5 ${headerBg}`}>
                  <div className={`flex items-center gap-1.5 min-w-0 ${metaText}`}>
                    <span aria-hidden>📄</span>
                    <span className="truncate font-mono">{file.newPath || file.oldPath || ''}</span>
                  </div>
                  <div className="flex items-center gap-1.5 shrink-0">
                    <span className={`px-1.5 py-0.5 rounded ${addText} ${addBg}`}>+{fst.add}</span>
                    <span className={`px-1.5 py-0.5 rounded ${delText} ${delBg}`}>−{fst.del}</span>
                    <button
                      type="button"
                      onClick={() => setExpanded((v) => !v)}
                      title={expanded ? T.diffCollapse : T.diffExpand}
                      aria-label={expanded ? T.diffCollapseAria : T.diffExpand}
                      className={`px-1 py-0.5 rounded ${mutedText} hover:bg-black/5 dark:hover:bg-white/10`}
                    >
                      <ChevronDown size={12} className={`transition-transform ${expanded ? 'rotate-180' : ''}`} />
                    </button>
                  </div>
                </div>

                {/* hunk 渲染:行号列 + 内容列,add/del 行带背景色 */}
                <div className="overflow-x-auto">
                  {file.hunks.map((h, hi) => (
                    <div key={hi}>
                      <div className={`px-3 py-0.5 text-[11px] font-mono ${mutedText} ${hunkBg}`}>{h.header}</div>
                      {h.lines.map((l, li) => {
                        // 行号列(右对齐,固定 4 字符宽)。空侧用 ''。
                        const oldStr = l.oldNo != null ? String(l.oldNo) : '';
                        const newStr = l.newNo != null ? String(l.newNo) : '';
                        let bg = '', txt = ctxText, marker = ' ';
                        if (l.kind === 'add') { bg = addBg; txt = addText; marker = '+'; }
                        else if (l.kind === 'del') { bg = delBg; txt = delText; marker = '−'; }
                        else if (l.kind === 'meta') { txt = mutedText; }
                        return (
                          <div
                            key={li}
                            data-testid="diff-line"
                            data-diff-kind={l.kind}
                            data-old-no={oldStr}
                            data-new-no={newStr}
                            className={`flex items-start ${bg} text-[12px] leading-[1.55]`}
                          >
                            <span className={`select-none text-right pr-2 pl-2 w-[3.5rem] shrink-0 ${mutedText}`}>{oldStr}</span>
                            <span className={`select-none text-right pr-2 w-[3.5rem] shrink-0 ${mutedText}`}>{newStr}</span>
                            <span className={`select-none w-4 shrink-0 ${txt}`}>{marker}</span>
                            <span className={`flex-1 pr-3 ${txt}`} style={noStyle}>{l.text || ' '}</span>
                          </div>
                        );
                      })}
                    </div>
                  ))}
                </div>
              </div>
            );
          })}

          {/* 摘要脚注:Replaced N occurrences / Created ... / Wrote ... bytes */}
          {parsed.summary ? (
            <div data-testid="diff-summary" className={`px-3 py-1.5 text-[11px] border-t border-black/5 dark:border-white/5 ${mutedText}`}>
              {parsed.summary}
            </div>
          ) : null}

          {/* LSP 诊断块(若后端 append),单独样式 */}
          {parsed.trailingDiagnostics ? (
            <div data-testid="diff-diagnostics" className={`px-3 py-1.5 text-[11px] border-t border-black/5 dark:border-white/5 ${delText}`} style={{ whiteSpace: 'pre-wrap', fontFamily: 'ui-monospace, SFMono-Regular, Menlo, Consolas, monospace' }}>
              {parsed.trailingDiagnostics}
            </div>
          ) : null}
        </div>
      );
    };
    const ShellView = ({ data, t }) => {
      const muted = 'text-[#757575] dark:text-[#8E8E8E]';
      const del = 'text-[#C5221F] dark:text-[#F28B82]';
      return (
        <div className={outBox()}>
          <div className={`mb-1 ${muted}`}>
            {data.status || ''}{data.exit_code != null ? ` · exit ${data.exit_code}` : ''}
            {data.duration_ms != null ? ` · ${data.duration_ms}ms` : ''}
            {data.stdout_truncated ? t.shellStdoutTrunc : ''}
          </div>
          {data.stdout != null && data.stdout !== '' && <pre style={{ whiteSpace: 'pre-wrap', margin: 0, fontFamily: 'monospace' }}>{data.stdout}</pre>}
          {data.stderr ? <pre className={del} style={{ whiteSpace: 'pre-wrap', margin: 0, fontFamily: 'monospace' }}>{data.stderr}</pre> : null}
        </div>
      );
    };
    // exec_shell 的 content 其实是纯 stdout 文本（结构化字段在 metadata，前端没拿）→ 给终端样式
    const ShellTextView = ({ cmd, text }) => {
      const muted = 'text-[#757575] dark:text-[#8E8E8E]';
      return (
        <div className={outBox()} style={{ fontFamily: 'monospace' }}>
          {cmd && <div className={muted} style={{ whiteSpace: 'pre-wrap' }}>$ {cmd}</div>}
          <pre style={{ whiteSpace: 'pre-wrap', margin: 0, fontFamily: 'inherit' }}>{typeof text === 'string' ? text : JSON.stringify(text)}</pre>
        </div>
      );
    };
    const TodoView = ({ snap, t }) => {
      const muted = 'text-[#757575] dark:text-[#8E8E8E]';
      const items = Array.isArray(snap.items) ? snap.items : [];
      return (
        <div className={outBox()}>
          {snap.completion_pct != null && <div className={`mb-1 ${muted}`}>{t.todoProgress(snap.completion_pct)}</div>}
          {snap.explanation && <div className="mb-1">{snap.explanation}</div>}
          {items.map((it, i) => (
            <div key={i} className={it.status === 'completed' ? muted : ''}>
              <span className={it.status === 'in_progress' ? 'text-[#E37400] dark:text-[#FDD663]' : ''}>{TODO_SYM[it.status] || '☐'}</span> {it.content || it.step || ''}
            </div>
          ))}
        </div>
      );
    };
    // 输出渲染分发：失败→红字；大输出→receipt；高频工具→定制视图；其余→纯文本
    const tsToolsData = [
      { id: 1, backendId: 'weather', mcpServer: true, title: '高德天气', subtitle: '高德地图实时天气与多日预报', category: 'life', type: 'MCP Server', version: 'v1.0.0', latency: '<50ms', desc: '通过高德地图 Web 服务 API 查询全国城市实时天气与未来多日预报。需要填写你自己的高德 Web 服务 API Key，密钥只写入本机系统凭据。', icon: CloudSun, color: 'bg-gradient-to-b from-sky-400 to-blue-500', installed: false, authRequired: true, configTitle: '高德天气 Key', configDescription: 'Key 只保存在本机凭据，不写入 mcp.json。', configDocUrl: 'https://console.amap.com/dev/key/app', configDocLabel: '去创建 Web 服务 Key', configFields: [{ key: 'AMAP_KEY', label: 'API Key', required: true, target: 'env', secret: true, placeholder: '粘贴高德 Web 服务 Key', helpText: '请选择「Web 服务」类型。' }], welcomeQueries: ['杭州今天天气', '北京这周会下雨吗', '上海明天穿什么'] },
      { id: 2, backendId: 'iwencai', mcpServer: true, title: '同花顺问财', subtitle: 'A股行情、财务、选股、宏观、新闻', category: 'finance', type: 'MCP Server', version: 'v1.0.0', latency: '<500ms', desc: '基于同花顺问财官方 API，提供 12 个金融查询工具。需要填写你自己的问财 API Key，密钥只写入本机系统凭据。', icon: LineChart, color: 'bg-gradient-to-b from-red-400 to-red-600', installed: false, authRequired: true, configTitle: '问财 Key', configDescription: 'Key 只保存在本机凭据，不写入 mcp.json。', configDocUrl: 'https://www.iwencai.com/skillhub', configDocLabel: '打开问财 SkillHub', configFields: [{ key: 'IWENCAI_API_KEY', label: 'API Key', required: true, target: 'env', secret: true, placeholder: '粘贴 IWENCAI_API_KEY', helpText: '进入任一官方 Skill，在「安装方式」中复制。' }], welcomeQueries: ['茅台最新股价', '今天大盘怎么样', '市盈率低于10的银行股', '最近降息新闻'] },
      { id: 3, backendId: null, title: 'QQ邮箱 API', subtitle: '智能邮件收发与线程提炼', category: 'collab', type: 'REST API', version: 'v1.4.2', latency: '<120ms', desc: '提供标准的邮件收发、搜索和整理接口。结合大模型可实现自然语言读取邮件内容、汇总长线程对话、自动归档管理文件夹。', icon: Mail, color: 'bg-gradient-to-b from-amber-400 to-orange-500', installed: false, authRequired: true },
      { id: 4, backendId: 'ima', imaOpenapi: true, title: '腾讯 ima', subtitle: '用 OpenAPI 操作 ima 笔记与知识库', category: 'docs', type: 'OpenAPI Skill', version: 'v1.1.8', latency: '云端', desc: '接入腾讯 ima OpenAPI Skill：通过 Pinvou 内置的受控工具调用 ima.qq.com 官方 OpenAPI，支持笔记搜索/读取/创建/追加，以及知识库搜索、浏览、网页导入和内容添加。需要填写你自己的 Client ID 和 API Key，凭据只写入本机系统凭据，不进入对话、环境变量、仓库或 mcp.json。', icon: BookOpen, color: 'bg-gradient-to-b from-sky-500 to-indigo-600', installed: false, authRequired: true, configTitle: '连接腾讯 ima', configDescription: '凭据只保存在本机，用于启用 IMA OpenAPI Skill。', configDocUrl: 'https://ima.qq.com/agent-interface', configDocLabel: '获取 Client ID / API Key', configFields: [{ key: 'IMA_CLIENT_ID', label: 'Client ID', required: true, target: 'credential', secret: true, placeholder: 'Client ID' }, { key: 'IMA_API_KEY', label: 'API Key', required: true, target: 'credential', secret: true, placeholder: 'API Key' }], welcomeQueries: ['搜索我的 ima 知识库', '列出我有哪些 ima 笔记', '把这段内容新建为 ima 笔记', '在 ima 知识库里查产品方案'] },
      { id: 5, backendId: null, title: '乐享文档连接器', subtitle: '企业知识文档全量检索', category: 'docs', type: 'Webhook/API', version: 'v1.1.0', latency: '<80ms', desc: '支持通过 API 搜索、创建和管理乐享知识库中的文档。支持批量导入 Markdown、按标签整理内容、实时订阅团队文档的更新动态。', icon: Hexagon, color: 'bg-gradient-to-b from-blue-400 to-blue-600', installed: false, authRequired: true },
      { id: 6, backendId: null, title: '腾讯文档 MCP', subtitle: '多人实时在线协作协议', category: 'docs', type: 'MCP Server', version: 'v1.0.5', latency: '<60ms', desc: '将腾讯文档能力接入 AI。允许大模型读取、分析甚至辅助编写在线表格、文档和幻灯片，轻松完成跨维度的内容查询与数据分析。', icon: FileText, color: 'bg-gradient-to-b from-blue-500 to-indigo-600', installed: false, authRequired: true },
      { id: 8, backendId: null, title: '企微 Bot Hook', subtitle: '连接企业内部与外部生态', category: 'collab', type: 'Webhook', version: 'v4.0', latency: '<40ms', desc: '深度对接企业微信。支持机器人主动推送图文消息、查询通讯录架构、联动审批流与日程管理。', icon: MessageCircle, color: 'bg-gradient-to-b from-cyan-400 to-blue-500', installed: false, authRequired: false },
      { id: 9, backendId: 'feishu', feishuCli: true, title: '飞书（Lark）', subtitle: '以你本人身份操作飞书文档/日历/表格/消息', category: 'collab', type: 'CLI + 官方技能', version: 'v1.0.87', latency: '云端', desc: '接入飞书官方 CLI + 官方域技能（MIT）：让 AI 以你本人身份读写云文档、查改日历、操作多维表格（Base）与电子表格、收发消息、管理知识库与任务。点「连接飞书」浏览器一键授权，全程不填 key。数据经飞书云 OpenAPI（可选联网功能，opt-in）。', icon: Send, color: 'bg-gradient-to-b from-teal-400 to-emerald-500', installed: false, authRequired: true, configFields: [], welcomeQueries: ['读飞书文档帮我做一份 PPT', '把飞书文档整理成摘要', '查我今天的飞书日历', '看看我飞书里的待办任务'] },
      { id: 22, backendId: 'tmeet', tmeetCli: true, title: '腾讯会议', subtitle: '以你本人身份管理会议/录制/纪要/参会报告', category: 'collab', type: 'CLI + 官方技能', version: 'v1.0.15', latency: '云端', desc: '接入腾讯会议官方 CLI（@tencentcloud/tmeet）+ 官方技能：让 AI 以你本人身份创建、查询、修改和取消腾讯会议，查询受邀人、参会报告、录制、转写与智能纪要，并支持会中呼叫成员入会。点「连接」打开腾讯会议授权页扫码登录，全程不填 key。', icon: Video, color: 'bg-gradient-to-b from-sky-400 to-blue-600', installed: false, authRequired: true, configFields: [], welcomeQueries: ['帮我创建一个腾讯会议', '查一下我的腾讯会议录制', '看看最近会议的智能纪要', '查询这场腾讯会议的参会人'] },
      { id: 99, backendId: 'wecom', wecomCli: true, title: '企业微信', subtitle: '以你本人身份操作企微消息/文档/会议/日程', category: 'collab', type: 'CLI + 官方技能', version: 'v0.1.9', latency: '云端', desc: '接入企业微信官方 CLI（@wecom/cli，MIT）+ 官方域技能：让 AI 以你本人身份收发消息、读写文档与智能表格、创建/查询会议与日程、管理待办、查询通讯录。点「连接」用企业微信 App 扫码授权，全程不填 key。数据经企业微信云（可选联网功能，opt-in）。', icon: MessageCircle, color: 'bg-gradient-to-b from-cyan-400 to-blue-500', installed: false, authRequired: true, configFields: [], welcomeQueries: ['把这段内容写成企微智能文档', '读一下我企微某篇文档的内容', '在企微智能表格里新建一张子表', '查一下企微智能表格里的数据'] },
      { id: 10, backendId: 'dingtalk', dingtalkCli: true, title: '钉钉', subtitle: '以你本人身份操作钉钉文档/日历/表格/消息', category: 'collab', type: 'CLI + 官方技能', version: 'v1.0.58', latency: '云端', desc: '接入钉钉官方 DingTalk Workspace CLI（dws，Apache-2.0）+ 官方技能：让 AI 以你本人身份读写钉钉文档、查改日历、操作 AI 表格/在线表格、收发群聊消息、处理待办/审批/日志/邮箱等。点「连接」用钉钉 App 扫码授权，全程不填 key。', icon: Navigation, color: 'bg-gradient-to-b from-blue-400 to-indigo-500', installed: false, authRequired: true, configFields: [], welcomeQueries: ['读一下我的钉钉文档', '查我今天的钉钉日程', '在钉钉 AI 表格里查数据', '看看我的钉钉待办'] },
      { id: 11, backendId: null, title: 'TAPD 敏捷研发', subtitle: '缺陷与迭代的自动化追踪', category: 'dev', type: 'Action Skill', version: 'v2.8.0', latency: '<60ms', desc: '研发管理核心工具。允许 AI 查询项目迭代进度、自动拆分需求条目、更新缺陷状态，实现从需求到发布的研发全生命周期数字化。', icon: Layout, color: 'bg-gradient-to-b from-violet-500 to-fuchsia-600', installed: false, authRequired: true },
      { id: 12, backendId: null, title: 'CNB 云原生管线', subtitle: '代码仓库与 CI/CD 调度', category: 'dev', type: 'MCP Server', version: 'v1.0.0', latency: '<40ms', desc: '将云原生开发能力赋予大模型。支持通过自然语言进行代码仓库检索、提交 Issue、审查 PR、触发并监控流水线部署等极客操作。', icon: Code, color: 'bg-gradient-to-b from-orange-400 to-rose-500', installed: false, authRequired: true },
      { id: 13, backendId: 'qcc', oauthMcp: true, oauthServerName: 'qcc-company', title: '企查查', subtitle: '企业工商数据授权查询', category: 'finance', type: 'Remote MCP', version: 'v1.0.0', latency: '云端', desc: '接入企查查智能体数据平台 qcc-company 远程 MCP。点「连接」后会打开浏览器进行企查查账号 OAuth 授权，全程不填写 API Key。', icon: Building2, color: 'bg-gradient-to-br from-blue-600 to-cyan-500', installed: false, authRequired: true, configFields: [], welcomeQueries: ['查一下华为的工商信息', '腾讯的工商登记信息', '比亚迪有哪些对外投资', '阿里巴巴的股东结构'] },
      { id: 20, backendId: 'patsnap-search', mcpServer: true, title: '智慧芽专利&文献', subtitle: '全球专利与论文融合检索，支持公开号详情获取', category: 'docs', type: 'MCP Server', version: 'v1.0.0', latency: '云端', desc: '接入智慧芽远程 MCP，在全球专利数据库和文献库中进行融合检索，支持自然语言、语义搜索、关键词检索和多维过滤，并可按专利公开号或结果 URL 拉取 Markdown 详情。需要填写智慧芽 API Key，密钥只写入本机系统凭据，mcp.json 仅保存环境变量占位符。', icon: Search, color: 'bg-gradient-to-b from-emerald-500 to-cyan-600', installed: false, authRequired: true, configTitle: '填写智慧芽 API Key', configDescription: 'API Key 仅存储在本机系统凭据中，不会明文写入 mcp.json；连接智慧芽服务时通过 Authorization 请求头发送。', configDocUrl: 'https://open.zhihuiya.com/dashboard/api-keys', configDocLabel: '查看 API Key 获取说明', configFields: [{ key: 'PATSNAP_API_KEY', label: '智慧芽 API Key', required: true, target: 'bearer', secret: true, placeholder: '粘贴你的智慧芽 API Key', helpText: '请从智慧芽开放平台或企业管理员提供的 MCP/API 凭证中获取。' }], welcomeQueries: ['检索固态电池电解质相关专利和论文', '查找近五年 CRISPR 递送系统核心专利和文献', '获取公开号 CN109123456A 的专利详情', '分析宁德时代钠离子电池方向专利布局'] },
      { id: 21, backendId: 'canva-mcp', oauthMcp: true, oauthServerName: 'canva_mcp', title: 'Canva 可画', subtitle: '海报、演示文稿、封面与品牌模板设计', category: 'life', type: 'Remote MCP', version: 'v1.0.0', latency: '云端', desc: '接入 Canva 可画远程 MCP。支持通过自然语言生成和编辑海报、演示文稿、小红书封面、品牌模板等设计内容；点「连接」后会打开浏览器进行 Canva 可画账号授权，全程不填写 API Key。设计指令、素材、文件夹和品牌模板相关内容会发送到 Canva 可画远程 MCP 服务。', icon: Palette, color: 'bg-gradient-to-b from-cyan-500 to-pink-500', installed: false, authRequired: true, configFields: [], welcomeQueries: ['帮我生成一张新品发布海报', '做一份三页产品介绍演示文稿', '设计一张小红书封面', '用品牌模板生成活动宣传图'] },
      { id: 14, backendId: 'obsidian', mcpServer: true, title: 'Obsidian 知识库', subtitle: '检索并管理本机 Obsidian 笔记，读写你的个人知识', category: 'docs', type: 'MCP Server', version: 'v1.1.0', latency: '<30ms', desc: '把你本机的 Obsidian 笔记库（vault）接入大模型。支持全文检索、读取、新建、编辑、改名（自动修双链）与删除——让 AI 基于并维护你自己沉淀的知识。自动识别当前打开的库，无需手动配置；笔记不出本机、模型也在本机，知识与算力全链路不出域。', icon: BookOpen, color: 'bg-gradient-to-b from-violet-500 to-purple-700', installed: false, authRequired: false, welcomeQueries: ['帮我搜一下我的笔记', '帮我新建一篇笔记记录今天的想法', '我的知识库有哪些文档？', '总结一下我的笔记'] },
      { id: 19, backendId: 'yuandian-mcp', oauthMcp: true, oauthServerName: 'yuandian_mcp', title: '华宇元典法律数据', subtitle: '法律法规、案例文书与企业司法风险查询', category: 'docs', type: 'Remote MCP', version: 'v1.0.0', latency: '云端', desc: '接入华宇元典开放平台远程 MCP。支持法律法规、裁判案例、企业司法风险等法律数据检索；点「连接」后会打开浏览器进行元典账号授权，全程不填写 API Key。', icon: BookOpen, color: 'bg-gradient-to-b from-emerald-500 to-cyan-700', installed: false, authRequired: true, configFields: [], welcomeQueries: ['检索一下劳动合同解除相关案例', '查一下公司股权责任相关法规', '帮我分析企业司法风险', '找一下最近的裁判观点'] },
      { id: 15, backendId: 'pptx', mcpServer: true, title: 'PPT 生成', subtitle: '本地直出可编辑 PowerPoint，套主题模板、真图表、带封面', category: 'docs', type: 'MCP Server', version: 'v1.0.0', latency: '本地', desc: '说“做个 PPT / 汇报”，AI 先列大纲让你确认，再按内容自动选主题（9 套）生成可编辑 .pptx——真·图表、自带封面缩略图，全程本地、数据不出机。首次安装会自动下载 python-pptx 依赖（需联网）。', icon: Presentation, color: 'bg-gradient-to-b from-orange-400 to-rose-500', installed: false, authRequired: false, welcomeQueries: ['做个 Q2 季度汇报 PPT', '帮我做一份产品介绍 PPT', '做个项目方案演示', '做个公司介绍 PPT'] },
      { id: 16, backendId: 'gongwen', mcpServer: true, title: '公文写作', subtitle: '党政机关公文直出 GB/T 9704 合规 .docx', category: 'docs', type: 'MCP Server', version: 'v1.0.0', latency: '本地', desc: '说“写个通知 / 起草意见”，AI 按文种结构与固定话术写好内容，渲染器套党政机关公文国标格式（方正小标宋标题、仿宋_GB2312 正文、国标页边距、红头与红色分隔线）直出 .docx，全程本地、数据不出机。配合「党政机关公文写作」技能效果最佳。首次安装自动下载 python-docx 依赖（需联网）。', icon: FileText, color: 'bg-gradient-to-b from-red-500 to-rose-700', installed: false, authRequired: false, welcomeQueries: ['起草一份关于印发管理办法的通知', '写一份加强某项工作的实施意见', '拟一份会议通知', '写一份情况报告'] },
    ];

    // overlay 命中规则：有 backendId 按 backendId 查，占位卡(backendId=null)按 'card'+id 查。
    // overlay 提供 configFields 时按字段 key 深合并，只覆盖 label/helpText/placeholder，其余源字段保留。
    const localizeTool = (tool, t) => {
      if (!tool) return tool;
      const tools = t?.uiToolDetails?.tools;
      const localized = tools && (tool.backendId ? tools[tool.backendId] : tools['card' + tool.id]);
      if (!localized) return tool;
      const merged = { ...tool, ...localized };
      if (Array.isArray(localized.configFields) && Array.isArray(tool.configFields)) {
        merged.configFields = tool.configFields.map((src) => {
          const ov = localized.configFields.find((f) => f && f.key === src.key);
          if (!ov) return src;
          const out = { ...src };
          for (const k of ['label', 'helpText', 'placeholder']) if (ov[k] != null) out[k] = ov[k];
          return out;
        });
      }
      return merged;
    };

    const weatherIconSvg = (code) => {
      const svgs = {
        sunny: '<svg viewBox="0 0 24 24" fill="currentColor" class="w-10 h-10 text-yellow-300"><circle cx="12" cy="12" r="5"></circle><path d="M12 2v2m0 16v2M4.93 4.93l1.41 1.41m11.32 11.32l1.41 1.41M2 12h2m16 0h2M6.34 17.66l-1.41 1.41M19.07 4.93l-1.41 1.41" stroke="currentColor" stroke-width="2" stroke-linecap="round"></path></svg>',
        partly_cloudy: '<svg viewBox="0 0 24 24" fill="currentColor" class="w-10 h-10 text-white"><path d="M6.5 19a4.5 4.5 0 0 1-.343-8.987 7.5 7.5 0 0 1 14.28 2.226A4 4 0 0 1 18 20H6.5z"></path><circle cx="7" cy="7" r="2.5" fill="currentColor" opacity="0.8"></circle></svg>',
        cloudy: '<svg viewBox="0 0 24 24" fill="currentColor" class="w-10 h-10 text-white opacity-90"><path d="M6.5 17.5A4.5 4.5 0 0 1 6.5 8.5h.342a7.5 7.5 0 0 1 14.316 2.625A4.002 4.002 0 0 1 18 19H6.5v-1.5z"></path></svg>',
        thunderstorm: '<svg viewBox="0 0 24 24" fill="currentColor" class="w-10 h-10 text-yellow-200"><path d="M6.5 17A4.5 4.5 0 0 1 6.5 8h.342a7.5 7.5 0 0 1 14.316 2.625A4 4 0 0 1 18 18H6.5z" opacity="0.7"></path><path d="M13 12l-2 5h3l-2 5" stroke="currentColor" stroke-width="1.5" fill="none"></path></svg>',
        heavy_rain: '<svg viewBox="0 0 24 24" fill="currentColor" class="w-10 h-10 text-blue-300"><path d="M6.5 15A4.5 4.5 0 0 1 6.5 6h.342a7.5 7.5 0 0 1 14.316 2.625A4 4 0 0 1 18 16H6.5z" opacity="0.7"></path><path d="M9 18v3m4-4v3m4-2v3" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"></path></svg>',
        rainy: '<svg viewBox="0 0 24 24" fill="currentColor" class="w-10 h-10 text-blue-200"><path d="M6.5 15A4.5 4.5 0 0 1 6.5 6h.342a7.5 7.5 0 0 1 14.316 2.625A4 4 0 0 1 18 16H6.5z" opacity="0.7"></path><path d="M10 18v2m5-3v2" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"></path></svg>',
        snowy: '<svg viewBox="0 0 24 24" fill="currentColor" class="w-10 h-10 text-blue-100"><path d="M6.5 15A4.5 4.5 0 0 1 6.5 6h.342a7.5 7.5 0 0 1 14.316 2.625A4 4 0 0 1 18 16H6.5z" opacity="0.7"></path><circle cx="9" cy="19" r="1"></circle><circle cx="14" cy="18" r="1"></circle><circle cx="11" cy="21" r="1"></circle></svg>',
        foggy: '<svg viewBox="0 0 24 24" fill="currentColor" class="w-10 h-10 text-gray-300"><path d="M3 14h18M5 18h14M7 10h10" stroke="currentColor" stroke-width="2" stroke-linecap="round" fill="none" opacity="0.6"></path></svg>',
      };
      return svgs[code] || svgs.partly_cloudy;
    };
    const WeatherCard = ({ data, t }) => {
      const T = tc(t);
      const mode = data.queryMode || 'today';
      const daily = Array.isArray(data.daily) ? data.daily : [];
      if (mode === 'multi' && daily.length > 1) {
        return (
          <div style={{ background: 'linear-gradient(135deg,#325a86 0%,#25466a 100%)', boxShadow: '0 10px 30px rgba(37,70,106,.15)' }}
            className="w-full rounded-[24px] p-8 text-white flex flex-col justify-center min-h-[220px]">
            <div className="grid gap-4 w-full h-full items-center" style={{ gridTemplateColumns: `repeat(${daily.length},minmax(0,1fr))` }}>
              {daily.map((item, i) => {
                const label = i === 0 ? T.today : i === 1 ? T.tomorrow : i === 2 ? T.dayAfterTomorrow : (item.week || T.dayN(i + 1));
                const dateText = (item.date || '').length >= 10 ? item.date.slice(5).replace(/-/g, '/') : (item.date || '--/--');
                return (
                  <div key={i} className="flex flex-col items-center justify-center gap-3 px-2" style={i > 0 ? { borderLeft: '1px solid rgba(255,255,255,0.1)' } : {}}>
                    <span className={`text-[15px] ${i === 0 ? 'font-medium' : 'text-white/90'}`}>{label} ({dateText})</span>
                    <span dangerouslySetInnerHTML={{ __html: weatherIconSvg(item.iconCode || 'partly_cloudy') }} />
                    <span className="text-[14px] text-white/80">{item.dayWeather || item.nightWeather || T.weatherFallback}</span>
                    <span className="text-[16px] font-semibold mt-1">{item.dayTemp || '--'}° <span className="text-white/50 font-normal">{item.nightTemp || '--'}°</span></span>
                  </div>
                );
              })}
            </div>
          </div>
        );
      }
      // today / single_day
      const forecastLabel = mode === 'single_day' ? T.forecastTomorrow : T.forecastToday;
      const windText = [data.windDirection || '', data.windPower ? T.windLevel(data.windPower) : ''].join(' ').trim();
      return (
        <div style={{ background: 'linear-gradient(135deg,#325a86 0%,#25466a 100%)', boxShadow: '0 10px 30px rgba(37,70,106,.15)' }}
          className="w-full rounded-[24px] p-8 text-white flex flex-col justify-between relative overflow-hidden min-h-[220px]">
          <div className="flex justify-between items-start w-full">
            <div className="flex flex-col gap-1">
              <h2 className="text-3xl font-medium tracking-wide">{data.city || T.currentCity}</h2>
              <p className="text-[15px] text-white/80">{data.weather || T.weatherFallback}</p>
            </div>
            <div className="mt-[-10px]" style={{ fontSize: '5.5rem', lineHeight: 1, fontWeight: 300, letterSpacing: '-2px' }}>{data.temperature || '--'}°</div>
          </div>
          <div className="flex justify-between items-end w-full mt-4">
            <div className="flex items-center">
              <span dangerouslySetInnerHTML={{ __html: weatherIconSvg(data.iconCode || (daily[0] && daily[0].iconCode) || 'partly_cloudy') }} />
              {(data.humidity || windText) && <div style={{ width: 1, height: 32, backgroundColor: 'rgba(255,255,255,0.2)', margin: '0 16px' }} />}
              <div className="flex gap-6 text-[13px] text-white/80 leading-relaxed">
                {data.humidity && <div className="flex flex-col"><span className="text-[12px] opacity-70">{T.humidity}</span><span className="text-[15px] font-medium text-white">{data.humidity}%</span></div>}
                {windText && <div className="flex flex-col"><span className="text-[12px] opacity-70">{T.wind}</span><span className="text-[15px] font-medium text-white">{windText}</span></div>}
              </div>
            </div>
            <div className="flex flex-col items-end text-right">
              <span className="text-[12px] text-white/70 mb-1">{forecastLabel}</span>
              <span className="text-[16px] font-medium tracking-wide">{T.tempHigh} {data.highTemp || '--'}° <span className="opacity-50 mx-1">/</span> {T.tempLow} {data.lowTemp || '--'}°</span>
            </div>
          </div>
        </div>
      );
    };
    const isWeatherTool = (name) => name === 'mcp_weather_get_weather';
    const isStockQuoteTool = (name) => name === 'mcp_iwencai_hithink_market_query';
    const StockQuoteCard = ({ data, t }) => {
      const T = tc(t);
      const price = typeof data.price === 'string' ? parseFloat(data.price) : data.price;
      const changePercent = typeof data.changePercent === 'string' ? parseFloat(data.changePercent) : data.changePercent;
      const open = typeof data.open === 'string' ? parseFloat(data.open) : data.open;
      const high = typeof data.high === 'string' ? parseFloat(data.high) : data.high;
      const low = typeof data.low === 'string' ? parseFloat(data.low) : data.low;
      const isPositive = changePercent >= 0;
      const mainColor = isPositive ? 'text-[#eb4335]' : 'text-[#34a853]';
      const bgGradient = isPositive
        ? 'bg-gradient-to-br from-red-50 to-white dark:from-red-950/30 dark:to-[#1C1C1E]'
        : 'bg-gradient-to-br from-green-50 to-white dark:from-green-950/30 dark:to-[#1C1C1E]';
      const badgeColor = isPositive
        ? 'bg-red-100 text-red-600 dark:bg-red-900/40 dark:text-red-400'
        : 'bg-green-100 text-green-600 dark:bg-green-900/40 dark:text-green-400';
      const TrendIcon = isPositive ? TrendingUp : TrendingDown;
      const fmt = (v) => isNaN(v) ? '--' : v.toFixed(2);
      return (
        <div className={`w-full max-w-md rounded-[24px] shadow-xl overflow-hidden border transition-all bg-white border-slate-100 shadow-slate-200/50 dark:bg-[#1C1C1E] dark:border-white/10 dark:shadow-none`}>
          <div className={`p-6 pb-5 ${bgGradient}`}>
            <div className="flex justify-between items-start mb-4">
              <div>
                <h2 className={`text-2xl font-extrabold tracking-tight text-slate-900 dark:text-white`}>{data.name}</h2>
                <span className={`text-sm font-mono tracking-wider mt-1 block text-slate-500 dark:text-slate-400`}>{data.code}</span>
              </div>
            </div>
            <div className="flex items-end justify-between mt-6">
              <div className="flex items-baseline space-x-4">
                <span className={`text-[3.5rem] leading-none font-black tracking-tighter font-mono ${mainColor}`} style={{ fontVariantNumeric: 'tabular-nums' }}>{fmt(price)}</span>
                <div className="flex flex-col justify-end pb-1 space-y-0.5">
                  <span className={`text-sm font-bold font-mono px-1.5 py-0.5 rounded ${badgeColor}`}>{isPositive ? '+' : ''}{isNaN(changePercent) ? '--' : changePercent.toFixed(2)}%</span>
                </div>
              </div>
              <TrendIcon className={`w-12 h-12 mb-2 opacity-15 ${mainColor}`} />
            </div>
          </div>
          <div className={`h-px bg-gradient-to-r from-transparent via-current to-transparent text-slate-200 dark:text-white/10`}></div>
          <div className={`p-6 bg-white dark:bg-[#1C1C1E]`}>
            <div className="grid grid-cols-3 gap-y-4 gap-x-4">
              {[
                { label: T.stockOpen, value: fmt(open) },
                { label: T.stockHigh, value: fmt(high) },
                { label: T.stockLow, value: fmt(low) },
              ].map((item, i) => (
                <div key={i} className="flex flex-col space-y-1">
                  <span className={`text-xs font-medium text-slate-500 dark:text-slate-400`}>{item.label}</span>
                  <span className={`text-sm font-mono text-slate-800 font-medium dark:text-slate-200`}>{item.value}</span>
                </div>
              ))}
            </div>
          </div>
        </div>
      );
    };

    // ── 每工具定制结果视图（仿 Claude Code）：解析失败一律 fallback 纯文本，永不崩 ──

    // 技能市场预置卡(backendId 必须匹配 Rust SkillManifest.id)。技能=SKILL.md 目录,
    // 装到 bundle/skills/ 进 system prompt;与上方 MCP 工具(tsToolsData)并列两个子页。
    const tsSkillsData = [
      { id: 's4', backendId: 'government-writing', title: '党政机关公文写作', subtitle: '通知/意见等法定文种，套话术、层级序号、自检', category: 'skill', type: 'Skill', version: '—', latency: '本地', desc: '撰写规范的党政机关公文（通知、意见…）：内置文种结构骨架、固定话术库、层级序号体系与立账核账自检，产出结构化公文内容。配合工具商店的「公文写作」工具即可直出 GB/T 9704 合规 .docx。', icon: FileText, color: 'bg-gradient-to-b from-red-500 to-rose-700', installed: false, authRequired: false },
      { id: 's6', backendId: 'pptx', title: 'PPT 生成', subtitle: '本地直出可编辑 PowerPoint，套主题模板、真图表、带封面', category: 'skill', type: 'Skill', version: '—', latency: '本地', desc: '本地直出可编辑 PowerPoint:套主题模板、真图表、带封面,输入主题即可快速生成结构化演示文稿。', icon: Presentation, color: 'bg-gradient-to-b from-orange-400 to-rose-500', installed: false, authRequired: false },
      { id: 's7', backendId: 'visualizer', title: '数据分析可视化', subtitle: 'Chart.js 仪表盘 / 图表分析 / HTML 可视化', category: 'skill', type: 'Skill', version: '—', latency: '本地', desc: '将结构化数据、表格汇总和业务指标转成符合 Pinvou 宿主体验的 HTML 可视化仪表盘。默认使用 Chart.js、无障碍 canvas、自定义图例、扁平配色，并通过 .html 产物卡交付。', icon: LineChart, color: 'bg-gradient-to-b from-blue-500 to-cyan-600', installed: false, authRequired: false },
      { id: 's5', title: '视觉设计', subtitle: '设计系统直出网页 / banner / 海报 / 简历', category: 'skill', type: 'Skill', version: '内置', latency: '本地', desc: '内置自动技能:模型按需自动加载,以设计系统级审美直出网页 / banner / 海报 / 简历等。无需安装、随时可用。', icon: Palette, color: 'bg-gradient-to-b from-pink-400 to-fuchsia-600', installed: true, authRequired: false, builtin: true },
    ];

    const tsCategories = [
      { id: 'all', label: '全部' },
      { id: 'collab', label: '沟通协作' },
      { id: 'docs', label: '文档知识' },
      { id: 'dev', label: '研发' },
      { id: 'finance', label: '金融数据' },
      { id: 'life', label: '生活实用' },
      { id: 'skill', label: '技能' },
    ];

    // ── 列表视图双维度分组(纯函数,ToolStoreView 消费;分支互斥、按优先级短路) ──
    // 类型分组顺序即展示顺序。bundleMcpIds = 含 companion_skills 的 MCP 工具 id 列表(工具包),
    // 由 list_marketplace_tools 反建;缺省视为无工具包。
    const TOOL_TYPE_GROUPS = ['bundle', 'mcp', 'skill', 'cli', 'api', 'upcoming'];
    const getToolTypeGroup = (tool, bundleMcpIds) => {
      if (!tool) return 'upcoming';
      if (tool.userUploaded || tool.builtin || tool.category === 'skill') return 'skill';
      if (!tool.backendId) return 'upcoming';
      if (tool.feishuCli || tool.wecomCli || tool.dingtalkCli || tool.tmeetCli) return 'cli';
      if (tool.imaOpenapi) return 'api';
      if (Array.isArray(bundleMcpIds) && bundleMcpIds.includes(tool.backendId)) return 'bundle';
      // mcpServer/oauthMcp 为显式标记位,分组不依赖可本地化的 type 文案;type 正则仅作兜底。
      if (tool.oauthMcp || tool.mcpServer || /mcp/i.test(tool.type || '')) return 'mcp';
      return 'api';
    };
    // 业务分组:直接取条目 category(数据即业务类 id);技能卡单列 'skill',不参与业务分类。
    const TOOL_BUSINESS_GROUPS = ['collab', 'docs', 'dev', 'finance', 'life'];
    const getToolBusinessGroup = (tool) => {
      if (!tool) return 'life';
      if (tool.category === 'skill') return 'skill';
      return TOOL_BUSINESS_GROUPS.includes(tool.category) ? tool.category : 'life';
    };

    const TsActionBtn = ({ tool, busy, onAction, size = 'sm', t }) => {
      const T = tc(t);
      const isLg = size === 'lg';
      const actionAttrs = {
        'data-testid': 'tool-store-action',
        'data-tool-id': tool.backendId || '',
        'data-tool-title': tool.title || '',
      };
      if (tool.builtin) {
        return (
          <span className={`${isLg ? 'px-6 py-2.5 text-[15px]' : 'px-4 py-1.5 text-[13px]'} rounded-full font-bold bg-slate-100 dark:bg-[#2C2C2E] text-slate-500 dark:text-slate-400 whitespace-nowrap`}>{T.builtinEnabled}</span>
        );
      }
      if (!tool.backendId) {
        return (
          <button {...actionAttrs} disabled className={`${isLg ? 'px-10 py-2.5 text-[15px]' : 'w-20 py-1.5 text-[14px]'} rounded-full font-bold bg-slate-100 dark:bg-[#1C1C1E] border border-slate-200 dark:border-slate-700 text-slate-400 dark:text-slate-500 cursor-not-allowed`}>
            {T.comingSoon}
          </button>
        );
      }
      if (busy) {
        return (
          <button {...actionAttrs} disabled className={`${isLg ? 'px-10 py-2.5 text-[15px]' : 'w-20 py-1.5 text-[14px]'} rounded-full font-bold opacity-50 cursor-wait bg-slate-100 dark:bg-[#1C1C1E] border border-slate-200 dark:border-slate-700 text-slate-500`}>
            ...
          </button>
        );
      }
      if (tool.installed) {
        return (
          <button
            {...actionAttrs}
            onClick={(e) => { e.stopPropagation(); onAction(tool.backendId, true); }}
            className={`${isLg ? 'px-10 py-2.5 text-[15px]' : 'w-20 py-1.5 text-[14px]'} rounded-full font-bold transition-all active:scale-95 bg-slate-100 dark:bg-[#2C2C2E] border border-slate-200 dark:border-slate-700 text-[#FF3B30] dark:text-[#FF453A] hover:bg-slate-200 dark:hover:bg-[#3A3A3C]`}
          >
            {(tool.feishuCli || tool.wecomCli || tool.dingtalkCli || tool.tmeetCli || tool.imaOpenapi || tool.oauthMcp) ? T.disconnect : T.uninstall}
          </button>
        );
      }
      if (tool.oauthMcp) {
        const retry = tool.authStatus && tool.authStatus !== 'not_installed';
        return (
          <button
            {...actionAttrs}
            onClick={(e) => { e.stopPropagation(); onAction(tool.backendId, false); }}
            className={`${isLg ? 'px-10 py-2.5 text-[15px] shadow-md shadow-blue-500/20' : 'w-20 py-1.5 text-[14px]'} rounded-full font-bold transition-all active:scale-95 bg-blue-600 hover:bg-blue-700 text-white`}
          >
            {retry ? T.reauthorize : T.connect}
          </button>
        );
      }
      const hasConfig = tool.configFields && tool.configFields.length > 0;
      return (
        <button
          {...actionAttrs}
          onClick={(e) => { e.stopPropagation(); onAction(tool.backendId, false); }}
          className={`${isLg ? 'px-10 py-2.5 text-[15px] shadow-md shadow-blue-500/20' : 'w-20 py-1.5 text-[14px]'} rounded-full font-bold transition-all active:scale-95 bg-blue-600 hover:bg-blue-700 text-white`}
        >
          {(tool.feishuCli || tool.wecomCli || tool.dingtalkCli || tool.tmeetCli || tool.imaOpenapi || tool.oauthMcp) ? (hasConfig ? T.configure : T.connect) : (hasConfig ? T.configure : T.install)}
        </button>
      );
    };

    // ── 飞书连接流程卡（内联、非阻塞；取代旧的阻塞式扫码浮层）──

export { AcFmtIcon, AcShieldCheck, AcSparkles, AcArrowUpRight, AcFolder, ArtifactCard, QUIET_TOOLS, isQuietTool, toolBasename, toolSummary, isReceipt, parseReceipt, ReceiptBlock, tryParseJson, tryTailJson, looksDiff, outBox, TODO_SYM, TODO_TOOLS, OutputPre, OutputError, ListDirView, GrepView, DiffView, ShellView, ShellTextView, TodoView, tsToolsData, localizeTool, weatherIconSvg, WeatherCard, isWeatherTool, isStockQuoteTool, StockQuoteCard, tsSkillsData, tsCategories, TOOL_TYPE_GROUPS, getToolTypeGroup, TOOL_BUSINESS_GROUPS, getToolBusinessGroup, TsActionBtn };
