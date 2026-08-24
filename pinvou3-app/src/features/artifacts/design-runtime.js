const DESIGN_MESSAGE_TYPES = {
  READY: 'pinvou:design-runtime-ready',
  ELEMENT_SELECTED: 'pinvou:design-element-selected',
  APPLY_CHANGE: 'pinvou:design-apply-change',
  CHANGE_APPLIED: 'pinvou:design-change-applied',
  ELEMENT_MUTATED: 'pinvou:design-element-mutated',
  CLEAR_CHANGES: 'pinvou:design-clear-changes',
  ERROR: 'pinvou:design-runtime-error',
  DESTROY: 'pinvou:design-runtime-destroy',
  DESTROYED: 'pinvou:design-runtime-destroyed',
};

function buildDesignRuntimeScript() {
  return `(${function designRuntime() {
    const TYPES = {
      READY: 'pinvou:design-runtime-ready',
      ELEMENT_SELECTED: 'pinvou:design-element-selected',
      APPLY_CHANGE: 'pinvou:design-apply-change',
      CHANGE_APPLIED: 'pinvou:design-change-applied',
      ELEMENT_MUTATED: 'pinvou:design-element-mutated',
      CLEAR_CHANGES: 'pinvou:design-clear-changes',
      ERROR: 'pinvou:design-runtime-error',
      DESTROY: 'pinvou:design-runtime-destroy',
      DESTROYED: 'pinvou:design-runtime-destroyed',
    };
    const STYLE_FIELDS = ['color','backgroundColor','fontSize','fontWeight','margin','padding','width','height','minWidth','maxWidth','minHeight','maxHeight','display','position','top','right','bottom','left','zIndex','opacity','lineHeight','letterSpacing','textAlign','fontFamily','backgroundImage','backgroundSize','backgroundPosition','backgroundRepeat','marginTop','marginRight','marginBottom','marginLeft','paddingTop','paddingRight','paddingBottom','paddingLeft','gap','rowGap','columnGap','flexDirection','justifyContent','alignItems','alignSelf','overflow','borderTopWidth','borderRightWidth','borderBottomWidth','borderLeftWidth','borderTopStyle','borderRightStyle','borderBottomStyle','borderLeftStyle','borderTopColor','borderRightColor','borderBottomColor','borderLeftColor','borderTopLeftRadius','borderTopRightRadius','borderBottomRightRadius','borderBottomLeftRadius','borderRadius','visibility','cursor'];
    const DATA_ID = 'data-pinvou-design-id';
    let nextId = 1;
    const MIN_SIZE = 8;
    const DRAG_THRESHOLD = 3;
    const handles = [
      ['nw','nwse-resize'], ['n','ns-resize'], ['ne','nesw-resize'], ['e','ew-resize'],
      ['se','nwse-resize'], ['s','ns-resize'], ['sw','nesw-resize'], ['w','ew-resize']
    ];
    if (window.__PINVOU_DESIGN_RUNTIME__ && window.__PINVOU_DESIGN_RUNTIME__.destroy) {
      window.__PINVOU_DESIGN_RUNTIME__.destroy();
    }

    function post(type, payload) {
      try {
        window.parent.postMessage({ source: 'pinvou-design-runtime', type, payload: payload || {} }, '*');
      } catch {
        /* noop */
      }
    }

    function escapeIdent(value) {
      return String(value || '').replaceAll(/[^a-zA-Z0-9_-]/g, function (ch) { return '\\\\' + ch; });
    }

    function selectorPart(element) {
      if (!element || !element.tagName) return '';
      const tag = element.tagName.toLowerCase();
      if (element.id) return tag + '#' + escapeIdent(element.id);
      const cls = Array.prototype.slice.call(element.classList || [])
        .filter(Boolean)
        .slice(0, 2)
        .map(function (name) { return '.' + escapeIdent(name); })
        .join('');
      let nth = '';
      const parent = element.parentElement;
      if (parent) {
        const siblings = Array.prototype.slice.call(parent.children || [])
          .filter(function (child) { return child.tagName === element.tagName; });
        if (siblings.length > 1) nth = ':nth-of-type(' + (siblings.indexOf(element) + 1) + ')';
      }
      return tag + cls + nth;
    }

    function selectorFor(element) {
      if (!element || !element.tagName) return '';
      if (element.id) return selectorPart(element);
      const parts = [];
      let current = element;
      while (current && current.nodeType === 1 && current !== document.documentElement) {
        parts.unshift(selectorPart(current));
        if (current.id || parts.length >= 5) break;
        current = current.parentElement;
      }
      return parts.filter(Boolean).join(' > ');
    }

    function elementLabel(element) {
      if (!element || !element.tagName) return 'element';
      const tag = element.tagName.toLowerCase();
      if (element.id) return tag + '#' + element.id;
      if (element.className && typeof element.className === 'string') {
        const cls = element.className.trim().split(/\s+/).filter(Boolean)[0];
        if (cls) return tag + '.' + cls;
      }
      const text = String(element.textContent || '').trim().replaceAll(/\s+/g, ' ');
      if (text) return tag + ' "' + (text.length > 24 ? text.slice(0, 23) + '...' : text) + '"';
      return tag;
    }

    function breadcrumbs(element) {
      const items = [];
      let current = element;
      while (current && current.nodeType === 1 && current !== document.documentElement && items.length < 8) {
        items.unshift(selectorPart(current));
        current = current.parentElement;
      }
      return items;
    }

    function elementId(element) {
      if (!element || !element.setAttribute) return '';
      let id = element.getAttribute(DATA_ID);
      if (!id) {
        id = 'pdm-' + nextId++;
        element.setAttribute(DATA_ID, id);
      }
      return id;
    }

    function makeBox(kind) {
      const box = document.createElement('div');
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
      document.documentElement.append(box);
      return box;
    }

    function makeBand(kind) {
      const band = document.createElement('div');
      band.setAttribute('data-pinvou-design-' + kind, 'true');
      band.style.cssText = [
        'position:fixed',
        'z-index:2147483646',
        'pointer-events:none',
        'box-sizing:border-box',
        'background:transparent',
        'border-style:solid',
        'display:none'
      ].join(';');
      document.documentElement.append(band);
      return band;
    }

    function makeLabel() {
      const label = document.createElement('div');
      label.setAttribute('data-pinvou-design-label', 'true');
      label.style.cssText = [
        'position:fixed',
        'z-index:2147483647',
        'pointer-events:none',
        'display:none',
        'padding:2px 6px',
        'border-radius:4px',
        'background:#34C759',
        'color:white',
        'font:600 10px ui-monospace,SFMono-Regular,Menlo,Consolas,monospace',
        'box-shadow:0 2px 8px rgba(0,0,0,.18)'
      ].join(';');
      document.documentElement.append(label);
      return label;
    }

    function makeHandleLayer() {
      const layer = document.createElement('div');
      layer.setAttribute('data-pinvou-design-handles', 'true');
      // No inset shorthand: the string reaches the bundle verbatim, Safari 14.0 cannot parse it, and the handle layer would lose its full-screen base.
      layer.style.cssText = 'position:fixed;top:0;right:0;bottom:0;left:0;z-index:2147483647;pointer-events:none;display:none';
      document.documentElement.append(layer);
      return layer;
    }

    const hoverBox = makeBox('hover');
    const selectedBox = makeBox('selected');
    const marginBand = makeBand('margin');
    const paddingBand = makeBand('padding');
    const dimensionLabel = makeLabel();
    const handleLayer = makeHandleLayer();
    let currentHover = null;
    let currentSelected = null;
    let originals = Object.create(null);
    let suppressClick = false;
    let editingElement = null;
    let editingOriginalText = '';

    function draw(box, element) {
      if (!element || !element.getBoundingClientRect) {
        box.style.display = 'none';
        return;
      }
      const rect = element.getBoundingClientRect();
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

    function px(value) {
      // eslint-disable-next-line unicorn/prefer-number-coercion -- parseFloat 可解析带 px 单位后缀的值,Number() 会得到 NaN
      const parsed = Number.parseFloat(String(value || ''));
      return Number.isFinite(parsed) ? parsed : 0;
    }

    function drawBands(element) {
      if (!element || !element.getBoundingClientRect) {
        marginBand.style.display = 'none';
        paddingBand.style.display = 'none';
        return;
      }
      const rect = element.getBoundingClientRect();
      const cs = window.getComputedStyle(element);
      const mt = px(cs.marginTop), mr = px(cs.marginRight), mb = px(cs.marginBottom), ml = px(cs.marginLeft);
      const pt = px(cs.paddingTop), pr = px(cs.paddingRight), pb = px(cs.paddingBottom), pl = px(cs.paddingLeft);
      const bt = px(cs.borderTopWidth), br = px(cs.borderRightWidth), bb = px(cs.borderBottomWidth), bl = px(cs.borderLeftWidth);
      if (mt || mr || mb || ml) {
        marginBand.style.display = 'block';
        marginBand.style.borderColor = 'rgba(255,99,99,.28)';
        marginBand.style.left = Math.round(rect.left - ml) + 'px';
        marginBand.style.top = Math.round(rect.top - mt) + 'px';
        marginBand.style.width = Math.round(rect.width + ml + mr) + 'px';
        marginBand.style.height = Math.round(rect.height + mt + mb) + 'px';
        marginBand.style.borderTopWidth = mt + 'px';
        marginBand.style.borderRightWidth = mr + 'px';
        marginBand.style.borderBottomWidth = mb + 'px';
        marginBand.style.borderLeftWidth = ml + 'px';
      } else {
        marginBand.style.display = 'none';
      }
      if (pt || pr || pb || pl) {
        paddingBand.style.display = 'block';
        paddingBand.style.borderColor = 'rgba(124,200,134,.30)';
        paddingBand.style.left = Math.round(rect.left + bl) + 'px';
        paddingBand.style.top = Math.round(rect.top + bt) + 'px';
        paddingBand.style.width = Math.max(0, Math.round(rect.width - bl - br)) + 'px';
        paddingBand.style.height = Math.max(0, Math.round(rect.height - bt - bb)) + 'px';
        paddingBand.style.borderTopWidth = pt + 'px';
        paddingBand.style.borderRightWidth = pr + 'px';
        paddingBand.style.borderBottomWidth = pb + 'px';
        paddingBand.style.borderLeftWidth = pl + 'px';
      } else {
        paddingBand.style.display = 'none';
      }
    }

    function handlePoint(rect, dir) {
      const midX = rect.left + rect.width / 2;
      const midY = rect.top + rect.height / 2;
      return {
        x: dir.includes('w') ? rect.left : dir.includes('e') ? rect.right : midX,
        y: dir.includes('n') ? rect.top : dir.includes('s') ? rect.bottom : midY,
      };
    }

    function drawHandles(element) {
      if (!element || !element.getBoundingClientRect) {
        handleLayer.style.display = 'none';
        handleLayer.replaceChildren();
        return;
      }
      const rect = element.getBoundingClientRect();
      handleLayer.style.display = 'block';
      handleLayer.replaceChildren();
      handles.forEach(function (item) {
        const dir = item[0], cursor = item[1];
        const p = handlePoint(rect, dir);
        const dot = document.createElement('div');
        dot.setAttribute('data-pinvou-design-handle', dir);
        dot.style.cssText = [
          'position:fixed',
          'left:' + Math.round(p.x) + 'px',
          'top:' + Math.round(p.y) + 'px',
          'width:10px',
          'height:10px',
          'margin:-5px 0 0 -5px',
          'border-radius:50%',
          'background:#34C759',
          'border:2px solid white',
          'box-shadow:0 2px 8px rgba(0,0,0,.25)',
          'cursor:' + cursor,
          'pointer-events:auto'
        ].join(';');
        dot.addEventListener('mousedown', function (event) { startResize(element, dir, event); }, true);
        handleLayer.append(dot);
      });
    }

    function drawSelected() {
      draw(selectedBox, currentSelected);
      drawBands(currentSelected);
      drawHandles(currentSelected);
      if (!currentSelected) {
        dimensionLabel.style.display = 'none';
        return;
      }
      const rect = currentSelected.getBoundingClientRect();
      if (!rect.width || !rect.height) {
        dimensionLabel.style.display = 'none';
        return;
      }
      dimensionLabel.style.display = 'block';
      dimensionLabel.textContent = Math.round(rect.width) + ' x ' + Math.round(rect.height);
      dimensionLabel.style.left = Math.round(rect.left) + 'px';
      dimensionLabel.style.top = Math.round(rect.bottom + 4) + 'px';
    }

    function isRuntimeNode(node) {
      return !!(node && node.closest && node.closest('[data-pinvou-design-hover],[data-pinvou-design-selected],[data-pinvou-design-margin],[data-pinvou-design-padding],[data-pinvou-design-label],[data-pinvou-design-handles],[data-pinvou-design-handle]'));
    }

    function isTextEditableElement(element) {
      if (!element || !element.tagName) return false;
      const tag = element.tagName.toLowerCase();
      if (/^(script|style|html|body|iframe|img|svg|canvas|input|textarea|select)$/.test(tag)) return false;
      if (/^(span|p|a|button|label|strong|em|b|i|small|h1|h2|h3|h4|h5|h6)$/.test(tag)) return true;
      // eslint-disable-next-line unicorn/prefer-dom-node-text-content -- innerText 取渲染后文本(<br>/块级换行),textContent 仅为游离节点兜底
      const text = String(element.innerText || element.textContent || '').trim();
      if (!text || text.length > 160) return false;
      return element.children.length <= 1;
    }

    function selectTextContents(element) {
      try {
        const range = document.createRange();
        range.selectNodeContents(element);
        const selection = window.getSelection();
        if (!selection) return;
        selection.removeAllRanges();
        selection.addRange(range);
      } catch { /* ignore */ }
    }

    function snapshot(element) {
      const rect = element.getBoundingClientRect();
      const computed = window.getComputedStyle(element);
      const computedStyle = {};
      STYLE_FIELDS.forEach(function (field) { computedStyle[field] = computed[field] || ''; });
      return {
        id: elementId(element),
        selector: selectorFor(element),
        label: elementLabel(element),
        tagName: element.tagName.toLowerCase(),
        className: element.className && typeof element.className === 'string' ? element.className : '',
        breadcrumbs: breadcrumbs(element),
        // eslint-disable-next-line unicorn/prefer-dom-node-text-content -- innerText 取渲染后文本(<br>/块级换行),textContent 仅为游离节点兜底
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

    function postSelection(element) {
      if (!element) return;
      post(TYPES.ELEMENT_SELECTED, { element: snapshot(element) });
    }

    function rememberOriginal(selector, element, type, property, originalValue, hasOriginalValue) {
      if (!selector || !element) return;
      if (!originals[selector]) originals[selector] = { text: null, styles: Object.create(null) };
      const bucket = originals[selector];
      if (type === 'text' && bucket.text == null) {
        bucket.text = hasOriginalValue ? String(originalValue == null ? '' : originalValue) : (element.textContent || '');
      }
      // biome-ignore lint/suspicious/noPrototypeBuiltins: Safari 14 下限,Object.hasOwn 不可用,本调用已是安全形态
      if (type === 'style' && property && !Object.prototype.hasOwnProperty.call(bucket.styles, property)) {
        bucket.styles[property] = hasOriginalValue
          ? String(originalValue == null ? '' : originalValue)
          : (element.style[property] || '');
      }
    }

    function applyChange(payload) {
      const selector = payload && payload.selector;
      const changeId = payload && payload.changeId;
      const type = payload && payload.changeType;
      const property = payload && payload.property;
      const value = payload && payload.value;
      // biome-ignore lint/suspicious/noPrototypeBuiltins: Safari 14 下限,Object.hasOwn 不可用,本调用已是安全形态
      const hasOriginalValue = !!(payload && Object.prototype.hasOwnProperty.call(payload, 'oldValue'));
      const originalValue = payload && payload.oldValue;
      try {
        const element = selector ? document.querySelector(selector) : currentSelected;
        if (!element) throw new Error('target element not found');
        rememberOriginal(selector, element, type, property, originalValue, hasOriginalValue);
        if (type === 'text') {
          element.textContent = value == null ? '' : String(value);
        } else if (type === 'style') {
          if (!property) throw new Error('style property is required');
          element.style[property] = value == null ? '' : String(value);
        } else {
          throw new Error('unsupported change type');
        }
        if (currentSelected === element) drawSelected();
        if (currentHover === element) draw(hoverBox, currentHover);
        post(TYPES.CHANGE_APPLIED, { changeId, selector, ok: true });
      } catch (error) {
        post(TYPES.CHANGE_APPLIED, { changeId, selector, ok: false, error: String(error && error.message || error) });
      }
    }

    function clearChanges() {
      Object.keys(originals).forEach(function (selector) {
        const element = document.querySelector(selector);
        const original = originals[selector];
        if (!element || !original) return;
        if (original.text != null) element.textContent = original.text;
        Object.keys(original.styles || {}).forEach(function (property) {
          element.style[property] = original.styles[property];
        });
      });
      originals = Object.create(null);
      draw(hoverBox, currentHover);
      drawSelected();
      post(TYPES.CHANGE_APPLIED, { changeId: 'clear', ok: true, cleared: true });
    }

    function commitMutations(element, changes, groupLabel) {
      if (!element || !changes || !changes.length) return;
      drawSelected();
      postSelection(element);
      post(TYPES.ELEMENT_MUTATED, {
        element: snapshot(element),
        groupLabel: groupLabel || 'Edit',
        changes,
      });
    }

    function finishTextEdit(commit) {
      if (!editingElement) return;
      const element = editingElement;
      const oldText = editingOriginalText;
      const nextText = String(element.textContent || '');
      element.removeAttribute('contenteditable');
      element.style.outline = '';
      editingElement = null;
      editingOriginalText = '';
      try {
        const selection = window.getSelection();
        if (selection) selection.removeAllRanges();
      } catch { /* ignore */ }
      if (!commit) {
        element.textContent = oldText;
        drawSelected();
        postSelection(element);
        return;
      }
      rememberOriginal(selectorFor(element), element, 'text', null, oldText, true);
      if (oldText === nextText) {
        drawSelected();
        postSelection(element);
      } else {
        commitMutations(element, [{ type: 'text', oldValue: oldText, newValue: nextText }], 'Text Edit');
      }
      suppressClick = true;
      setTimeout(function () { suppressClick = false; }, 0);
    }

    function startTextEdit(element, event) {
      if (!isTextEditableElement(element)) return false;
      event.preventDefault();
      event.stopPropagation();
      if (editingElement && editingElement !== element) finishTextEdit(true);
      currentSelected = element;
      elementId(currentSelected);
      drawSelected();
      postSelection(element);
      editingElement = element;
      editingOriginalText = String(element.textContent || '');
      element.setAttribute('contenteditable', 'true');
      element.style.outline = '2px solid rgba(0,122,255,0.65)';
      element.focus({ preventScroll: true });
      selectTextContents(element);
      return true;
    }

    function startResize(element, dir, event) {
      event.preventDefault();
      event.stopPropagation();
      const start = element.getBoundingClientRect();
      const cs = window.getComputedStyle(element);
      const borderBox = cs.boxSizing === 'border-box';
      const extraX = borderBox ? 0 : px(cs.paddingLeft) + px(cs.paddingRight) + px(cs.borderLeftWidth) + px(cs.borderRightWidth);
      const extraY = borderBox ? 0 : px(cs.paddingTop) + px(cs.paddingBottom) + px(cs.borderTopWidth) + px(cs.borderBottomWidth);
      const startX = event.clientX;
      const startY = event.clientY;
      const oldWidth = cs.width;
      const oldHeight = cs.height;
      const targetSelector = selectorFor(element);
      rememberOriginal(targetSelector, element, 'style', 'width');
      rememberOriginal(targetSelector, element, 'style', 'height');
      selectedBox.style.transition = 'none';
      dimensionLabel.style.transition = 'none';
      function onMove(ev) {
        const dx = ev.clientX - startX;
        const dy = ev.clientY - startY;
        let w = start.width;
        let h = start.height;
        if (dir.includes('e')) w = start.width + dx;
        if (dir.includes('w')) w = start.width - dx;
        if (dir.includes('s')) h = start.height + dy;
        if (dir.includes('n')) h = start.height - dy;
        if (dir.includes('e') || dir.includes('w')) element.style.setProperty('width', Math.max(MIN_SIZE, w - extraX) + 'px', 'important');
        if (dir.includes('n') || dir.includes('s')) element.style.setProperty('height', Math.max(MIN_SIZE, h - extraY) + 'px', 'important');
        drawSelected();
      }
      function onUp() {
        document.removeEventListener('mousemove', onMove, true);
        document.removeEventListener('mouseup', onUp, true);
        selectedBox.style.transition = '';
        dimensionLabel.style.transition = '';
        const next = window.getComputedStyle(element);
        const changes = [];
        if (oldWidth !== next.width) changes.push({ type: 'style', property: 'width', oldValue: oldWidth, newValue: next.width });
        if (oldHeight !== next.height) changes.push({ type: 'style', property: 'height', oldValue: oldHeight, newValue: next.height });
        suppressClick = true;
        setTimeout(function () { suppressClick = false; }, 0);
        commitMutations(element, changes, 'Resize');
      }
      document.addEventListener('mousemove', onMove, true);
      document.addEventListener('mouseup', onUp, true);
    }

    function startMove(element, event) {
      const startX = event.clientX;
      const startY = event.clientY;
      let started = false;
      const cs = window.getComputedStyle(element);
      const wasStatic = cs.position === 'static';
      const oldPosition = cs.position;
      const oldLeft = cs.left;
      const oldTop = cs.top;
      const baseLeft = wasStatic ? 0 : px(cs.left);
      const baseTop = wasStatic ? 0 : px(cs.top);
      const targetSelector = selectorFor(element);
      rememberOriginal(targetSelector, element, 'style', 'position');
      rememberOriginal(targetSelector, element, 'style', 'left');
      rememberOriginal(targetSelector, element, 'style', 'top');
      function onMove(ev) {
        let dx = ev.clientX - startX;
        let dy = ev.clientY - startY;
        if (!started) {
          if (Math.abs(dx) < DRAG_THRESHOLD && Math.abs(dy) < DRAG_THRESHOLD) return;
          started = true;
          selectedBox.style.transition = 'none';
          dimensionLabel.style.transition = 'none';
          if (wasStatic) element.style.setProperty('position', 'relative', 'important');
        }
        if (ev.shiftKey) {
          if (Math.abs(dx) >= Math.abs(dy)) dy = 0;
          else dx = 0;
        }
        element.style.setProperty('left', Math.round(baseLeft + dx) + 'px', 'important');
        element.style.setProperty('top', Math.round(baseTop + dy) + 'px', 'important');
        drawSelected();
      }
      function onUp() {
        document.removeEventListener('mousemove', onMove, true);
        document.removeEventListener('mouseup', onUp, true);
        selectedBox.style.transition = '';
        dimensionLabel.style.transition = '';
        if (!started) return;
        const next = window.getComputedStyle(element);
        const changes = [];
        if (wasStatic) changes.push({ type: 'style', property: 'position', oldValue: oldPosition, newValue: next.position });
        if (oldLeft !== next.left) changes.push({ type: 'style', property: 'left', oldValue: oldLeft, newValue: next.left });
        if (oldTop !== next.top) changes.push({ type: 'style', property: 'top', oldValue: oldTop, newValue: next.top });
        suppressClick = true;
        setTimeout(function () { suppressClick = false; }, 0);
        commitMutations(element, changes, 'Move');
      }
      document.addEventListener('mousemove', onMove, true);
      document.addEventListener('mouseup', onUp, true);
    }

    function onMove(event) {
      if (editingElement) return;
      const target = event.target;
      if (!target || target === document.documentElement || target === document.body || isRuntimeNode(target)) return;
      currentHover = target;
      draw(hoverBox, currentHover);
    }

    function onDown(event) {
      if (editingElement) return;
      const target = event.target;
      if (!target || target === document.documentElement || target === document.body || isRuntimeNode(target)) return;
      if (currentSelected && target === currentSelected) startMove(currentSelected, event);
    }

    function onClick(event) {
      const target = event.target;
      if (!target || target === document.documentElement || target === document.body || isRuntimeNode(target)) return;
      if (editingElement) {
        if (target === editingElement || editingElement.contains(target)) return;
        finishTextEdit(true);
        return;
      }
      if (suppressClick) {
        event.preventDefault();
        event.stopPropagation();
        event.stopImmediatePropagation();
        return;
      }
      event.preventDefault();
      event.stopPropagation();
      currentSelected = target;
      elementId(currentSelected);
      drawSelected();
      postSelection(target);
    }

    function onDoubleClick(event) {
      const target = event.target;
      if (!target || target === document.documentElement || target === document.body || isRuntimeNode(target)) return;
      startTextEdit(target, event);
    }

    function onKeyDown(event) {
      if (!editingElement) return;
      if (event.key === 'Escape') {
        event.preventDefault();
        event.stopPropagation();
        finishTextEdit(false);
      } else if (event.key === 'Enter' && !event.shiftKey
        // IME 守卫:此处运行在隔离 iframe 内(由 buildDesignRuntimeScript 生成脚本注入,
        // 测试以 vm.runInContext 模拟),无法 ESM import,故内联与 src/shared/ime-guard.mjs
        // 中 isImeComposing 等价的判断。keyCode === 229 兜底 macOS WKWebView bug 165004。
        && !(event.isComposing || event.keyCode === 229)) {
        event.preventDefault();
        event.stopPropagation();
        finishTextEdit(true);
      }
    }

    function onFocusOut(event) {
      if (!editingElement) return;
      const next = event.relatedTarget;
      if (next && (next === editingElement || editingElement.contains(next))) return;
      setTimeout(function () {
        if (editingElement && document.activeElement !== editingElement && !editingElement.contains(document.activeElement)) {
          finishTextEdit(true);
        }
      }, 0);
    }

    function onScrollOrResize() {
      draw(hoverBox, currentHover);
      drawSelected();
    }

    function onMessage(event) {
      const data = event && event.data;
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
      if (editingElement) finishTextEdit(false);
      document.removeEventListener('mousemove', onMove, true);
      document.removeEventListener('mousedown', onDown, true);
      document.removeEventListener('click', onClick, true);
      document.removeEventListener('dblclick', onDoubleClick, true);
      document.removeEventListener('keydown', onKeyDown, true);
      document.removeEventListener('focusout', onFocusOut, true);
      window.removeEventListener('scroll', onScrollOrResize, true);
      window.removeEventListener('resize', onScrollOrResize, true);
      window.removeEventListener('message', onMessage, true);
      if (hoverBox && hoverBox.parentNode) hoverBox.remove();
      if (selectedBox && selectedBox.parentNode) selectedBox.remove();
      if (marginBand && marginBand.parentNode) marginBand.remove();
      if (paddingBand && paddingBand.parentNode) paddingBand.remove();
      if (dimensionLabel && dimensionLabel.parentNode) dimensionLabel.remove();
      if (handleLayer && handleLayer.parentNode) handleLayer.remove();
      currentHover = null;
      currentSelected = null;
      window.__PINVOU_DESIGN_RUNTIME__ = null;
      post(TYPES.DESTROYED);
    }

    try {
      document.addEventListener('mousemove', onMove, true);
      document.addEventListener('mousedown', onDown, true);
      document.addEventListener('click', onClick, true);
      document.addEventListener('dblclick', onDoubleClick, true);
      document.addEventListener('keydown', onKeyDown, true);
      document.addEventListener('focusout', onFocusOut, true);
      window.addEventListener('scroll', onScrollOrResize, true);
      window.addEventListener('resize', onScrollOrResize, true);
      window.addEventListener('message', onMessage, true);
      window.__PINVOU_DESIGN_RUNTIME__ = { destroy };
      post(TYPES.READY);
    } catch (error) {
      post(TYPES.ERROR, { error: String(error && error.message || error) });
    }
  }.toString()})();`;
}

export {
  DESIGN_MESSAGE_TYPES,
  buildDesignRuntimeScript,
};
