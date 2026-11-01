/**
 * Serialize one outbound frame and measure its byte length. Extracted from
 * send() so the poison-frame guard stays unit-testable: once #781's ingress
 * depth cap rejects deep frames at the door, no e2e frame can reach the
 * stringify throw any more, so the only reachable triggers are locally
 * constructed values — a throwing getter, a BigInt, a cyclic reference.
 *
 * JSON.stringify is recursive and throws RangeError on deeply nested values
 * that JSON.parse accepted, so a forwarded frame can be unserializable even
 * though ingress let it through. byteLength stays inside the same guard:
 * stringify of JSON.parse output cannot be `undefined` today, but a future
 * `send(ws, undefined)` call site would throw here and escape the
 * destination-only cleanup.
 */
export function encodeFrame(value) {
  const encoded = JSON.stringify(value);
  return { encoded, bytes: Buffer.byteLength(encoded) };
}
