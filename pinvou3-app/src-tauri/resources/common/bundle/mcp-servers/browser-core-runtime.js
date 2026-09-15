/*
 * Pinvou BrowserCore page runtime.
 *
 * This script intentionally exposes no host bridge and no filesystem or native
 * capability.  It only turns the current document into a platform-neutral
 * accessibility snapshot and resolves snapshot refs back to page coordinates.
 * Windows, macOS and Linux inject the same source; trusted input is dispatched
 * by the platform driver after the Rust host has validated the task lease.
 */
(() => {
  'use strict';

  const VERSION = 1;
  const GLOBAL_KEY = '__PINVOU_BROWSER_CORE_V1__';
  if (globalThis[GLOBAL_KEY]?.version === VERSION) return;

  const elementToRef = new WeakMap();
  const refToElement = new Map();
  let nextRef = 1;

  const normalize = (value) => String(value ?? '').replace(/\s+/g, ' ').trim();

  function refFor(element) {
    let ref = elementToRef.get(element);
    if (!ref) {
      ref = `p${nextRef++}`;
      elementToRef.set(element, ref);
    }
    refToElement.set(ref, element);
    return ref;
  }

  function visible(element) {
    if (!(element instanceof Element)) return false;
    const style = getComputedStyle(element);
    if (style.display === 'none' || style.visibility === 'hidden' || Number(style.opacity) === 0) {
      return false;
    }
    const rect = element.getBoundingClientRect();
    return rect.width > 0 && rect.height > 0;
  }

  function implicitRole(element) {
    const tag = element.localName;
    if (tag === 'a' && element.hasAttribute('href')) return 'link';
    if (tag === 'button') return 'button';
    if (tag === 'textarea') return 'textbox';
    if (tag === 'select') return 'combobox';
    if (tag === 'option') return 'option';
    if (tag === 'img') return 'img';
    if (/^h[1-6]$/.test(tag)) return 'heading';
    if (tag === 'input') {
      const type = (element.getAttribute('type') || 'text').toLowerCase();
      if (type === 'checkbox') return 'checkbox';
      if (type === 'radio') return 'radio';
      if (['button', 'submit', 'reset'].includes(type)) return 'button';
      if (type === 'range') return 'slider';
      return 'textbox';
    }
    return '';
  }

  function roleFor(element) {
    return normalize(element.getAttribute('role')) || implicitRole(element);
  }

  function labelledByName(element) {
    const ids = normalize(element.getAttribute('aria-labelledby')).split(' ').filter(Boolean);
    if (!ids.length) return '';
    return normalize(ids.map((id) => document.getElementById(id)?.textContent || '').join(' '));
  }

  function nameFor(element) {
    const ariaLabel = normalize(element.getAttribute('aria-label'));
    if (ariaLabel) return ariaLabel;
    const labelled = labelledByName(element);
    if (labelled) return labelled;
    if (element.labels?.length) {
      const label = normalize(Array.from(element.labels, (item) => item.textContent || '').join(' '));
      if (label) return label;
    }
    const alt = normalize(element.getAttribute('alt'));
    if (alt) return alt;
    const title = normalize(element.getAttribute('title'));
    if (title) return title;
    if (element instanceof HTMLInputElement) {
      const value = normalize(element.value || element.getAttribute('placeholder'));
      if (value) return value;
    }
    return normalize(element.textContent).slice(0, 240);
  }

  function stateFor(element) {
    const state = [];
    if ('disabled' in element && element.disabled) state.push('disabled');
    if ('checked' in element && typeof element.checked === 'boolean') {
      state.push(element.checked ? 'checked' : 'unchecked');
    }
    if (element.getAttribute('aria-expanded') === 'true') state.push('expanded');
    if (element.getAttribute('aria-expanded') === 'false') state.push('collapsed');
    if (document.activeElement === element) state.push('focused');
    return state;
  }

  function childRoots(element) {
    const roots = [];
    if (element.shadowRoot?.mode === 'open') roots.push(element.shadowRoot);
    return roots;
  }

  function walk(root, output, verbose) {
    const walker = document.createTreeWalker(root, NodeFilter.SHOW_ELEMENT);
    let element = walker.currentNode instanceof Element ? walker.currentNode : walker.nextNode();
    while (element) {
      if (visible(element)) {
        const role = roleFor(element);
        const interactive = Boolean(
          role ||
          element.tabIndex >= 0 ||
          element.hasAttribute('contenteditable') ||
          element.hasAttribute('onclick')
        );
        if (interactive || verbose) {
          const name = nameFor(element);
          if (interactive || name) {
            output.push({
              uid: refFor(element),
              role: role || 'generic',
              name,
              states: stateFor(element),
              tag: element.localName,
            });
          }
        }
        for (const childRoot of childRoots(element)) walk(childRoot, output, verbose);
      }
      element = walker.nextNode();
    }
  }

  function snapshot(options = {}) {
    refToElement.clear();
    const nodes = [];
    walk(document, nodes, options.verbose === true);
    const lines = [`document ${JSON.stringify(document.title || '')} ${JSON.stringify(location.href)}`];
    for (const node of nodes) {
      const states = node.states.length ? ` [${node.states.join(', ')}]` : '';
      const name = node.name ? ` ${JSON.stringify(node.name)}` : '';
      lines.push(`${node.role}${name} uid=${node.uid}${states}`);
    }
    return { text: lines.join('\n'), nodes, title: document.title || '', url: location.href };
  }

  function elementFor(uid) {
    const element = refToElement.get(String(uid || ''));
    if (!(element instanceof Element) || !element.isConnected || !visible(element)) {
      throw new Error(`browser/stale-ref: ${uid}`);
    }
    return element;
  }

  function point(uid) {
    const element = elementFor(uid);
    let rect = element.getBoundingClientRect();
    const intersectsViewport = () => (
      rect.right > 0
      && rect.bottom > 0
      && rect.left < innerWidth
      && rect.top < innerHeight
    );
    if (!intersectsViewport()) {
      element.scrollIntoView({ block: 'center', inline: 'center' });
      rect = element.getBoundingClientRect();
    }
    const left = Math.max(0, rect.left);
    const right = Math.min(innerWidth, rect.right);
    const top = Math.max(0, rect.top);
    const bottom = Math.min(innerHeight, rect.bottom);
    if (right <= left || bottom <= top) {
      throw new Error(`browser/element-outside-viewport: ${uid}`);
    }
    const x = left + (right - left) / 2;
    const y = top + (bottom - top) / 2;
    const hit = document.elementFromPoint(x, y);
    if (hit && hit !== element && !element.contains(hit) && !hit.contains(element)) {
      throw new Error(`browser/element-obscured: ${uid}`);
    }
    return { x, y, uid: String(uid), tag: element.localName };
  }

  function argumentFor(value) {
    return typeof value === 'string' && refToElement.has(value) ? elementFor(value) : value;
  }

  async function evaluate(functionDeclaration, args = []) {
    if (typeof functionDeclaration !== 'string' || !functionDeclaration.trim()) {
      throw new Error('browser/invalid-function');
    }
    const callable = (0, eval)(`(${functionDeclaration})`);
    if (typeof callable !== 'function') throw new Error('browser/function-required');
    return await callable(...args.map(argumentFor));
  }

  async function waitFor(texts, timeout = 10000) {
    const wanted = Array.isArray(texts) ? texts.map(normalize).filter(Boolean) : [];
    if (!wanted.length) throw new Error('browser/wait-text-required');
    const deadline = Date.now() + Math.max(1, Number(timeout) || 10000);
    while (Date.now() <= deadline) {
      const bodyText = normalize(document.body?.innerText || document.documentElement?.innerText || '');
      const match = wanted.find((text) => bodyText.includes(text));
      if (match) return { match };
      await new Promise((resolve) => setTimeout(resolve, 50));
    }
    throw new Error(`browser/wait-timeout: ${wanted.join(' | ')}`);
  }

  Object.defineProperty(globalThis, GLOBAL_KEY, {
    value: Object.freeze({
      version: VERSION,
      snapshot,
      point,
      element: elementFor,
      evaluate,
      waitFor,
    }),
    configurable: false,
    enumerable: false,
    writable: false,
  });
})();
