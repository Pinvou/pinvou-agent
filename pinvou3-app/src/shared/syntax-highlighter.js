import hljs from 'highlight.js/lib/core';
import bash from 'highlight.js/lib/languages/bash';
import c from 'highlight.js/lib/languages/c';
import cpp from 'highlight.js/lib/languages/cpp';
import csharp from 'highlight.js/lib/languages/csharp';
import css from 'highlight.js/lib/languages/css';
import diff from 'highlight.js/lib/languages/diff';
import dockerfile from 'highlight.js/lib/languages/dockerfile';
import go from 'highlight.js/lib/languages/go';
import http from 'highlight.js/lib/languages/http';
import ini from 'highlight.js/lib/languages/ini';
import java from 'highlight.js/lib/languages/java';
import javascript from 'highlight.js/lib/languages/javascript';
import json from 'highlight.js/lib/languages/json';
import markdown from 'highlight.js/lib/languages/markdown';
import nginx from 'highlight.js/lib/languages/nginx';
import php from 'highlight.js/lib/languages/php';
import plaintext from 'highlight.js/lib/languages/plaintext';
import powershell from 'highlight.js/lib/languages/powershell';
import python from 'highlight.js/lib/languages/python';
import ruby from 'highlight.js/lib/languages/ruby';
import rust from 'highlight.js/lib/languages/rust';
import shell from 'highlight.js/lib/languages/shell';
import sql from 'highlight.js/lib/languages/sql';
import typescript from 'highlight.js/lib/languages/typescript';
import xml from 'highlight.js/lib/languages/xml';
import yaml from 'highlight.js/lib/languages/yaml';

function genericLog(hljsApi) {
  const httpMethod = {
    scope: 'keyword',
    begin: /\b(?:GET|HEAD|POST|PUT|PATCH|DELETE|OPTIONS|CONNECT|TRACE)\b/u,
  };
  const requestPath = {
    scope: 'string',
    begin: /\/[A-Za-z0-9._~!$&'()*+,;=:@%/?#-]*/u,
  };
  const errorStatus = { scope: 'deletion', begin: /\b[45]\d{2}\b/u };
  const normalStatus = { scope: 'literal', begin: /\b[123]\d{2}\b/u };

  return {
    name: 'Log',
    aliases: ['logs'],
    case_insensitive: true,
    contains: [
      {
        scope: 'meta',
        begin: /\b\d{4}-\d{2}-\d{2}(?:[T ][0-2]\d:[0-5]\d:[0-5]\d(?:[.,]\d+)?(?:Z|[+-]\d{2}:?\d{2})?)?/u,
      },
      { scope: 'meta', begin: /\[[^\]\r\n]*(?:\d{2}:){2}\d{2}[^\]\r\n]*\]/u },
      { scope: 'deletion', begin: /\b(?:FATAL|CRITICAL|ERROR|ERR)\b/u },
      { scope: 'warning', begin: /\b(?:WARN|WARNING)\b/u },
      { scope: 'built_in', begin: /\b(?:TRACE|DEBUG|INFO|NOTICE)\b/u },
      errorStatus,
      normalStatus,
      httpMethod,
      { scope: 'attr', begin: /\b[A-Za-z_][\w.-]*(?==)/u },
      requestPath,
      {
        scope: 'string',
        begin: /"/u,
        end: /"/u,
        contains: [httpMethod, requestPath, errorStatus, normalStatus],
      },
      hljsApi.APOS_STRING_MODE,
      hljsApi.NUMBER_MODE,
    ],
  };
}

