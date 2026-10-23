// Builder-owned local controls. These blocks never supply runtime authority.
const fs = require("node:fs");
const path = require("node:path");
const { canonicalBytes, MAXIMUM_MANIFEST_BYTES, exactObject, refuse, sha256 } = require("./windows-host-profile.js");
const { MAXIMUM_FILES, FILES_PER_SHARD, relativeName } = require("./windows-host-inventory.js");

const MAXIMUM_CONTROL_BLOCKS = 16;
const MAXIMUM_SOURCE_BYTES = 3072;
function controlFile(index) { return `native-inventory-${String(index).padStart(3, "0")}.json`; }

async function writeNativeInventory(directory, entries) {
  if (!Array.isArray(entries) || entries.length < 1 || entries.length > MAXIMUM_FILES) {
    refuse("HOST_BUILD_NATIVE_INVENTORY_CAPACITY");
  }
  const wrapper = (index, items) => ({ hostNativeInventoryBlockVersion: 1, index, entries: items });
  const blocks = [];
  let current = [], size = (await canonicalBytes(wrapper(0, []))).length;
  async function complete() {
    if (!current.length || blocks.length >= MAXIMUM_CONTROL_BLOCKS) refuse("HOST_BUILD_NATIVE_INVENTORY_CAPACITY");
    const index = blocks.length, bytes = await canonicalBytes(wrapper(index, current));
    if (bytes.length !== size || bytes.length > MAXIMUM_MANIFEST_BYTES) refuse("HOST_BUILD_NATIVE_INVENTORY_CAPACITY");
    blocks.push({ index, fileName: controlFile(index), size: bytes.length,
      sha256: sha256(bytes), fileCount: current.length, bytes });
  }
  for (const entry of entries) {
    exactObject(entry, ["relativeName", "source"]);
    relativeName(entry.relativeName);
    if (typeof entry.source !== "string" || !path.isAbsolute(entry.source)
        || /[\x00-\x1f\x7f]/u.test(entry.source)
        || Buffer.byteLength(entry.source) > MAXIMUM_SOURCE_BYTES) refuse("HOST_BUILD_NATIVE_SOURCE_PATH_UNSUPPORTED");
    const itemBytes = (await canonicalBytes(entry)).length;
    let next = size + itemBytes + (current.length ? 1 : 0);
    if (current.length >= FILES_PER_SHARD || next > MAXIMUM_MANIFEST_BYTES) {
      await complete();
      if (blocks.length >= MAXIMUM_CONTROL_BLOCKS) refuse("HOST_BUILD_NATIVE_INVENTORY_CAPACITY");
      current = [];
      size = (await canonicalBytes(wrapper(blocks.length, []))).length;
      next = size + itemBytes;
    }
    if (next > MAXIMUM_MANIFEST_BYTES) refuse("HOST_BUILD_NATIVE_INVENTORY_CAPACITY");
    current.push(entry); size = next;
  }
  await complete();
  const references = blocks.map(({ bytes, ...reference }) => reference);
  const indexBytes = await canonicalBytes({ hostNativeInventoryVersion: 1, fileCount: entries.length, blocks: references });
  // Complete all bounded measurements before creating any control file. A
  // capacity refusal cannot leave a deceptively complete prefix index behind.
  for (const block of blocks) fs.writeFileSync(path.join(directory, block.fileName), block.bytes, { mode: 0o600, flag: "wx" });
  const indexPath = path.join(directory, "native-inventory.json");
  fs.writeFileSync(indexPath, indexBytes, { mode: 0o600, flag: "wx" });
  return { indexPath, fileCount: entries.length, blocks: references };
}

module.exports = { MAXIMUM_CONTROL_BLOCKS, MAXIMUM_SOURCE_BYTES, controlFile, writeNativeInventory };
