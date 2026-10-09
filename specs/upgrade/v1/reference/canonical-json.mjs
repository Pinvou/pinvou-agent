import { ContractError, requireCondition } from './errors.mjs';

export const LIMITS = Object.freeze({
  metadataBytes: 1024 * 1024,
  credentialBytes: 64 * 1024,
  depth: 32,
  stringBytes: 64 * 1024,
  members: 4096,
});

const encoder = new TextEncoder();
const decoder = new TextDecoder('utf-8', { fatal: true, ignoreBOM: true });

function validateString(value) {
  requireCondition(encoder.encode(value).length <= LIMITS.stringBytes, 'JSON_STRING_LIMIT');
  for (let i = 0; i < value.length; i++) {
    const unit = value.charCodeAt(i);
    if (unit >= 0xd800 && unit <= 0xdbff) {
      const next = value.charCodeAt(++i);
      requireCondition(next >= 0xdc00 && next <= 0xdfff, 'JSON_INVALID_UNICODE');
    } else {
      requireCondition(unit < 0xdc00 || unit > 0xdfff, 'JSON_INVALID_UNICODE');
    }
  }
}

/** Parse before trusting JSON.parse: duplicate keys and invalid Unicode are fatal.
 * A bounded recursive descent also rejects oversized containers before building
 * the full tree. Objects have null prototypes, including a literal __proto__ key.
 */
export function parseJson(bytes, maximumBytes = LIMITS.metadataBytes) {
  requireCondition(bytes instanceof Uint8Array, 'JSON_BYTES_REQUIRED');
  requireCondition(Number.isSafeInteger(maximumBytes) && maximumBytes > 0
    && maximumBytes <= LIMITS.metadataBytes, 'JSON_LIMIT_INVALID');
  requireCondition(bytes.byteLength <= maximumBytes, 'JSON_INPUT_LIMIT');
  let input;
  try {
    input = decoder.decode(bytes);
  } catch {
    throw new ContractError('JSON_INVALID_UTF8');
  }
  let cursor = 0;
  const fail = () => { throw new ContractError('JSON_INVALID'); };
  function whitespace() {
    while (cursor < input.length && /[\x20\x09\x0a\x0d]/u.test(input[cursor])) cursor++;
  }
  function string() {
    if (input[cursor] !== '"') fail();
    const start = cursor++;
    while (cursor < input.length) {
      const character = input[cursor++];
      if (character === '"') {
        let result;
        try { result = JSON.parse(input.slice(start, cursor)); } catch { fail(); }
        validateString(result);
        return result;
      }
      if (character === '\\') cursor++;
      else if (character.charCodeAt(0) < 0x20) fail();
    }
    fail();
  }
  function value(depth) {
    requireCondition(depth <= LIMITS.depth, 'JSON_DEPTH_LIMIT');
    whitespace();
    const character = input[cursor];
    if (character === '"') return string();
    if (character === '{' || character === '[') {
      const object = character === '{';
      const result = object ? Object.create(null) : [];
      const keys = new Set();
      const closing = object ? '}' : ']';
      cursor++;
      whitespace();
      if (input[cursor] === closing) { cursor++; return result; }
      let count = 0;
      while (cursor < input.length) {
        requireCondition(++count <= LIMITS.members, 'JSON_MEMBER_LIMIT');
        if (object) {
          const key = string();
          requireCondition(!keys.has(key), 'JSON_DUPLICATE_KEY');
          keys.add(key);
          whitespace();
          if (input[cursor++] !== ':') fail();
          result[key] = value(depth + 1);
        } else {
          result.push(value(depth + 1));
        }
        whitespace();
        const separator = input[cursor++];
        if (separator === closing) return result;
        if (separator !== ',') fail();
        whitespace();
      }
      fail();
    }
    for (const [literal, parsed] of [['null', null], ['true', true], ['false', false]]) {
      if (input.startsWith(literal, cursor)) { cursor += literal.length; return parsed; }
    }
    const match = /^-?(?:0|[1-9]\d*)(?:\.\d+)?(?:[eE][+-]?\d+)?/u.exec(input.slice(cursor));
    if (!match) fail();
    cursor += match[0].length;
    const number = Number(match[0]);
    requireCondition(Number.isFinite(number), 'JSON_INVALID_NUMBER');
    return number;
  }
  const result = value(0);
  whitespace();
  if (cursor !== input.length) fail();
  return result;
}

/** RFC 8785 uses ECMAScript primitive serialization and UTF-16 key order.
 * This routine accepts JSON data only: no getters, prototypes, toJSON hooks,
 * sparse arrays, cycles, undefined or class instances can influence the bytes.
 */
export function canonicalize(value) {
  const ancestors = new Set();
  let size = 0;
  function account(piece) {
    size += Buffer.byteLength(piece, 'utf8');
    requireCondition(size <= LIMITS.metadataBytes, 'JSON_INPUT_LIMIT');
    return piece;
  }
  function visit(item, depth) {
    requireCondition(depth <= LIMITS.depth, 'JSON_DEPTH_LIMIT');
    if (item === null || typeof item === 'boolean') return account(JSON.stringify(item));
    if (typeof item === 'number') {
      requireCondition(Number.isFinite(item), 'JSON_INVALID_NUMBER');
      return account(JSON.stringify(item));
    }
    if (typeof item === 'string') { validateString(item); return account(JSON.stringify(item)); }
    requireCondition(typeof item === 'object' && item !== null, 'JSON_INVALID_VALUE');
    requireCondition(!ancestors.has(item), 'JSON_CYCLE');
    const prototype = Object.getPrototypeOf(item);
    const array = Array.isArray(item);
    requireCondition(array ? prototype === Array.prototype
      : prototype === null || prototype === Object.prototype, 'JSON_INVALID_VALUE');
    requireCondition(Object.getOwnPropertySymbols(item).length === 0, 'JSON_INVALID_VALUE');
    const descriptors = Object.getOwnPropertyDescriptors(item);
    const keys = Object.keys(descriptors).filter((key) => !(array && key === 'length'));
    requireCondition(keys.length <= LIMITS.members, 'JSON_MEMBER_LIMIT');
    requireCondition(keys.every((key) => descriptors[key].enumerable
      && Object.hasOwn(descriptors[key], 'value')), 'JSON_INVALID_VALUE');
    ancestors.add(item);
    let result;
    account((array ? '[]' : '{}') + ','.repeat(Math.max(0, keys.length - 1)));
    if (array) {
      requireCondition(keys.length === item.length
        && keys.every((key, index) => key === String(index)), 'JSON_INVALID_VALUE');
      result = '[' + keys.map((key) => visit(descriptors[key].value, depth + 1)).join(',') + ']';
    } else {
      keys.sort();
      result = '{' + keys.map((key) => {
        validateString(key);
        return account(JSON.stringify(key) + ':') + visit(descriptors[key].value, depth + 1);
      }).join(',') + '}';
    }
    ancestors.delete(item);
    return result;
  }
  return encoder.encode(visit(value, 0));
}
