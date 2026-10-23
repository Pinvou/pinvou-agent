const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { createHash } = require("node:crypto");
const { snapshotCargoApp } = require("../scripts/tauri/windows-host-build-input.js");
const { hashFile } = require("../scripts/tauri/windows-host-manifest.js");

async function fixture(run) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "pinvou-cargo-input-test-"));
  const source = path.join(root, "input", "app.exe");
  const destination = path.join(root, "output", "app.exe");
  fs.mkdirSync(path.dirname(source)); fs.mkdirSync(path.dirname(destination));
  const bytes = Buffer.alloc(150000, 37); fs.writeFileSync(source, bytes);
  try { await run({ root, source, destination, bytes }); }
  finally {
    const resolved = path.resolve(root);
    assert.equal(path.dirname(resolved), path.resolve(os.tmpdir()));
    assert.ok(path.basename(resolved).startsWith("pinvou-cargo-input-test-"));
    fs.rmSync(resolved, { recursive: true });
  }
}
async function replaceMethod(object, key, replacement, run) {
  const original = object[key]; object[key] = replacement(original);
  try { await run(); } finally { object[key] = original; }
}
test("only explicit Cargo snapshot accepts hard links; final strict hashing still refuses them", () => fixture(async ({ source, destination, root, bytes }) => {
  const alias = path.join(root, "app-link.exe"); fs.linkSync(source, alias);
  await assert.rejects(hashFile(source), /HOST_BUILD_FILE_INVALID/);
  const observed = await snapshotCargoApp(source, destination);
  assert.equal(observed.sourceLinkCount, "2"); assert.equal(fs.statSync(destination).nlink, 1);
  assert.deepEqual(fs.readFileSync(destination), bytes);
  assert.deepEqual(await hashFile(destination), { size: bytes.length, sha256: createHash("sha256").update(bytes).digest("hex") });
  await assert.rejects(hashFile(source), /HOST_BUILD_FILE_INVALID/);
  fs.unlinkSync(alias); assert.equal((await hashFile(source)).size, bytes.length);
}));
test("existing destination is never replaced", () => fixture(async ({ source, destination }) => {
  fs.writeFileSync(destination, "original");
  await assert.rejects(snapshotCargoApp(source, destination), /EEXIST/);
  assert.equal(fs.readFileSync(destination, "utf8"), "original");
}));
test("source mutation between read passes refuses snapshot", () => fixture(async ({ source, destination }) => {
  let changed = false;
  await replaceMethod(fs, "readSync", (original) => (...args) => {
    const count = original(...args);
    if (!changed) { changed = true; fs.writeFileSync(source, Buffer.alloc(150000, 52)); }
    return count;
  }, async () => { await assert.rejects(snapshotCargoApp(source, destination), /HOST_BUILD_INPUT_CHANGED/); });
}));
test("source name replacement refuses even when held bytes remain readable", () => fixture(async ({ source, destination }) => {
  let changed = false;
  await replaceMethod(fs, "readSync", (original) => (...args) => {
    const count = original(...args);
    if (!changed) { changed = true; fs.renameSync(source, source + ".old"); fs.writeFileSync(source, Buffer.alloc(150000, 37)); }
    return count;
  }, async () => { await assert.rejects(snapshotCargoApp(source, destination), /HOST_BUILD_INPUT_CHANGED/); });
}));
test("source link count changes are rejected", () => fixture(async ({ source, destination, root }) => {
  let changed = false;
  await replaceMethod(fs, "readSync", (original) => (...args) => {
    const count = original(...args);
    if (!changed) { changed = true; fs.linkSync(source, path.join(root, "added-link.exe")); }
    return count;
  }, async () => { await assert.rejects(snapshotCargoApp(source, destination), /HOST_BUILD_INPUT_CHANGED/); });
}));
test("short source reads are refused", () => fixture(async ({ source, destination }) => {
  await replaceMethod(fs, "readSync", (original) => (fd, buffer, offset, length, position) =>
    original(fd, buffer, offset, Math.min(length, 1), position), async () => {
    await assert.rejects(snapshotCargoApp(source, destination), /HOST_BUILD_INPUT_CHANGED/);
  });
}));
test("partial destination writes finish without dropping bytes", () => fixture(async ({ source, destination, bytes }) => {
  await replaceMethod(fs, "writeSync", (original) => (fd, buffer, offset, length, position) =>
    original(fd, buffer, offset, Math.min(length, 97), position), async () => {
    await snapshotCargoApp(source, destination);
  });
  assert.deepEqual(fs.readFileSync(destination), bytes);
}));
test("zero-progress destination writes refuse and preserve diagnostic output", () => fixture(async ({ source, destination }) => {
  await replaceMethod(fs, "writeSync", () => () => 0, async () => {
    await assert.rejects(snapshotCargoApp(source, destination), /HOST_BUILD_INPUT_CHANGED/);
  });
  assert.equal(fs.existsSync(destination), true);
}));
test("source parent reparse is refused", () => fixture(async ({ source, destination, root }) => {
  const alias = path.join(root, "junction"); fs.symlinkSync(path.dirname(source), alias, "junction");
  await assert.rejects(snapshotCargoApp(path.join(alias, "app.exe"), destination));
  assert.equal(fs.existsSync(destination), false);
  fs.unlinkSync(alias);
}));
test("destination replacement during final strict hash is refused", () => fixture(async ({ source, destination }) => {
  await replaceMethod(fs.promises, "open", (original) => async (...args) => {
    if (args[0] === destination) { fs.renameSync(destination, destination + ".old"); fs.copyFileSync(source, destination); }
    return original.apply(fs.promises, args);
  }, async () => { await assert.rejects(snapshotCargoApp(source, destination), /HOST_BUILD_INPUT_(CHANGED|INVALID)/); });
}));
test("empty Cargo app and unsupported input types fail before output creation", () => fixture(async ({ source, destination }) => {
  fs.writeFileSync(source, ""); await assert.rejects(snapshotCargoApp(source, destination), /HOST_BUILD_FILE_INVALID/);
  await assert.rejects(snapshotCargoApp(path.dirname(source), destination), /HOST_BUILD_FILE_INVALID/);
  assert.equal(fs.existsSync(destination), false);
}));
test("metadata and copy reads stay bounded for larger input", () => fixture(async ({ source, destination }) => {
  let maximum = 0;
  await replaceMethod(fs, "readSync", (original) => (...args) => {
    maximum = Math.max(maximum, args[3]); return original(...args);
  }, async () => { await snapshotCargoApp(source, destination); });
  assert.equal(maximum, 65536);
}));
