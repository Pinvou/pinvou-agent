// Private bounded inventory pages. Public protocol V1 framing stays unchanged.
const path = require("node:path");
const { canonicalBytes, MAXIMUM_MANIFEST_BYTES, exactObject, HASH, refuse, sha256 } = require("./windows-host-profile.js");

const FILES_PER_SHARD = 4096;
const MAXIMUM_SHARDS = 8;
const MAXIMUM_FILES = FILES_PER_SHARD * MAXIMUM_SHARDS;
const FIRST_SHARD_RESOURCE_ID = 34102;

function relativeName(name) {
  if (typeof name !== "string" || Buffer.byteLength(name) > 1024 || !name
      || /[\\:\x00-\x1f\x7f]/u.test(name) || path.posix.isAbsolute(name)) refuse();
  const parts = name.split("/");
  if (parts.length > 32 || parts.some((part) => !part || part === "." || part === ".."
      || /[. ]$/u.test(part) || /[<>"|?*]/u.test(part)
      || /^(con|prn|aux|nul|conin\$|conout\$|com[1-9\u00b9\u00b2\u00b3]|lpt[1-9\u00b9\u00b2\u00b3])(?:\.|$)/iu.test(part))) refuse();
  return name;
}

function checkedSum(left, right) {
  if (!Number.isSafeInteger(left) || left < 0 || !Number.isSafeInteger(right) || right < 0
      || left > Number.MAX_SAFE_INTEGER - right) refuse("HOST_BUILD_MANIFEST_CAPACITY");
  return left + right;
}

// Entry bytes are measured individually. Only each completed bounded wrapper
// enters the original canonical parser; the full internal aggregate never does.
async function buildAppInventory(profile, files) {
  if (!Array.isArray(files) || files.length < 1 || files.length > MAXIMUM_FILES) refuse("HOST_BUILD_MANIFEST_CAPACITY");
  const seed = profile.seed;
  const wrapper = (index, entries) => ({ hostInventoryShardVersion: 1,
    profileId: seed.profileId, buildId: seed.buildId, seedSha256: profile.seedSha256, index, files: entries });
  const shards = [];
  let current = [], size = (await canonicalBytes(wrapper(0, []))).length;
  let totalSize = 0, aggregateBytes = 0, previous = null;
  async function complete() {
    if (!current.length || shards.length >= MAXIMUM_SHARDS) refuse("HOST_BUILD_MANIFEST_CAPACITY");
    const index = shards.length, bytes = await canonicalBytes(wrapper(index, current));
    if (bytes.length !== size || bytes.length > MAXIMUM_MANIFEST_BYTES) refuse("HOST_BUILD_MANIFEST_CAPACITY");
    aggregateBytes = checkedSum(aggregateBytes, bytes.length);
    if (aggregateBytes > MAXIMUM_SHARDS * MAXIMUM_MANIFEST_BYTES) refuse("HOST_BUILD_MANIFEST_CAPACITY");
    shards.push({ index, resourceId: FIRST_SHARD_RESOURCE_ID + index, bytes, files: current });
  }
  for (const file of files) {
    exactObject(file, ["relativeName", "size", "sha256"]);
    relativeName(file.relativeName);
    if (!Number.isSafeInteger(file.size) || file.size < 0 || typeof file.sha256 !== "string" || !HASH.test(file.sha256)) refuse();
    const name = Buffer.from(file.relativeName, "utf8");
    if (previous !== null && Buffer.compare(previous, name) >= 0) refuse("HOST_BUILD_RESOURCE_ALIAS");
    previous = name;
    const entryBytes = (await canonicalBytes(file)).length;
    let next = checkedSum(size, entryBytes + (current.length ? 1 : 0));
    if (current.length >= FILES_PER_SHARD || next > MAXIMUM_MANIFEST_BYTES) {
      await complete();
      if (shards.length >= MAXIMUM_SHARDS) refuse("HOST_BUILD_MANIFEST_CAPACITY");
      current = [];
      size = (await canonicalBytes(wrapper(shards.length, []))).length;
      next = checkedSum(size, entryBytes);
    }
    if (next > MAXIMUM_MANIFEST_BYTES) refuse("HOST_BUILD_MANIFEST_CAPACITY");
    current.push(file);
    size = next;
    totalSize = checkedSum(totalSize, file.size);
  }
  await complete();
  const references = shards.map(shard => ({ index: shard.index, resourceId: shard.resourceId,
    size: shard.bytes.length, sha256: sha256(shard.bytes), fileCount: shard.files.length }));
  return { appInventory: { hostInventoryVersion: 1, fileCount: files.length, totalSize, shards: references }, shards };
}

module.exports = { FILES_PER_SHARD, MAXIMUM_SHARDS, MAXIMUM_FILES, FIRST_SHARD_RESOURCE_ID,
  relativeName, checkedSum, buildAppInventory };
