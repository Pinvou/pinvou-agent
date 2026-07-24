const DESIGN_MESSAGE_TYPES = {
  READY: 'pinvou:design-runtime-ready',
  ELEMENT_SELECTED: 'pinvou:design-element-selected',
  APPLY_CHANGE: 'pinvou:design-apply-change',
  CHANGE_APPLIED: 'pinvou:design-change-applied',
  CLEAR_CHANGES: 'pinvou:design-clear-changes',
  ERROR: 'pinvou:design-runtime-error',
  DESTROY: 'pinvou:design-runtime-destroy',
  DESTROYED: 'pinvou:design-runtime-destroyed',
};

const DESIGN_STYLE_FIELDS = [
  'color',
  'backgroundColor',
  'fontSize',
  'fontWeight',
  'margin',
  'padding',
  'width',
  'height',
  'borderRadius',
];

function cssEscapeIdent(value) {
  return String(value || '').replace(/[^a-zA-Z0-9_-]/g, (ch) => '\\' + ch);
}

function selectorPartForElement(element) {
  if (!element || !element.tagName) return '';
  const tag = element.tagName.toLowerCase();
  if (element.id) return `${tag}#${cssEscapeIdent(element.id)}`;
  const classes = Array.from(element.classList || [])
    .filter(Boolean)
    .slice(0, 2)
    .map((name) => `.${cssEscapeIdent(name)}`)
    .join('');
  let nth = '';
  const parent = element.parentElement;
  if (parent) {
    const siblings = Array.from(parent.children || [])
      .filter((child) => child.tagName === element.tagName);
    if (siblings.length > 1) nth = `:nth-of-type(${siblings.indexOf(element) + 1})`;
  }
  return `${tag}${classes}${nth}`;
}

function getDesignElementSelector(element) {
  if (!element || !element.tagName) return '';
  if (element.id) return selectorPartForElement(element);
  const parts = [];
  let current = element;
  while (current && current.nodeType === 1 && current !== current.ownerDocument.documentElement) {
    parts.unshift(selectorPartForElement(current));
    if (current.id || parts.length >= 5) break;
    current = current.parentElement;
  }
  return parts.filter(Boolean).join(' > ');
}

