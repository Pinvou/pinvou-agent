export const ARTIFACT_PREVIEW_OPEN_EXTERNAL = 'pinvou:artifact-preview:open-external';

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

export function buildArtifactPreviewDocument(html) {
  const messageType = JSON.stringify(ARTIFACT_PREVIEW_OPEN_EXTERNAL);
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
    '})();',
  ].join('');
  // \u003c 转义保持源码中不出现字面 "</script>",避免被内联进 HTML 时截断。
  return '<script>' + bootstrap + '\u003C/script>'
    + '<style>html,body{background:#15171a;margin:0;}</style>'
    + String(html || '');
}
