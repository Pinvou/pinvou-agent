export const ARTIFACT_PREVIEW_OPEN_EXTERNAL = 'pinvou:artifact-preview:open-external';

// Frame→parent 通道（sandbox="allow-scripts" 的不透明源 iframe 与父窗口跨源，
// contentDocument/事件监听注入都不可用）：内容自然尺寸上报、ctrl+滚轮转发、
// design runtime 安装中继全部走 postMessage。父窗口侧以 event.source ===
// frame.contentWindow 校验来源。
export const ARTIFACT_PREVIEW_CONTENT_SIZE = 'pinvou:artifact-preview:content-size';
export const ARTIFACT_PREVIEW_REQUEST_SIZE = 'pinvou:artifact-preview:request-size';
export const ARTIFACT_PREVIEW_FRAME_WHEEL = 'pinvou:artifact-preview:frame-wheel';
export const ARTIFACT_PREVIEW_INSTALL_RUNTIME = 'pinvou:artifact-preview:install-runtime';

export function normalizeUserExternalUrl(value) {
  const raw = String(value || '').trim();
  if (!/^https?:\/\/[^/\\\s]/i.test(raw)) return '';
  try {
    const parsed = new URL(raw);
    if (!['http:', 'https:'].includes(parsed.protocol)) return '';
    if (!parsed.hostname || parsed.username || parsed.password) return '';
    return parsed.href;
  } catch {
    return '';
  }
}

export function artifactPreviewExternalUrlFromMessage(data) {
  if (!data || data.type !== ARTIFACT_PREVIEW_OPEN_EXTERNAL) return '';
  return normalizeUserExternalUrl(data.url);
}

export function artifactPreviewContentSizeFromMessage(data) {
  if (!data || data.type !== ARTIFACT_PREVIEW_CONTENT_SIZE) return null;
  const w = Number(data.w);
  const h = Number(data.h);
  if (!Number.isFinite(w) || !Number.isFinite(h) || w <= 0 || h <= 0) return null;
  return { w, h };
}

export function artifactPreviewFrameWheelFromMessage(data) {
  if (!data || data.type !== ARTIFACT_PREVIEW_FRAME_WHEEL) return null;
  const deltaY = Number(data.deltaY);
  return Number.isFinite(deltaY) ? deltaY : null;
}

export function artifactPreviewInstallRuntimeMessage(script) {
  return { type: ARTIFACT_PREVIEW_INSTALL_RUNTIME, script: String(script || '') };
}

export function buildArtifactPreviewDocument(html) {
  const messageType = JSON.stringify(ARTIFACT_PREVIEW_OPEN_EXTERNAL);
  const sizeType = JSON.stringify(ARTIFACT_PREVIEW_CONTENT_SIZE);
  const requestType = JSON.stringify(ARTIFACT_PREVIEW_REQUEST_SIZE);
  const wheelType = JSON.stringify(ARTIFACT_PREVIEW_FRAME_WHEEL);
  const installType = JSON.stringify(ARTIFACT_PREVIEW_INSTALL_RUNTIME);
  const bootstrap = [
    '(function(){',
    'function anchorFromEvent(e){return e.target&&e.target.closest?e.target.closest("a[href]"):null;}',
    'document.addEventListener("contextmenu",function(e){e.preventDefault();});',
    'document.addEventListener("click",function(e){',
    'var a=anchorFromEvent(e);if(!a)return;',
    'var h=(a.getAttribute("href")||"").trim();',
    'e.preventDefault();',
    'if(h.charAt(0)==="#"&&h.length>1){var el=document.getElementById(h.slice(1));if(el)el.scrollIntoView({behavior:"smooth"});return;}',
    String.raw`if(/^https?:\/\//i.test(h)){window.parent.postMessage({type:` + messageType + ',url:h},"*");}',
    '},true);',
    'document.addEventListener("submit",function(e){e.preventDefault();},true);',
    // 内容自然尺寸上报：跨源 iframe 无法被父窗口测量（contentDocument 抛错），
    // 由帧内自报。scrollWidth/scrollHeight 为 0 的空测量不上报（等价于旧的
    // 父窗口侧「空测量不入缓存」守卫）；同尺寸去重，避免父子互相触发的死循环。
    'var lastW=-1,lastH=-1;',
    'function reportSize(){var de=document.documentElement,bd=document.body;',
    'var w=Math.max(de?de.scrollWidth:0,bd?bd.scrollWidth:0);',
    'var h=Math.max(de?de.scrollHeight:0,bd?bd.scrollHeight:0);',
    'if(w>0&&h>0&&(w!==lastW||h!==lastH)){lastW=w;lastH=h;',
    'window.parent.postMessage({type:' + sizeType + ',w:w,h:h},"*");}}',
    'document.addEventListener("DOMContentLoaded",reportSize);',
    'window.addEventListener("load",reportSize);',
    'if(typeof ResizeObserver!=="undefined"){try{new ResizeObserver(reportSize).observe(document.documentElement);}catch(noop){}}',
    // ctrl+滚轮转发：父窗口无法再把 wheel 监听挂进帧文档，由帧内转发给父窗口
    // 的受控缩放；非 ctrl 滚动保持帧内默认行为。
    'document.addEventListener("wheel",function(e){if(e.ctrlKey){e.preventDefault();',
    'window.parent.postMessage({type:' + wheelType + ',deltaY:e.deltaY},"*");}},{passive:false,capture:true});',
    // 父→帧命令通道：按需测量 + design runtime 安装中继。安装即在本帧内 eval——
    // 该帧允许脚本执行，被安装方与本页脚本权限相同，无提权；失败经 design
    // runtime 的错误通道回报父窗口（ArtifactsPanel 既有监听器消费）。
    'window.addEventListener("message",function(e){var d=e&&e.data;if(!d)return;',
    'if(d.type===' + requestType + '){reportSize();return;}',
    'if(d.type===' + installType + '&&typeof d.script==="string"&&d.script){',
    'try{(0,eval)(d.script);}catch(err){try{',
    'window.parent.postMessage({source:"pinvou-design-runtime",type:"pinvou:design-runtime-error",payload:{error:String(err)}},"*");',
    // 收尾顺序：`}`(catch err) `}`(install if) `}`(function(e) 回调) `)`
    // （addEventListener 调用）`}`（IIFE 函数体）`)`（IIFE 分组括号）`();`
    // （调用）。此前写成 `})();`——IIFE 体先于 addEventListener 的闭括号
    // 收口，多出的 `}` 吞掉分组括号后整段 bootstrap 语法错误、一个字都不
    // 执行：尺寸自报/滚轮转发/外链中继/运行时安装全部死路，而旧探针在
    // 更早处崩溃从未跑到这些用例（r7 R2 的真正根因）。
    '}catch(noop){}}}}',
    ')})();',
  ].join('');
  // \u003c escape keeps the literal "</script>" out of the source so it is not truncated when inlined into HTML.
  // margin:0 stays (the zoom measurer assumes no UA body margin); the canvas
  // background is intentionally NOT forced here — the doc stays transparent so
  // the themed iframe surface decides light/dark, keeping light-authored
  // documents readable in the light theme.
  return '<script>' + bootstrap + '\u003C/script>'
    + '<style>html,body{margin:0;}</style>'
    + String(html || '');
}
