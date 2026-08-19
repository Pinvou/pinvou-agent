import createDOMPurify from 'dompurify';

export const ARTIFACT_PREVIEW_OPEN_EXTERNAL = 'pinvou:artifact-preview:open-external';
export const ARTIFACT_PREVIEW_REQUEST_CLOSE = 'pinvou:artifact-preview:request-close';
export const ARTIFACT_PREVIEW_FOCUS_BOUNDARY = 'pinvou:artifact-preview:focus-boundary';
export const ARTIFACT_PREVIEW_SIZE = 'pinvou:artifact-preview:size';
export const ARTIFACT_PREVIEW_ZOOM = 'pinvou:artifact-preview:zoom';

export function normalizeUserExternalUrl(value) {
  const raw = String(value || '').trim();
  if (!/^https?:\/\/[^/\\\s]/i.test(raw)) return '';
  try {
    const parsed = new URL(raw);
    if (!['http:', 'https:'].includes(parsed.protocol)) return '';
    if (!parsed.hostname || parsed.username || parsed.password) return '';
    return parsed.href;
  } catch (_) {
    return '';
  }
}

export function artifactPreviewExternalUrlFromMessage(data) {
  if (!data || data.type !== ARTIFACT_PREVIEW_OPEN_EXTERNAL) return '';
  return normalizeUserExternalUrl(data.url);
}

export function artifactPreviewRequestsCloseFromMessage(data) {
  return Boolean(data && data.type === ARTIFACT_PREVIEW_REQUEST_CLOSE);
}

export function artifactPreviewFocusDirectionFromMessage(data) {
  if (!data || data.type !== ARTIFACT_PREVIEW_FOCUS_BOUNDARY) return '';
  return data.direction === 'previous' || data.direction === 'next' ? data.direction : '';
}

export function artifactPreviewSizeFromMessage(data) {
  if (!data || data.type !== ARTIFACT_PREVIEW_SIZE) return null;
  const width = Math.ceil(Number(data.width));
  const height = Math.ceil(Number(data.height));
  if (!Number.isFinite(width) || !Number.isFinite(height) || width < 1 || height < 1) return null;
  return { width: Math.min(width, 20_000), height: Math.min(height, 20_000) };
}

export function artifactPreviewZoomDirectionFromMessage(data) {
  if (!data || data.type !== ARTIFACT_PREVIEW_ZOOM) return '';
  return data.direction === 'in' || data.direction === 'out' ? data.direction : '';
}

function escapeHtml(value) {
  return String(value || '')
    .replaceAll('&', '&amp;')
    .replaceAll('<', '&lt;')
    .replaceAll('>', '&gt;');
}

let artifactPurifier;
function getArtifactPurifier() {
  if (artifactPurifier) return artifactPurifier;
  if (typeof createDOMPurify.sanitize === 'function') artifactPurifier = createDOMPurify;
  else if (typeof window !== 'undefined') artifactPurifier = createDOMPurify(window);
  return artifactPurifier;
}