// 别名表是反查表(LAZY_ALIAS_TO_CANONICAL)与注册后别名注册(registerAliasesFor)
// 的单一真相源:核心语言的常用别名 + 懒语言的 hljs 内建别名(注册前围栏写这些
// 别名也必须能触发懒加载,否则永久回落纯文本)。内建别名清单来自 highlight.js
// 11.11.1 各语言模块自带 aliases(注册后回读核验)。
const EXPLICIT_ALIASES = {
  javascript: ['js', 'jsx', 'mjs', 'cjs'],
  typescript: ['ts', 'tsx', 'mts', 'cts'],
  python: ['py', 'py3'],
  csharp: ['cs', 'c#'],
  cpp: ['c++', 'cc', 'cxx', 'hpp'],
  objectivec: ['objc', 'objective-c', 'mm', 'obj-c', 'obj-c++', 'objective-c++'],
  fsharp: ['fs', 'f#'],
  bash: ['sh', 'zsh'],
  powershell: ['ps1', 'pwsh'],
  dos: ['bat', 'cmd'],
  yaml: ['yml'],
  markdown: ['md', 'mdown'],
  protobuf: ['proto'],
  dockerfile: ['docker'],
  makefile: ['make', 'mk', 'mak'],
  x86asm: ['asm', 'assembly'],
  xml: ['html', 'xhtml', 'svg', 'vue', 'svelte'],
  plaintext: ['text', 'txt', 'plain'],
  pgsql: ['postgres', 'postgresql'],
  properties: ['props'],
  handlebars: ['hbs', 'html.hbs', 'html.handlebars', 'htmlbars'],
  apache: ['apacheconf'],
  clojure: ['clj', 'edn'],
  cmake: ['cmake.in'],
  coffeescript: ['coffee', 'cson', 'iced'],
  dns: ['bind', 'zone'],
  elixir: ['ex', 'exs'],
  erlang: ['erl'],
  graphql: ['gql'],
  haskell: ['hs'],
  kotlin: ['kt', 'kts'],
  latex: ['tex'],
  lua: ['pluto'],
  mipsasm: ['mips'],
  ocaml: ['ml'],
  perl: ['pl', 'pm'],
  q: ['k', 'kdb'],
  qml: ['qt'],
  reasonml: ['re'],
  scheme: ['scm'],
  stata: ['do', 'ado'],
  tcl: ['tk'],
  vbnet: ['vb'],
};
const LANGUAGE_DEFINITIONS = [
  ['bash', bash], ['c', c], ['cpp', cpp], ['csharp', csharp], ['css', css], ['diff', diff], ['dockerfile', dockerfile], ['go', go], ['http', http], ['ini', ini], ['java', java], ['javascript', javascript], ['json', json], ['markdown', markdown], ['nginx', nginx], ['php', php], ['plaintext', plaintext], ['powershell', powershell], ['python', python], ['ruby', ruby], ['rust', rust], ['shell', shell], ['sql', sql], ['typescript', typescript], ['xml', xml], ['yaml', yaml],
  ['log', genericLog],
];

// 两级注册:启动只同步注册核心集(自动检测 18 语 + 聊天高频),其余 48 语
// 首次被显式 ```lang 围栏用到时动态 import 注册——聊天主路径不再为低频
// 语言付出 300KB 级的同步解析成本。normalizeSyntaxLanguage 对未注册语言
// 返回原始 token(不识别),注册后自然恢复高亮;期间首渲染回落纯文本,
// 语言标签(label)始终保留原提示。
const LAZY_LANGUAGE_LOADERS = {
  'accesslog': () => import('highlight.js/lib/languages/accesslog').then(m => m.default), 'apache': () => import('highlight.js/lib/languages/apache').then(m => m.default), 'awk': () => import('highlight.js/lib/languages/awk').then(m => m.default), 'clojure': () => import('highlight.js/lib/languages/clojure').then(m => m.default), 'cmake': () => import('highlight.js/lib/languages/cmake').then(m => m.default), 'coffeescript': () => import('highlight.js/lib/languages/coffeescript').then(m => m.default), 'dart': () => import('highlight.js/lib/languages/dart').then(m => m.default), 'dns': () => import('highlight.js/lib/languages/dns').then(m => m.default), 'dos': () => import('highlight.js/lib/languages/dos').then(m => m.default), 'elixir': () => import('highlight.js/lib/languages/elixir').then(m => m.default), 'erlang': () => import('highlight.js/lib/languages/erlang').then(m => m.default), 'fsharp': () => import('highlight.js/lib/languages/fsharp').then(m => m.default), 'gradle': () => import('highlight.js/lib/languages/gradle').then(m => m.default), 'graphql': () => import('highlight.js/lib/languages/graphql').then(m => m.default), 'groovy': () => import('highlight.js/lib/languages/groovy').then(m => m.default), 'handlebars': () => import('highlight.js/lib/languages/handlebars').then(m => m.default), 'haskell': () => import('highlight.js/lib/languages/haskell').then(m => m.default), 'julia': () => import('highlight.js/lib/languages/julia').then(m => m.default), 'kotlin': () => import('highlight.js/lib/languages/kotlin').then(m => m.default), 'latex': () => import('highlight.js/lib/languages/latex').then(m => m.default), 'less': () => import('highlight.js/lib/languages/less').then(m => m.default), 'lisp': () => import('highlight.js/lib/languages/lisp').then(m => m.default), 'lua': () => import('highlight.js/lib/languages/lua').then(m => m.default), 'makefile': () => import('highlight.js/lib/languages/makefile').then(m => m.default), 'matlab': () => import('highlight.js/lib/languages/matlab').then(m => m.default), 'mipsasm': () => import('highlight.js/lib/languages/mipsasm').then(m => m.default), 'nim': () => import('highlight.js/lib/languages/nim').then(m => m.default), 'objectivec': () => import('highlight.js/lib/languages/objectivec').then(m => m.default), 'ocaml': () => import('highlight.js/lib/languages/ocaml').then(m => m.default), 'perl': () => import('highlight.js/lib/languages/perl').then(m => m.default), 'pgsql': () => import('highlight.js/lib/languages/pgsql').then(m => m.default), 'profile': () => import('highlight.js/lib/languages/profile').then(m => m.default), 'properties': () => import('highlight.js/lib/languages/properties').then(m => m.default), 'protobuf': () => import('highlight.js/lib/languages/protobuf').then(m => m.default), 'q': () => import('highlight.js/lib/languages/q').then(m => m.default), 'qml': () => import('highlight.js/lib/languages/qml').then(m => m.default), 'r': () => import('highlight.js/lib/languages/r').then(m => m.default), 'reasonml': () => import('highlight.js/lib/languages/reasonml').then(m => m.default), 'sas': () => import('highlight.js/lib/languages/sas').then(m => m.default), 'scala': () => import('highlight.js/lib/languages/scala').then(m => m.default), 'scheme': () => import('highlight.js/lib/languages/scheme').then(m => m.default), 'scss': () => import('highlight.js/lib/languages/scss').then(m => m.default), 'stata': () => import('highlight.js/lib/languages/stata').then(m => m.default), 'swift': () => import('highlight.js/lib/languages/swift').then(m => m.default), 'tcl': () => import('highlight.js/lib/languages/tcl').then(m => m.default), 'vbnet': () => import('highlight.js/lib/languages/vbnet').then(m => m.default), 'wasm': () => import('highlight.js/lib/languages/wasm').then(m => m.default), 'x86asm': () => import('highlight.js/lib/languages/x86asm').then(m => m.default),
};

