#!/usr/bin/env node
// WebView compatibility auditor for the minimum supported baseline
// (macOS 11 WKWebView = Safari 14.0; Windows 10 1809 WebView2 = evergreen
// Chromium and therefore not the binding constraint).
//
// What it checks, and why each layer exists:
//   1. Parse output with acorn at ES2021: Safari 14.0 supports everything in
//      ES2020/2021 (incl. logical assignment), while ES2022+ syntax it cannot
//      parse (class fields, private fields, static blocks, top-level await)
//      fails as a SyntaxError and blanks the whole chunk.
//   2. RegExp literals: lookbehind assertions "(?<=" / "(?<!" need Safari
//      16.4, and the "v"/"d" flags need 17/15.4 — regex literals are never
//      downlevelled by bundlers, so they must not enter the bundle at all.
//   3. Runtime member/global APIs added after Safari 14.0 (.at(), findLast,
//      copy-methods, Object.hasOwn, structuredClone, ...): parse-time clean
//      but a TypeError the moment a code path runs.
//
// Inputs: built chunks under dist/assets, plus the verbatim-copied static
// runtime scripts and the inline <script> blocks of the HTML entries (the
// first code to execute on startup). Run via `npm run audit:compat` after a
// build; tests/compat_audit.test.mjs gates this in CI.
import { existsSync, readFileSync, readdirSync } from 'node:fs';
import { extname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import * as acorn from 'acorn';
import { staticRuntimeScripts, staticRuntimeScriptPrefixes } from '../vite.config.js';

const appRoot = resolve(fileURLToPath(import.meta.url), '../..');
const sourceRoot = join(appRoot, 'src');
const distRoot = join(appRoot, 'dist');

// APIs the startup polyfill (src/shared/legacy-polyfills.js) installs in every
// HTML entry before any other script runs. Once the contract test pins that
// wiring, these two APIs are legal everywhere *except* inside the polyfill
// itself. Everything else in the tables below stays enforced everywhere.
const POLYFILLED_APIS = new Set(['at', 'hasOwn']);
const POLYFILL_SCRIPT = 'shared/legacy-polyfills.js';

// Property accesses requiring newer engines than Safari 14.0.
// Keyed by callee property name; HOST_OBJECT restricts Object./Promise. forms.
// Null-prototype map: a plain object would let MEMBER_API_BASELINE['toString']
// resolve to Object.prototype.toString and phantom-flag every .toString() call.
const MEMBER_API_BASELINE = Object.assign(Object.create(null), {
  at: { note: 'Array/String .at() — Safari 15.4' },
  findLast: { note: 'findLast — Safari 15.4' },
  findLastIndex: { note: 'findLastIndex — Safari 15.4' },
  toSorted: { note: 'toSorted — Safari 15.4' },
  toReversed: { note: 'toReversed — Safari 15.4' },
  toSpliced: { note: 'toSpliced — Safari 15.4' },
  hasOwn: { note: 'Object.hasOwn — Safari 15.4', hostObject: 'Object' },
  groupBy: { note: 'Object/Map.groupBy — Safari 17.4', hostObject: 'Object' },
  withResolvers: { note: 'Promise.withResolvers — Safari 17.4', hostObject: 'Promise' },
  any: { note: 'AbortSignal.any — Safari 17.4', hostObject: 'AbortSignal' },
  timeout: { note: 'AbortSignal.timeout — Safari 15.4', hostObject: 'AbortSignal' },
  randomUUID: { note: 'crypto.randomUUID — Safari 15.4', hostObject: 'crypto' },
});
// Constructor/identifier globals requiring newer engines than Safari 14.0.
const GLOBAL_API_BASELINE = Object.assign(Object.create(null), {
  BroadcastChannel: 'BroadcastChannel — Safari 15.4',
  structuredClone: 'structuredClone — Safari 15.4',
  requestIdleCallback: 'requestIdleCallback — Safari 18.1',
  WeakRef: 'WeakRef — Safari 14.1',
  FinalizationRegistry: 'FinalizationRegistry — Safari 14.1',
});

function lineOfOffset(source, offset) {
  let line = 1;
  let lineStart = 0;
  for (let i = 0; i < offset && i < source.length; i += 1) {
    if (source.charCodeAt(i) === 10) {
      line += 1;
      lineStart = i + 1;
    }
  }
  let lineEnd = source.indexOf('\n', lineStart);
  if (lineEnd === -1) lineEnd = source.length;
  return { line, text: source.slice(lineStart, lineEnd) };
}

// A `safari14-ok` marker on the same line acknowledges a guarded call with a
// runtime fallback (e.g. `typeof structuredClone === 'function'` + JSON
// fallback) and suppresses the report, mirroring eslint-disable practice.
function suppressedLines(code) {
  const lines = new Set();
  const pattern = /\/\/\s*safari14-ok|\/\*\s*safari14-ok\s*\*\//g;
  let match;
  while ((match = pattern.exec(code)) !== null) {
    lines.add(lineOfOffset(code, match.index).line);
  }
  return lines;
}

// Minimal recursive walker over the acorn AST child keys we care about.
const CHILD_KEYS = {
  Program: ['body'],
  ExpressionStatement: ['expression'],
  ChainExpression: ['expression'],
  ParenthesizedExpression: ['expression'],
  CallExpression: ['callee', 'arguments'],
  NewExpression: ['callee', 'arguments'],
  MemberExpression: ['object', 'property'],
  AssignmentExpression: ['left', 'right'],
  BinaryExpression: ['left', 'right'],
  LogicalExpression: ['left', 'right'],
  UnaryExpression: ['argument'],
  UpdateExpression: ['argument'],
  ConditionalExpression: ['test', 'consequent', 'alternate'],
  SpreadElement: ['argument'],
  YieldExpression: ['argument'],
  AwaitExpression: ['argument'],
  TaggedTemplateExpression: ['tag', 'quasi'],
  TemplateLiteral: ['expressions'],
  ObjectExpression: ['properties'],
  Property: ['key', 'value'],
  ArrayExpression: ['elements'],
  VariableDeclarator: ['init'],
  VariableDeclaration: ['declarations'],
  ReturnStatement: ['argument'],
  IfStatement: ['test', 'consequent', 'alternate'],
  ForStatement: ['init', 'test', 'update', 'body'],
  ForInStatement: ['left', 'right', 'body'],
  ForOfStatement: ['left', 'right', 'body'],
  WhileStatement: ['test', 'body'],
  DoWhileStatement: ['test', 'body'],
  BlockStatement: ['body'],
  FunctionDeclaration: ['params', 'body'],
  FunctionExpression: ['params', 'body'],
  ArrowFunctionExpression: ['params', 'body'],
  ClassDeclaration: ['superClass', 'body'],
  ClassExpression: ['superClass', 'body'],
  ClassBody: ['body'],
  MethodDefinition: ['key', 'value'],
  PropertyDefinition: ['key', 'value'],
  StaticBlock: ['body'],
  SwitchStatement: ['discriminant', 'cases'],
  SwitchCase: ['test', 'consequent'],
  TryStatement: ['block', 'handler', 'finalizer'],
  CatchClause: ['param', 'body'],
  ThrowStatement: ['argument'],
  LabeledStatement: ['body'],
  ExportNamedDeclaration: ['declaration'],
  ExportDefaultDeclaration: ['declaration'],
  ExportAllDeclaration: ['source'],
  ImportExpression: ['source'],
  OptionalMemberExpression: ['object', 'property'],
  OptionalCallExpression: ['callee', 'arguments'],
};

function walk(node, visit) {
  if (!node || typeof node.type !== 'string') return;
  visit(node);
  const keys = CHILD_KEYS[node.type] || [];
  for (const key of keys) {
    const child = node[key];
    if (Array.isArray(child)) {
      for (const item of child) walk(item, visit);
    } else {
      walk(child, visit);
    }
  }
}

function propertyName(member) {
  if (!member.computed && member.property && member.property.type === 'Identifier') {
    return member.property.name;
  }
  if (member.computed && member.property && member.property.type === 'Literal') {
    return String(member.property.value);
  }
  return null;
}

function auditSource(label, code, { sourceType = 'module', isPolyfillScript = false } = {}) {
  const violations = [];
  const suppressed = suppressedLines(code);
  let ast;
  try {
    ast = acorn.parse(code, {
      ecmaVersion: 2021,
      sourceType,
      allowHashBang: true,
      locations: false,
    });
  } catch (error) {
    const line = typeof error.pos === 'number' ? lineOfOffset(code, error.pos).line : (error.loc?.line ?? 0);
    violations.push(`${label}:${line}: parse failure at ES2021 (Safari 14 ceiling) — ${error.message}`);
    return violations;
  }
  // Only flag APIs when they are actually *invoked* (callee position): bare
  // property reads like `b.at || null` are plain data fields, not the
  // Array.prototype.at builtin.
  const calleeOf = (node) => {
    const callee = node.callee;
    if (!callee || (node.type !== 'CallExpression' && node.type !== 'NewExpression' && node.type !== 'OptionalCallExpression')) return null;
    if ((callee.type === 'MemberExpression' || callee.type === 'OptionalMemberExpression') && !callee.computed) {
      return { kind: 'member', node: callee };
    }
    if (callee.type === 'Identifier') return { kind: 'global', name: callee.name, node: callee };
    return null;
  };
  walk(ast, (node) => {
    // Minified dist chunks carry no safari14-ok comments, so an empty marker
    // set skips the per-node line lookup — quadratic on multi-MB chunks and
    // the audit's dominant cost (minutes) without this guard.
    if (suppressed.size > 0 && suppressed.has(lineOfOffset(code, node.start).line)) return;
    if (node.type === 'Literal' && node.regex) {
      const { pattern, flags } = node.regex;
      if (/\(\?<[=!]/.test(pattern)) {
        violations.push(`${label}:${lineOfOffset(code, node.start).line}: lookbehind assertion in /${pattern.slice(0, 60)}/ — Safari 16.4`);
      }
      if (/[vd]/.test(flags)) {
        violations.push(`${label}:${lineOfOffset(code, node.start).line}: regex flag "${flags}" — Safari ${flags.includes('v') ? '17' : '15.4'}`);
      }
      return;
    }
    const callee = calleeOf(node);
    if (!callee) return;
    if (callee.kind === 'global') {
      if (Object.hasOwn(GLOBAL_API_BASELINE, callee.name)) {
        violations.push(`${label}:${lineOfOffset(code, node.start).line}: ${callee.name} — ${GLOBAL_API_BASELINE[callee.name]}`);
      }
      return;
    }
    const member = callee.node;
    const name = propertyName(member);
    const spec = name != null ? MEMBER_API_BASELINE[name] : undefined;
    if (!spec) return;
    // The startup polyfill ships at/hasOwn in every entry, so those two APIs
    // are legal everywhere else — but the polyfill itself must not rely on
    // what it installs (bootstrapping circularity).
    if (!isPolyfillScript && POLYFILLED_APIS.has(name)) return;
    if (spec.hostObject) {
      const host = member.object;
      const hostName = host && host.type === 'Identifier' ? host.name : null;
      if (hostName !== spec.hostObject
        && !(spec.hostObject === 'crypto' && host && host.type === 'MemberExpression' && propertyName(host) === 'crypto')) {
        return;
      }
    }
    violations.push(`${label}:${lineOfOffset(code, node.start).line}: .${name}() invocation — ${spec.note}`);
  });
  return violations;
}

function auditHtmlInlineScripts(htmlPath, label) {
  const html = readFileSync(htmlPath, 'utf8');
  const violations = [];
  const pattern = /<script(?![^>]*\bsrc=)[^>]*>([\s\S]*?)<\/script>/gi;
  let match;
  while ((match = pattern.exec(html)) !== null) {
    const typeAttr = /type=["']([^"']+)["']/i.exec(match[0])?.[1] || '';
    const isModule = typeAttr.toLowerCase() === 'module';
    const code = match[1];
    if (!code.trim()) continue;
    violations.push(...auditSource(`${label}#inline${isModule ? ' (module)' : ''}`, code, {
      sourceType: isModule ? 'module' : 'script',
    }));
  }
  return violations;
}

function collectStaticRuntimeScripts() {
  const files = [];
  const visit = (dir) => {
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
      const full = join(dir, entry.name);
      if (entry.isDirectory()) {
        visit(full);
        continue;
      }
      const relative = full.slice(sourceRoot.length + 1).replaceAll('\\', '/');
      if (extname(entry.name).toLowerCase() !== '.js') continue;
      if (staticRuntimeScripts.has(relative) || staticRuntimeScriptPrefixes.some(prefix => relative.startsWith(prefix))) {
        files.push({ relative, full });
      }
    }
  };
  visit(sourceRoot);
  return files;
}

export function runAudit({ distDir = distRoot } = {}) {
  const violations = [];

  for (const { relative, full } of collectStaticRuntimeScripts()) {
    violations.push(...auditSource(`static:${relative}`, readFileSync(full, 'utf8'), {
      sourceType: 'script',
      isPolyfillScript: relative === POLYFILL_SCRIPT,
    }));
  }

  for (const entry of ['index.html', 'pet.html', 'reader.html']) {
    const htmlPath = join(sourceRoot, entry);
    if (existsSync(htmlPath)) {
      violations.push(...auditHtmlInlineScripts(htmlPath, `inline:${entry}`));
    }
  }

  const assetsDir = join(distDir, 'assets');
  if (existsSync(assetsDir)) {
    for (const name of readdirSync(assetsDir)) {
      if (!name.endsWith('.js')) continue;
      violations.push(...auditSource(`dist:${name}`, readFileSync(join(assetsDir, name), 'utf8')));
    }
  }

  return violations;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const violations = runAudit();
  if (!existsSync(join(distRoot, 'assets'))) {
    // Fail closed: the dist layer is the one that catches a marked@16-style
    // parse-time regression, so silently green-lighting without it would
    // defeat the gate. The static/inline layers were still audited above.
    console.error('audit-compat: dist/assets not found — run `npm run build:ui` first');
    process.exitCode = 1;
  }
  if (violations.length) {
    console.error(`audit-compat: ${violations.length} violation(s) against the Safari 14 baseline:`);
    for (const violation of violations) console.error(`  ${violation}`);
    process.exitCode = 1;
  } else {
    console.log('audit-compat: clean against the Safari 14 baseline');
  }
}
