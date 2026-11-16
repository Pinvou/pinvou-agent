#!/usr/bin/env node
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const source = readFileSync(
  path.join(here, '..', 'src', 'features', 'knowledge', 'KnowledgeView.jsx'),
  'utf8',
);

// The collection card carries an inline style.boxShadow (the active card's
// collColor ring, and in light theme every card's panelShadow). Tailwind ring-*
// utilities compose into box-shadow, so an inline boxShadow always wins the
// cascade and a ring-based focus indicator would never render. The keyboard
// focus indicator (WCAG 2.4.7) must therefore stay outline-based.
const cardClass = source
  .split('\n')
  .find((line) => line.includes('rounded-2xl cursor-pointer') && line.includes('focus-visible:'));
assert.ok(cardClass, 'collection card focus class line is missing');

assert.match(cardClass, /focus-visible:outline-2\b/);
assert.match(cardClass, /focus-visible:outline-\[#0B57D0\]/);
assert.doesNotMatch(
  cardClass,
  /focus-visible:ring-/,
  'ring-based focus indicator is dead under the card inline boxShadow — keep the outline form',
);

// The card still keeps its keyboard operability from the r8 a11y rework.
assert.match(source, /role="button" tabIndex=\{0\} onClick=\{\(\) => openColl\(c\)\}/);
