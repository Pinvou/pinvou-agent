import hljs from 'highlight.js/lib/core';
import accesslog from 'highlight.js/lib/languages/accesslog';
import apache from 'highlight.js/lib/languages/apache';
import awk from 'highlight.js/lib/languages/awk';
import bash from 'highlight.js/lib/languages/bash';
import c from 'highlight.js/lib/languages/c';
import clojure from 'highlight.js/lib/languages/clojure';
import cmake from 'highlight.js/lib/languages/cmake';
import coffeescript from 'highlight.js/lib/languages/coffeescript';
import cpp from 'highlight.js/lib/languages/cpp';
import csharp from 'highlight.js/lib/languages/csharp';
import css from 'highlight.js/lib/languages/css';
import dart from 'highlight.js/lib/languages/dart';
import diff from 'highlight.js/lib/languages/diff';
import dns from 'highlight.js/lib/languages/dns';
import dockerfile from 'highlight.js/lib/languages/dockerfile';
import dos from 'highlight.js/lib/languages/dos';
import elixir from 'highlight.js/lib/languages/elixir';
import erlang from 'highlight.js/lib/languages/erlang';
import fsharp from 'highlight.js/lib/languages/fsharp';
import go from 'highlight.js/lib/languages/go';
import gradle from 'highlight.js/lib/languages/gradle';
import graphql from 'highlight.js/lib/languages/graphql';
import groovy from 'highlight.js/lib/languages/groovy';
import handlebars from 'highlight.js/lib/languages/handlebars';
import haskell from 'highlight.js/lib/languages/haskell';
import http from 'highlight.js/lib/languages/http';
import ini from 'highlight.js/lib/languages/ini';
import java from 'highlight.js/lib/languages/java';
import javascript from 'highlight.js/lib/languages/javascript';
import json from 'highlight.js/lib/languages/json';
import julia from 'highlight.js/lib/languages/julia';
import kotlin from 'highlight.js/lib/languages/kotlin';
import latex from 'highlight.js/lib/languages/latex';
import less from 'highlight.js/lib/languages/less';
import lisp from 'highlight.js/lib/languages/lisp';
import lua from 'highlight.js/lib/languages/lua';
import makefile from 'highlight.js/lib/languages/makefile';
import markdown from 'highlight.js/lib/languages/markdown';
import matlab from 'highlight.js/lib/languages/matlab';
import mipsasm from 'highlight.js/lib/languages/mipsasm';
import nginx from 'highlight.js/lib/languages/nginx';
import nim from 'highlight.js/lib/languages/nim';
import objectivec from 'highlight.js/lib/languages/objectivec';
import ocaml from 'highlight.js/lib/languages/ocaml';
import perl from 'highlight.js/lib/languages/perl';
import pgsql from 'highlight.js/lib/languages/pgsql';
import php from 'highlight.js/lib/languages/php';
import plaintext from 'highlight.js/lib/languages/plaintext';
import powershell from 'highlight.js/lib/languages/powershell';
import profile from 'highlight.js/lib/languages/profile';
import properties from 'highlight.js/lib/languages/properties';
import protobuf from 'highlight.js/lib/languages/protobuf';
import python from 'highlight.js/lib/languages/python';
import q from 'highlight.js/lib/languages/q';
import qml from 'highlight.js/lib/languages/qml';
import r from 'highlight.js/lib/languages/r';
import reasonml from 'highlight.js/lib/languages/reasonml';
import ruby from 'highlight.js/lib/languages/ruby';
import rust from 'highlight.js/lib/languages/rust';
import sas from 'highlight.js/lib/languages/sas';
import scala from 'highlight.js/lib/languages/scala';
import scheme from 'highlight.js/lib/languages/scheme';
import scss from 'highlight.js/lib/languages/scss';
import shell from 'highlight.js/lib/languages/shell';
import sql from 'highlight.js/lib/languages/sql';
import stata from 'highlight.js/lib/languages/stata';
import swift from 'highlight.js/lib/languages/swift';
import tcl from 'highlight.js/lib/languages/tcl';
import typescript from 'highlight.js/lib/languages/typescript';
import vbnet from 'highlight.js/lib/languages/vbnet';
import wasm from 'highlight.js/lib/languages/wasm';
import x86asm from 'highlight.js/lib/languages/x86asm';
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

