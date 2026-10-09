import test from 'node:test';
import assert from 'node:assert/strict';
import { canonicalize, LIMITS, parseJson } from '../canonical-json.mjs';

const bytes = (text) => Buffer.from(text, 'utf8');
const canonical = (text) => Buffer.from(canonicalize(parseJson(bytes(text)))).toString('utf8');
const rejects = (operation, code) => assert.throws(operation, { code });

test('RFC 8785 section 3.2.2 example has independently specified output', () => {
  const input = '{"numbers":[333333333.33333329,1E30,4.50,2e-3,0.000000000000000000000000001],'
    + '"string":"\\u20ac$\\u000f\\nA\'B\\\"\\\\\\\\\\\"/","literals":[null,true,false]}';
  const expected = '{"literals":[null,true,false],"numbers":[333333333.3333333,1e+30,4.5,0.002,1e-27],'
    + '"string":"€$\\u000f\\nA\'B\\\"\\\\\\\\\\\"/"}';
  assert.equal(canonical(input), expected);
});

test('UTF-16 code units determine property order, not locale or UTF-8', () => {
  const object = { 'דּ': 7, '😀': 6, '€': 5, 'ö': 4, '\u0080': 3, '1': 2, '\r': 1 };
  assert.equal(Buffer.from(canonicalize(object)).toString(), '{"\\r":1,"1":2,"\u0080":3,"ö":4,"€":5,"😀":6,"דּ":7}');
});

test('ECMAScript shortest finite number representation and negative zero', () => {
  assert.equal(Buffer.from(canonicalize([-0, 1e-6, 1e-7, 1e20, 1e21, Number.MIN_VALUE])).toString(),
    '[0,0.000001,1e-7,100000000000000000000,1e+21,5e-324]');
});

test('duplicate keys are rejected at every depth after escape decoding', () => {
  for (const input of ['{"a":1,"a":2}', '{"a":{"x":1,"\\u0078":2}}', '[{"a":1,"a":2}]']) {
    rejects(() => parseJson(bytes(input)), 'JSON_DUPLICATE_KEY');
  }
});

test('malformed JSON, trailing data, non-JSON whitespace and UTF-8 BOM are rejected', () => {
  for (const input of ['[1,]', '{"a":}', '{a:1}', '01', 'true false', '1e', '"\\x20"', '\uFEFF{}', '\v{}']) {
    rejects(() => parseJson(bytes(input)), 'JSON_INVALID');
  }
  rejects(() => parseJson(Buffer.from([0xc0, 0xaf])), 'JSON_INVALID_UTF8');
});

test('unpaired surrogates, non-finite numbers and non-JSON JavaScript values are rejected', () => {
  for (const input of ['"\\ud800"', '"\\udc00"', '{"\\ud800":1}']) {
    rejects(() => parseJson(bytes(input)), 'JSON_INVALID_UNICODE');
  }
  assert.equal(canonical('"\\ud83d\\ude00"'), '"😀"');
  rejects(() => parseJson(bytes('1e999')), 'JSON_INVALID_NUMBER');
  for (const value of [undefined, 1n, new Date(), new Map(), { a: undefined }, [1, , 3], { get a() { throw new Error(); } }]) {
    rejects(() => canonicalize(value), 'JSON_INVALID_VALUE');
  }
  const cycle = {}; cycle.self = cycle;
  rejects(() => canonicalize(cycle), 'JSON_CYCLE');
  rejects(() => canonicalize(NaN), 'JSON_INVALID_NUMBER');
  rejects(() => canonicalize(Infinity), 'JSON_INVALID_NUMBER');
});

test('size, depth, string and member limits have boundary coverage', () => {
  rejects(() => parseJson(Buffer.alloc(LIMITS.metadataBytes + 1)), 'JSON_INPUT_LIMIT');
  rejects(() => parseJson(bytes('{}'), 1), 'JSON_INPUT_LIMIT');
  rejects(() => parseJson(bytes('"' + 'a'.repeat(LIMITS.stringBytes + 1) + '"')), 'JSON_STRING_LIMIT');
  assert.equal(parseJson(bytes('"' + 'a'.repeat(LIMITS.stringBytes) + '"')).length, LIMITS.stringBytes);
  rejects(() => parseJson(bytes('['.repeat(33) + '0' + ']'.repeat(33))), 'JSON_DEPTH_LIMIT');
  assert.deepEqual(parseJson(bytes('['.repeat(32) + '0' + ']'.repeat(32))),
    Array.from({ length: 32 }).reduce((value) => [value], 0));
  rejects(() => parseJson(bytes(JSON.stringify(Array(4097).fill(0)))), 'JSON_MEMBER_LIMIT');
  assert.equal(parseJson(bytes(JSON.stringify(Array(4096).fill(0)))).length, 4096);
});

test('literal __proto__ remains a harmless own member', () => {
  const result = parseJson(bytes('{"__proto__":{"polluted":true},"constructor":1}'));
  assert.equal(Object.getPrototypeOf(result), null);
  assert.equal(Object.hasOwn(result, '__proto__'), true);
  assert.equal({}.polluted, undefined);
  assert.equal(Buffer.from(canonicalize(result)).toString(), '{"__proto__":{"polluted":true},"constructor":1}');
});
