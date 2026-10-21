// Local build snapshots, not protected installation or hostile-admin custody.
const fs = require('node:fs');
const path = require('node:path');
const { createHash } = require('node:crypto');

function refuse(code) { throw new Error(code); }
function object(value, keys) {
  if (!value || Array.isArray(value) || typeof value !== 'object'
      || Object.keys(value).sort().join('\0') !== [...keys].sort().join('\0')) refuse('T04_SHAPE');
}
function positive(value) {
  if (!Number.isSafeInteger(value) || value <= 0) refuse('T04_CAPACITY');
  return value;
}
function reference(value) {
  object(value, ['path', 'size', 'sha256']);
  positive(value.size);
  if (typeof value.path !== 'string' || !path.isAbsolute(value.path)
      || typeof value.sha256 !== 'string' || value.sha256.length !== 64
      || !/^[0-9a-f]{64}$/u.test(value.sha256)) refuse('T04_REFERENCE');
  return value;
}
function ordinary(filename, directory = false) {
  const absolute = path.resolve(filename);
  let current = path.parse(absolute).root;
  const components = absolute.slice(current.length).split(path.sep).filter(Boolean);
  for (let i = 0; i < components.length; i++) {
    current = path.join(current, components[i]);
    const st = fs.lstatSync(current, { bigint: true });
    const isDirectory = directory || i < components.length - 1;
    if (st.isSymbolicLink() || (isDirectory ? !st.isDirectory() : !st.isFile())
        || (!isDirectory && st.nlink !== 1n)) refuse('T04_FILE_TYPE');
  }
  return fs.statSync(absolute, { bigint: true });
}
function identity(st) {
  return [st.dev, st.ino, st.nlink, st.size, st.mtimeNs, st.ctimeNs, st.birthtimeNs].map(String).join(':');
}
function crossIdentity(st) {
  // Path-stat and handle-stat ctime differ on some Windows runtimes. Keep ctime
  // in each same-interface before/after tuple, never compare it across APIs.
  return [st.dev, st.ino, st.nlink, st.size, st.mtimeNs, st.birthtimeNs].map(String).join(':');
}
function sameDirectory(directory, expected) {
  if (directoryIdentity(ordinary(directory, true)) !== expected) refuse('T04_DIRECTORY_CHANGED');
}
function directoryIdentity(st) { return [st.dev, st.ino, st.birthtimeNs].map(String).join(':'); }
function checkSignal(signal) { if (signal?.aborted) refuse('T04_CANCELLED'); }