const LANGUAGE_DEFINITIONS = [
  ['accesslog', accesslog], ['apache', apache], ['awk', awk], ['bash', bash],
  ['c', c], ['clojure', clojure], ['cmake', cmake], ['coffeescript', coffeescript],
  ['cpp', cpp], ['csharp', csharp], ['css', css], ['dart', dart], ['diff', diff],
  ['dns', dns], ['dockerfile', dockerfile], ['dos', dos], ['elixir', elixir],
  ['erlang', erlang], ['fsharp', fsharp], ['go', go], ['gradle', gradle],
  ['graphql', graphql], ['groovy', groovy], ['handlebars', handlebars],
  ['haskell', haskell], ['http', http], ['ini', ini], ['java', java],
  ['javascript', javascript], ['json', json], ['julia', julia], ['kotlin', kotlin],
  ['latex', latex], ['less', less], ['lisp', lisp], ['lua', lua],
  ['makefile', makefile], ['markdown', markdown], ['matlab', matlab],
  ['mipsasm', mipsasm], ['nginx', nginx], ['nim', nim], ['objectivec', objectivec],
  ['ocaml', ocaml], ['perl', perl], ['pgsql', pgsql], ['php', php],
  ['plaintext', plaintext], ['powershell', powershell], ['profile', profile],
  ['properties', properties], ['protobuf', protobuf], ['python', python], ['q', q],
  ['qml', qml], ['r', r], ['reasonml', reasonml], ['ruby', ruby], ['rust', rust],
  ['sas', sas], ['scala', scala], ['scheme', scheme], ['scss', scss],
  ['shell', shell], ['sql', sql], ['stata', stata], ['swift', swift], ['tcl', tcl],
  ['typescript', typescript], ['vbnet', vbnet], ['wasm', wasm],
  ['x86asm', x86asm], ['xml', xml], ['yaml', yaml],
  ['log', genericLog],
];

for (const [name, definition] of LANGUAGE_DEFINITIONS) {
  hljs.registerLanguage(name, definition);
}

const EXPLICIT_ALIASES = {
  javascript: ['js', 'jsx', 'mjs', 'cjs'],
  typescript: ['ts', 'tsx', 'mts', 'cts'],
  python: ['py', 'py3'],
  csharp: ['cs', 'c#'],
  cpp: ['c++', 'cc', 'cxx', 'hpp'],
  objectivec: ['objc', 'objective-c'],
  fsharp: ['fs', 'f#'],
  bash: ['sh', 'zsh'],
  powershell: ['ps1', 'pwsh'],
  dos: ['bat', 'cmd'],
  yaml: ['yml'],
  markdown: ['md', 'mdown'],
  protobuf: ['proto'],
  dockerfile: ['docker'],
  makefile: ['make'],
  x86asm: ['asm', 'assembly'],
  xml: ['html', 'xhtml', 'svg', 'vue', 'svelte'],
  plaintext: ['text', 'txt', 'plain'],
  pgsql: ['postgres', 'postgresql'],
  properties: ['props'],
  handlebars: ['hbs'],
};

for (const [languageName, aliases] of Object.entries(EXPLICIT_ALIASES)) {
  hljs.registerAliases(aliases, { languageName });
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

export const supportedSyntaxLanguages = Object.freeze(
  LANGUAGE_DEFINITIONS.map(([name]) => name),
);

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
  if (!language) return token;
  const canonical = LANGUAGE_DEFINITIONS.find(([name]) => hljs.getLanguage(name) === language);
  return canonical ? canonical[0] : token;
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
// token 类名与 hljs diff 语言一致（hljs-meta / hljs-addition / hljs-deletion / hljs-comment），
// 视觉与既有配色零差异。
export function highlightDiffCode(code) {
  const lines = String(code || '').split('\n');
  let html = '';
  for (const line of lines) {
    const escaped = escapeCodeHtml(line);
    if (line.startsWith('diff --git') || line.startsWith('index ') || line.startsWith('@@')) {
      html += `<span class="hljs-meta">${escaped}</span>\n`;
    } else if (line.startsWith('+')) {
      html += `<span class="hljs-addition">${escaped}</span>\n`;
    } else if (line.startsWith('-')) {
      html += `<span class="hljs-deletion">${escaped}</span>\n`;
    } else if (line.startsWith('\\')) {
      html += `<span class="hljs-comment">${escaped}</span>\n`;
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
