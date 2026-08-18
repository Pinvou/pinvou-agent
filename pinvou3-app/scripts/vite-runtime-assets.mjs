import path, { posix } from 'node:path';

const HTML_WHITESPACE = /[\t\n\f\r ]/u;
const INVALID_ATTRIBUTE_NAME_CHARACTER = /[\t\n\f\r "'<>/=\0]/u;
const INVALID_UNQUOTED_VALUE_CHARACTER = /["'`=<>\0]/u;
const RAW_TEXT_ELEMENTS = new Set([
  'iframe',
  'noembed',
  'noframes',
  'noscript',
  'script',
  'style',
  'textarea',
  'title',
  'xmp',
]);

function findTagEnd(html, start) {
  let quote = null;
  for (let index = start; index < html.length; index += 1) {
    const character = html[index];
    if (quote) {
      if (character === quote) quote = null;
      continue;
    }
    if (character === '"' || character === "'") {
      quote = character;
      continue;
    }
    if (character === '>') return index;
  }
  throw new Error('unterminated script start tag');
}

function parseAttributes(source) {
  const attributes = new Map();
  let index = 0;
  while (index < source.length) {
    const separatorStart = index;
    while (index < source.length && HTML_WHITESPACE.test(source[index])) index += 1;
    if (index === source.length) break;
    if (source[index] === '/') {
      index += 1;
      while (index < source.length && HTML_WHITESPACE.test(source[index])) index += 1;
      if (index !== source.length) throw new Error('invalid script tag terminator');
      break;
    }
    if (index === separatorStart) {
      throw new Error('script attributes must be separated by HTML whitespace');
    }

    const nameStart = index;
    while (index < source.length && !INVALID_ATTRIBUTE_NAME_CHARACTER.test(source[index])) index += 1;
    if (index === nameStart) throw new Error('invalid script attribute name');
    const name = source.slice(nameStart, index).toLowerCase();

    const nameEnd = index;
    while (index < source.length && HTML_WHITESPACE.test(source[index])) index += 1;
    let value = null;
    if (source[index] === '=') {
      index += 1;
      while (index < source.length && HTML_WHITESPACE.test(source[index])) index += 1;
      if (index === source.length) throw new Error(`missing value for script attribute: ${name}`);

      const quote = source[index];
      if (quote === '"' || quote === "'") {
        const valueStart = index + 1;
        index = source.indexOf(quote, valueStart);
        if (index < 0) throw new Error(`unterminated value for script attribute: ${name}`);
        value = source.slice(valueStart, index);
        index += 1;
      } else {
        const valueStart = index;
        while (index < source.length && !HTML_WHITESPACE.test(source[index])) {
          if (INVALID_UNQUOTED_VALUE_CHARACTER.test(source[index])) {
            throw new Error(`invalid unquoted value for script attribute: ${name}`);
          }
          index += 1;
        }
        if (index === valueStart) throw new Error(`missing value for script attribute: ${name}`);
        value = source.slice(valueStart, index);
      }
    } else {
      // Leave separating whitespace for the next iteration. Otherwise a
      // boolean attribute such as `crossorigin src=...` could hide the fact
      // that the following attribute does (or does not) have a separator.
      index = nameEnd;
    }

    if (attributes.has(name)) throw new Error(`duplicate script attribute: ${name}`);
    attributes.set(name, value);
  }
  return attributes;
}

function startTagNameAt(html, index) {
  if (html[index] !== '<' || !/[a-z]/iu.test(html[index + 1] || '')) return null;
  let end = index + 2;
  while (html[end] && !/[\t\n\f\r />\0]/u.test(html[end])) end += 1;
  const boundary = html[end];
  if (boundary !== '>' && boundary !== '/' && !HTML_WHITESPACE.test(boundary || '')) return null;
  return html.slice(index + 1, end).toLowerCase();
}

function findClosingRawTextEnd(html, start, tagName) {
  const lower = html.toLowerCase();
  let index = start;
  const closingTag = `</${tagName}`;
  while ((index = lower.indexOf(closingTag, index)) >= 0) {
    const boundary = html[index + closingTag.length];
    if (boundary === '>' || HTML_WHITESPACE.test(boundary || '')) {
      return findTagEnd(html, index + closingTag.length);
    }
    index += closingTag.length;
  }
  throw new Error(`${tagName} element is missing its closing tag`);
}

function scriptAttributes(html) {
  const source = String(html || '');
  const results = [];
  let cursor = 0;
  while (cursor < source.length) {
    const tagStart = source.indexOf('<', cursor);
    if (tagStart < 0) break;
    if (source.startsWith('<!--', tagStart)) {
      const commentEnd = source.indexOf('-->', tagStart + 4);
      if (commentEnd < 0) throw new Error('unterminated HTML comment');
      cursor = commentEnd + 3;
      continue;
    }
    const tagName = startTagNameAt(source, tagStart);
    if (!tagName) {
      cursor = tagStart + 1;
      continue;
    }

    const attributesStart = tagStart + tagName.length + 1;
    const tagEnd = findTagEnd(source, attributesStart);
    if (tagName === 'script') {
      results.push(parseAttributes(source.slice(attributesStart, tagEnd)));
    }
    if (tagName === 'plaintext') {
      cursor = source.length;
    } else if (RAW_TEXT_ELEMENTS.has(tagName)) {
      cursor = findClosingRawTextEnd(source, tagEnd + 1, tagName) + 1;
    } else {
      cursor = tagEnd + 1;
    }
  }
  return results;
}

export function localClassicScriptPaths(html) {
  const scripts = [];
  for (const attributes of scriptAttributes(html)) {
    if (!attributes.has('src')) continue;
    const src = attributes.get('src');
    if (typeof src !== 'string' || !src.trim()) {
      throw new Error('script src must be a non-empty string');
    }
    const normalizedSrc = src.trim();
    if (normalizedSrc.includes('\\')) throw new Error(`invalid local classic runtime script path: ${normalizedSrc}`);
    const type = attributes.get('type')?.trim().toLowerCase();
    if (type === 'module') continue;
    if (/^(?:[a-z][a-z0-9+.-]*:|\/\/)/iu.test(normalizedSrc)) continue;

    const withoutBase = normalizedSrc.replace(/^%BASE_URL%/u, '').replace(/^\/+/, '');
    const relative = posix.normalize(withoutBase.split(/[?#]/u, 1)[0]);
    if (!relative || relative === '.' || relative === '..' || relative.startsWith('../') || relative.includes('%')) {
      throw new Error(`invalid local classic runtime script path: ${normalizedSrc}`);
    }
    scripts.push(relative);
  }
  return [...new Set(scripts)];
}

export function resolveContainedRuntimePath(root, relative) {
  if (typeof relative !== 'string' || !relative || relative.includes('\\')) {
    throw new Error(`invalid runtime script path: ${String(relative)}`);
  }
  const resolvedRoot = path.resolve(root);
  const candidate = path.resolve(resolvedRoot, relative);
  const fromRoot = path.relative(resolvedRoot, candidate);
  if (!fromRoot || fromRoot === '..' || fromRoot.startsWith(`..${path.sep}`) || path.isAbsolute(fromRoot)) {
    throw new Error(`runtime script path escapes its root: ${relative}`);
  }
  return candidate;
}
