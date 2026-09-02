import { resolveAppAssetUrl } from '../shared/asset-url.mjs';

// personas-i18n overlay(141KB)的 UI 语言兜底注入,主窗(App 的 language effect)
// 与撕离窗(DetachedShell 的 useDetachedBase)共用。
//
// 背景:overlay 快速路径在 index.html 按系统语言注入;但消费端(persona-shared.jsx
// personaText / bridge personas.js personaName)看的是用户设置的 UI 语言。
// 「系统中文 + 手动切英/日 UI」时 overlay 缺失,卡名会停在中文——本函数兜底:
// 已加载则同步回调(不吞 onLoaded);在途(index.html 快速路径注入中)则只挂
// onload,避免重复注入;加载完成由调用方 bump state 触发卡名重渲染。
export function ensurePersonaI18nOverlay(onLoaded) {
  if (typeof document === 'undefined') return;
  if (window.PERSONA_I18N) { onLoaded(); return; }
  const existing = document.querySelector('script[data-personas-i18n]');
  if (existing) {
    // index.html 快速路径的在途脚本:挂 load 恢复回调。index.html 的 onerror 会
    // 移除元素,因此这里只会见到真正在途的脚本,不会挂在死元素上。
    existing.addEventListener('load', onLoaded, { once: true });
    return;
  }
  const s = document.createElement('script');
  s.setAttribute('data-personas-i18n', '1');
  s.src = resolveAppAssetUrl('features/personas/personas-i18n.js');
  s.onload = onLoaded;
  s.onerror = () => s.remove();
  document.head.append(s);
}
