import assert from "node:assert/strict";
import { test } from "node:test";
import { encodeFrame } from "../frame.js";

test("encodeFrame returns encoded JSON and UTF-8 byte length for a normal frame", () => {
  const { encoded, bytes } = encodeFrame({ x: "你好" });
  assert.equal(encoded, '{"x":"你好"}');
  // Multibyte on purpose: the length guard counts bytes, not UTF-16 units.
  assert.equal(bytes, 14);
});

test("encodeFrame throws on a throwing getter — the poison shape JSON.parse cannot produce", () => {
  assert.throws(() => encodeFrame({ get x() { throw new Error("boom"); } }), /boom/);
});

test("encodeFrame throws on a cyclic reference", () => {
  const a = {};
  a.self = a;
  assert.throws(() => encodeFrame(a), /circular|cyclic/i);
});

test("encodeFrame throws on BigInt", () => {
  assert.throws(() => encodeFrame({ x: 1n }), /BigInt/);
});
