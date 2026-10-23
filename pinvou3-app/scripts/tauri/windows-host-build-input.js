// Cargo may hard-link its output. Snapshot only that explicit build input;
// installation files and every other packaging input retain strict single links.
const fs = require("node:fs");
const path = require("node:path");
const { createHash } = require("node:crypto");
const { hashFile, noReparseAncestors } = require("./windows-host-manifest.js");
const { refuse } = require("./windows-host-profile.js");

const BLOCK_BYTES = 65536;
const FILE_IDENTITY = ["dev", "ino", "birthtimeNs", "size", "mtimeNs", "ctimeNs", "nlink"];
function sameFile(first, second) {
  return first.isFile() && second.isFile()
    && FILE_IDENTITY.every((field) => first[field] === second[field]);
}
function ancestors(file) {
  const result = [];
  let current = path.dirname(path.resolve(file));
  for (;;) {
    const item = fs.lstatSync(current, { bigint: true });
    if (!item.isDirectory() || item.isSymbolicLink()) refuse("HOST_BUILD_INPUT_CHANGED");
    result.push({ path: current, dev: item.dev, ino: item.ino, birthtimeNs: item.birthtimeNs });
    const parent = path.dirname(current);
    if (parent === current) return result;
    current = parent;
  }
}
function recheckAncestors(expected, file) {
  const actual = ancestors(file);
  if (actual.length !== expected.length || expected.some((item, index) =>
    ["path", "dev", "ino", "birthtimeNs"].some((field) => item[field] !== actual[index][field]))) {
    refuse("HOST_BUILD_INPUT_CHANGED");
  }
}
function readHash(fd, expectedSize, copyTo) {
  const buffer = Buffer.alloc(BLOCK_BYTES);
  const hash = createHash("sha256");
  let position = 0;
  while (position < expectedSize) {
    const wanted = Math.min(buffer.length, expectedSize - position);
    const read = fs.readSync(fd, buffer, 0, wanted, position);
    if (read !== wanted) refuse("HOST_BUILD_INPUT_CHANGED");
    hash.update(buffer.subarray(0, read));
    if (copyTo !== undefined) {
      let written = 0;
      while (written < read) {
        const count = fs.writeSync(copyTo, buffer, written, read - written, position + written);
        if (count <= 0) refuse("HOST_BUILD_INPUT_CHANGED");
        written += count;
      }
    }
    position += read;
  }
  if (fs.readSync(fd, buffer, 0, 1, expectedSize) !== 0) refuse("HOST_BUILD_INPUT_CHANGED");
  return hash.digest("hex");
}

async function snapshotCargoApp(source, destination) {
  noReparseAncestors(source);
  noReparseAncestors(path.dirname(destination));
  const sourceParents = ancestors(source);
  const destinationParents = ancestors(destination);
  const before = fs.lstatSync(source, { bigint: true });
  if (!before.isFile() || before.nlink < 1n || before.size < 1n
      || before.size > BigInt(Number.MAX_SAFE_INTEGER)) refuse("HOST_BUILD_FILE_INVALID");
  let input;
  let output;
  try {
    input = fs.openSync(source, "r");
    const verifyInput = () => {
      if (!sameFile(before, fs.fstatSync(input, { bigint: true }))
          || !sameFile(before, fs.lstatSync(source, { bigint: true }))) refuse("HOST_BUILD_INPUT_CHANGED");
      recheckAncestors(sourceParents, source);
    };
    verifyInput();
    const size = Number(before.size);
    const originalHash = readHash(input, size);
    verifyInput();
    // Exclusive creation refuses every existing destination, including links.
    output = fs.openSync(destination, "wx", 0o600);
    recheckAncestors(destinationParents, destination);
    const created = fs.fstatSync(output, { bigint: true });
    if (!created.isFile() || created.nlink !== 1n) refuse("HOST_BUILD_INPUT_CHANGED");
    const copiedHash = readHash(input, size, output);
    fs.fsyncSync(output);
    const completedOutput = output;
    output = undefined;
    fs.closeSync(completedOutput);
    verifyInput();
    const afterHash = readHash(input, size);
    verifyInput();
    const snapshot = fs.lstatSync(destination, { bigint: true });
    if (snapshot.dev !== created.dev || snapshot.ino !== created.ino
        || snapshot.birthtimeNs !== created.birthtimeNs || snapshot.nlink !== 1n) {
      refuse("HOST_BUILD_INPUT_CHANGED");
    }
    const final = await hashFile(destination, { allowEmpty: false });
    verifyInput();
    recheckAncestors(destinationParents, destination);
    if (originalHash !== copiedHash || originalHash !== afterHash
        || final.sha256 !== originalHash || final.size !== size
        || !sameFile(snapshot, fs.lstatSync(destination, { bigint: true }))) {
      refuse("HOST_BUILD_INPUT_CHANGED");
    }
    return { size, sha256: originalHash, sourceLinkCount: before.nlink.toString(),
      sourceIdentity: Object.fromEntries(FILE_IDENTITY.map((key) => [key, before[key].toString()])),
      snapshotIdentity: Object.fromEntries(FILE_IDENTITY.map((key) => [key, snapshot[key].toString()])) };
  } finally {
    try { if (output !== undefined) fs.closeSync(output); }
    finally { if (input !== undefined) fs.closeSync(input); }
  }
}

module.exports = { snapshotCargoApp };
