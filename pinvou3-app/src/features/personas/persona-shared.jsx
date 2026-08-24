// 人格卡共享件:ChatView / tool-renderers / SubagentTranscriptPanel 等主 chunk
// 消费的小件(头像块、部门配色、多语言取名)从 Personas.jsx 抽出,使卡池视图
// (CardPoolView/PersonaEditorModal)可独立成懒加载 chunk,不再把
// 主 chunk 的常驻引用一并拖进卡池。内容与 Personas.jsx 原实现的唯一差异:
// AppIcon 的头像 img 补了 loading="lazy" decoding="async"(本 PR 主题内优化)。
import { useState } from 'react';
import { AppWindow, Award, Briefcase, Cpu, Feather, Globe, Lock, Navigation, Palette, Radio, Terminal, TrendingUp, User } from '../../components/icons.jsx';
const DEPT_LABELS = { academic:'学术', design:'设计', engineering:'工程', finance:'金融', 'game-development':'游戏', gis:'地理信息', hr:'人力', legal:'法务', marketing:'营销', 'paid-media':'投放', product:'产品', 'project-management':'项管', sales:'销售', security:'安全', 'spatial-computing':'空间计算', specialized:'专项', 'supply-chain':'供应链', support:'客服', testing:'测试', tool:'工具' };
    // 部门标签按当前 UI 语言取词(t.depts),DEPT_LABELS(中文)兜底
export function deptLabelFor(t, k) { return (t && t.depts && t.depts[k]) || DEPT_LABELS[k] || k; }
    // 内置卡名称/简介按 UI 语言显示(personas-i18n.js overlay,按 id 查),中文兜底;自制卡不翻
export function personaText(c, t) {
      const L = t && t.langTag;
      if (!c || !L || L === 'zh' || c.source === 'user') return c || {};
      const overlays = window.PERSONA_I18N;
      const tr = (overlays && overlays[c.id] && overlays[c.id][L]) || null;
      if (!tr) return c;
      return { ...c, name: tr.name || c.name, description: tr.description || c.description };
    }
export const DEPT_ORDER = ['engineering','marketing','specialized','design','product','finance','sales','testing','project-management','paid-media','support','academic','game-development','spatial-computing','gis','security','supply-chain','hr','legal','tool'];
export const ALL_DEPT = '__all__'; // 分类「全部」哨兵(语言中立,显示走 t.cpAll)
export const DEPT_COLOR = { academic:'#8B5CF6', design:'#EC4899', engineering:'#06B6D4', finance:'#10B981', 'game-development':'#F59E0B', gis:'#0EA5E9', hr:'#F472B6', legal:'#6B7280', marketing:'#F97316', 'paid-media':'#EF4444', product:'#7C3AED', 'project-management':'#3B82F6', sales:'#14B8A6', security:'#F43F5E', 'spatial-computing':'#6366F1', specialized:'#64748B', 'supply-chain':'#84CC16', support:'#22D3EE', testing:'#A855F7', tool:'#7C3AED' };
    // 可在编辑器下拉选的部门(排除 tool —— 那是内置工具卡专用)
export const DEPT_OPTIONS = DEPT_ORDER.filter(function(d){ return d !== 'tool'; });
export const deptColor = (d) => DEPT_COLOR[d] || '#9aa0a6';
    // 本地内置头像(50 张 Micah,src/avatars/),按卡 id 哈希固定分配
    const AVATAR_N = 50;
export function avatarSrc(id) {
      let h = 0; const s = String(id || '');
      for (let i = 0; i < s.length; i++) h = (h * 31 + s.codePointAt(i)) >>> 0;
      const n = (h % AVATAR_N) + 1;
      return 'avatars/avatar-' + (n < 10 ? '0' + n : n) + '.svg';
    }
    // 头像加载失败时按部门降级到本地图标
    const DEPT_ICON = { engineering: Terminal, design: Palette, product: AppWindow, marketing: TrendingUp, finance: Briefcase, sales: TrendingUp, testing: Cpu, 'project-management': Briefcase, 'paid-media': Radio, support: User, academic: Award, 'game-development': Cpu, 'spatial-computing': Globe, gis: Navigation, security: Lock, 'supply-chain': Briefcase, hr: User, legal: Award, specialized: Feather, tool: Cpu };
    // App Store 风头像块:本地头像图,失败降级图标(绝不显示文字)
export const AppIcon = ({ card, cls = 'w-14 h-14 rounded-[14px]', fb = 26 }) => {
      const [err, setErr] = useState(false);
      const Fallback = DEPT_ICON[(card && card.dept)] || User;
      return (
        <div className={cls + ' shrink-0 overflow-hidden flex items-center justify-center bg-[#F2F2F7] dark:bg-[#2C2C2E]'}>
          {err
            ? <Fallback size={fb} style={{ color: '#8E8E93' }} strokeWidth={1.5} />
            : <img src={avatarSrc(card && (card.id || card.name))} alt="" loading="lazy" decoding="async" className="w-full h-full object-cover" onError={() => setErr(true)} />}
        </div>
      );
    };

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
            {/* isDark dynamic-value: 保留 (linear-gradient 无法纯 className 化) */}
            <div style={{ width:2, height:52, background: isDark?'linear-gradient(#3a3a3c,#5a5a5e)':'linear-gradient(#d1d1d6,#aeaeb2)' }}></div>
            <div className="w-2.5 h-2.5 rounded-full -mt-1 mb-2 bg-[#e8e8ed] dark:bg-[#1c1c1e] border-2 border-[#c7c7cc] dark:border-[#48484a]"></div>
            {/* 卡片 */}
            {/* biome-ignore lint/a11y/useKeyWithClickEvents: 挂绳卡点击为鼠标快捷方式,键盘路径待卡池弹层内控件承担 */}
            {/* biome-ignore lint/a11y/noStaticElementInteractions: 挂绳卡点击热区,非独立交互控件 */}
            <div onClick={onOpenPicker} title={t.cpLanyardSwap}
              className="relative rounded-[14px] p-3 w-[150px] cursor-pointer transition-transform hover:-translate-y-0.5 bg-[#fff] dark:bg-[#1C1C1E] border border-[rgba(0,0,0,.06)] dark:border-[#2c2c2e]"
              style={{ boxShadow:'0 8px 24px -8px rgba(0,0,0,.3)' }}>
              <button type="button" onClick={(e)=>{ e.stopPropagation(); onRemove(); }} title={t.cpLanyardRemove}
                className="absolute -top-2 -right-2 w-5 h-5 rounded-full flex items-center justify-center text-[11px] leading-none bg-[#fff] dark:bg-[#2C2C2E] border border-[rgba(0,0,0,.1)] dark:border-[#48484a]"
                style={{ color:'#8E8E93', boxShadow:'0 2px 6px rgba(0,0,0,.15)' }}>✕</button>
              <div className="flex flex-col items-center text-center gap-2">
                <AppIcon card={persona} cls="w-12 h-12 rounded-[14px]" fb={22} />
                <div className="w-full min-w-0">
                  <div className="text-[13px] font-semibold leading-tight truncate text-[#000] dark:text-[#fff]">{cd.name}</div>
                  <div className="text-[11px] mt-0.5 truncate text-[rgba(60,60,67,.6)] dark:text-[rgba(235,235,245,.6)]">{deptLabelFor(t, persona.dept)}</div>
                </div>
              </div>
            </div>
          </div>
        </div>
      );
    };
export { Lanyard };
