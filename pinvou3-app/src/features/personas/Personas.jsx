import { useEffect, useRef, useState, useSyncExternalStore } from 'react';
import { createPortal } from 'react-dom';
import { ChevronLeft, ChevronRight, Plus, Sparkles, User, X } from '../../components/icons.jsx';
import { IosSearchField } from '../../components/IosControls.jsx';
import { bridge } from '../../hooks/useBridge.js';
import { getSyntaxHighlightVersion, subscribeSyntaxHighlight } from '../../shared/syntax-highlighter.js';

// 共享小件(部门配色/头像块/多语言取名)抽到 persona-shared.jsx:ChatView 等
// 主 chunk 消费方直接从那里 import,卡池视图才可能独立懒加载 chunk。
import { deptLabelFor, personaText, DEPT_ORDER, ALL_DEPT, DEPT_OPTIONS, deptColor, AppIcon } from './persona-shared.jsx';

    // 双击卡片打开的详情 modal —— FLIP 转场 + 拉取并展示完整人设正文(body)。
    // 唯一消费方是本文件,留在卡池 chunk;不进 persona-shared(主 chunk 常驻,
    // 会把弹层 + bridge.rendering 依赖一并拖进主 chunk)。
    const PersonaDetailModal = ({ card, originRect, equipped, onClose, onEquip, t }) => {
      const panelRef = useRef(null);
      const [body, setBody] = useState(null); // null=loading
      // 懒语言注册完成后 bump 版本号:人设正文是一次性 innerHTML,重算恢复高亮。
      const syntaxVersion = useSyncExternalStore(subscribeSyntaxHighlight, getSyntaxHighlightVersion);
      useEffect(() => {
        const panel = panelRef.current;
        if (panel && originRect) {
          const last = panel.getBoundingClientRect();
          const dx = originRect.left - last.left, dy = originRect.top - last.top;
          const sx = Math.max(0.05, originRect.width / last.width), sy = Math.max(0.05, originRect.height / last.height);
          panel.style.transformOrigin = 'top left';
          panel.style.transform = `translate(${dx}px,${dy}px) scale(${sx},${sy})`;
          panel.style.opacity = '0.5';
          panel.getBoundingClientRect();
          panel.style.transition = 'transform .34s cubic-bezier(.2,.85,.25,1), opacity .34s';
          panel.style.transform = 'none';
          panel.style.opacity = '1';
        }
        if (bridge.available && bridge.personas.readPersonaBody) {
          bridge.personas.readPersonaBody(card.id).then(b=>setBody(b||'')).catch(()=>setBody(t.cpBodyLoadFailed));
        } else {
          // eslint-disable-next-line react-hooks/set-state-in-effect -- synchronously clear the body when there is no backend read channel
          setBody('');
        }
        // eslint-disable-next-line react-hooks/exhaustive-deps -- fetch the body once by card.id when the popover mounts; post-fetch body state is not a trigger condition
      }, []);
      const cd = personaText(card, t);
      return (
        // biome-ignore lint/a11y/useKeyWithClickEvents: backdrop click-to-close layer; the keyboard path is handled by the dialog's top-right close button
        // biome-ignore lint/a11y/noStaticElementInteractions: backdrop click-to-close layer, a non-interactive container
        <div className="fixed inset-0 z-50 flex items-end md:items-center justify-center md:p-6" style={{ background:'rgba(0,0,0,.6)', backdropFilter:'blur(2px)', WebkitBackdropFilter:'blur(2px)', fontFamily:'-apple-system, BlinkMacSystemFont, "SF Pro Text", "PingFang SC", "Microsoft YaHei", sans-serif' }} onClick={onClose}>
          {/* biome-ignore lint/a11y/useKeyWithClickEvents: click-bubbling stop layer; keyboard events need no bubbling handling */}
          {/* biome-ignore lint/a11y/noStaticElementInteractions: click-bubbling stop layer, a non-interactive container */}
          <div ref={panelRef} onClick={(e)=>e.stopPropagation()}
            className="w-full h-[88vh] md:h-auto md:max-h-[84vh] md:max-w-[560px] flex flex-col rounded-t-[14px] md:rounded-[14px] overflow-hidden bg-[#F2F2F7] dark:bg-[#1C1C1E] text-[#000] dark:text-[#fff]">
            {/* 顶部:头像 + 名称/部门 + 关闭 */}
            <div className="flex items-center gap-4 px-5 pt-5 pb-4 shrink-0 bg-[#fff] dark:bg-[#000]">
              <AppIcon card={card} />
              <div className="min-w-0 flex-1">
                <h2 className="text-[20px] font-semibold truncate text-[#000] dark:text-[#fff]">{cd.name}</h2>
                <p className="text-[13px] mt-0.5" style={{ color:'#8E8E93' }}>{deptLabelFor(t, card.dept)}{card.source==='user'?' · ' + t.cpBadgeUser:''}</p>
              </div>
              <button type="button" onClick={onClose} className="shrink-0 w-8 h-8 rounded-full flex items-center justify-center bg-[#F2F2F7] dark:bg-[#2C2C2E]" style={{ color:'#8E8E93' }}><X size={16}/></button>
            </div>
            {cd.description ? <div className="px-5 py-3 text-[14px] shrink-0 text-[#3C3C43] dark:text-[#C7C7CC] bg-[#fff] dark:bg-[#000] border-t border-[rgba(198,198,200,.4)] dark:border-[#1C1C1E]">{cd.description}</div> : null}
            {/* 完整人设 */}
            <div className="flex-1 overflow-y-auto custom-scrollbar px-5 py-4 min-h-0">
              <p className="text-[12px] uppercase mb-2" style={{ color:'#8E8E93' }}>{t.cpFullBody}</p>
              {body===null
                ? <div className="text-[14px] py-8 text-center" style={{ color:'#8E8E93' }}>{t.cpBodyLoading}</div>
                : <div key={`syntax-${syntaxVersion}`} className="persona-body text-[14px] leading-relaxed light-code dark-code text-[#1C1C1E] dark:text-[#C7C7CC]" dangerouslySetInnerHTML={{ __html: bridge.rendering.renderMarkdown ? bridge.rendering.renderMarkdown(body) : body }} />}
            </div>
            {/* 加持/取消 */}
            <div className="p-4 shrink-0 border-t border-[rgba(198,198,200,.5)] dark:border-[#38383A]">
              <button type="button" onClick={()=>onEquip(card)} className={"w-full py-3 rounded-[12px] text-[16px] font-semibold transition-colors " + (equipped ? 'bg-[#E5E5EA] dark:bg-[#2C2C2E] text-[#000] dark:text-[#fff]' : 'bg-[#0A84FF] dark:bg-[#007AFF] text-[#fff]')}>
                {equipped ? t.cpDetailUnequip : t.cpEquipShort}
              </button>
            </div>
          </div>
        </div>
      );
    };

    // 自创卡编辑器:新建 / 编辑 / ③ 草稿预填都复用它。initial.source==='user' 且有 id → 编辑(update),否则新建(create)。
    const PersonaEditorModal = ({ initial, onClose, onSaved, onDeleted, t }) => {
      const init = initial || {};
      const isEdit = !!(init.id && init.source === 'user');
      const [confirmDel, setConfirmDel] = useState(false);
      const [name, setName] = useState(init.name || '');
      const [dept, setDept] = useState(init.dept && init.dept !== 'tool' ? init.dept : 'specialized');
      const [emoji] = useState(init.emoji || '🃏');
      const [description, setDescription] = useState(init.description || '');
      const [body, setBody] = useState(init.body || '');
      const [saving, setSaving] = useState(false);
      const [err, setErr] = useState('');
      // 编辑已有卡时, summary 不含 body, 拉一次完整正文
      useEffect(() => {
        if (isEdit && !init.body && bridge.available && bridge.personas.readPersonaBody) {
          bridge.personas.readPersonaBody(init.id).then(function (b) { setBody(b || ''); }).catch(function () {});
        }
        // eslint-disable-next-line react-hooks/exhaustive-deps -- fetch the full body once by init.id when the editor mounts; body state is not a trigger condition
      }, []);
      async function save() {
        if (!name.trim()) { setErr(t.cpErrName); return; }
        if (!body.trim()) { setErr(t.cpErrBody); return; }
        setSaving(true); setErr('');
        const input = { name: name.trim(), dept, emoji: emoji || '🃏', color: init.color || deptColor(dept), description: description.trim(), body };
        try {
          const sum = isEdit ? await bridge.personas.updatePersona(init.id, input) : await bridge.personas.createPersona(input);
          if (onSaved) onSaved(sum);
          onClose();
        } catch (e) { setErr(t.cpErrSave(e)); setSaving(false); }
      }
      return (
        // biome-ignore lint/a11y/useKeyWithClickEvents: backdrop click-to-close layer; the keyboard path is handled by the nav-bar cancel button
        // biome-ignore lint/a11y/noStaticElementInteractions: backdrop click-to-close layer, a non-interactive container
        <div className="fixed inset-0 z-[60] flex items-end md:items-center justify-center md:p-4"
          style={{ background:'rgba(0,0,0,.6)', backdropFilter:'blur(2px)', WebkitBackdropFilter:'blur(2px)', fontFamily: '-apple-system, BlinkMacSystemFont, "SF Pro Text", "PingFang SC", "Microsoft YaHei", sans-serif' }} onClick={onClose}>
          {/* biome-ignore lint/a11y/useKeyWithClickEvents: click-bubbling stop layer; keyboard events need no bubbling handling */}
          {/* biome-ignore lint/a11y/noStaticElementInteractions: click-bubbling stop layer, a non-interactive container */}
          <div onClick={(e)=>e.stopPropagation()} className="w-full h-[90vh] md:h-auto md:max-h-[85vh] md:max-w-md flex flex-col rounded-t-[14px] md:rounded-[14px] overflow-hidden bg-[#F2F2F7] dark:bg-[#1C1C1E] text-[#000] dark:text-[#fff]">
            {/* 导航栏 */}
            <div className="flex justify-between items-center px-4 h-14 shrink-0 border-b border-[rgba(198,198,200,.5)] dark:border-[#38383A]">
              <button type="button" onClick={onClose} className="text-[17px] text-[#007AFF] dark:text-[#0A84FF]">{t.cpCancel}</button>
              <span className="text-[17px] font-semibold">{isEdit ? t.cpEditCard : t.cpNewCard}</span>
              <button type="button" onClick={save} disabled={saving} className={"text-[17px] font-semibold " + (saving ? 'text-[#8E8E93]' : 'text-[#007AFF] dark:text-[#0A84FF]')}>{saving ? t.cpSaving : (isEdit ? t.cpSaveEdit : t.cpCreate)}</button>
            </div>
            {/* 表单 */}
            <div className="flex-1 overflow-y-auto p-4 space-y-6">
              {err ? <div className="text-[13px] px-2" style={{ color:'#FF3B30' }}>{err}</div> : null}
              {/* 名称 + 部门 */}
              <div className="rounded-[10px] overflow-hidden bg-[#fff] dark:bg-[#000]">
                <div className="flex items-center px-4 py-3 border-b border-[rgba(198,198,200,.5)] dark:border-[#38383A]">
                  <span className="w-20 text-[17px] shrink-0">{t.cpFieldName}</span>
                  <input value={name} onChange={e=>setName(e.target.value)} placeholder={t.cpReqPh} className="flex-1 text-[17px] bg-transparent outline-none text-[#000] dark:text-[#fff]" />
                </div>
                <div className="flex items-center px-4 py-3">
                  <span className="w-20 text-[17px] shrink-0">{t.cpDept}</span>
                  <select value={dept} onChange={e=>setDept(e.target.value)} className="flex-1 text-[17px] bg-transparent outline-none text-[#000] dark:text-[#fff]">
                    {DEPT_OPTIONS.map(function(d){ return <option key={d} value={d} style={{ color:'#000' }}>{deptLabelFor(t, d)}</option>; })}
                  </select>
                </div>
              </div>
              {/* 描述 */}
              <div className="rounded-[10px] overflow-hidden bg-[#fff] dark:bg-[#000]">
                <div className="px-4 py-3">
                  <textarea rows={3} value={description} onChange={e=>setDescription(e.target.value)} placeholder={t.cpFieldDescPh} className="w-full text-[17px] bg-transparent outline-none resize-none text-[#000] dark:text-[#fff]" />
                </div>
              </div>
              {/* 系统人设 (必填) */}
              <div>
                <p className="text-[13px] ml-4 mb-1.5 uppercase" style={{ color:'#8E8E93' }}>{t.cpFieldBody} *  ·  {t.cpMarkdownHint}</p>
                <div className="rounded-[10px] overflow-hidden bg-[#fff] dark:bg-[#000]">
                  <div className="px-4 py-3">
                    <textarea rows={7} value={body} onChange={e=>setBody(e.target.value)} placeholder={t.cpBodyPh} className="w-full text-[15px] font-mono leading-relaxed bg-transparent outline-none resize-none custom-scrollbar text-[#000] dark:text-[#fff]" style={{ maxHeight:'40vh' }} />
                  </div>
                </div>
              </div>
              {/* 删除(编辑态) */}
              {isEdit ? (
                <button type="button" onClick={()=>{ if(confirmDel){ bridge.personas.deletePersona(init.id).then(()=>{ if(onDeleted) { onDeleted(init); } onClose(); }); } else { setConfirmDel(true); } }}
                  className="w-full rounded-[10px] py-3 text-[17px] transition-colors bg-[#fff] dark:bg-[#000]" style={{ color:'#FF3B30' }}>
                  {confirmDel ? t.cpDelThisConfirm : t.cpDeleteThis}
                </button>
              ) : null}
            </div>
          </div>
        </div>
      );
    };

    // AI 造卡推广 banner(卡池顶部 hero):彩色渐变 + 标题 + 开始造卡 + 三步条 + 右侧浮动本地头像。
    // 整条/按钮点击 → onStart(=startAICard)。深浅模式同一条彩色卡。窄屏由 .bnr-avatars 媒体查询隐藏头像。
    const AICardBanner = ({ onStart, isDark, t }) => {
      const F = (t && t.bnrFaces) || [];
      const face = (i) => ({ name: (F[i] && F[i][0]) || '', role: (F[i] && F[i][1]) || '' });
      const avs = [
        { src:'avatars/banner-1.svg', ...face(0) },
        { src:'avatars/banner-3.svg', ...face(1) },
        { src:'avatars/banner-5.svg', ...face(2) },
        { src:'avatars/banner-2.svg', ...face(3) },
        { src:'avatars/banner-4.svg', ...face(4) },
      ];
      return (
        // biome-ignore lint/a11y/useKeyWithClickEvents: the whole banner is clickable as a mouse shortcut; the keyboard path is handled by the real start-card button
        // biome-ignore lint/a11y/noStaticElementInteractions: promo banner click hotspot, not a standalone interactive control
        <div onClick={onStart} className="relative w-full overflow-hidden cursor-pointer select-none flex items-stretch"
          style={{ height:200, borderRadius:20, background:'linear-gradient(135deg, #EEEDFE 0%, #E6F1FB 50%, #E1F5EE 100%)',
            boxShadow: isDark // isDark dynamic-value: 保留 (multi-stop boxShadow)
              ? '0 2px 8px rgba(0,0,0,.45), 0 24px 60px rgba(0,0,0,.65)'
              : '0 8px 28px rgba(83,74,183,.16)',
            border: isDark ? '1px solid rgba(255,255,255,.18)' : '1px solid rgba(83,74,183,.14)', // isDark dynamic-value: 保留 (rgba border + banner 整体 isDark 同深浅)
            fontFamily:'-apple-system, BlinkMacSystemFont, "PingFang SC", sans-serif' }}>
          <div className="absolute rounded-full" style={{ width:240, height:240, background:'#AFA9EC', opacity:.16, top:-80, right:-40 }} />
          <div className="absolute rounded-full" style={{ width:150, height:150, background:'#9FE1CB', opacity:.15, bottom:-50, right:120 }} />
          {/* 左侧内容 */}
          <div className="relative z-10 shrink-0 flex flex-col justify-center" style={{ width:430, padding:'0 36px' }}>
            <div style={{ fontSize:28, fontWeight:800, color:'#26215C', lineHeight:1.15, letterSpacing:'-.5px', marginBottom:18 }}>{t.cpAICreate} <span style={{ color:'#534AB7' }}>{t.bnrTitleHi}</span></div>
            <button type="button" onClick={(e)=>{ e.stopPropagation(); onStart && onStart(); }} className="inline-flex items-center gap-1.5 self-start active:opacity-80" style={{ background:'#534AB7', color:'#fff', padding:'10px 22px', borderRadius:14, fontSize:14, fontWeight:600, marginBottom:18, boxShadow:'0 4px 14px rgba(83,74,183,.35)' }}>{t.bnrStart}</button>
            <div className="flex gap-2">
              {[['1',t.bnrStep1],['2',t.bnrStep2],['3',t.bnrStep3]].map(s => (
                <div key={s[0]} className="flex items-center gap-1.5" style={{ background:'rgba(255,255,255,.85)', borderRadius:10, padding:'7px 11px', border:'1px solid rgba(175,169,236,.3)' }}>
                  <span className="flex items-center justify-center shrink-0" style={{ width:18, height:18, borderRadius:'50%', background:'#EEEDFE', color:'#534AB7', fontSize:10, fontWeight:700 }}>{s[0]}</span>
                  <span style={{ fontSize:11.5, fontWeight:600, color:'#26215C', whiteSpace:'nowrap' }}>{s[1]}</span>
                </div>
              ))}
            </div>
          </div>
          {/* 右侧:头像横向一排,垂直居中,各自浮动 */}
          <div className="bnr-avatars relative z-0 flex-1 flex items-center justify-evenly" style={{ paddingRight:16 }}>
            {avs.map((a,i) => (
              <div key={i} className="flex flex-col items-center shrink-0" style={{ background:'#fff', borderRadius:18, padding:'14px 14px 11px', boxShadow:'0 10px 26px rgba(83,74,183,.22)', animation:`bnrFloat${(i%3)+1} ${(3.2+i*0.25).toFixed(2)}s ease-in-out infinite ${(i*0.3).toFixed(1)}s` }}>
                <img src={a.src} width={80} height={80} alt="" loading="lazy" decoding="async" style={{ borderRadius:14, display:'block' }} />
                <div style={{ fontSize:13, fontWeight:600, color:'#26215C', marginTop:6, lineHeight:1.15, whiteSpace:'nowrap' }}>{a.name}</div>
                <div style={{ fontSize:11, color:'#534AB7', opacity:.7, lineHeight:1.15, whiteSpace:'nowrap' }}>{a.role}</div>
              </div>
            ))}
          </div>
        </div>
      );
    };
    const CardPoolView = ({ theme, t, bs, onAICreate, initialMyOnly }) => {
      const isDark = theme === 'dark';
      const pool = (bs && bs.personaPool) || { loadState: 'idle' };
      // 268 张卡走模块级缓存(不进 notify 快照),loadState 变化驱动重渲染。
      const list = (bridge.available && bridge.personas.getPersonas) ? bridge.personas.getPersonas() : [];
      const active = (bs && bs.activePersona) || null;
      // 加持目标 = 当前对话(equipPersona 注入到 state.activeSessionId)。让用户始终知道注入到哪。
      const target = (bs && bs.sessions && bs.activeSessionId) ? bs.sessions.find(s => s.id === bs.activeSessionId) : null;
      const targetTitle = target ? (target.title || t.newChat) : null;
      const [activeDept, setActiveDept] = useState(ALL_DEPT);
      const [query, setQuery] = useState('');
      const [visible, setVisible] = useState(60);
      const [detail, setDetail] = useState(null);
      const [toast, setToast] = useState(null);
      const [editor, setEditor] = useState(null); // null | { initial }
      const [chooser, setChooser] = useState(false); // 造卡方式选择(AI/手动)
      const [myOnly, setMyOnly] = useState(!!initialMyOnly); // 「我的卡牌」facet(从存入确认窗"去查看"进来则默认开)
      // biome-ignore lint/correctness/noUnusedVariables: delete-confirmation state is only written in the event flow (legacy placeholder); list rows never read it
      const [confirmDelId, setConfirmDelId] = useState(null); // eslint-disable-line no-unused-vars, sonarjs/no-unused-vars, sonarjs/no-dead-store -- delete-confirmation state is only written in the event flow (legacy placeholder); list rows never read it

      useEffect(() => { if (bridge.available) bridge.personas.loadPersonas(); }, []);
      useEffect(() => {
        // eslint-disable-next-line react-hooks/set-state-in-effect -- synchronously reset the visible count on filter changes; a one-off mirror
        setVisible(60);
      }, [query, activeDept, myOnly]);
      useEffect(() => { if (!toast) { return; } const id = setTimeout(() => setToast(null), 2400); return () => clearTimeout(id); }, [toast]);

      const counts = {}; list.forEach(c => { counts[c.dept] = (counts[c.dept]||0)+1; });
      const q = query.trim().toLowerCase();
      const filtered = list.filter(c => {
        if (myOnly && c.source !== 'user') return false;
        if (activeDept !== ALL_DEPT && c.dept !== activeDept) return false;
        if (q) {
          // 原文 + 本地化名/简介都进搜索域,中英日关键词均可命中
          const loc = personaText(c, t);
          const hay = (c.name+' '+c.description+' '+loc.name+' '+loc.description+' '+c.dept+' '+deptLabelFor(t, c.dept)).toLowerCase();
          if (!hay.includes(q)) return false;
        }
        return true;
      });
      const shown = filtered.slice(0, visible);

      function editCard(card, e){ if(e) { e.stopPropagation(); } setEditor({ initial: card }); }
      function doDelete(card, e){ if(e) { e.stopPropagation(); } setConfirmDelId(null);
        Promise.resolve(bridge.personas.deletePersona(card.id)).then(function(){ setToast(t.cpToastDeleted(card.name)); }).catch(function(){ setToast(t.cpToastDelFailed); }); }
      // The card 3D hover effect (onMove/onLeave) and the resetFacets quick view reset are not wired up; the original implementation remains in git history
      function equip(card, e){ if(e) { e.stopPropagation(); }
        if (active && active.id===card.id) { bridge.personas.unequipPersona(); setToast(t.cpToastUnequipped(personaText(card, t).name)); }
        else { Promise.resolve(bridge.personas.equipPersona(card.id)).then(s => { if (s) setToast(t.cpToastEquipped(targetTitle || t.cpCurrentChat, personaText(s, t).name)); }); } }
      function openDetail(card, e){ const r=e.currentTarget.getBoundingClientRect();
        setDetail({ card, rect:{ left:r.left, top:r.top, width:r.width, height:r.height } }); }

      // 分类横向滚动箭头(PC 鼠标无左滑)
      const scrollRef = useRef(null);
      const [showL, setShowL] = useState(false);
      const [showR, setShowR] = useState(false);
      const checkScroll = () => { const el = scrollRef.current; if (!el) { return; } setShowL(el.scrollLeft > 2); setShowR(el.scrollLeft < el.scrollWidth - el.clientWidth - 2); };
      useEffect(() => { const el = scrollRef.current; if (!el) { return; } checkScroll(); el.addEventListener('scroll', checkScroll); window.addEventListener('resize', checkScroll); return () => { el.removeEventListener('scroll', checkScroll); window.removeEventListener('resize', checkScroll); }; }, [list.length]);
      const scrollPills = (dx) => { if (scrollRef.current) { scrollRef.current.scrollBy({ left: dx, behavior: 'smooth' }); } };
      // 右键上下文菜单(自制卡 编辑/删除,macOS 风);点别处/滚动关闭
      const [ctx, setCtx] = useState(null); // { card, x, y }
      useEffect(() => { if (!ctx) { return; } const close = () => setCtx(null); window.addEventListener('click', close); window.addEventListener('scroll', close, true); return () => { window.removeEventListener('click', close); window.removeEventListener('scroll', close, true); }; }, [ctx]);
      function openCtx(card, e){ if (card.source !== 'user') { return; } e.preventDefault(); e.stopPropagation(); setCtx({ card, x: e.clientX, y: e.clientY }); }

      return (
        <div className="flex-1 flex flex-col w-full h-full relative z-10 overflow-hidden animate-in fade-in duration-300 bg-[#fff] dark:bg-[#131314]"
          style={{ fontFamily: '-apple-system, BlinkMacSystemFont, "SF Pro Text", "PingFang SC", "Microsoft YaHei", sans-serif' }}>
          <div className="relative z-10 flex-1 min-h-0 overflow-y-auto p-4 custom-scrollbar sm:p-6 lg:p-10">
            <div className="max-w-[1400px] mx-auto">

              {/* 顶部操作 */}
              <div className="border-b border-slate-200/50 dark:border-white/10">
                <div className="flex flex-col gap-3 pb-6 lg:flex-row lg:items-center lg:justify-between">
                  <div className="flex min-w-0 flex-col gap-3 overflow-hidden lg:ml-8 lg:flex-1 lg:flex-row lg:items-center lg:justify-end">
                    <IosSearchField
                      value={query}
                      onChange={(e) => setQuery(e.target.value)}
                      placeholder={t.cpSearchPh}
                      isDark={isDark}
                      compact
                      className="w-full min-w-0 lg:max-w-[360px] lg:flex-1"
                    />
                    <div className="flex shrink-0 items-center justify-end gap-3">
                      <button type="button"
                        data-testid="my-personas-toggle"
                        data-active={myOnly ? 'true' : 'false'}
                        onClick={() => setMyOnly(!myOnly)}
                        className="inline-flex h-9 items-center rounded-full px-4 text-[13px] font-semibold shadow-sm transition-colors whitespace-nowrap bg-[#E9E9EB] dark:bg-[#2C2C2E] text-[#1D1D1F] dark:text-white hover:bg-[#DADADD] dark:hover:bg-[#3A3A3C]">
                        <User size={14} className="mr-2 opacity-70" />
                        {t.cpMyCards}
                      </button>
                      <button type="button" onClick={() => setChooser(true)} title={t.cpNewCardTitle}
                        className="inline-flex h-9 items-center rounded-full bg-[#007AFF] px-4 text-[13px] font-semibold text-white shadow-sm transition-colors hover:bg-[#0066D6]">
                        <Plus size={14} className="mr-2" />
                        {t.cpNewCard}
                      </button>
                    </div>
                  </div>
                </div>
              </div>

              <>
                {/* AI 造卡推广 banner(搜索框下方,一直显示) */}
                <div className="pt-3 pb-4">
                  <AICardBanner onStart={onAICreate} isDark={isDark} t={t} />
                </div>

                {/* 分类药丸 + 左右滚动箭头 */}
                <div className="relative pt-2 pb-4 group">
                  {showL ? <button type="button" onClick={() => scrollPills(-220)} className="absolute left-0 top-1/2 -translate-y-1/2 z-10 w-8 h-8 rounded-full flex items-center justify-center border opacity-0 group-hover:opacity-100 transition shadow-sm bg-[#fff] dark:bg-[#2C2C2E] text-[#000] dark:text-[#fff] border-[rgba(198,198,200,.6)] dark:border-[#38383A]"><ChevronLeft size={18} /></button> : null}
                  <div ref={scrollRef} className="flex overflow-x-auto gap-2 no-scrollbar scroll-smooth">
                    {[ALL_DEPT, ...DEPT_ORDER.filter(k => counts[k])].map(k => {
                      const isAll = k === ALL_DEPT; const on = isAll ? activeDept === ALL_DEPT : activeDept === k;
                      return (
                        <button type="button" key={k} onClick={() => setActiveDept(k)} className={"h-9 whitespace-nowrap shrink-0 text-[13px] px-3.5 rounded-full font-semibold transition-colors " + (on ? 'bg-[#3A3A3C] dark:bg-[#fff] text-[#fff] dark:text-[#000]' : 'bg-[#F2F2F7] dark:bg-[#2C2C2E] text-[#000] dark:text-[#fff]')}>
                          {isAll ? t.cpAll : deptLabelFor(t, k)}
                        </button>
                      );
                    })}
                  </div>
                  {showR ? <button type="button" onClick={() => scrollPills(220)} className="absolute right-0 top-1/2 -translate-y-1/2 z-10 w-8 h-8 rounded-full flex items-center justify-center border opacity-0 group-hover:opacity-100 transition shadow-sm bg-[#fff] dark:bg-[#2C2C2E] text-[#000] dark:text-[#fff] border-[rgba(198,198,200,.6)] dark:border-[#38383A]"><ChevronRight size={18} /></button> : null}
                </div>

                {/* 列表 */}
                <div className="pb-12">
                  {pool.loadState === 'loading' ? (
                    <div className="py-24 text-center text-[15px]" style={{ color: '#8E8E93' }}>{t.cpLoading}</div>
                  ) : pool.loadState === 'error' ? (
                    <div className="py-24 text-center text-[15px] text-[#FF3B30]">{t.cpLoadError}</div>
                  ) : shown.length > 0 ? (
                    <div className="grid gap-x-8 gap-y-0" style={{ gridTemplateColumns: 'repeat(auto-fill, minmax(340px, 1fr))' }}>
                      {shown.map(c => { const isEmpowered = active && active.id === c.id; const isUser = c.source === 'user'; const cd = personaText(c, t);
                        return (
                          // biome-ignore lint/a11y/useKeyWithClickEvents: card-row click is a shortcut; the keyboard path is handled by the in-row action buttons
                          // biome-ignore lint/a11y/noStaticElementInteractions: card-row click hotspot, not a standalone interactive control
                          <div key={c.id} onClick={(e) => openDetail(c, e)} onContextMenu={(e) => openCtx(c, e)}
                            className="group py-4 flex flex-col gap-2.5 border-b cursor-pointer border-b-[rgba(198,198,200,.5)] dark:border-b-[#38383A]">
                            <div className="flex items-center gap-4">
                              <AppIcon card={c} />
                              <div className="flex-1 min-w-0">
                                <h2 className="text-[17px] font-semibold tracking-tight truncate mb-0.5 text-[#000] dark:text-[#fff]">{cd.name}</h2>
                                <p className="text-[13px] truncate text-[rgba(60,60,67,.6)] dark:text-[rgba(235,235,245,.6)]">{deptLabelFor(t, c.dept)}{isUser ? ' · ' + t.cpBadgeUser : ''}</p>
                              </div>
                              {isUser ? <button type="button" onClick={(e) => openCtx(c, e)} title={t.cpEdit} className="shrink-0 w-8 h-8 rounded-full flex items-center justify-center text-[18px] opacity-0 group-hover:opacity-100 transition-opacity" style={{ color: '#8E8E93' }}>⋯</button> : null}
                              <button type="button" onClick={(e) => equip(c, e)} className={"shrink-0 text-[15px] font-bold px-5 py-1.5 rounded-full transition active:opacity-70 bg-[#F2F2F7] dark:bg-[#2C2C2E] " + (isEmpowered ? 'text-[#8E8E93]' : 'text-[#007AFF] dark:text-[#0A84FF]')}>
                                {isEmpowered ? t.cpUnequip : t.cpEquipShort}
                              </button>
                            </div>
                            <p className="text-[13px] leading-snug line-clamp-2" style={{ color: '#8E8E93' }}>{cd.description || t.cpNoDesc}</p>
                          </div>
                        );
                      })}
                    </div>
                  ) : (
                    <div className="py-24 text-center">
                      <p className="text-[17px] font-semibold text-[#000] dark:text-[#fff]">{t.cpNoMatch}</p>
                      <p className="text-[15px] mt-1" style={{ color: '#8E8E93' }}>{t.cpEmptyHint}</p>
                    </div>
                  )}
                  {filtered.length > visible ? (
                    <div className="flex justify-center mt-6">
                      <button type="button" onClick={() => setVisible(visible + 60)} className="text-[15px] px-5 py-2 text-[#007AFF] dark:text-[#0A84FF]">{t.cpShowMore(filtered.length - visible)}</button>
                    </div>
                  ) : null}
                </div>
              </>

            </div>
          </div>

          {/* 右键菜单（自制卡 编辑/删除，macOS 风） */}
          {ctx ? (
            // biome-ignore lint/a11y/useKeyWithClickEvents: click-bubbling stop layer; the keyboard path is handled by the real buttons inside the menu
            // biome-ignore lint/a11y/noStaticElementInteractions: context-menu positioning container; menu items are real buttons
            <div className="fixed z-[100] min-w-[128px] rounded-[10px] overflow-hidden border text-[14px] bg-[#fff] dark:bg-[#2C2C2E] border-[rgba(0,0,0,.08)] dark:border-[#38383A]"
              style={{ left: Math.min(ctx.x, (typeof window === 'undefined' ? 9999 : window.innerWidth) - 150), top: Math.min(ctx.y, (typeof window === 'undefined' ? 9999 : window.innerHeight) - 100), boxShadow: '0 10px 40px rgba(0,0,0,.25)' }}
              onClick={(e) => e.stopPropagation()}>
              <button type="button" onClick={() => { editCard(ctx.card); setCtx(null); }} className="w-full text-left px-4 py-2.5 text-[#000] dark:text-[#fff]">{t.cpMenuEdit}</button>
              <div className="h-px bg-[rgba(0,0,0,.06)] dark:bg-[#38383A]" />
              <button type="button" onClick={() => { doDelete(ctx.card); setCtx(null); }} className="w-full text-left px-4 py-2.5" style={{ color: '#FF3B30' }}>{t.cpDelete}</button>
            </div>
          ) : null}

          {detail ? createPortal((
            <PersonaDetailModal card={detail.card} originRect={detail.rect} equipped={active && active.id===detail.card.id}
              t={t} onClose={()=>setDetail(null)}
              onEquip={(card)=>{ equip(card); }} />
          ), document.body) : null}

          {/* 自创卡编辑器(新建/编辑) —— portal 到 body,蒙层盖住侧边栏 */}
          {editor ? createPortal((
            <PersonaEditorModal initial={editor.initial} t={t}
              onClose={()=>setEditor(null)}
              onSaved={(sum)=>setToast((editor.initial && editor.initial.id ? t.cpToastSaved : t.cpToastCreated)(sum.name))}
              onDeleted={(card)=>setToast(t.cpToastDeleted(card.name))} />
          ), document.body) : null}

          {/* 造卡方式选择(iOS action sheet) —— portal 到 body,跳出卡池 z-10 上下文,蒙层才能盖住侧边栏 */}
          {chooser ? createPortal((
            // biome-ignore lint/a11y/useKeyWithClickEvents: backdrop click-to-close layer; the keyboard path is handled by the real buttons inside the panel
            // biome-ignore lint/a11y/noStaticElementInteractions: backdrop click-to-close layer, a non-interactive container
            <div className="fixed inset-0 z-[70] flex items-end md:items-center justify-center md:p-4" style={{ background:'rgba(0,0,0,.6)', backdropFilter:'blur(2px)', WebkitBackdropFilter:'blur(2px)' }} onClick={()=>setChooser(false)}>
              {/* biome-ignore lint/a11y/useKeyWithClickEvents: click-bubbling stop layer; keyboard events need no bubbling handling */}
              {/* biome-ignore lint/a11y/noStaticElementInteractions: click-bubbling stop layer, a non-interactive container */}
              <div onClick={(e)=>e.stopPropagation()} className="w-full md:max-w-sm flex flex-col gap-2 p-3 md:p-0">
                <div className="rounded-[14px] overflow-hidden bg-[#fff] dark:bg-[#1C1C1E]">
                  <button type="button" onClick={()=>{ setChooser(false); if (onAICreate) onAICreate(); }} className="w-full flex items-center gap-3 px-4 py-4 text-left border-b border-[rgba(198,198,200,.5)] dark:border-[#38383A]">
                    <div className="w-10 h-10 rounded-[12px] flex items-center justify-center shrink-0 bg-[#007AFF] dark:bg-[#0A84FF]"><Sparkles size={20} style={{ color:'#fff' }} /></div>
                    <div className="min-w-0">
                      <div className="text-[16px] font-semibold flex items-center gap-2 text-[#000] dark:text-[#fff]">{t.cpAICreate} <span className="text-[10px] px-1.5 py-0.5 rounded bg-[rgba(0,122,255,.12)] dark:bg-[rgba(10,132,255,.2)] text-[#007AFF] dark:text-[#0A84FF]">{t.chooserRecommend}</span></div>
                      <div className="text-[13px] mt-0.5 text-[rgba(60,60,67,.6)] dark:text-[rgba(235,235,245,.6)]">{t.chooserAIDesc}</div>
                    </div>
                  </button>
                  <button type="button" onClick={()=>{ setChooser(false); setEditor({ initial: null }); }} className="w-full flex items-center gap-3 px-4 py-4 text-left">
                    <div className="w-10 h-10 rounded-[12px] flex items-center justify-center shrink-0 bg-[#F2F2F7] dark:bg-[#2C2C2E] text-[#007AFF] dark:text-[#0A84FF]"><Plus size={20} /></div>
                    <div className="min-w-0">
                      <div className="text-[16px] font-semibold text-[#000] dark:text-[#fff]">{t.chooserManualTitle}</div>
                      <div className="text-[13px] mt-0.5 text-[rgba(60,60,67,.6)] dark:text-[rgba(235,235,245,.6)]">{t.chooserManualDesc}</div>
                    </div>
                  </button>
                </div>
                <button type="button" onClick={()=>setChooser(false)} className="w-full rounded-[14px] py-3.5 text-[17px] font-semibold bg-[#fff] dark:bg-[#2C2C2E] text-[#007AFF] dark:text-[#0A84FF]">{t.cpCancel}</button>
              </div>
            </div>
          ), document.body) : null}

          {/* iOS 风 toast */}
          {toast ? (
            <div className="fixed bottom-8 left-1/2 -translate-x-1/2 z-[90] px-5 py-2.5 rounded-full text-[14px] font-medium animate-in fade-in slide-in-from-bottom-2 duration-200 bg-[rgba(0,0,0,.85)] dark:bg-[rgba(44,44,46,.96)] text-[#fff]"
              style={{ boxShadow: '0 8px 30px rgba(0,0,0,.3)' }}>
              {toast}
            </div>
          ) : null}
        </div>
      );
    };

    // ==========================================
    // Shared Components
    // ==========================================

export { PersonaEditorModal, AICardBanner, CardPoolView };