export function artifactPreviewBootstrap(options = {}) {
  return [
    '(function(){',
    `var OPEN=${JSON.stringify(ARTIFACT_PREVIEW_OPEN_EXTERNAL)};`,
    `var CLOSE=${JSON.stringify(ARTIFACT_PREVIEW_REQUEST_CLOSE)};`,
    `var FOCUS=${JSON.stringify(ARTIFACT_PREVIEW_FOCUS_BOUNDARY)};`,
    `var SIZE=${JSON.stringify(ARTIFACT_PREVIEW_SIZE)};`,
    `var ZOOM=${JSON.stringify(ARTIFACT_PREVIEW_ZOOM)};`,
    `var CAN_CLOSE=${options.requestClose === true ? 'true' : 'false'};`,
    'function externalTarget(node){return node&&node.closest?node.closest("[data-pinvou-external-url]"):null;}',
    'function requestOpen(node){if(!node)return;var url=node.getAttribute("data-pinvou-external-url")||"";window.parent.postMessage({type:OPEN,url:url},"*");}',
    `function focusable(){return Array.prototype.slice.call(document.querySelectorAll('[data-pinvou-external-url],a[href],area[href],summary,audio[controls],video[controls],[contenteditable="true"],[tabindex]:not([tabindex="-1"])')).filter(function(node){var style=window.getComputedStyle(node);return !node.hasAttribute('disabled')&&!node.hasAttribute('hidden')&&node.getClientRects().length>0&&style.visibility!=='hidden';});}`,
    'document.addEventListener("click",function(event){if(!event.isTrusted)return;var node=externalTarget(event.target);if(!node)return;event.preventDefault();requestOpen(node);},true);',
    'document.addEventListener("keydown",function(event){if(!event.isTrusted)return;if(event.key==="Escape"&&CAN_CLOSE){event.preventDefault();window.parent.postMessage({type:CLOSE},"*");return;}if(event.key==="Tab"){var nodes=focusable();var edge=event.shiftKey?nodes[0]:nodes[nodes.length-1];if(!nodes.length||document.activeElement===edge){event.preventDefault();window.parent.postMessage({type:FOCUS,direction:event.shiftKey?"previous":"next"},"*");return;}}if((event.key==="Enter"||event.key===" ")&&externalTarget(event.target)){event.preventDefault();requestOpen(externalTarget(event.target));}},true);',
    'document.addEventListener("submit",function(event){event.preventDefault();},true);',
    'document.addEventListener("wheel",function(event){if(!event.isTrusted||!event.ctrlKey||event.deltaY===0)return;event.preventDefault();event.stopPropagation();window.parent.postMessage({type:ZOOM,direction:event.deltaY<0?"in":"out"},"*");},{passive:false,capture:true});',
    'var lastSize="";function reportSize(){var de=document.documentElement,bd=document.body;var width=Math.max(de?de.scrollWidth:0,bd?bd.scrollWidth:0);var height=Math.max(de?de.scrollHeight:0,bd?bd.scrollHeight:0);var next=width+"x"+height;if(width>0&&height>0&&next!==lastSize){lastSize=next;window.parent.postMessage({type:SIZE,width:width,height:height},"*");}}',
    'if(document.readyState==="loading")document.addEventListener("DOMContentLoaded",reportSize,{once:true});else reportSize();',
    'if(typeof ResizeObserver!=="undefined")new ResizeObserver(reportSize).observe(document.documentElement);window.addEventListener("load",reportSize,{once:true});',
    '})();',
  ].join('');
}