// 懒语言别名反查表:懒语言的别名(proto/bat/objc 等)在注册完成前不存在于
// hljs,围栏写别名时必须先把 token 解析回 canonical 才能命中
// LAZY_LANGUAGE_LOADERS,否则别名围栏永远不会触发加载、永久回落纯文本。
const LAZY_ALIAS_TO_CANONICAL = {};
for (const [canonical, aliases] of Object.entries(EXPLICIT_ALIASES)) {
  if (!Object.hasOwn(LAZY_LANGUAGE_LOADERS, canonical)) continue;
  for (const alias of aliases) LAZY_ALIAS_TO_CANONICAL[alias] = canonical;
}

// 懒语言注册完成的版本号与订阅:静态消费方(renderMarkdown/highlightCode 的
// useMemo)把版本号纳入 deps,注册完成后重算,让已渲染内容恢复高亮。
let syntaxHighlightVersion = 0;
const syntaxHighlightListeners = new Set();

export function getSyntaxHighlightVersion() {
  return syntaxHighlightVersion;
}

export function subscribeSyntaxHighlight(listener) {
  syntaxHighlightListeners.add(listener);
  return () => syntaxHighlightListeners.delete(listener);
}

function notifySyntaxHighlightReady() {
  syntaxHighlightVersion += 1;
  for (const listener of syntaxHighlightListeners) listener();
}

const lazyPending = new Map();
function ensureLazyLanguage(name) {
  // Object.hasOwn 防止 constructor/__proto__ 等与原型链同名的 token 命中
  // 原型属性,把 Object()/Object.prototype 当 loader 调用而抛 TypeError。
  if (Object.hasOwn(LAZY_LANGUAGE_LOADERS, name)) {
    let pending = lazyPending.get(name);
    if (!pending) {
      pending = LAZY_LANGUAGE_LOADERS[name]()
        .then(definition => {
          hljs.registerLanguage(name, definition);
          registerAliasesFor(name);
          notifySyntaxHighlightReady();
        })
        // 懒 chunk 加载失败时吞掉 rejection 避免 unhandled rejection;
        // finally 已清 lazyPending,下次围栏触发会重新加载。
        .catch(() => {})
        .finally(() => lazyPending.delete(name));
      lazyPending.set(name, pending);
    }
    return pending;
  }
  return null;
}

function registerAliasesFor(name) {
  const aliases = EXPLICIT_ALIASES[name];
  if (aliases) hljs.registerAliases(aliases, { languageName: name });
}

for (const [name, definition] of LANGUAGE_DEFINITIONS) {
  hljs.registerLanguage(name, definition);
  registerAliasesFor(name);
}




