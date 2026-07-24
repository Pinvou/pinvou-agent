#!/usr/bin/env node
const assert = require('assert');
const fs = require('fs');
const path = require('path');
const vm = require('vm');

const logicPath = path.join(__dirname, '..', 'src', 'features', 'artifacts', 'design-runtime.js');
const code = fs.readFileSync(logicPath, 'utf8')
  .replace(/\bexport\s+\{[^}]+\};?/g, '')
  .replace(/\bexport\s+/g, '');

const ctx = {};
vm.createContext(ctx);
vm.runInContext(`${code}
this.DESIGN_MESSAGE_TYPES = DESIGN_MESSAGE_TYPES;
this.DESIGN_STYLE_FIELDS = DESIGN_STYLE_FIELDS;
this.buildDesignRuntimeScript = buildDesignRuntimeScript;
this.getDesignElementSelector = getDesignElementSelector;
this.snapshotDesignElement = snapshotDesignElement;`, ctx, {
  filename: logicPath,
});

const {
  DESIGN_MESSAGE_TYPES,
  DESIGN_STYLE_FIELDS,
  buildDesignRuntimeScript,
  getDesignElementSelector,
  snapshotDesignElement,
} = ctx;

const plain = (value) => JSON.parse(JSON.stringify(value));

assert.strictEqual(DESIGN_MESSAGE_TYPES.READY, 'pinvou:design-runtime-ready');
assert.strictEqual(DESIGN_MESSAGE_TYPES.ELEMENT_SELECTED, 'pinvou:design-element-selected');
assert.strictEqual(DESIGN_MESSAGE_TYPES.APPLY_CHANGE, 'pinvou:design-apply-change');
assert.strictEqual(DESIGN_MESSAGE_TYPES.CHANGE_APPLIED, 'pinvou:design-change-applied');
assert.strictEqual(DESIGN_MESSAGE_TYPES.CLEAR_CHANGES, 'pinvou:design-clear-changes');
assert.strictEqual(DESIGN_MESSAGE_TYPES.DESTROY, 'pinvou:design-runtime-destroy');
assert.ok(plain(DESIGN_STYLE_FIELDS).includes('fontSize'));
assert.ok(plain(DESIGN_STYLE_FIELDS).includes('borderRadius'));

function makeElement(tagName, options = {}) {
  const element = {
    tagName: tagName.toUpperCase(),
    id: options.id || '',
    className: (options.classes || []).join(' '),
    classList: options.classes || [],
    parentElement: null,
    children: [],
    nodeType: 1,
    innerText: options.text || '',
    textContent: options.text || '',
    getBoundingClientRect() {
      return { x: 10, y: 20, width: 300, height: 80 };
    },
    ownerDocument: {
      documentElement: null,
      defaultView: {
        getComputedStyle() {
          return {
            color: 'rgb(17, 24, 39)',
            backgroundColor: 'rgba(0, 0, 0, 0)',
            fontSize: '48px',
            fontWeight: '700',
            margin: '0px',
            padding: '8px',
            width: '300px',
            height: '80px',
            borderRadius: '12px',
          };
        },
      },
    },
  };
  element.ownerDocument.documentElement = options.documentElement || { nodeType: 1 };
  return element;
}

const root = makeElement('div', { id: 'app' });
const section = makeElement('section', { classes: ['hero'] });
const firstTitle = makeElement('h1', { classes: ['hero-title', 'large'], text: 'Hello' });
const secondTitle = makeElement('h1', { classes: ['hero-title', 'large'], text: 'Pinvou' });
section.parentElement = root;
root.children = [section];
firstTitle.parentElement = section;
secondTitle.parentElement = section;
section.children = [firstTitle, secondTitle];

assert.strictEqual(getDesignElementSelector(root), 'div#app');
assert.strictEqual(getDesignElementSelector(secondTitle), 'div#app > section.hero > h1.hero-title.large:nth-of-type(2)');

const snapshot = snapshotDesignElement(secondTitle);
assert.strictEqual(snapshot.selector, 'div#app > section.hero > h1.hero-title.large:nth-of-type(2)');
assert.strictEqual(snapshot.tagName, 'h1');
assert.strictEqual(snapshot.text, 'Pinvou');
assert.deepStrictEqual(plain(snapshot.rect), { x: 10, y: 20, width: 300, height: 80 });
assert.strictEqual(snapshot.computedStyle.fontSize, '48px');
assert.strictEqual(snapshot.computedStyle.borderRadius, '12px');

const script = buildDesignRuntimeScript();
assert.ok(script.includes('pinvou:design-runtime-ready'));
assert.ok(script.includes('pinvou:design-element-selected'));
assert.ok(script.includes('pinvou:design-apply-change'));
assert.ok(script.includes('pinvou:design-change-applied'));
assert.ok(script.includes('pinvou:design-clear-changes'));
assert.ok(script.includes('pinvou:design-runtime-destroy'));
assert.ok(script.includes('data-pinvou-design-hover'));

console.log('design_runtime_logic: ok');