function snapshotDesignElement(element) {
  if (!element || !element.getBoundingClientRect) return null;
  const rect = element.getBoundingClientRect();
  const computed = element.ownerDocument.defaultView.getComputedStyle(element);
  const computedStyle = {};
  DESIGN_STYLE_FIELDS.forEach((field) => { computedStyle[field] = computed[field] || ''; });
  return {
    id: `dm-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
    selector: getDesignElementSelector(element),
    tagName: element.tagName.toLowerCase(),
    className: element.className && typeof element.className === 'string' ? element.className : '',
    text: String(element.innerText || element.textContent || '').trim().slice(0, 240),
    rect: {
      x: Math.round(rect.x),
      y: Math.round(rect.y),
      width: Math.round(rect.width),
      height: Math.round(rect.height),
    },
    computedStyle,
  };
}

function buildDesignRuntimeScript() {
  return `(${function designRuntime() {
    var TYPES = {
      READY: 'pinvou:design-runtime-ready',
      ELEMENT_SELECTED: 'pinvou:design-element-selected',
      APPLY_CHANGE: 'pinvou:design-apply-change',
      CHANGE_APPLIED: 'pinvou:design-change-applied',
      CLEAR_CHANGES: 'pinvou:design-clear-changes',
      ERROR: 'pinvou:design-runtime-error',
      DESTROY: 'pinvou:design-runtime-destroy',
      DESTROYED: 'pinvou:design-runtime-destroyed',
    };
    var STYLE_FIELDS = ['color', 'backgroundColor', 'fontSize', 'fontWeight', 'margin', 'padding', 'width', 'height', 'borderRadius'];
    if (window.__PINVOU_DESIGN_RUNTIME__ && window.__PINVOU_DESIGN_RUNTIME__.destroy) {
      window.__PINVOU_DESIGN_RUNTIME__.destroy();
    }

    function post(type, payload) {
      try {
        window.parent.postMessage({ source: 'pinvou-design-runtime', type: type, payload: payload || {} }, '*');
      } catch (error) {
        /* noop */
      }
    }

    function escapeIdent(value) {
      return String(value || '').replace(/[^a-zA-Z0-9_-]/g, function (ch) { return '\\\\' + ch; });
    }

    function selectorPart(element) {
      if (!element || !element.tagName) return '';
      var tag = element.tagName.toLowerCase();
      if (element.id) return tag + '#' + escapeIdent(element.id);
      var cls = Array.prototype.slice.call(element.classList || [])
        .filter(Boolean)
        .slice(0, 2)
        .map(function (name) { return '.' + escapeIdent(name); })
        .join('');
      var nth = '';
      var parent = element.parentElement;
      if (parent) {
        var siblings = Array.prototype.slice.call(parent.children || [])
          .filter(function (child) { return child.tagName === element.tagName; });
        if (siblings.length > 1) nth = ':nth-of-type(' + (siblings.indexOf(element) + 1) + ')';
      }
      return tag + cls + nth;
    }

    function selectorFor(element) {
      if (!element || !element.tagName) return '';
      if (element.id) return selectorPart(element);
      var parts = [];
      var current = element;
      while (current && current.nodeType === 1 && current !== document.documentElement) {
        parts.unshift(selectorPart(current));
        if (current.id || parts.length >= 5) break;
        current = current.parentElement;
      }
      return parts.filter(Boolean).join(' > ');
    }

    function makeBox(kind) {
      var box = document.createElement('div');
      box.setAttribute('data-pinvou-design-' + kind, 'true');
      box.style.cssText = [
        'position:fixed',
        'z-index:2147483647',
        'pointer-events:none',
        'box-sizing:border-box',
        'border:2px solid ' + (kind === 'selected' ? '#34C759' : '#0A84FF'),
        'background:' + (kind === 'selected' ? 'rgba(52,199,89,.10)' : 'rgba(10,132,255,.08)'),
        'box-shadow:0 0 0 1px rgba(255,255,255,.85),0 10px 30px rgba(0,0,0,.18)',
        'display:none'
      ].join(';');
      document.documentElement.appendChild(box);
      return box;
    }

    var hoverBox = makeBox('hover');
    var selectedBox = makeBox('selected');
    var currentHover = null;
    var currentSelected = null;
    var originals = Object.create(null);

    function draw(box, element) {
      if (!element || !element.getBoundingClientRect) {
        box.style.display = 'none';
        return;
      }
      var rect = element.getBoundingClientRect();
      if (!rect.width || !rect.height) {
        box.style.display = 'none';
        return;
      }
      box.style.display = 'block';
      box.style.left = Math.round(rect.left) + 'px';
      box.style.top = Math.round(rect.top) + 'px';
      box.style.width = Math.round(rect.width) + 'px';
      box.style.height = Math.round(rect.height) + 'px';
    }

    function isRuntimeNode(node) {
      return !!(node && node.closest && node.closest('[data-pinvou-design-hover],[data-pinvou-design-selected]'));
    }

    function snapshot(element) {
      var rect = element.getBoundingClientRect();
      var computed = window.getComputedStyle(element);
      var computedStyle = {};
      STYLE_FIELDS.forEach(function (field) { computedStyle[field] = computed[field] || ''; });
      return {
        id: 'dm-' + Date.now() + '-' + Math.random().toString(36).slice(2, 8),
        selector: selectorFor(element),
        tagName: element.tagName.toLowerCase(),
        className: element.className && typeof element.className === 'string' ? element.className : '',
        text: String(element.innerText || element.textContent || '').trim().slice(0, 240),
        rect: {
          x: Math.round(rect.x),
          y: Math.round(rect.y),
          width: Math.round(rect.width),
          height: Math.round(rect.height),
        },
        computedStyle: computedStyle,
      };
    }

    function rememberOriginal(selector, element, type, property) {
      if (!selector || !element) return;
      var bucket = originals[selector] || (originals[selector] = { text: null, styles: Object.create(null) });
      if (type === 'text' && bucket.text == null) bucket.text = element.textContent || '';
      if (type === 'style' && property && !Object.prototype.hasOwnProperty.call(bucket.styles, property)) {
        bucket.styles[property] = element.style[property] || '';
      }
    }

    function applyChange(payload) {
      var selector = payload && payload.selector;
      var changeId = payload && payload.changeId;
      var type = payload && payload.changeType;
      var property = payload && payload.property;
      var value = payload && payload.value;
      try {
        var element = selector ? document.querySelector(selector) : currentSelected;
        if (!element) throw new Error('target element not found');
        rememberOriginal(selector, element, type, property);
        if (type === 'text') {
          element.textContent = value == null ? '' : String(value);
        } else if (type === 'style') {
          if (!property) throw new Error('style property is required');
          element.style[property] = value == null ? '' : String(value);
        } else {
          throw new Error('unsupported change type');
        }
        if (currentSelected === element) draw(selectedBox, currentSelected);
        if (currentHover === element) draw(hoverBox, currentHover);
        post(TYPES.CHANGE_APPLIED, { changeId: changeId, selector: selector, ok: true });
      } catch (error) {
        post(TYPES.CHANGE_APPLIED, { changeId: changeId, selector: selector, ok: false, error: String(error && error.message || error) });
      }
    }

    function clearChanges() {
      Object.keys(originals).forEach(function (selector) {
        var element = document.querySelector(selector);
        var original = originals[selector];
        if (!element || !original) return;
        if (original.text != null) element.textContent = original.text;
        Object.keys(original.styles || {}).forEach(function (property) {
          element.style[property] = original.styles[property];
        });
      });
      originals = Object.create(null);
      draw(hoverBox, currentHover);
      draw(selectedBox, currentSelected);
      post(TYPES.CHANGE_APPLIED, { changeId: 'clear', ok: true, cleared: true });
    }

    function onMove(event) {
      var target = event.target;
      if (!target || target === document.documentElement || target === document.body || isRuntimeNode(target)) return;
      currentHover = target;
      draw(hoverBox, currentHover);
    }

    function onClick(event) {
      var target = event.target;
      if (!target || target === document.documentElement || target === document.body || isRuntimeNode(target)) return;
      event.preventDefault();
      event.stopPropagation();
      currentSelected = target;
      draw(selectedBox, currentSelected);
      post(TYPES.ELEMENT_SELECTED, { element: snapshot(target) });
    }

    function onScrollOrResize() {
      draw(hoverBox, currentHover);
      draw(selectedBox, currentSelected);
    }

    function onMessage(event) {
      var data = event && event.data;
      if (!data || !data.type) return;
      if (data.type === TYPES.DESTROY) {
        destroy();
      } else if (data.type === TYPES.APPLY_CHANGE) {
        applyChange(data.payload || {});
      } else if (data.type === TYPES.CLEAR_CHANGES) {
        clearChanges();
      }
    }

    function destroy() {
      document.removeEventListener('mousemove', onMove, true);
      document.removeEventListener('click', onClick, true);
      window.removeEventListener('scroll', onScrollOrResize, true);
      window.removeEventListener('resize', onScrollOrResize, true);
      window.removeEventListener('message', onMessage, true);
      if (hoverBox && hoverBox.parentNode) hoverBox.parentNode.removeChild(hoverBox);
      if (selectedBox && selectedBox.parentNode) selectedBox.parentNode.removeChild(selectedBox);
      currentHover = null;
      currentSelected = null;
      window.__PINVOU_DESIGN_RUNTIME__ = null;
      post(TYPES.DESTROYED);
    }

    try {
      document.addEventListener('mousemove', onMove, true);
      document.addEventListener('click', onClick, true);
      window.addEventListener('scroll', onScrollOrResize, true);
      window.addEventListener('resize', onScrollOrResize, true);
      window.addEventListener('message', onMessage, true);
      window.__PINVOU_DESIGN_RUNTIME__ = { destroy: destroy };
      post(TYPES.READY);
    } catch (error) {
      post(TYPES.ERROR, { error: String(error && error.message || error) });
    }
  }.toString()})();`;
}

export {
  DESIGN_MESSAGE_TYPES,
  DESIGN_STYLE_FIELDS,
  buildDesignRuntimeScript,
  getDesignElementSelector,
  snapshotDesignElement,
};