const DISPLAY_NAMES = {
  accesslog: 'Access Log', apache: 'Apache', bash: 'Shell', c: 'C', cpp: 'C++',
  csharp: 'C#', cmake: 'CMake', coffeescript: 'CoffeeScript', css: 'CSS',
  diff: 'Diff', dockerfile: 'Dockerfile', dos: 'Batch', fsharp: 'F#', go: 'Go',
  graphql: 'GraphQL', html: 'HTML', http: 'HTTP', ini: 'INI / TOML',
  java: 'Java', javascript: 'JavaScript', json: 'JSON', kotlin: 'Kotlin',
  latex: 'LaTeX', less: 'Less', makefile: 'Makefile', markdown: 'Markdown',
  mipsasm: 'MIPS Assembly', nginx: 'Nginx', objectivec: 'Objective-C',
  pgsql: 'PostgreSQL', php: 'PHP', plaintext: 'Text', powershell: 'PowerShell',
  protobuf: 'Protocol Buffers', python: 'Python', qml: 'QML', r: 'R',
  reasonml: 'ReasonML', ruby: 'Ruby', rust: 'Rust', scss: 'SCSS', sql: 'SQL',
  typescript: 'TypeScript', vbnet: 'VB.NET', wasm: 'WebAssembly',
  x86asm: 'x86 Assembly', xml: 'HTML / XML', yaml: 'YAML',
  log: 'Log',
};

const AUTO_DETECT_LANGUAGES = [
  'javascript', 'typescript', 'python', 'java', 'c', 'cpp', 'csharp', 'go', 'rust',
  'bash', 'powershell', 'sql', 'json', 'yaml', 'xml', 'css', 'php', 'ruby',
];

export const MAX_HIGHLIGHT_SOURCE_BYTES = 128 * 1024;
const MAX_CACHED_SOURCE_BYTES = 64 * 1024;
const MAX_HIGHLIGHT_CACHE_ENTRIES = 24;
const highlightCache = new Map();

export const supportedSyntaxLanguages = Object.freeze([
  ...LANGUAGE_DEFINITIONS.map(([name]) => name),
  ...Object.keys(LAZY_LANGUAGE_LOADERS),
]);

export function escapeCodeHtml(value) {
  return String(value).replace(/[&<>"']/g, character => ({
    '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;',
  })[character]);
}

function utf8ByteLength(value) {
  if (value.length > MAX_HIGHLIGHT_SOURCE_BYTES) return value.length;
  return new TextEncoder().encode(value).byteLength;
}

function plainTextResult(source, originalHint, explicitLanguage, extra = {}) {
  return {
    html: escapeCodeHtml(source),
    language: 'plaintext',
    languageId: 'plaintext',
    label: explicitLanguage
      ? displayName(explicitLanguage, originalHint)
      : (originalHint ? originalHint.toUpperCase() : 'Text'),
    highlighted: false,
    ...extra,
  };
}

function cachedHighlight(key) {
  const value = highlightCache.get(key);
  if (!value) return null;
  highlightCache.delete(key);
  highlightCache.set(key, value);
  return value;
}

function rememberHighlight(key, value, sourceBytes) {
  if (sourceBytes > MAX_CACHED_SOURCE_BYTES) return value;
  highlightCache.set(key, value);
  while (highlightCache.size > MAX_HIGHLIGHT_CACHE_ENTRIES) {
    highlightCache.delete(highlightCache.keys().next().value);
  }
  return value;
}

function languageToken(languageHint) {
  return String(languageHint || '')
    .trim()
    .split(/\s+/u, 1)[0]
    .replace(/^\{?\.?/u, '')
    .replace(/\}?$/u, '')
    .toLowerCase();
}

export function normalizeSyntaxLanguage(languageHint) {
  const token = languageToken(languageHint);
  if (!token) return '';
  const language = hljs.getLanguage(token);
  if (!language) {
    // 低频语言未注册:别名先解析回 canonical 再触发后台注册,本次返回原
    // token(调用方按未识别回落纯文本)。查找一律 Object.hasOwn,理由同上。
    const canonical = Object.hasOwn(LAZY_ALIAS_TO_CANONICAL, token)
      ? LAZY_ALIAS_TO_CANONICAL[token]
      : token;
    if (Object.hasOwn(LAZY_LANGUAGE_LOADERS, canonical)) ensureLazyLanguage(canonical);
    return token;
  }
  // 注册后把别名归一到 canonical(覆盖核心集与懒加载集),保证 label 稳定。
  const canonical = supportedSyntaxLanguages.find(name => hljs.getLanguage(name) === language);
  return canonical || token;
}

