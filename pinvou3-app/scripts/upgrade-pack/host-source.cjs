// Snapshot the actual host build's source tree for the fixed Go host profile.
// The Go tool, not this directory walk, authenticates the embedded catalog.
const fs = require('node:fs');
const path = require('node:path');
const { createHash } = require('node:crypto');
const F = require('./files.cjs');
const { relativeName } = require('../tauri/windows-host-inventory.js');

const HOST_PROFILE = 'pinvou3-windows-host-source.v1';
const ROLES = ['installer.exe', 'pinvou-upgrade-helper.exe', 'pinvou-upgrade-launcher.exe'];

function overlaps(left, right) {
  const relative = path.relative(left, right);
  return relative === '' || (!path.isAbsolute(relative) && relative !== '..' && !relative.startsWith(`..${path.sep}`));
}
function validateHostLocations(source, output) {
  if (!path.isAbsolute(source) || overlaps(source, output) || overlaps(output, source)) F.refuse('T04_SOURCE_DIRECTORY');
  const sourceIdentity = F.directoryIdentity(F.ordinary(source, true));
  // Compare actual ancestor objects as well as spelling. In particular, an
  // 8.3 alias or another path to the same volume must not place the output
  // inside the source tree before snapshotting starts.
  let parent = path.dirname(output);
  for (;;) {
    if (F.directoryIdentity(F.ordinary(parent, true)) === sourceIdentity) F.refuse('T04_SOURCE_DIRECTORY');
    const next = path.dirname(parent);
    if (next === parent) break;
    parent = next;
  }
}

// Each file becomes a fresh exclusive snapshot. The authenticated installer
// inventory and the Go fixed profile remain the authority for membership.
function snapshotHostSource(source, output, limits, snapshot, signal, baseInputBytes) {
  validateHostLocations(source, output);
  const directories = [];
  const destinations = new Map();
  const observed = createHash('sha256');
  let fileCount = 0;
  let entriesSeen = 0;
  let appFiles = 0;
  let inputBytes = baseInputBytes;
  const maxNodes = Math.min(Number.MAX_SAFE_INTEGER, limits.Archive.Entries * 33 + 4);
  function captureDirectory(directory) {
    F.checkSignal(signal);
    const identity = F.directoryIdentity(F.ordinary(directory, true));
    const names = fs.readdirSync(directory).sort();
    if (names.length > maxNodes - entriesSeen) F.refuse('T04_CAPACITY');
    entriesSeen += names.length;
    F.sameDirectory(directory, identity);
    directories.push({ directory, identity, names });
    return names;
  }
  const rootNames = captureDirectory(source);
  if (rootNames.join('\0') !== ['app', ...ROLES].sort().join('\0')) F.refuse('T04_SOURCE_DIRECTORY');
  const destination = path.join(output, 'host-source');
  fs.mkdirSync(destination, { mode: 0o700 });
  destinations.set(destination, F.directoryIdentity(F.ordinary(destination, true)));
  function copy(sourceFile, relative, allowEmpty) {
    const initial = F.ordinary(sourceFile);
    if (initial.size > BigInt(limits.TotalInputBytes - inputBytes)) F.refuse('T04_CAPACITY');
    const ref = F.hashReference(sourceFile, limits.InputBytes, signal, { allowEmpty });
    if (ref.size > limits.TotalInputBytes - inputBytes) F.refuse('T04_CAPACITY');
    inputBytes += ref.size;
    const target = path.join(destination, ...relative.split('/'));
    const parent = path.dirname(target);
    const expected = destinations.get(parent);
    if (!expected) F.refuse('T04_SOURCE_DIRECTORY');
    F.sameDirectory(parent, expected);
    snapshot(ref, path.join('host-source', ...relative.split('/')), limits.InputBytes, allowEmpty, F.identity(initial));
    F.sameDirectory(parent, expected);
    // Incremental, unambiguous record stream; receipt size stays bounded even
    // for a full multi-shard inventory. Names/bytes remain in the snapshots.
    observed.update(`${JSON.stringify([relative, ref.size, ref.sha256])}\n`, 'utf8');
    fileCount++;
  }
  for (const role of ROLES) copy(path.join(source, role), role, false);
  const app = path.join(source, 'app');
  const target = path.join(destination, 'app');
  fs.mkdirSync(target, { mode: 0o700 });
  destinations.set(target, F.directoryIdentity(F.ordinary(target, true)));
  function walk(directory, targetDirectory, prefix, depth) {
    if (depth > 32) F.refuse('T04_CAPACITY');
    for (const name of captureDirectory(directory)) {
      const relative = prefix ? `${prefix}/${name}` : name;
      relativeName(relative);
      const from = path.join(directory, name);
      const to = path.join(targetDirectory, name);
      const stat = fs.lstatSync(from, { bigint: true });
      if (stat.isDirectory() && !stat.isSymbolicLink()) {
        fs.mkdirSync(to, { mode: 0o700 });
        destinations.set(to, F.directoryIdentity(F.ordinary(to, true)));
        walk(from, to, relative, depth + 1);
      } else {
        // ordinary()/inspect() reject links, aliases and changed file identity.
        if (++appFiles > limits.Archive.Entries - ROLES.length) F.refuse('T04_CAPACITY');
        copy(from, `app/${relative}`, true);
      }
    }
  }
  walk(app, target, '', 1);
  if (appFiles === 0) F.refuse('T04_SOURCE_DIRECTORY');
  function recheckDirectories() {
    for (const item of directories) {
      F.checkSignal(signal);
      F.sameDirectory(item.directory, item.identity);
      if (JSON.stringify(fs.readdirSync(item.directory).sort()) !== JSON.stringify(item.names)) F.refuse('T04_SOURCE_CHANGED');
      F.sameDirectory(item.directory, item.identity);
    }
    for (const [directory, identity] of destinations) F.sameDirectory(directory, identity);
  }
  recheckDirectories();
  return { fileCount, totalBytes: inputBytes - baseInputBytes,
    membersSha256: observed.digest('hex'), recheckDirectories };
}

module.exports = { HOST_PROFILE, validateHostLocations, snapshotHostSource };
