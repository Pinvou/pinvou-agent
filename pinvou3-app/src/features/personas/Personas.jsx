import React, { useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { AppWindow, Award, Briefcase, Check, ChevronLeft, ChevronRight, Cpu, Feather, Globe, Palette, Plus, Radio, Sparkles, Terminal, TrendingUp, User, X } from '../../components/icons.jsx';
import { IosSearchField, IosSegmentedControl } from '../../components/IosControls.jsx';
import { bridge } from '../../hooks/useBridge.js';
import { ExpertTeamsPanel } from '../workflow/WorkflowView.jsx';

const DEPT_LABELS = { academic:'学术', design:'设计', engineering:'工程', finance:'金融', 'game-development':'游戏', hr:'人力', legal:'法务', marketing:'营销', 'paid-media':'投放', product:'产品', 'project-management':'项管', sales:'销售', 'spatial-computing':'空间计算', specialized:'专项', 'supply-chain':'供应链', support:'客服', testing:'测试', tool:'工具' };
    // 部门标签按当前 UI 语言取词(t.depts),DEPT_LABELS(中文)兜底
    function deptLabelFor(t, k) { return (t && t.depts && t.depts[k]) || DEPT_LABELS[k] || k; }
    // 内置卡名称/简介按 UI 语言显示(personas-i18n.js overlay,按 id 查),中文兜底;自制卡不翻
    function personaText(c, t) {
      const L = t && t.langTag;
      if (!c || !L || L === 'zh' || c.source === 'user') return c || {};
      const overlays = window.PERSONA_I18N;
      const tr = (overlays && overlays[c.id] && overlays[c.id][L]) || null;
      if (!tr) return c;
      return { ...c, name: tr.name || c.name, description: tr.description || c.description };
    }
    const DEPT_ORDER = ['engineering','marketing','specialized','design','product','finance','sales','testing','project-management','paid-media','support','academic','game-development','spatial-computing','supply-chain','hr','legal','tool'];
    const ALL_DEPT = '__all__'; // 分类「全部」哨兵(语言中立,显示走 t.cpAll)
    const DEPT_COLOR = { academic:'#8B5CF6', design:'#EC4899', engineering:'#06B6D4', finance:'#10B981', 'game-development':'#F59E0B', hr:'#F472B6', legal:'#6B7280', marketing:'#F97316', 'paid-media':'#EF4444', product:'#7C3AED', 'project-management':'#3B82F6', sales:'#14B8A6', 'spatial-computing':'#6366F1', specialized:'#64748B', 'supply-chain':'#84CC16', support:'#22D3EE', testing:'#A855F7', tool:'#7C3AED' };
    // 可在编辑器下拉选的部门(排除 tool —— 那是内置工具卡专用)
    const DEPT_OPTIONS = DEPT_ORDER.filter(function(d){ return d !== 'tool'; });
    const deptColor = (d) => DEPT_COLOR[d] || '#9aa0a6';
    // 本地内置头像(50 张 Micah,src/avatars/),按卡 id 哈希固定分配
    const AVATAR_N = 50;
    function avatarSrc(id) {
      let h = 0; const s = String(id || '');
      for (let i = 0; i < s.length; i++) h = (h * 31 + s.charCodeAt(i)) >>> 0;
      const n = (h % AVATAR_N) + 1;
      return 'avatars/avatar-' + (n < 10 ? '0' + n : n) + '.svg';
    }
    // 头像加载失败时按部门降级到本地图标
    const DEPT_ICON = { engineering: Terminal, design: Palette, product: AppWindow, marketing: TrendingUp, finance: Briefcase, sales: TrendingUp, testing: Cpu, 'project-management': Briefcase, 'paid-media': Radio, support: User, academic: Award, 'game-development': Cpu, 'spatial-computing': Globe, 'supply-chain': Briefcase, hr: User, legal: Award, specialized: Feather, tool: Cpu };
    // App Store 风头像块:本地头像图,失败降级图标(绝不显示文字)
    const AppIcon = ({ card, isDark, cls = 'w-14 h-14 rounded-[14px]', fb = 26 }) => {
      const [err, setErr] = useState(false);
      const Fallback = DEPT_ICON[(card && card.dept)] || User;
      return (
        <div className={cls + ' shrink-0 overflow-hidden flex items-center justify-center'} style={{ background: isDark ? '#2C2C2E' : '#F2F2F7' }}>
          {!err
            ? <img src={avatarSrc(card && (card.id || card.name))} alt="" className="w-full h-full object-cover" onError={() => setErr(true)} />
            : <Fallback size={fb} style={{ color: '#8E8E93' }} strokeWidth={1.5} />}
        </div>
      );
    };
    const CLAMP3 = { display:'-webkit-box', WebkitLineClamp:3, WebkitBoxOrient:'vertical', overflow:'hidden' };
    const CLAMP2 = { display:'-webkit-box', WebkitLineClamp:2, WebkitBoxOrient:'vertical', overflow:'hidden' };

    // 聊天室左上角的专家面具挂件(工牌)。常驻:有对话就挂。
    //   未加持 → 占位卡"＋ 加持专家", 整卡点击打开专家池(入口)。
    //   已加持 → 显专家, 点卡片主体=换专家, 点 ✕=摘下。
    const Lanyard = ({ persona, isDark, onRemove, onOpenPicker, t }) => {
      // 没加持任何专家 → 整个挂件不显示
      if (!persona) return null;
      const cd = personaText(persona, t);
      return (
        <div className="absolute top-0 left-6 z-[15]" style={{ width:150, fontFamily:'-apple-system, BlinkMacSystemFont, "SF Pro Text", "PingFang SC", "Microsoft YaHei", sans-serif' }}>
          <div className="flex flex-col items-center">
            {/* 挂绳 + 扣环 */}
            <div style={{ width:2, height:52, background: isDark?'linear-gradient(#3a3a3c,#5a5a5e)':'linear-gradient(#d1d1d6,#aeaeb2)' }}></div>
            <div className="w-2.5 h-2.5 rounded-full -mt-1 mb-2" style={{ background: isDark?'#1c1c1e':'#e8e8ed', border:'2px solid '+(isDark?'#48484a':'#c7c7cc') }}></div>
            {/* 卡片 */}
            <div onClick={onOpenPicker} title={t.cpLanyardSwap}
              className="relative rounded-[14px] p-3 w-[150px] cursor-pointer transition-transform hover:-translate-y-0.5"
              style={{ background: isDark?'#1C1C1E':'#fff', border:'1px solid '+(isDark?'#2c2c2e':'rgba(0,0,0,.06)'), boxShadow:'0 8px 24px -8px rgba(0,0,0,.3)' }}>
              <button onClick={(e)=>{ e.stopPropagation(); onRemove(); }} title={t.cpLanyardRemove}
                className="absolute -top-2 -right-2 w-5 h-5 rounded-full flex items-center justify-center text-[11px] leading-none"
                style={{ background: isDark?'#2C2C2E':'#fff', color:'#8E8E93', border:'1px solid '+(isDark?'#48484a':'rgba(0,0,0,.1)'), boxShadow:'0 2px 6px rgba(0,0,0,.15)' }}>✕</button>
              <div className="flex flex-col items-center text-center gap-2">
                <AppIcon card={persona} isDark={isDark} cls="w-12 h-12 rounded-[14px]" fb={22} />
                <div className="w-full min-w-0">
                  <div className="text-[13px] font-semibold leading-tight truncate" style={{ color: isDark?'#fff':'#000' }}>{cd.name}</div>
                  <div className="text-[11px] mt-0.5 truncate" style={{ color: isDark?'rgba(235,235,245,.6)':'rgba(60,60,67,.6)' }}>{deptLabelFor(t, persona.dept)}</div>
                </div>
              </div>
            </div>
          </div>
        </div>
      );
    };

    // 双击卡片打开的详情 modal —— FLIP 转场 + 拉取并展示完整人设正文(body)。
    const PersonaDetailModal = ({ card, originRect, equipped, onClose, onEquip, isDark, t }) => {
      const panelRef = useRef(null);
      const [body, setBody] = useState(null); // null=loading
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
        if (bridge.available && bridge.readPersonaBody) {
          bridge.readPersonaBody(card.id).then(b=>setBody(b||'')).catch(()=>setBody(t.cpBodyLoadFailed));
        } else { setBody(''); }
      }, []);
      const tc = deptColor(card.dept);
      const cd = personaText(card, t);
      return (
        <div className="fixed inset-0 z-50 flex items-end md:items-center justify-center md:p-6" style={{ background:'rgba(0,0,0,.6)', backdropFilter:'blur(2px)', WebkitBackdropFilter:'blur(2px)', fontFamily:'-apple-system, BlinkMacSystemFont, "SF Pro Text", "PingFang SC", "Microsoft YaHei", sans-serif' }} onClick={onClose}>
          <div ref={panelRef} onClick={(e)=>e.stopPropagation()}
            className="w-full h-[88vh] md:h-auto md:max-h-[84vh] md:max-w-[560px] flex flex-col rounded-t-[14px] md:rounded-[14px] overflow-hidden"
            style={{ background: isDark?'#1C1C1E':'#F2F2F7', color: isDark?'#fff':'#000' }}>
            {/* 顶部:头像 + 名称/部门 + 关闭 */}
            <div className="flex items-center gap-4 px-5 pt-5 pb-4 shrink-0" style={{ background: isDark?'#000':'#fff' }}>
              <AppIcon card={card} isDark={isDark} />
              <div className="min-w-0 flex-1">
                <h2 className="text-[20px] font-semibold truncate" style={{ color: isDark?'#fff':'#000' }}>{cd.name}</h2>
                <p className="text-[13px] mt-0.5" style={{ color:'#8E8E93' }}>{deptLabelFor(t, card.dept)}{card.source==='user'?' · ' + t.cpBadgeUser:''}</p>
              </div>
              <button onClick={onClose} className="shrink-0 w-8 h-8 rounded-full flex items-center justify-center" style={{ background: isDark?'#2C2C2E':'#F2F2F7', color:'#8E8E93' }}><X size={16}/></button>
            </div>
            {cd.description ? <div className="px-5 py-3 text-[14px] shrink-0" style={{ color: isDark?'#C7C7CC':'#3C3C43', background: isDark?'#000':'#fff', borderTop:'1px solid '+(isDark?'#1C1C1E':'rgba(198,198,200,.4)') }}>{cd.description}</div> : null}
            {/* 完整人设 */}
            <div className="flex-1 overflow-y-auto custom-scrollbar px-5 py-4 min-h-0">
              <p className="text-[12px] uppercase mb-2" style={{ color:'#8E8E93' }}>{t.cpFullBody}</p>
              {body===null
                ? <div className="text-[14px] py-8 text-center" style={{ color:'#8E8E93' }}>{t.cpBodyLoading}</div>
                : <div className="persona-body text-[14px] leading-relaxed" style={{ color: isDark?'#C7C7CC':'#1C1C1E' }} dangerouslySetInnerHTML={{ __html: bridge.renderMarkdown ? bridge.renderMarkdown(body) : body }} />}
            </div>
            {/* 加持/取消 */}
            <div className="p-4 shrink-0" style={{ borderTop:'1px solid '+(isDark?'#38383A':'rgba(198,198,200,.5)') }}>
              <button onClick={()=>onEquip(card)} className="w-full py-3 rounded-[12px] text-[16px] font-semibold transition-colors"
                style={ equipped ? { background: isDark?'#2C2C2E':'#E5E5EA', color: isDark?'#fff':'#000' } : { background: isDark?'#0A84FF':'#007AFF', color:'#fff' } }>
                {equipped ? t.cpDetailUnequip : t.cpEquipShort}
              </button>
            </div>
          </div>
        </div>
      );
    };

    // 自创卡编辑器:新建 / 编辑 / ③ 草稿预填都复用它。initial.source==='user' 且有 id → 编辑(update),否则新建(create)。
    const PersonaEditorModal = ({ initial, onClose, onSaved, onDeleted, isDark, t }) => {
      const init = initial || {};
      const isEdit = !!(init.id && init.source === 'user');
      const [confirmDel, setConfirmDel] = useState(false);
      const [name, setName] = useState(init.name || '');
      const [dept, setDept] = useState(init.dept && init.dept !== 'tool' ? init.dept : 'specialized');
      const [emoji, setEmoji] = useState(init.emoji || '🃏');
      const [description, setDescription] = useState(init.description || '');
      const [body, setBody] = useState(init.body || '');
      const [saving, setSaving] = useState(false);
      const [err, setErr] = useState('');
      const [deptPickerOpen, setDeptPickerOpen] = useState(false);
      // 编辑已有卡时, summary 不含 body, 拉一次完整正文
      useEffect(() => {
        if (isEdit && !init.body && bridge.available && bridge.readPersonaBody) {
          bridge.readPersonaBody(init.id).then(function (b) { setBody(b || ''); }).catch(function () {});
        }
      }, []);
      const tc = deptColor(dept);
      const inputCls = "w-full px-5 py-4 rounded-2xl text-[15px] outline-none border transition-all";
      const inputStyle = isDark ? { background:'rgba(24,24,27,.5)', borderColor:'#3f3f46', color:'#fff' } : { background:'#fafafa', borderColor:'#e4e4e7', color:'#18181b' };
      async function save() {
        if (!name.trim()) { setErr(t.cpErrName); return; }
        if (!body.trim()) { setErr(t.cpErrBody); return; }
        setSaving(true); setErr('');
        var input = { name: name.trim(), dept: dept, emoji: emoji || '🃏', color: init.color || deptColor(dept), description: description.trim(), body: body };
        try {
          var sum = isEdit ? await bridge.updatePersona(init.id, input) : await bridge.createPersona(input);
          if (onSaved) onSaved(sum);
          onClose();
        } catch (e) { setErr(t.cpErrSave(e)); setSaving(false); }
      }
      const ph = isDark ? '#636366' : '#C7C7CC';
      const modalBg = isDark ? '#1C1C1E' : '#F2F2F7';
      const groupedCellBg = isDark ? '#2C2C2E' : '#fff';
      const separatorColor = isDark ? 'rgba(84,84,88,.65)' : 'rgba(198,198,200,.5)';
      const primaryText = isDark ? '#F2F2F7' : '#000';
      const secondaryText = isDark ? '#8E8E93' : '#8E8E93';
      return (
        <div className="fixed inset-0 z-[60] flex items-end md:items-center justify-center md:p-4"
          style={{ background:'rgba(0,0,0,.48)', backdropFilter:'blur(8px)', WebkitBackdropFilter:'blur(8px)', fontFamily: '-apple-system, BlinkMacSystemFont, "SF Pro Text", "PingFang SC", "Microsoft YaHei", sans-serif' }} onClick={onClose}>
          <style>{`
            .persona-ios-input::placeholder,
            .persona-ios-textarea::placeholder { color: ${ph}; opacity: 1; }
          `}</style>
          <div onClick={(e)=>e.stopPropagation()} className="w-full h-[90vh] md:h-auto md:max-h-[85vh] md:max-w-md flex flex-col rounded-t-[20px] md:rounded-[20px] overflow-hidden"
            style={{ background: modalBg, color: primaryText }}>
            {/* 导航栏 */}
            <div className="flex justify-between items-center px-4 h-14 shrink-0 border-b" style={{ borderColor: separatorColor }}>
              <button onClick={onClose} className="text-[17px]" style={{ color: isDark ? '#0A84FF' : '#007AFF' }}>{t.cpCancel}</button>
              <span className="text-[17px] font-semibold">{isEdit ? t.cpEditCard : t.cpNewCard}</span>
              <button onClick={save} disabled={saving} className="text-[17px] font-semibold" style={{ color: saving ? '#8E8E93' : (isDark ? '#0A84FF' : '#007AFF') }}>{saving ? t.cpSaving : (isEdit ? t.cpSaveEdit : t.cpCreate)}</button>
            </div>
            {/* 表单 */}
            <div className="flex-1 overflow-y-auto p-4 space-y-6">
              {err ? <div className="text-[13px] px-2" style={{ color:'#FF3B30' }}>{err}</div> : null}
              {/* 名称 + 部门 */}
              <div className="rounded-[12px] overflow-hidden" style={{ background: groupedCellBg }}>
                <div className="flex items-center px-4 min-h-[48px] border-b" style={{ borderColor: separatorColor }}>
                  <span className="w-20 text-[17px] shrink-0">{t.cpFieldName}</span>
                  <input value={name} onChange={e=>setName(e.target.value)} placeholder={t.cpReqPh} className="persona-ios-input flex-1 text-[17px] bg-transparent outline-none" style={{ color: primaryText }} />
                </div>
                <div className="flex items-center px-4 min-h-[48px]">
                  <span className="w-20 text-[17px] shrink-0">{t.cpDept}</span>
                  <button type="button" onClick={() => setDeptPickerOpen(true)}
                    className="flex-1 min-w-0 flex items-center justify-end gap-2 text-[17px] text-right outline-none"
                    style={{ color: secondaryText }}>
                    <span className="truncate">{deptLabelFor(t, dept)}</span>
                    <ChevronRight size={18} className="shrink-0" style={{ color: secondaryText }} />
                  </button>
                </div>
              </div>
              {/* 描述 */}
              <div className="rounded-[12px] overflow-hidden" style={{ background: groupedCellBg }}>
                <div className="px-4 py-3">
                  <textarea rows={3} value={description} onChange={e=>setDescription(e.target.value)} placeholder={t.cpFieldDescPh} className="persona-ios-textarea w-full text-[17px] bg-transparent outline-none resize-none leading-snug" style={{ color: primaryText }} />
                </div>
              </div>
              {/* 系统人设 (必填) */}
              <div>
                <p className="text-[13px] ml-4 mb-1.5 uppercase" style={{ color: secondaryText }}>{t.cpFieldBody} *  ·  {t.cpMarkdownHint}</p>
                <div className="rounded-[12px] overflow-hidden" style={{ background: groupedCellBg }}>
                  <div className="px-4 py-3">
                    <textarea rows={7} value={body} onChange={e=>setBody(e.target.value)} placeholder={t.cpBodyPh} className="persona-ios-textarea w-full text-[16px] leading-relaxed bg-transparent outline-none resize-none custom-scrollbar" style={{ color: primaryText, maxHeight:'40vh' }} />
                  </div>
                </div>
              </div>
              {/* 删除(编辑态) */}
              {isEdit ? (
                <button onClick={()=>{ if(confirmDel){ bridge.deletePersona(init.id).then(()=>{ if(onDeleted) onDeleted(init); onClose(); }); } else setConfirmDel(true); }}
                  className="w-full rounded-[12px] py-3 text-[17px] transition-colors" style={{ background: groupedCellBg, color:'#FF3B30' }}>
                  {confirmDel ? t.cpDelThisConfirm : t.cpDeleteThis}
                </button>
              ) : null}
            </div>
          </div>
          {deptPickerOpen && (
            <div className="fixed inset-0 z-[70] flex items-end md:items-center justify-center md:p-4"
              style={{ background:'rgba(0,0,0,.35)' }}
              onClick={(e) => { e.stopPropagation(); setDeptPickerOpen(false); }}>
              <div onClick={e => e.stopPropagation()}
                className="w-full md:max-w-sm max-h-[72vh] rounded-t-[20px] md:rounded-[20px] overflow-hidden shadow-2xl"
                style={{ background: modalBg, color: primaryText }}>
                <div className="h-12 flex items-center justify-between px-4 border-b" style={{ borderColor: separatorColor }}>
                  <button type="button" className="text-[17px]" style={{ color: isDark ? '#0A84FF' : '#007AFF' }} onClick={() => setDeptPickerOpen(false)}>{t.cpCancel}</button>
                  <div className="text-[17px] font-semibold">{t.cpDept}</div>
                  <div className="w-[34px]" />
                </div>
                <div className="max-h-[calc(72vh-48px)] overflow-y-auto custom-scrollbar p-2">
                  {DEPT_OPTIONS.map(function(d){
                    const active = d === dept;
                    return (
                      <button key={d} type="button"
                        onClick={() => { setDept(d); setDeptPickerOpen(false); }}
                        className="w-full min-h-[44px] px-3 rounded-[10px] flex items-center justify-between text-left text-[17px]"
                        style={{ background: active ? groupedCellBg : 'transparent', color: active ? (isDark ? '#0A84FF' : '#007AFF') : primaryText }}>
                        <span>{deptLabelFor(t, d)}</span>
                        {active ? <Check size={18} className="shrink-0" /> : null}
                      </button>
                    );
                  })}
                </div>
              </div>
            </div>
          )}
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
        <div onClick={onStart} className="relative w-full overflow-hidden cursor-pointer select-none flex items-stretch"
          style={{ height:200, borderRadius:20, background:'linear-gradient(135deg, #EEEDFE 0%, #E6F1FB 50%, #E1F5EE 100%)',
            boxShadow: isDark
              ? '0 2px 8px rgba(0,0,0,.45), 0 24px 60px rgba(0,0,0,.65)'
              : '0 8px 28px rgba(83,74,183,.16)',
            border: isDark ? '1px solid rgba(255,255,255,.18)' : '1px solid rgba(83,74,183,.14)',
            fontFamily:'-apple-system, BlinkMacSystemFont, "PingFang SC", sans-serif' }}>
          <div className="absolute rounded-full" style={{ width:240, height:240, background:'#AFA9EC', opacity:.16, top:-80, right:-40 }} />
          <div className="absolute rounded-full" style={{ width:150, height:150, background:'#9FE1CB', opacity:.15, bottom:-50, right:120 }} />
          {/* 左侧内容 */}
          <div className="relative z-10 shrink-0 flex flex-col justify-center" style={{ width:430, padding:'0 36px' }}>
            <div style={{ fontSize:28, fontWeight:800, color:'#26215C', lineHeight:1.15, letterSpacing:'-.5px', marginBottom:18 }}>{t.cpAICreate} <span style={{ color:'#534AB7' }}>{t.bnrTitleHi}</span></div>
            <button onClick={(e)=>{ e.stopPropagation(); onStart && onStart(); }} className="inline-flex items-center gap-1.5 self-start active:opacity-80" style={{ background:'#534AB7', color:'#fff', padding:'10px 22px', borderRadius:14, fontSize:14, fontWeight:600, marginBottom:18, boxShadow:'0 4px 14px rgba(83,74,183,.35)' }}>{t.bnrStart}</button>
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
                <img src={a.src} width={80} height={80} alt="" style={{ borderRadius:14, display:'block' }} />
                <div style={{ fontSize:13, fontWeight:600, color:'#26215C', marginTop:6, lineHeight:1.15, whiteSpace:'nowrap' }}>{a.name}</div>
                <div style={{ fontSize:11, color:'#534AB7', opacity:.7, lineHeight:1.15, whiteSpace:'nowrap' }}>{a.role}</div>
              </div>
            ))}
          </div>
        </div>
      );
    };
    const CardPoolView = ({ theme, t, bs, onEquipped, onAICreate, initialMyOnly }) => {
      const isDark = theme === 'dark';
      const pool = (bs && bs.personaPool) || { loadState: 'idle' };
      // 201 张卡走模块级缓存(不进 notify 快照),loadState 变化驱动重渲染。
      const list = (bridge.available && bridge.getPersonas) ? bridge.getPersonas() : [];
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
      const [confirmDelId, setConfirmDelId] = useState(null); // 卡上删除二次确认
      const [activeTab, setActiveTab] = useState('individual');

      useEffect(() => { if (bridge.available) bridge.loadPersonas(); }, []);
      useEffect(() => { setVisible(60); }, [query, activeDept, myOnly]);
      useEffect(() => { if (!toast) return; const id = setTimeout(() => setToast(null), 2400); return () => clearTimeout(id); }, [toast]);

      const counts = {}; list.forEach(c => { counts[c.dept] = (counts[c.dept]||0)+1; });
      const userCount = list.filter(c => c.source === 'user').length;
      const q = query.trim().toLowerCase();
      const filtered = list.filter(c => {
        if (myOnly && c.source !== 'user') return false;
        if (activeDept !== ALL_DEPT && c.dept !== activeDept) return false;
        if (q) {
          // 原文 + 本地化名/简介都进搜索域,中英日关键词均可命中
          const loc = personaText(c, t);
          const hay = (c.name+' '+c.description+' '+loc.name+' '+loc.description+' '+c.dept+' '+deptLabelFor(t, c.dept)).toLowerCase();
          if (hay.indexOf(q) < 0) return false;
        }
        return true;
      });
      const shown = filtered.slice(0, visible);

      function resetFacets() { setActiveDept(ALL_DEPT); setQuery(''); setMyOnly(false); }
      function editCard(card, e){ if(e) e.stopPropagation(); setEditor({ initial: card }); }
      function doDelete(card, e){ if(e) e.stopPropagation(); setConfirmDelId(null);
        Promise.resolve(bridge.deletePersona(card.id)).then(function(){ setToast(t.cpToastDeleted(card.name)); }).catch(function(){ setToast(t.cpToastDelFailed); }); }
      function onMove(e){ const el=e.currentTarget; const r=el.getBoundingClientRect(); const px=(e.clientX-r.left)/r.width, py=(e.clientY-r.top)/r.height;
        el.style.transform=`perspective(900px) rotateX(${(0.5-py)*9}deg) rotateY(${(px-0.5)*11}deg) translateY(-3px)`;
        el.style.setProperty('--mx',(px*100)+'%'); el.style.setProperty('--my',(py*100)+'%'); }
      function onLeave(e){ e.currentTarget.style.transform=''; }
      function equip(card, e){ if(e) e.stopPropagation();
        if (active && active.id===card.id) { bridge.unequipPersona(); setToast(t.cpToastUnequipped(personaText(card, t).name)); }
        else { Promise.resolve(bridge.equipPersona(card.id)).then(s => { if (s) setToast(t.cpToastEquipped(targetTitle || t.cpCurrentChat, personaText(s, t).name)); }); } }
      function openDetail(card, e){ const r=e.currentTarget.getBoundingClientRect();
        setDetail({ card, rect:{ left:r.left, top:r.top, width:r.width, height:r.height } }); }

      // 分类横向滚动箭头(PC 鼠标无左滑)
      const scrollRef = useRef(null);
      const [showL, setShowL] = useState(false);
      const [showR, setShowR] = useState(false);
      const checkScroll = () => { const el = scrollRef.current; if (!el) return; setShowL(el.scrollLeft > 2); setShowR(el.scrollLeft < el.scrollWidth - el.clientWidth - 2); };
      useEffect(() => { const el = scrollRef.current; if (!el) return; checkScroll(); el.addEventListener('scroll', checkScroll); window.addEventListener('resize', checkScroll); return () => { el.removeEventListener('scroll', checkScroll); window.removeEventListener('resize', checkScroll); }; }, [list.length]);
      const scrollPills = (dx) => { if (scrollRef.current) scrollRef.current.scrollBy({ left: dx, behavior: 'smooth' }); };
      // 右键上下文菜单(自制卡 编辑/删除,macOS 风);点别处/滚动关闭
      const [ctx, setCtx] = useState(null); // { card, x, y }
      useEffect(() => { if (!ctx) return; const close = () => setCtx(null); window.addEventListener('click', close); window.addEventListener('scroll', close, true); return () => { window.removeEventListener('click', close); window.removeEventListener('scroll', close, true); }; }, [ctx]);
      function openCtx(card, e){ if (card.source !== 'user') return; e.preventDefault(); e.stopPropagation(); setCtx({ card, x: e.clientX, y: e.clientY }); }

      return (
        <div className="flex-1 flex flex-col w-full h-full relative z-10 overflow-hidden animate-in fade-in duration-300"
          style={{ fontFamily: '-apple-system, BlinkMacSystemFont, "SF Pro Text", "PingFang SC", "Microsoft YaHei", sans-serif', background: isDark ? '#131314' : '#fff' }}>
          <div className="relative z-10 flex-1 min-h-0 overflow-y-auto p-4 custom-scrollbar sm:p-6 lg:p-10">
            <div className="max-w-[1400px] mx-auto">

              {/* 顶部: iOS Tab + 操作 */}
              <div className="border-b border-slate-200/50 px-2 dark:border-white/10">
                <div className="flex flex-col gap-3 px-6 pb-6 md:px-10 lg:flex-row lg:items-center lg:justify-between">
                  <IosSegmentedControl
                    value={activeTab}
                    onChange={setActiveTab}
                    isDark={isDark}
                    segments={[
                      { key: 'individual', label: t.expertPoolIndividualTab || '个人专家' },
                      { key: 'team', label: t.expertPoolTeamTab || '专家团队' },
                    ]}
                  />
                  <div className="flex min-w-0 flex-col gap-3 overflow-hidden lg:ml-8 lg:flex-1 lg:flex-row lg:items-center lg:justify-end">
                    {activeTab === 'individual' ? (
                      <IosSearchField
                        value={query}
                        onChange={(e) => setQuery(e.target.value)}
                        placeholder={t.cpSearchPh}
                        isDark={isDark}
                        compact
                        className="w-full min-w-0 lg:max-w-[360px] lg:flex-1"
                      />
                    ) : null}
                    <div className="flex shrink-0 items-center justify-end gap-3">
                      <button onClick={() => { setActiveTab('individual'); setMyOnly(!myOnly); }}
                        className={`inline-flex h-9 items-center rounded-full px-4 text-[13px] font-semibold shadow-sm transition-colors whitespace-nowrap ${isDark ? 'bg-[#2C2C2E] text-white hover:bg-[#3A3A3C]' : 'bg-[#E9E9EB] text-[#1D1D1F] hover:bg-[#DADADD]'}`}>
                        <User size={14} className="mr-2 opacity-70" />
                        {t.cpMyCards}
                      </button>
                      <button onClick={() => { setActiveTab('individual'); setChooser(true); }} title={t.cpNewCardTitle}
                        className="inline-flex h-9 items-center rounded-full bg-[#007AFF] px-4 text-[13px] font-semibold text-white shadow-sm transition-colors hover:bg-[#0066D6]">
                        <Plus size={14} className="mr-2" />
                        {t.cpNewCard || '新建卡牌'}
                      </button>
                    </div>
                  </div>
                </div>
              </div>

              {activeTab === 'individual' ? (
                <>
                {/* AI 造卡推广 banner(搜索框下方,一直显示) */}
                <div className="px-6 md:px-10 pt-3 pb-4">
                  <AICardBanner onStart={onAICreate} isDark={isDark} t={t} />
                </div>

                {/* 分类药丸 + 左右滚动箭头 */}
                <div className="relative px-6 md:px-10 pt-2 pb-4 group">
                  {showL ? <button onClick={() => scrollPills(-220)} className="absolute left-4 top-1/2 -translate-y-1/2 z-10 w-8 h-8 rounded-full flex items-center justify-center border opacity-0 group-hover:opacity-100 transition shadow-sm" style={{ background: isDark ? '#2C2C2E' : '#fff', color: isDark ? '#fff' : '#000', borderColor: isDark ? '#38383A' : 'rgba(198,198,200,.6)' }}><ChevronLeft size={18} /></button> : null}
                  <div ref={scrollRef} className="flex overflow-x-auto gap-2 no-scrollbar scroll-smooth">
                    {[ALL_DEPT].concat(DEPT_ORDER.filter(k => counts[k])).map(k => {
                      const isAll = k === ALL_DEPT; const on = isAll ? activeDept === ALL_DEPT : activeDept === k;
                      return (
                        <button key={k} onClick={() => setActiveDept(k)} className="h-9 whitespace-nowrap shrink-0 text-[13px] px-3.5 rounded-full font-semibold transition-colors"
                          style={ on ? { background: isDark ? '#fff' : '#3A3A3C', color: isDark ? '#000' : '#fff' } : { background: isDark ? '#2C2C2E' : '#F2F2F7', color: isDark ? '#fff' : '#000' } }>
                          {isAll ? t.cpAll : deptLabelFor(t, k)}
                        </button>
                      );
                    })}
                  </div>
                  {showR ? <button onClick={() => scrollPills(220)} className="absolute right-4 top-1/2 -translate-y-1/2 z-10 w-8 h-8 rounded-full flex items-center justify-center border opacity-0 group-hover:opacity-100 transition shadow-sm" style={{ background: isDark ? '#2C2C2E' : '#fff', color: isDark ? '#fff' : '#000', borderColor: isDark ? '#38383A' : 'rgba(198,198,200,.6)' }}><ChevronRight size={18} /></button> : null}
                </div>

                {/* 列表 */}
                <div className="px-6 md:px-10 pb-12">
                  {pool.loadState === 'loading' ? (
                    <div className="py-24 text-center text-[15px]" style={{ color: '#8E8E93' }}>{t.cpLoading}</div>
                  ) : pool.loadState === 'error' ? (
                    <div className="py-24 text-center text-[15px] text-[#FF3B30]">{t.cpLoadError}</div>
                  ) : shown.length > 0 ? (
                    <div className="grid gap-x-8 gap-y-0" style={{ gridTemplateColumns: 'repeat(auto-fill, minmax(340px, 1fr))' }}>
                      {shown.map(c => { const isEmpowered = active && active.id === c.id; const isUser = c.source === 'user'; const cd = personaText(c, t);
                        return (
                          <div key={c.id} onClick={(e) => openDetail(c, e)} onContextMenu={(e) => openCtx(c, e)}
                            className="group py-4 flex flex-col gap-2.5 border-b cursor-pointer" style={{ borderColor: isDark ? '#38383A' : 'rgba(198,198,200,.5)' }}>
                            <div className="flex items-center gap-4">
                              <AppIcon card={c} isDark={isDark} />
                              <div className="flex-1 min-w-0">
                                <h2 className="text-[17px] font-semibold tracking-tight truncate mb-0.5" style={{ color: isDark ? '#fff' : '#000' }}>{cd.name}</h2>
                                <p className="text-[13px] truncate" style={{ color: isDark ? 'rgba(235,235,245,.6)' : 'rgba(60,60,67,.6)' }}>{deptLabelFor(t, c.dept)}{isUser ? ' · ' + t.cpBadgeUser : ''}</p>
                              </div>
                              {isUser ? <button onClick={(e) => openCtx(c, e)} title={t.cpEdit} className="shrink-0 w-8 h-8 rounded-full flex items-center justify-center text-[18px] opacity-0 group-hover:opacity-100 transition-opacity" style={{ color: '#8E8E93' }}>⋯</button> : null}
                              <button onClick={(e) => equip(c, e)} className="shrink-0 text-[15px] font-bold px-5 py-1.5 rounded-full transition active:opacity-70"
                                style={ isEmpowered ? { background: isDark ? '#2C2C2E' : '#F2F2F7', color: '#8E8E93' } : { background: isDark ? '#2C2C2E' : '#F2F2F7', color: isDark ? '#0A84FF' : '#007AFF' } }>
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
                      <p className="text-[17px] font-semibold" style={{ color: isDark ? '#fff' : '#000' }}>{t.cpNoMatch}</p>
                      <p className="text-[15px] mt-1" style={{ color: '#8E8E93' }}>{t.cpEmptyHint}</p>
                    </div>
                  )}
                  {filtered.length > visible ? (
                    <div className="flex justify-center mt-6">
                      <button onClick={() => setVisible(visible + 60)} className="text-[15px] px-5 py-2" style={{ color: isDark ? '#0A84FF' : '#007AFF' }}>{t.cpShowMore(filtered.length - visible)}</button>
                    </div>
                  ) : null}
                </div>
                </>
              ) : (
                <div className="min-h-[520px]">
                  <ExpertTeamsPanel bs={bs} theme={theme} t={t} />
                </div>
              )}

            </div>
          </div>

          {/* 右键菜单（自制卡 编辑/删除，macOS 风） */}
          {ctx ? (
            <div className="fixed z-[100] min-w-[128px] rounded-[10px] overflow-hidden border text-[14px]"
              style={{ left: Math.min(ctx.x, (typeof window !== 'undefined' ? window.innerWidth : 9999) - 150), top: Math.min(ctx.y, (typeof window !== 'undefined' ? window.innerHeight : 9999) - 100), background: isDark ? '#2C2C2E' : '#fff', borderColor: isDark ? '#38383A' : 'rgba(0,0,0,.08)', boxShadow: '0 10px 40px rgba(0,0,0,.25)' }}
              onClick={(e) => e.stopPropagation()}>
              <button onClick={() => { editCard(ctx.card); setCtx(null); }} className="w-full text-left px-4 py-2.5" style={{ color: isDark ? '#fff' : '#000' }}>{t.cpMenuEdit}</button>
              <div style={{ height: 1, background: isDark ? '#38383A' : 'rgba(0,0,0,.06)' }} />
              <button onClick={() => { doDelete(ctx.card); setCtx(null); }} className="w-full text-left px-4 py-2.5" style={{ color: '#FF3B30' }}>{t.cpDelete}</button>
            </div>
          ) : null}

          {detail ? createPortal((
            <PersonaDetailModal card={detail.card} originRect={detail.rect} equipped={active && active.id===detail.card.id}
              isDark={isDark} t={t} onClose={()=>setDetail(null)}
              onEquip={(card)=>{ equip(card); }} />
          ), document.body) : null}

          {/* 自创卡编辑器(新建/编辑) —— portal 到 body,蒙层盖住侧边栏 */}
          {editor ? createPortal((
            <PersonaEditorModal initial={editor.initial} isDark={isDark} t={t}
              onClose={()=>setEditor(null)}
              onSaved={(sum)=>setToast((editor.initial && editor.initial.id ? t.cpToastSaved : t.cpToastCreated)(sum.name))}
              onDeleted={(card)=>setToast(t.cpToastDeleted(card.name))} />
          ), document.body) : null}

          {/* 造卡方式选择(iOS action sheet) —— portal 到 body,跳出卡池 z-10 上下文,蒙层才能盖住侧边栏 */}
          {chooser ? createPortal((
            <div className="fixed inset-0 z-[70] flex items-end md:items-center justify-center md:p-4" style={{ background:'rgba(0,0,0,.6)', backdropFilter:'blur(2px)', WebkitBackdropFilter:'blur(2px)' }} onClick={()=>setChooser(false)}>
              <div onClick={(e)=>e.stopPropagation()} className="w-full md:max-w-sm flex flex-col gap-2 p-3 md:p-0">
                <div className="rounded-[14px] overflow-hidden" style={{ background: isDark?'#1C1C1E':'#fff' }}>
                  <button onClick={()=>{ setChooser(false); if (onAICreate) onAICreate(); }} className="w-full flex items-center gap-3 px-4 py-4 text-left border-b" style={{ borderColor: isDark?'#38383A':'rgba(198,198,200,.5)' }}>
                    <div className="w-10 h-10 rounded-[12px] flex items-center justify-center shrink-0" style={{ background: isDark?'#0A84FF':'#007AFF' }}><Sparkles size={20} style={{ color:'#fff' }} /></div>
                    <div className="min-w-0">
                      <div className="text-[16px] font-semibold flex items-center gap-2" style={{ color: isDark?'#fff':'#000' }}>{t.cpAICreate} <span className="text-[10px] px-1.5 py-0.5 rounded" style={{ background: isDark?'rgba(10,132,255,.2)':'rgba(0,122,255,.12)', color: isDark?'#0A84FF':'#007AFF' }}>{t.chooserRecommend}</span></div>
                      <div className="text-[13px] mt-0.5" style={{ color: isDark?'rgba(235,235,245,.6)':'rgba(60,60,67,.6)' }}>{t.chooserAIDesc}</div>
                    </div>
                  </button>
                  <button onClick={()=>{ setChooser(false); setEditor({ initial: null }); }} className="w-full flex items-center gap-3 px-4 py-4 text-left">
                    <div className="w-10 h-10 rounded-[12px] flex items-center justify-center shrink-0" style={{ background: isDark?'#2C2C2E':'#F2F2F7', color: isDark?'#0A84FF':'#007AFF' }}><Plus size={20} /></div>
                    <div className="min-w-0">
                      <div className="text-[16px] font-semibold" style={{ color: isDark?'#fff':'#000' }}>{t.chooserManualTitle}</div>
                      <div className="text-[13px] mt-0.5" style={{ color: isDark?'rgba(235,235,245,.6)':'rgba(60,60,67,.6)' }}>{t.chooserManualDesc}</div>
                    </div>
                  </button>
                </div>
                <button onClick={()=>setChooser(false)} className="w-full rounded-[14px] py-3.5 text-[17px] font-semibold" style={{ background: isDark?'#2C2C2E':'#fff', color: isDark?'#0A84FF':'#007AFF' }}>{t.cpCancel}</button>
              </div>
            </div>
          ), document.body) : null}

          {/* iOS 风 toast */}
          {toast ? (
            <div className="fixed bottom-8 left-1/2 -translate-x-1/2 z-[90] px-5 py-2.5 rounded-full text-[14px] font-medium animate-in fade-in slide-in-from-bottom-2 duration-200"
              style={{ background: isDark ? 'rgba(44,44,46,.96)' : 'rgba(0,0,0,.85)', color: '#fff', boxShadow: '0 8px 30px rgba(0,0,0,.3)' }}>
              {toast}
            </div>
          ) : null}
        </div>
      );
    };

    // ==========================================
    // Shared Components
    // ==========================================

export { DEPT_LABELS, deptLabelFor, personaText, DEPT_ORDER, ALL_DEPT, DEPT_COLOR, DEPT_OPTIONS, deptColor, AVATAR_N, avatarSrc, DEPT_ICON, AppIcon, CLAMP3, CLAMP2, Lanyard, PersonaDetailModal, PersonaEditorModal, AICardBanner, CardPoolView };