function displayName(language, originalHint) {
  const original = languageToken(originalHint);
  if (original === 'vue') return 'Vue';
  if (original === 'svelte') return 'Svelte';
  if (original === 'tsx') return 'TSX';
  if (original === 'jsx') return 'JSX';
  if (original === 'html' || original === 'xhtml') return 'HTML';
  if (original === 'svg') return 'SVG';
  if (original === 'toml') return 'TOML';
  return DISPLAY_NAMES[language] || language.replace(/(^|[-_])([a-z])/gu, (_, separator, letter) => `${separator ? ' ' : ''}${letter.toUpperCase()}`);
}

// diff 视图专用快速着色：diff 文法是行级的（+/−/@@/头），逐行按前缀着色即可，
// 避免 hljs 全量词法扫描（实测 82KB 文本 16.5ms → 2.3ms），且不再受
// MAX_HIGHLIGHT_SOURCE_BYTES 回落限制（500KB 约 5ms，后端 1MB 截断内均可处理）。
// 类名分配与主流 diff 工具一致：@@ → hljs-meta；diff --git / index / \ No newline
// → hljs-comment；--- / +++ 文件头 → hljs-diff-file-header old/new（红/绿文字、
// 无背景块，提示加减侧，对齐 GitHub/VS Code）；+ → hljs-addition；- → hljs-deletion。
export function highlightDiffCode(code) {
  const lines = String(code || '').split('\n');
  let html = '';
  for (const line of lines) {
    const escaped = escapeCodeHtml(line);
    if (line.startsWith('@@')) {
      html += `<span class="hljs-meta">${escaped}</span>\n`;
    } else if (line.startsWith('---')) {
      html += `<span class="hljs-diff-file-header old">${escaped}</span>\n`;
    } else if (line.startsWith('+++')) {
      html += `<span class="hljs-diff-file-header new">${escaped}</span>\n`;
    } else if (
      line.startsWith('diff --git')
      || line.startsWith('index ')
      || line.startsWith('\\')
    ) {
      html += `<span class="hljs-comment">${escaped}</span>\n`;
    } else if (line.startsWith('+')) {
      html += `<span class="hljs-addition">${escaped}</span>\n`;
    } else if (line.startsWith('-')) {
      html += `<span class="hljs-deletion">${escaped}</span>\n`;
    } else {
      html += `${escaped}\n`;
    }
  }
  return {
    html,
    language: 'diff',
    languageId: 'diff',
    label: 'Diff',
    highlighted: true,
  };
}

export function highlightCode(code, languageHint, options = {}) {
  const source = String(code || '');
  const originalHint = languageToken(languageHint);
  const normalized = normalizeSyntaxLanguage(originalHint);
  const explicitLanguage = originalHint && hljs.getLanguage(originalHint) ? normalized : '';
  const sourceBytes = utf8ByteLength(source);

  if (sourceBytes > MAX_HIGHLIGHT_SOURCE_BYTES) {
    return plainTextResult(source, originalHint, explicitLanguage, { oversized: true });
  }

  if (options.allowHighlight === false) {
    return plainTextResult(source, originalHint, explicitLanguage, { deferred: true });
  }

  try {
    if (explicitLanguage) {
      const cacheKey = `explicit\u0000${explicitLanguage}\u0000${source}`;
      const cached = cachedHighlight(cacheKey);
      if (cached) return cached;
      const result = hljs.highlight(source, { language: explicitLanguage, ignoreIllegals: true });
      return rememberHighlight(cacheKey, {
        html: result.value,
        language: explicitLanguage,
        languageId: originalHint || explicitLanguage,
        label: displayName(explicitLanguage, originalHint),
        highlighted: true,
      }, sourceBytes);
    }

    const allowAutoDetect = options.allowAutoDetect !== false;
    if (!originalHint && allowAutoDetect && source.length >= 20 && source.length <= 50_000) {
      const cacheKey = `auto\u0000${source}`;
      const cached = cachedHighlight(cacheKey);
      if (cached) return cached;
      const result = hljs.highlightAuto(source, AUTO_DETECT_LANGUAGES);
      if (result.language && result.relevance >= 3) {
        return rememberHighlight(cacheKey, {
          html: result.value,
          language: result.language,
          languageId: result.language,
          label: displayName(result.language, ''),
          highlighted: true,
        }, sourceBytes);
      }
    }
  } catch (_) {
    // Invalid or partially streamed code must remain readable as plain text.
  }

  return plainTextResult(source, originalHint, explicitLanguage);
}