// No allocation from an unvalidated file size. Short reads are legal; growth,
// premature EOF, rebind, same-byte replacement, and failed closes all fail.
function inspect(ref, maximum, { destination, signal } = {}) {
  reference(ref); positive(maximum); checkSignal(signal);
  if (ref.size > maximum) refuse('T04_CAPACITY');
  const before = ordinary(ref.path);
  if (before.size !== BigInt(ref.size)) refuse('T04_SIZE');
  const source = fs.openSync(ref.path, fs.constants.O_RDONLY | (fs.constants.O_NOFOLLOW || 0));
  let output;
  try {
    const opened = fs.fstatSync(source, { bigint: true });
    if (!opened.isFile() || opened.nlink !== 1n || crossIdentity(opened) !== crossIdentity(before)) refuse('T04_REBIND');
    if (destination !== undefined) output = fs.openSync(destination, 'wx', 0o700);
    const hash = createHash('sha256');
    const scratch = Buffer.alloc(Math.min(1024 * 1024, ref.size));
    let offset = 0;
    while (offset < ref.size) {
      checkSignal(signal);
      const wanted = Math.min(scratch.length, ref.size - offset);
      const count = fs.readSync(source, scratch, 0, wanted, offset);
      if (!Number.isInteger(count) || count <= 0 || count > wanted) refuse('T04_READ');
      hash.update(scratch.subarray(0, count));
      if (output !== undefined) {
        let written = 0;
        while (written < count) {
          const n = fs.writeSync(output, scratch, written, count - written);
          if (!Number.isInteger(n) || n <= 0 || n > count - written) refuse('T04_WRITE');
          written += n;
        }
      }
      offset += count;
    }
    if (fs.readSync(source, Buffer.alloc(1), 0, 1, offset) !== 0) refuse('T04_GROWTH');
    if (hash.digest('hex') !== ref.sha256) refuse('T04_HASH');
    if (identity(opened) !== identity(fs.fstatSync(source, { bigint: true }))
        || identity(before) !== identity(ordinary(ref.path))) refuse('T04_FILE_CHANGED');
    if (output !== undefined) fs.fsyncSync(output);
  } finally {
    // A failed output close may not suppress the attempt to close the input.
    try { if (output !== undefined) fs.closeSync(output); } finally { fs.closeSync(source); }
  }
  checkSignal(signal);
  return { ...ref, identity: identity(before) };
}
function hashReference(filename, maximum, signal) {
  const before = ordinary(filename);
  positive(maximum); checkSignal(signal);
  if (before.size <= 0n || before.size > BigInt(maximum)) refuse('T04_CAPACITY');
  // Generated files are bounded too. Hash through a stable held file, without
  // requiring a previously known digest from an untrusted output document.
  const fd = fs.openSync(filename, fs.constants.O_RDONLY | (fs.constants.O_NOFOLLOW || 0));
  let sha256;
  try {
    const opened = fs.fstatSync(fd, { bigint: true });
    if (!opened.isFile() || opened.nlink !== 1n || crossIdentity(opened) !== crossIdentity(before)) refuse('T04_REBIND');
    const hash = createHash('sha256'); const buffer = Buffer.alloc(Math.min(1024 * 1024, Number(before.size)));
    let offset = 0;
    while (offset < Number(before.size)) {
      checkSignal(signal);
      const wanted = Math.min(buffer.length, Number(before.size) - offset);
      const n = fs.readSync(fd, buffer, 0, wanted, offset);
      if (!Number.isInteger(n) || n <= 0 || n > wanted) refuse('T04_READ');
      hash.update(buffer.subarray(0, n)); offset += n;
    }
    if (fs.readSync(fd, Buffer.alloc(1), 0, 1, offset) !== 0) refuse('T04_GROWTH');
    if (identity(opened) !== identity(fs.fstatSync(fd, { bigint: true }))
        || identity(before) !== identity(ordinary(filename))) refuse('T04_FILE_CHANGED');
    sha256 = hash.digest('hex');
  } finally { fs.closeSync(fd); }
  return { path: filename, size: Number(before.size), sha256 };
}
function readBounded(filename, maximum, signal) {
  const ref = hashReference(filename, maximum, signal);
  const bytes = Buffer.alloc(ref.size);
  const fd = fs.openSync(filename, fs.constants.O_RDONLY | (fs.constants.O_NOFOLLOW || 0));
  try {
    let offset = 0;
    while (offset < bytes.length) {
      checkSignal(signal);
      const n = fs.readSync(fd, bytes, offset, bytes.length - offset, offset);
      if (!Number.isInteger(n) || n <= 0 || n > bytes.length - offset) refuse('T04_READ');
      offset += n;
    }
    if (fs.readSync(fd, Buffer.alloc(1), 0, 1, offset) !== 0) refuse('T04_GROWTH');
  } finally { fs.closeSync(fd); }
  if (createHash('sha256').update(bytes).digest('hex') !== ref.sha256) refuse('T04_FILE_CHANGED');
  inspect(ref, maximum, { signal });
  return bytes;
}
function writeExclusive(filename, bytes) {
  const fd = fs.openSync(filename, 'wx', 0o600);
  try {
    let offset = 0;
    while (offset < bytes.length) {
      const n = fs.writeSync(fd, bytes, offset, bytes.length - offset);
      if (!Number.isInteger(n) || n <= 0 || n > bytes.length - offset) refuse('T04_WRITE');
      offset += n;
    }
    fs.fsyncSync(fd);
  } finally { fs.closeSync(fd); }
}
function publishCompletion(directory, bytes, expectedDirectory) {
  sameDirectory(directory, expectedDirectory);
  const pending = path.join(directory, '.completion.pending');
  writeExclusive(pending, bytes);
  sameDirectory(directory, expectedDirectory);
  // Exclusive publication. Keep the pending link; no fallible cleanup after
  // this commit point. The receipt's two links are intentional output only.
  fs.linkSync(pending, path.join(directory, 'completion.json'));
}
module.exports = { refuse, object, positive, reference, ordinary, identity, directoryIdentity, sameDirectory,
  checkSignal, inspect, hashReference, readBounded, writeExclusive, publishCompletion };