function isolatedArtifactDocument(html, options = {}) {
  if (typeof DOMParser !== 'function') {
    return `<!doctype html><html><head><meta charset="utf-8"><meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline'; img-src data: blob:; font-src data:; connect-src 'none'; frame-src 'none'; object-src 'none'; form-action 'none'; base-uri 'none'"></head><body><pre>${escapeHtml(html)}</pre></body></html>`;
  }

  const parser = new DOMParser();
  const purifier = getArtifactPurifier();
  const source = purifier?.sanitize
    ? purifier.sanitize(String(html || ''), {
      WHOLE_DOCUMENT: true,
      USE_PROFILES: { html: true, svg: true },
      FORBID_TAGS: ['script', 'iframe', 'frame', 'object', 'embed', 'form', 'input', 'button', 'textarea', 'select', 'option', 'meta', 'base', 'link'],
      FORBID_ATTR: ['srcdoc', 'action', 'formaction', 'ping'],
    })
    : String(html || '');
  const documentNode = parser.parseFromString(source, 'text/html');
  const nonceBytes = new Uint32Array(4);
  globalThis.crypto?.getRandomValues?.(nonceBytes);
  const scriptNonce = [...nonceBytes].map(value => value.toString(36)).join('-') || `pinvou-${Date.now().toString(36)}`;
  documentNode.querySelectorAll('script,iframe,frame,object,embed,form,input,button,textarea,select,option,meta,base,link').forEach(node => node.remove());

  for (const element of documentNode.querySelectorAll('*')) {
    for (const attribute of [...element.attributes]) {
      const name = attribute.name.toLowerCase();
      const value = attribute.value.trim();
      if (name.startsWith('on') || name === 'srcdoc' || name === 'action' || name === 'formaction' || name === 'ping') {
        element.removeAttribute(attribute.name);
        continue;
      }
      if (name === 'data-pinvou-external-url') {
        element.removeAttribute(attribute.name);
        continue;
      }
      if (name === 'href' || name.endsWith(':href')) {
        const externalUrl = element.tagName.toLowerCase() === 'a'
          ? normalizeUserExternalUrl(value)
          : '';
        if (externalUrl) {
          element.removeAttribute(attribute.name);
          element.setAttribute('data-pinvou-external-url', externalUrl);
          element.setAttribute('role', 'link');
          element.setAttribute('tabindex', '0');
        } else if (!value.startsWith('#')) {
          element.removeAttribute(attribute.name);
        }
        continue;
      }
      if (['src', 'poster', 'background'].includes(name) && value && !/^(?:data|blob):/i.test(value)) {
        if (element.tagName.toLowerCase() === 'img') element.remove();
        else element.removeAttribute(attribute.name);
        continue;
      }
      if (name === 'srcset') element.removeAttribute(attribute.name);
    }
  }

  for (const style of documentNode.querySelectorAll('style')) {
    style.textContent = String(style.textContent || '').replace(/@import\s+[^;]+;?/gi, '');
  }

  const policy = documentNode.createElement('meta');
  policy.setAttribute('http-equiv', 'Content-Security-Policy');
  policy.setAttribute('content', `default-src 'none'; img-src data: blob:; media-src data: blob:; font-src data:; style-src 'unsafe-inline'; script-src 'nonce-${scriptNonce}'; connect-src 'none'; frame-src 'none'; child-src 'none'; object-src 'none'; worker-src 'none'; form-action 'none'; base-uri 'none'`);
  documentNode.head.prepend(policy);

  const baseline = documentNode.createElement('style');
  baseline.textContent = 'html,body{margin:0;min-height:100%;background:#fff;color:#182230} [data-pinvou-external-url]{cursor:pointer;text-decoration:underline;text-underline-offset:.16em} @media(prefers-reduced-motion:reduce){*,*::before,*::after{scroll-behavior:auto!important;animation:none!important;transition:none!important}}';
  documentNode.head.append(baseline);

  const bootstrap = documentNode.createElement('script');
  bootstrap.setAttribute('nonce', scriptNonce);
  bootstrap.textContent = artifactPreviewBootstrap(options);
  documentNode.head.append(bootstrap);

  if (options.trustedScript) {
    const trustedBootstrap = documentNode.createElement('script');
    trustedBootstrap.setAttribute('nonce', scriptNonce);
    trustedBootstrap.textContent = `document.addEventListener('DOMContentLoaded',function(){${String(options.trustedScript)}},{once:true});`;
    documentNode.head.append(trustedBootstrap);
  }

  return `<!doctype html>${documentNode.documentElement.outerHTML}`;
}

export function buildArtifactPreviewDocument(html, options = {}) {
  if (options.isolated === true) return isolatedArtifactDocument(html, options);
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
    'if(/^https?:\\/\\//i.test(h)){window.parent.postMessage({type:' + messageType + ',url:h},"*");}',
    '},true);',
    'document.addEventListener("submit",function(e){e.preventDefault();},true);',
    '})();',
  ].join('');
  return '<script>' + bootstrap + '<\/script>'
    + '<style>html,body{background:#15171a;margin:0;}</style>'
    + String(html || '');
}
