// 内置人格卡 en/ja 文案 overlay 的共用查找核心。
// 数据表是 features/personas/personas-i18n.js——经典脚本,按需 <script> 注入并向
// window 挂 PERSONA_I18N(141KB,不宜作静态 ES 依赖),因此这里与各消费端一样只读
// 全局表。zh 为原文语言不查表;自制卡(source=user)不翻;查不到兜底原文,返回 null。
// @param {{ id?: string, source?: string }} card - 人格卡对象
// @param {string} lang - 目标 UI 语言 tag('en' | 'ja';其他值一律返回 null)
// @returns {{ name: string, description: string } | null} overlay 词条,无则 null
export function personaOverlayFor(card, lang) {
  if (!card || !lang || lang === 'zh' || card.source === 'user') return null;
  const overlays = typeof window === 'undefined' ? null : window.PERSONA_I18N;
  const entry = overlays && overlays[card.id];
  return (entry && entry[lang]) || null;
}
