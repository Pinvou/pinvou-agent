// Exact final signed-file inventory. No file is implicitly omitted to fit a bound.
const fs = require("node:fs");
const path = require("node:path");
const { createHash } = require("node:crypto");
const { canonicalBytes, MAXIMUM_MANIFEST_BYTES, refuse } = require("./windows-host-profile.js");
const { MAXIMUM_FILES, relativeName, buildAppInventory } = require("./windows-host-inventory.js");
function noReparseAncestors(file) {
  let current = path.resolve(file);
  for (;;) {
    const item = fs.lstatSync(current);
    if (item.isSymbolicLink() || (!item.isFile() && !item.isDirectory())) refuse();
    const parent = path.dirname(current);
    if (parent === current) break;
    current = parent;
  }
}
async function hashFile(file, { allowEmpty = true } = {}) {
  noReparseAncestors(file);
  const before = fs.lstatSync(file);
  if (!before.isFile() || before.nlink !== 1 || !Number.isSafeInteger(before.size)
      || before.size < (allowEmpty ? 0 : 1)) refuse("HOST_BUILD_FILE_INVALID");
  const handle = await fs.promises.open(file, "r");
  try {
    const start = await handle.stat();
    if (start.ino !== before.ino || start.dev !== before.dev || start.nlink !== 1
        || start.size !== before.size) refuse();
    const hash = createHash("sha256");
    let size = 0;
    for await (const chunk of handle.createReadStream({ autoClose: false })) {
      size += chunk.length;
      if (!Number.isSafeInteger(size) || size > start.size) refuse();
      hash.update(chunk);
    }
    const end = await handle.stat();
    if (size !== start.size || end.size !== start.size || end.mtimeMs !== start.mtimeMs
        || end.ctimeMs !== start.ctimeMs || end.nlink !== 1) {
      // Preserve refusal and its public message. Only closed numeric metadata
      // accompanies this failure; paths and file contents are never included.
      const fields = ["size", "mtimeMs", "ctimeMs", "nlink"];
      const snapshot = (value) => Object.freeze(Object.fromEntries(fields.map((key) => [key, value[key]])));
      const cause = Object.freeze({ reason: "HOST_BUILD_FILE_UNSTABLE", readSize: size,
        start: snapshot(start), end: snapshot(end),
        delta: Object.freeze({ readSize: size - start.size,
          ...Object.fromEntries(fields.map((key) => [key, end[key] - start[key]])) }) });
      throw new Error("HOST_BUILD_INPUT_INVALID", { cause });
    }
    return { size, sha256: hash.digest("hex") };
  } finally { await handle.close(); }
}

function inventorySources({ appBinary, resourceManifest, tauriRoot }) {
  if (!Array.isArray(resourceManifest?.files) || resourceManifest.files.length >= MAXIMUM_FILES) {
    refuse("HOST_BUILD_MANIFEST_CAPACITY");
  }
  const entries = [{ relativeName: "pinvou3-tauri.exe", source: path.resolve(appBinary) }];
  for (const item of resourceManifest.files) {
    if (item.kind !== "file") refuse("HOST_BUILD_RESOURCE_UNSUPPORTED");
    const source = path.resolve(tauriRoot, item.source);
    const inside = path.relative(tauriRoot, source);
    if (inside.startsWith(`..${path.sep}`) || inside === ".." || path.isAbsolute(inside)) refuse();
    entries.push({ relativeName: relativeName(item.destination), source });
  }
  if (entries.length > MAXIMUM_FILES) refuse("HOST_BUILD_MANIFEST_CAPACITY");
  entries.sort((a, b) => Buffer.compare(Buffer.from(a.relativeName, "utf8"), Buffer.from(b.relativeName, "utf8")));
  const seen = new Set();
  for (const entry of entries) {
    relativeName(entry.relativeName);
    // ASCII aliases use the exact same ASCII mapping as Windows. Non-ASCII
    // requires the actual SDK ordinal comparator supplied by nativeCheck below.
    const asciiKey = entry.relativeName.replace(/[A-Z]/gu, (x) => x.toLowerCase());
    if (seen.has(asciiKey)) refuse("HOST_BUILD_RESOURCE_ALIAS");
    seen.add(asciiKey);
  }
  return entries;
}

async function buildInstallationManifest({ profile, helper, launcher, appBinary,
  resourceManifest, tauriRoot, nativeCheck }) {
  if (typeof nativeCheck !== "function") refuse("HOST_BUILD_NATIVE_CHECK_REQUIRED");
  const entries = inventorySources({ appBinary, resourceManifest, tauriRoot });
  // Actual Windows CompareStringOrdinal and native file-attribute inspection,
  // including non-ASCII aliases, is mandatory for an enabled Windows package.
  await nativeCheck(entries);
  const appFiles = [];
  for (const entry of entries) appFiles.push({ relativeName: entry.relativeName, ...await hashFile(entry.source) });
  const seed = profile.seed;
  const image = (value) => ({ buildId: seed.buildId, version: seed.appVersion,
    ...value, layoutId: seed.layoutId });
  const main = appFiles.find((item) => item.relativeName === "pinvou3-tauri.exe");
  if (main.size < 1) refuse("HOST_BUILD_FILE_INVALID");
  const inventory = await buildAppInventory(profile, appFiles);
  const manifest = { hostManifestVersion: 2, profileKind: seed.profileKind,
    profileId: seed.profileId, product: seed.product, component: seed.component,
    targetKey: seed.targetKey, buildId: seed.buildId, seedSha256: profile.seedSha256,
    originalRoot: seed.originalRoot, layoutId: seed.layoutId,
    helper: image(await hashFile(helper, { allowEmpty: false })), launcher: image(await hashFile(launcher, { allowEmpty: false })),
    app: image({ size: main.size, sha256: main.sha256 }), appInventory: inventory.appInventory };
  const bytes = await canonicalBytes(manifest);
  if (manifest.app.size < 1) refuse("HOST_BUILD_FILE_INVALID");
  if (bytes.length > MAXIMUM_MANIFEST_BYTES) refuse("HOST_BUILD_MANIFEST_CAPACITY");
  // appFiles is an internal typed aggregate used for final file rehashing only;
  // its canonical wire representation consists exclusively of bounded shards.
  return { manifest, bytes, entries, appFiles, shards: inventory.shards };
}

module.exports = { MAXIMUM_FILES, relativeName, noReparseAncestors, hashFile,
  inventorySources, buildInstallationManifest };
