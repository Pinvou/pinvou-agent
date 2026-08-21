import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const source = fs.readFileSync(path.join(here, '..', 'src', 'features', 'chat', 'ChatView.jsx'), 'utf8');

const headerClass = source.match(/bg-gradient-to-b from-blue-50\/80 to-white border-slate-100[^'"]+/)?.[0] || '';

assert.ok(
  headerClass.includes('dark:bg-none') && headerClass.includes('dark:bg-[#1E1F20]'),
  'ToolWelcomeCard header must remove the light gradient in dark mode so connected tool cards keep a dark header',
);

console.log('tool welcome card theme contract passed');
