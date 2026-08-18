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
    var TYPES = {
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
    var STYLE_FIELDS = ['color','backgroundColor','fontSize','fontWeight','margin','padding','width','height','minWidth','maxWidth','minHeight','maxHeight','display','position','top','right','bottom','left','zIndex','opacity','lineHeight','letterSpacing','textAlign','fontFamily','backgroundImage','backgroundSize','backgroundPosition','backgroundRepeat','marginTop','marginRight','marginBottom','marginLeft','paddingTop','paddingRight','paddingBottom','paddingLeft','gap','rowGap','columnGap','flexDirection','justifyContent','alignItems','alignSelf','overflow','borderTopWidth','borderRightWidth','borderBottomWidth','borderLeftWidth','borderTopStyle','borderRightStyle','borderBottomStyle','borderLeftStyle','borderTopColor','borderRightColor','borderBottomColor','borderLeftColor','borderTopLeftRadius','borderTopRightRadius','borderBottomRightRadius','borderBottomLeftRadius','borderRadius','visibility','cursor'];
    var DATA_ID = 'data-pinvou-design-id';
    var nextId = 1;
    var MIN_SIZE = 8;
    var DRAG_THRESHOLD = 3;
    var handles = [
      ['nw','nwse-resize'], ['n','ns-resize'], ['ne','nesw-resize'], ['e','ew-resize'],
      ['se','nwse-resize'], ['s','ns-resize'], ['sw','nesw-resize'], ['w','ew-resize']
    ];
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

    function elementLabel(element) {
      if (!element || !element.tagName) return 'element';
      var tag = element.tagName.toLowerCase();
      if (element.id) return tag + '#' + element.id;
      if (element.className && typeof element.className === 'string') {
        var cls = element.className.trim().split(/\s+/).filter(Boolean)[0];
        if (cls) return tag + '.' + cls;
      }
      var text = String(element.textContent || '').trim().replace(/\s+/g, ' ');
      if (text) return tag + ' "' + (text.length > 24 ? text.slice(0, 23) + '...' : text) + '"';
      return tag;
    }

    function breadcrumbs(element) {
      var items = [];
      var current = element;
      while (current && current.nodeType === 1 && current !== document.documentElement && items.length < 8) {
        items.unshift(selectorPart(current));
        current = current.parentElement;
      }
      return items;
    }

    function elementId(element) {
      if (!element || !element.setAttribute) return '';
      var id = element.getAttribute(DATA_ID);
      if (!id) {
        id = 'pdm-' + nextId++;
        element.setAttribute(DATA_ID, id);
      }
      return id;
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

    function makeBand(kind) {
      var band = document.createElement('div');
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
      document.documentElement.appendChild(band);
      return band;
    }

    function makeLabel() {
      var label = document.createElement('div');
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
      document.documentElement.appendChild(label);
      return label;
    }

    function makeHandleLayer() {
      var layer = document.createElement('div');
      layer.setAttribute('data-pinvou-design-handles', 'true');
      layer.style.cssText = 'position:fixed;inset:0;z-index:2147483647;pointer-events:none;display:none';
      document.documentElement.appendChild(layer);
      return layer;
    }

    var hoverBox = makeBox('hover');
    var selectedBox = makeBox('selected');
    var marginBand = makeBand('margin');
    var paddingBand = makeBand('padding');
    var dimensionLabel = makeLabel();
    var handleLayer = makeHandleLayer();
    var currentHover = null;
    var currentSelected = null;
    var originals = Object.create(null);
    var suppressClick = false;
    var editingElement = null;
    var editingOriginalText = '';

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

    function px(value) {
      var parsed = parseFloat(String(value || ''));
      return Number.isFinite(parsed) ? parsed : 0;
    }

    function drawBands(element) {
      if (!element || !element.getBoundingClientRect) {
        marginBand.style.display = 'none';
        paddingBand.style.display = 'none';
        return;
      }
      var rect = element.getBoundingClientRect();
      var cs = window.getComputedStyle(element);
      var mt = px(cs.marginTop), mr = px(cs.marginRight), mb = px(cs.marginBottom), ml = px(cs.marginLeft);
      var pt = px(cs.paddingTop), pr = px(cs.paddingRight), pb = px(cs.paddingBottom), pl = px(cs.paddingLeft);
      var bt = px(cs.borderTopWidth), br = px(cs.borderRightWidth), bb = px(cs.borderBottomWidth), bl = px(cs.borderLeftWidth);
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
      var midX = rect.left + rect.width / 2;
      var midY = rect.top + rect.height / 2;
      return {
        x: dir.indexOf('w') >= 0 ? rect.left : dir.indexOf('e') >= 0 ? rect.right : midX,
        y: dir.indexOf('n') >= 0 ? rect.top : dir.indexOf('s') >= 0 ? rect.bottom : midY,
      };
    }

    function drawHandles(element) {
      if (!element || !element.getBoundingClientRect) {
        handleLayer.style.display = 'none';
        handleLayer.replaceChildren();
        return;
      }
      var rect = element.getBoundingClientRect();
      handleLayer.style.display = 'block';
      handleLayer.replaceChildren();
      handles.forEach(function (item) {
        var dir = item[0], cursor = item[1];
        var p = handlePoint(rect, dir);
        var dot = document.createElement('div');
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
        handleLayer.appendChild(dot);
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
      var rect = currentSelected.getBoundingClientRect();
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
      var tag = element.tagName.toLowerCase();
      if (/^(script|style|html|body|iframe|img|svg|canvas|input|textarea|select)$/.test(tag)) return false;
      if (/^(span|p|a|button|label|strong|em|b|i|small|h1|h2|h3|h4|h5|h6)$/.test(tag)) return true;
      var text = String(element.innerText || element.textContent || '').trim();
      if (!text || text.length > 160) return false;
      return element.children.length <= 1;
    }

    function selectTextContents(element) {
      try {
        var range = document.createRange();
        range.selectNodeContents(element);
        var selection = window.getSelection();
        if (!selection) return;
        selection.removeAllRanges();
        selection.addRange(range);
      } catch (e) { /* ignore */ }
    }

    function snapshot(element) {
      var rect = element.getBoundingClientRect();
      var computed = window.getComputedStyle(element);
      var computedStyle = {};
      STYLE_FIELDS.forEach(function (field) { computedStyle[field] = computed[field] || ''; });
      return {
        id: elementId(element),
        selector: selectorFor(element),
        label: elementLabel(element),
        tagName: element.tagName.toLowerCase(),
        className: element.className && typeof element.className === 'string' ? element.className : '',
        breadcrumbs: breadcrumbs(element),
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

    function postSelection(element) {
      if (!element) return;
      post(TYPES.ELEMENT_SELECTED, { element: snapshot(element) });
    }

    function rememberOriginal(selector, element, type, property, originalValue, hasOriginalValue) {
      if (!selector || !element) return;
      var bucket = originals[selector] || (originals[selector] = { text: null, styles: Object.create(null) });
      if (type === 'text' && bucket.text == null) {
        bucket.text = hasOriginalValue ? String(originalValue == null ? '' : originalValue) : (element.textContent || '');
      }
      if (type === 'style' && property && !Object.prototype.hasOwnProperty.call(bucket.styles, property)) {
        bucket.styles[property] = hasOriginalValue
          ? String(originalValue == null ? '' : originalValue)
          : (element.style[property] || '');
      }
    }

    function applyChange(payload) {
      var selector = payload && payload.selector;
      var changeId = payload && payload.changeId;
      var type = payload && payload.changeType;
      var property = payload && payload.property;
      var value = payload && payload.value;
      var hasOriginalValue = !!(payload && Object.prototype.hasOwnProperty.call(payload, 'oldValue'));
      var originalValue = payload && payload.oldValue;
      try {
        var element = selector ? document.querySelector(selector) : currentSelected;
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
        changes: changes,
      });
    }

    function finishTextEdit(commit) {
      if (!editingElement) return;
      var element = editingElement;
      var oldText = editingOriginalText;
      var nextText = String(element.textContent || '');
      element.removeAttribute('contenteditable');
      element.style.outline = '';
      editingElement = null;
      editingOriginalText = '';
      try {
        var selection = window.getSelection();
        if (selection) selection.removeAllRanges();
      } catch (e) { /* ignore */ }
      if (!commit) {
        element.textContent = oldText;
        drawSelected();
        postSelection(element);
        return;
      }
      rememberOriginal(selectorFor(element), element, 'text', null, oldText, true);
      if (oldText !== nextText) {
        commitMutations(element, [{ type: 'text', oldValue: oldText, newValue: nextText }], 'Text Edit');
      } else {
        drawSelected();
        postSelection(element);
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
      var start = element.getBoundingClientRect();
      var cs = window.getComputedStyle(element);
      var borderBox = cs.boxSizing === 'border-box';
      var extraX = borderBox ? 0 : px(cs.paddingLeft) + px(cs.paddingRight) + px(cs.borderLeftWidth) + px(cs.borderRightWidth);
      var extraY = borderBox ? 0 : px(cs.paddingTop) + px(cs.paddingBottom) + px(cs.borderTopWidth) + px(cs.borderBottomWidth);
      var startX = event.clientX;
      var startY = event.clientY;
      var oldWidth = cs.width;
      var oldHeight = cs.height;
      var targetSelector = selectorFor(element);
      rememberOriginal(targetSelector, element, 'style', 'width');
      rememberOriginal(targetSelector, element, 'style', 'height');
      selectedBox.style.transition = 'none';
      dimensionLabel.style.transition = 'none';
      function onMove(ev) {
        var dx = ev.clientX - startX;
        var dy = ev.clientY - startY;
        var w = start.width;
        var h = start.height;
        if (dir.indexOf('e') >= 0) w = start.width + dx;
        if (dir.indexOf('w') >= 0) w = start.width - dx;
        if (dir.indexOf('s') >= 0) h = start.height + dy;
        if (dir.indexOf('n') >= 0) h = start.height - dy;
        if (dir.indexOf('e') >= 0 || dir.indexOf('w') >= 0) element.style.setProperty('width', Math.max(MIN_SIZE, w - extraX) + 'px', 'important');
        if (dir.indexOf('n') >= 0 || dir.indexOf('s') >= 0) element.style.setProperty('height', Math.max(MIN_SIZE, h - extraY) + 'px', 'important');
        drawSelected();
      }
      function onUp() {
        document.removeEventListener('mousemove', onMove, true);
        document.removeEventListener('mouseup', onUp, true);
        selectedBox.style.transition = '';
        dimensionLabel.style.transition = '';
        var next = window.getComputedStyle(element);
        var changes = [];
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
      var startX = event.clientX;
      var startY = event.clientY;
      var started = false;
      var cs = window.getComputedStyle(element);
      var wasStatic = cs.position === 'static';
      var oldPosition = cs.position;
      var oldLeft = cs.left;
      var oldTop = cs.top;
      var baseLeft = wasStatic ? 0 : px(cs.left);
      var baseTop = wasStatic ? 0 : px(cs.top);
      var targetSelector = selectorFor(element);
      rememberOriginal(targetSelector, element, 'style', 'position');
      rememberOriginal(targetSelector, element, 'style', 'left');
      rememberOriginal(targetSelector, element, 'style', 'top');
      function onMove(ev) {
        var dx = ev.clientX - startX;
        var dy = ev.clientY - startY;
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
        var next = window.getComputedStyle(element);
        var changes = [];
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
      var target = event.target;
      if (!target || target === document.documentElement || target === document.body || isRuntimeNode(target)) return;
      currentHover = target;
      draw(hoverBox, currentHover);
    }

    function onDown(event) {
      if (editingElement) return;
      var target = event.target;
      if (!target || target === document.documentElement || target === document.body || isRuntimeNode(target)) return;
      if (currentSelected && target === currentSelected) startMove(currentSelected, event);
    }

    function onClick(event) {
      var target = event.target;
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
      var target = event.target;
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
      var next = event.relatedTarget;
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
      if (hoverBox && hoverBox.parentNode) hoverBox.parentNode.removeChild(hoverBox);
      if (selectedBox && selectedBox.parentNode) selectedBox.parentNode.removeChild(selectedBox);
      if (marginBand && marginBand.parentNode) marginBand.parentNode.removeChild(marginBand);
      if (paddingBand && paddingBand.parentNode) paddingBand.parentNode.removeChild(paddingBand);
      if (dimensionLabel && dimensionLabel.parentNode) dimensionLabel.parentNode.removeChild(dimensionLabel);
      if (handleLayer && handleLayer.parentNode) handleLayer.parentNode.removeChild(handleLayer);
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
      window.__PINVOU_DESIGN_RUNTIME__ = { destroy: destroy };
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
