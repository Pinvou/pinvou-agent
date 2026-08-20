const { spawnSync } = require("node:child_process");
const crypto = require("node:crypto");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { writeEffectiveArtifacts } = require("./effective-config.js");
const {
  prepareCodexBridge,
  prepareWindowsCodexBridge,
  WINDOWS_BRIDGE_CONFIG_PATH,
} = require("./codex-bridge.js");
const {
  APP_ROOT,
  platformArchitectureConfigPath,
  platformConfigPath,
} = require("./platform-config.js");
const { linuxStartupWindowConfigSpec } = require("./startup-window-config.js");
const { WRAPPER_ENV } = require("./require-wrapper.js");
const { stageWindowsInstaller } = require("./windows-installer.js");
const {
  stageWindowsOnnxRuntime,
  stageWindowsRuntime,
} = require("./windows-runtime.js");

const LINUX_SUPERVISOR_MANIFEST = path.join(
  APP_ROOT,
  "src-tauri",
  "packaging",
  "linux",
  "supervisor",
  "Cargo.toml",
);
const LINUX_SUPERVISOR_TARGET_DIR = path.join(
  APP_ROOT,
  "src-tauri",
  "target",
  "pinvou-supervisor",
);
const LINUX_SUPERVISOR_BINARY = path.join(
  LINUX_SUPERVISOR_TARGET_DIR,
  "release",
  "pinvou-supervisor",
);
const LINUX_PLATFORM_CONFIG = path.join(
  APP_ROOT,
  "src-tauri",
  "config",
  "platforms",
  "linux",
  "tauri.conf.json",
);
const LINUX_DEB_FIXED_FILE_MODES = Object.freeze({
  "/usr/lib/pinvou3/supervisor/pinvou-supervisor": 0o755,
  "/usr/lib/pinvou3/supervisor/pinvou-megabook-profile": 0o755,
  "/usr/lib/systemd/user/pinvou3-supervisor.socket": 0o644,
  "/usr/lib/systemd/user/pinvou3-supervisor.service": 0o644,
  "/usr/lib/systemd/user/pinvou3-app.service": 0o644,
  "/usr/lib/systemd/user/pinvou-qwen3-asr.service.d/50-pinvou-supervisor.conf": 0o644,
  "/usr/share/pinvou3/supervisor/descriptors/pinvou-app-v1.json": 0o644,
  "/usr/share/pinvou3/supervisor/descriptors/pinvou-asr-v1.json": 0o644,
  "/usr/share/pinvou3/supervisor/profiles/megabook-canary.conf": 0o644,
  "/usr/share/pinvou3/supervisor/profiles/pinvou3-megabook-canary.desktop": 0o644,
});

function sha256(bytes) {
  return crypto.createHash("sha256").update(bytes).digest("hex");
}

function exactObjectKeys(actual, expected, label) {
  const actualKeys = Object.keys(actual || {}).sort();
  const expectedKeys = Object.keys(expected || {}).sort();
  if (JSON.stringify(actualKeys) !== JSON.stringify(expectedKeys)) {
    throw new Error(
      `${label} mismatch: expected ${expectedKeys.join(", ")}, got ${actualKeys.join(", ")}`,
    );
  }
  return expectedKeys;
}

function normalizedDebDestination(destination) {
  if (
    typeof destination !== "string"
    || !destination.startsWith("/")
    || destination === "/"
    || destination.endsWith("/")
    || path.posix.normalize(destination) !== destination
    || destination.includes("\0")
  ) {
    throw new Error(`invalid fixed deb destination: ${destination}`);
  }
  return destination;
}

function regularNonSymlink(file, label, lstat = fs.lstatSync) {
  const metadata = lstat(file);
  if (metadata.isSymbolicLink() || !metadata.isFile()) {
    throw new Error(`${label} must be a regular non-symlink file: ${file}`);
  }
  return metadata;
}

function regularSingleLink(file, label, lstat = fs.lstatSync) {
  const metadata = regularNonSymlink(file, label, lstat);
  if (metadata.nlink !== 1) {
    throw new Error(`${label} must have exactly one hard link: ${file}`);
  }
  return metadata;
}

function setLinuxDebBuildUmask(setUmask = (mode) => process.umask(mode)) {
  return setUmask(0o022);
}

function lstatIfPresent(file) {
  try {
    return fs.lstatSync(file);
  } catch (error) {
    if (error.code === "ENOENT") return null;
    throw error;
  }
}

function assertSafeLinuxDebStagingPath(stage) {
  const tauriRoot = path.resolve(stage.tauriRoot);
  const targetRoot = path.resolve(stage.targetRoot);
  const stagingRoot = path.resolve(stage.stagingRoot);
  const expectedTargetRoot = path.resolve(tauriRoot, "target");
  const expectedStagingRoot = path.resolve(
    expectedTargetRoot,
    "tauri-config",
    "linux",
    "deb-files",
  );
  if (targetRoot !== expectedTargetRoot || stagingRoot !== expectedStagingRoot) {
    throw new Error(`unsafe Linux deb staging directory: ${stagingRoot}`);
  }

  const tauriMetadata = lstatIfPresent(tauriRoot);
  if (!tauriMetadata || tauriMetadata.isSymbolicLink() || !tauriMetadata.isDirectory()) {
    throw new Error(`Linux deb tauri root must be a real directory: ${tauriRoot}`);
  }
  const tauriRealRoot = fs.realpathSync(tauriRoot);
  const ancestors = [
    targetRoot,
    path.join(targetRoot, "tauri-config"),
    path.join(targetRoot, "tauri-config", "linux"),
    stagingRoot,
  ];
  let missingParent = false;
  let stagingExists = false;
  for (const ancestor of ancestors) {
    const metadata = lstatIfPresent(ancestor);
    if (!metadata) {
      missingParent = true;
      continue;
    }
    if (missingParent || metadata.isSymbolicLink() || !metadata.isDirectory()) {
      throw new Error(`Linux deb staging ancestor must be a real directory: ${ancestor}`);
    }
    if (ancestor === stagingRoot) stagingExists = true;
    const expectedRealPath = path.resolve(
      tauriRealRoot,
      path.relative(tauriRoot, ancestor),
    );
    const actualRealPath = fs.realpathSync(ancestor);
    const relativeToTauri = path.relative(tauriRealRoot, actualRealPath);
    if (
      actualRealPath !== expectedRealPath
      || relativeToTauri === ".."
      || relativeToTauri.startsWith(`..${path.sep}`)
      || path.isAbsolute(relativeToTauri)
    ) {
      throw new Error(`Linux deb staging ancestor escaped tauri root: ${ancestor}`);
    }
  }
  return { stagingRoot, stagingExists };
}

function cleanupLinuxDebFixedFilesStage(stage) {
  if (!stage) return;
  const { stagingRoot, stagingExists } = assertSafeLinuxDebStagingPath(stage);
  if (stagingExists) {
    fs.rmSync(stagingRoot, { recursive: true, force: true, maxRetries: 3 });
  }
}

function ensureDirectoryWithinLinuxDebStage(stage, directory) {
  const { stagingRoot, stagingExists } = assertSafeLinuxDebStagingPath(stage);
  if (!stagingExists) {
    throw new Error(`Linux deb staging root does not exist: ${stagingRoot}`);
  }
  const requested = path.resolve(directory);
  const requestedRelative = path.relative(stagingRoot, requested);
  if (
    requestedRelative === ".."
    || requestedRelative.startsWith(`..${path.sep}`)
    || path.isAbsolute(requestedRelative)
  ) {
    throw new Error(`Linux deb staging directory escaped its root: ${requested}`);
  }

  const stagingRealRoot = fs.realpathSync(stagingRoot);
  let current = stagingRoot;
  for (const component of requestedRelative.split(path.sep).filter(Boolean)) {
    current = path.join(current, component);
    let metadata = lstatIfPresent(current);
    if (!metadata) {
      assertSafeLinuxDebStagingPath(stage);
      const parent = path.dirname(current);
      const parentRelative = path.relative(stagingRoot, parent);
      if (fs.realpathSync(parent) !== path.resolve(stagingRealRoot, parentRelative)) {
        throw new Error(`Linux deb staging parent escaped its root: ${parent}`);
      }
      fs.mkdirSync(current, { mode: 0o755 });
      metadata = lstatIfPresent(current);
    }
    if (!metadata || metadata.isSymbolicLink() || !metadata.isDirectory()) {
      throw new Error(`Linux deb staging child must be a real directory: ${current}`);
    }
    const currentRelative = path.relative(stagingRoot, current);
    if (fs.realpathSync(current) !== path.resolve(stagingRealRoot, currentRelative)) {
      throw new Error(`Linux deb staging child escaped its root: ${current}`);
    }
  }
}

function stageLinuxDebFixedFiles({
  tauriRoot = path.join(APP_ROOT, "src-tauri"),
  configPath = LINUX_PLATFORM_CONFIG,
  fixedFileModes = LINUX_DEB_FIXED_FILE_MODES,
} = {}) {
  const targetRoot = path.resolve(tauriRoot, "target");
  const stagingRoot = path.resolve(targetRoot, "tauri-config", "linux", "deb-files");
  const stagingRelative = path.relative(targetRoot, stagingRoot);
  if (
    !stagingRelative
    || stagingRelative === ".."
    || stagingRelative.startsWith(`..${path.sep}`)
    || path.isAbsolute(stagingRelative)
  ) {
    throw new Error(`Linux deb staging directory escaped target: ${stagingRoot}`);
  }

  const stageDescriptor = {
    stagingRoot,
    targetRoot,
    tauriRoot: path.resolve(tauriRoot),
  };
  // A killed build may leave this fixed target subtree behind. Remove it before
  // reading any new input so even a subsequently invalid config cannot reuse it.
  cleanupLinuxDebFixedFilesStage(stageDescriptor);

  const linuxConfig = JSON.parse(fs.readFileSync(configPath, "utf8"));
  const configuredFiles = linuxConfig?.bundle?.linux?.deb?.files;
  if (!configuredFiles || typeof configuredFiles !== "object" || Array.isArray(configuredFiles)) {
    throw new Error("Linux tauri config must define bundle.linux.deb.files as an object");
  }
  const destinations = exactObjectKeys(
    configuredFiles,
    fixedFileModes,
    "Linux deb fixed file allowlist",
  );

  let completed = false;
  try {
    assertSafeLinuxDebStagingPath(stageDescriptor);
    fs.mkdirSync(stagingRoot, { recursive: true, mode: 0o700 });
    assertSafeLinuxDebStagingPath(stageDescriptor);
    fs.chmodSync(stagingRoot, 0o700);

    const overlayFiles = {};
    const expectedFiles = {};
    for (const destination of destinations) {
      normalizedDebDestination(destination);
      const configuredSource = configuredFiles[destination];
      if (
        typeof configuredSource !== "string"
        || configuredSource.length === 0
        || path.isAbsolute(configuredSource)
        || configuredSource.includes("\0")
      ) {
        throw new Error(`invalid source for fixed deb destination ${destination}`);
      }
      const source = path.resolve(tauriRoot, configuredSource);
      const sourceRelative = path.relative(tauriRoot, source);
      if (
        !sourceRelative
        || sourceRelative === ".."
        || sourceRelative.startsWith(`..${path.sep}`)
        || path.isAbsolute(sourceRelative)
      ) {
        throw new Error(`fixed deb source escaped src-tauri: ${configuredSource}`);
      }
      regularNonSymlink(source, "fixed deb source");
      if (fs.realpathSync(source) !== source) {
        throw new Error(`fixed deb source path must not traverse a symlink: ${configuredSource}`);
      }

      const sourceBytes = fs.readFileSync(source);
      const staged = path.join(stagingRoot, "rootfs", ...destination.slice(1).split("/"));
      const stagedRelative = path.relative(stagingRoot, staged);
      if (
        !stagedRelative
        || stagedRelative === ".."
        || stagedRelative.startsWith(`..${path.sep}`)
        || path.isAbsolute(stagedRelative)
      ) {
        throw new Error(`fixed deb staged path escaped staging root: ${destination}`);
      }
      ensureDirectoryWithinLinuxDebStage(stageDescriptor, path.dirname(staged));
      fs.copyFileSync(source, staged, fs.constants.COPYFILE_EXCL);
      const expectedMode = fixedFileModes[destination];
      fs.chmodSync(staged, expectedMode);
      const stagedMetadata = regularSingleLink(staged, "staged deb file");
      if ((stagedMetadata.mode & 0o7777) !== expectedMode) {
        throw new Error(`staged deb file mode mismatch for ${destination}`);
      }
      const stagedBytes = fs.readFileSync(staged);
      if (!sourceBytes.equals(stagedBytes)) {
        throw new Error(`staged deb file content changed for ${destination}`);
      }

      const stagedSource = path.relative(tauriRoot, staged).split(path.sep).join("/");
      overlayFiles[destination] = stagedSource;
      expectedFiles[destination] = {
        mode: expectedMode,
        sha256: sha256(sourceBytes),
        size: sourceBytes.length,
        stagedSource,
      };
    }

    const overlayPath = path.join(stagingRoot, "tauri.fixed-deb-files.conf.json");
    fs.writeFileSync(
      overlayPath,
      `${JSON.stringify({ bundle: { linux: { deb: { files: overlayFiles } } } }, null, 2)}\n`,
      { encoding: "utf8", mode: 0o644, flag: "wx" },
    );
    fs.chmodSync(overlayPath, 0o644);
    regularSingleLink(overlayPath, "fixed deb overlay");
    completed = true;
    return {
      ...stageDescriptor,
      expectedFiles,
      overlayFiles,
      overlayPath,
    };
  } finally {
    if (!completed) cleanupLinuxDebFixedFilesStage(stageDescriptor);
  }
}

function appendFinalConfigSpecs(args, specs) {
  const prepared = [...args];
  const separator = prepared.indexOf("--");
  const insertion = separator < 0 ? prepared.length : separator;
  prepared.splice(insertion, 0, ...specs.flatMap((spec) => ["--config", spec]));
  return prepared;
}

function assertLinuxDebFixedFilesOverlay(effectiveConfig, expectedOverlayFiles) {
  const actual = effectiveConfig?.bundle?.linux?.deb?.files;
  if (!actual || typeof actual !== "object" || Array.isArray(actual)) {
    throw new Error("effective Linux deb config has no fixed files mapping");
  }
  for (const destination of exactObjectKeys(
    actual,
    expectedOverlayFiles,
    "effective Linux deb fixed file allowlist",
  )) {
    if (actual[destination] !== expectedOverlayFiles[destination]) {
      throw new Error(`effective Linux deb source bypassed staging for ${destination}`);
    }
  }
}

function nativeLinuxArchitecture(architecture = process.arch) {
  if (architecture === "x64") {
    return { rustTarget: "x86_64-unknown-linux-gnu", elfMachine: 62, debArchitecture: "amd64" };
  }
  if (architecture === "arm64") {
    return { rustTarget: "aarch64-unknown-linux-gnu", elfMachine: 183, debArchitecture: "arm64" };
  }
  throw new Error(`pinvou-supervisor does not support Linux architecture ${architecture}`);
}

function explicitTauriTarget(args = []) {
  const targets = [];
  for (let index = 0; index < args.length; index += 1) {
    if (args[index] === "--target") {
      if (!args[index + 1]) throw new Error("--target 缺少 target triple");
      targets.push(args[index + 1]);
      index += 1;
    } else if (args[index].startsWith("--target=")) {
      targets.push(args[index].slice("--target=".length));
    }
  }
  const unique = [...new Set(targets)];
  if (unique.length > 1) throw new Error(`conflicting Tauri targets: ${unique.join(", ")}`);
  return unique[0] || null;
}

function verifyElfArchitecture(file, expectedMachine, read = fs.readFileSync) {
  const header = read(file).subarray(0, 20);
  if (
    header.length < 20
    || header[0] !== 0x7f
    || header[1] !== 0x45
    || header[2] !== 0x4c
    || header[3] !== 0x46
    || header[4] !== 2
    || header[5] !== 1
  ) {
    throw new Error("pinvou-supervisor is not a 64-bit little-endian ELF binary");
  }
  const actualMachine = header.readUInt16LE(18);
  if (actualMachine !== expectedMachine) {
    throw new Error(
      `pinvou-supervisor ELF machine mismatch: expected ${expectedMachine}, got ${actualMachine}`,
    );
  }
}

function prepareLinuxSupervisor({
  platform = process.platform,
  architecture = process.arch,
  tauriArgs = [],
  spawn = spawnSync,
  exists = fs.existsSync,
  chmod = fs.chmodSync,
  executable = (file) => {
    fs.accessSync(file, fs.constants.X_OK);
    return true;
  },
  verifyElf = verifyElfArchitecture,
} = {}) {
  if (platform !== "linux") return null;
  const native = nativeLinuxArchitecture(architecture);
  const requestedTarget = explicitTauriTarget(tauriArgs);
  if (requestedTarget && requestedTarget !== native.rustTarget) {
    throw new Error(
      `cross-target Linux packaging is refused: Tauri target ${requestedTarget} cannot use native ${native.rustTarget} supervisor`,
    );
  }
  const args = [
    "build",
    "--release",
    "--locked",
    "--manifest-path",
    LINUX_SUPERVISOR_MANIFEST,
    "--target-dir",
    LINUX_SUPERVISOR_TARGET_DIR,
  ];
  const result = spawn("cargo", args, {
    cwd: APP_ROOT,
    env: process.env,
    stdio: "inherit",
  });
  if (result.error) throw result.error;
  if (result.status !== 0) {
    throw new Error(`pinvou-supervisor release build failed (${result.status})`);
  }
  if (!exists(LINUX_SUPERVISOR_BINARY)) {
    throw new Error(`pinvou-supervisor release binary missing: ${LINUX_SUPERVISOR_BINARY}`);
  }
  chmod(LINUX_SUPERVISOR_BINARY, 0o755);
  if (!executable(LINUX_SUPERVISOR_BINARY)) {
    throw new Error(`pinvou-supervisor release binary is not executable: ${LINUX_SUPERVISOR_BINARY}`);
  }
  verifyElf(LINUX_SUPERVISOR_BINARY, native.elfMachine);
  return LINUX_SUPERVISOR_BINARY;
}

function linuxDebRequested(args) {
  if (args.includes("--no-bundle")) return false;
  const explicit = [];
  for (let index = 0; index < args.length; index += 1) {
    const argument = args[index];
    if (argument === "--bundles" || argument === "-b") {
      if (!args[index + 1]) throw new Error(`${argument} 缺少 bundle 类型`);
      explicit.push(args[index + 1]);
      index += 1;
    } else if (argument.startsWith("--bundles=")) {
      explicit.push(argument.slice("--bundles=".length));
    }
  }
  if (explicit.length === 0) return true;
  return explicit.flatMap((value) => value.split(",")).some((value) => value === "deb" || value === "all");
}

function fingerprintLinuxDebArtifact(file) {
  const pathMetadata = regularNonSymlink(file, "Linux deb artifact");
  const noFollow = fs.constants.O_NOFOLLOW || 0;
  const descriptor = fs.openSync(file, fs.constants.O_RDONLY | noFollow);
  try {
    const before = fs.fstatSync(descriptor);
    if (
      !before.isFile()
      || before.dev !== pathMetadata.dev
      || before.ino !== pathMetadata.ino
    ) {
      throw new Error(`Linux deb artifact changed while opening: ${file}`);
    }
    const digest = crypto.createHash("sha256");
    const buffer = Buffer.allocUnsafe(1024 * 1024);
    let bytesReadTotal = 0;
    while (true) {
      const bytesRead = fs.readSync(
        descriptor,
        buffer,
        0,
        buffer.length,
        bytesReadTotal,
      );
      if (bytesRead === 0) break;
      digest.update(buffer.subarray(0, bytesRead));
      bytesReadTotal += bytesRead;
    }
    const after = fs.fstatSync(descriptor);
    if (
      after.dev !== before.dev
      || after.ino !== before.ino
      || after.size !== before.size
      || after.mtimeMs !== before.mtimeMs
      || after.ctimeMs !== before.ctimeMs
      || bytesReadTotal !== before.size
    ) {
      throw new Error(`Linux deb artifact changed while hashing: ${file}`);
    }
    return {
      ctimeMs: before.ctimeMs,
      dev: before.dev,
      ino: before.ino,
      mtimeMs: before.mtimeMs,
      sha256: digest.digest("hex"),
      size: before.size,
    };
  } finally {
    fs.closeSync(descriptor);
  }
}

function sameLinuxDebFingerprint(left, right) {
  return Boolean(left && right)
    && left.ctimeMs === right.ctimeMs
    && left.dev === right.dev
    && left.ino === right.ino
    && left.mtimeMs === right.mtimeMs
    && left.sha256 === right.sha256
    && left.size === right.size;
}

function snapshotLinuxDebArtifacts({
  platform = process.platform,
  targetDirectory = path.join(APP_ROOT, "src-tauri", "target"),
  exists = fs.existsSync,
  readdir = fs.readdirSync,
  fingerprint = fingerprintLinuxDebArtifact,
} = {}) {
  const snapshot = new Map();
  if (platform !== "linux") return snapshot;
  const debDirectory = path.join(targetDirectory, "release", "bundle", "deb");
  if (!exists(debDirectory)) return snapshot;
  const directoryMetadata = fs.lstatSync(debDirectory);
  if (directoryMetadata.isSymbolicLink() || !directoryMetadata.isDirectory()) {
    throw new Error(`Linux deb output directory must be a real directory: ${debDirectory}`);
  }
  for (const name of readdir(debDirectory).filter((entry) => entry.endsWith(".deb")).sort()) {
    const artifact = path.join(debDirectory, name);
    snapshot.set(artifact, fingerprint(artifact));
  }
  return snapshot;
}

function verifyLinuxDebArchitecture({
  platform = process.platform,
  architecture = process.arch,
  targetDirectory = path.join(APP_ROOT, "src-tauri", "target"),
  spawn = spawnSync,
  exists = fs.existsSync,
  readdir = fs.readdirSync,
  stat = fs.statSync,
  beforeArtifacts = null,
  fingerprint = fingerprintLinuxDebArtifact,
} = {}) {
  if (platform !== "linux") return null;
  const native = nativeLinuxArchitecture(architecture);
  const debDirectory = path.join(targetDirectory, "release", "bundle", "deb");
  if (!exists(debDirectory)) {
    throw new Error(`Linux deb output directory is missing: ${debDirectory}`);
  }
  let candidates = readdir(debDirectory)
    .filter((name) => name.endsWith(".deb"))
    .map((name) => path.join(debDirectory, name))
    .sort((left, right) => stat(right).mtimeMs - stat(left).mtimeMs);
  if (candidates.length === 0) throw new Error("Linux build produced no deb artifact");
  if (beforeArtifacts !== null) {
    if (!(beforeArtifacts instanceof Map)) {
      throw new Error("Linux deb pre-build snapshot must be a Map");
    }
    candidates = candidates.filter((candidate) => {
      const before = beforeArtifacts.get(candidate);
      return !before || !sameLinuxDebFingerprint(before, fingerprint(candidate));
    });
    if (candidates.length === 0) {
      throw new Error("Linux build produced no new or updated deb artifact");
    }
  }
  const dpkgDeb = ["/usr/bin/dpkg-deb", "/bin/dpkg-deb"].find(exists);
  if (!dpkgDeb) throw new Error("dpkg-deb is required to verify Linux package architecture");
  const artifact = candidates[0];
  const result = spawn(dpkgDeb, ["--field", artifact, "Architecture"], {
    cwd: APP_ROOT,
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
  });
  if (result.error) throw result.error;
  if (result.status !== 0) {
    throw new Error(`cannot inspect deb architecture (${result.status}): ${result.stderr || ""}`);
  }
  const actual = String(result.stdout || "").trim();
  if (actual !== native.debArchitecture) {
    throw new Error(
      `Linux deb architecture mismatch: expected ${native.debArchitecture}, got ${actual || "empty"}`,
    );
  }
  return artifact;
}

function symbolicRegularMode(mode) {
  if (mode === 0o755) return "-rwxr-xr-x";
  if (mode === 0o644) return "-rw-r--r--";
  throw new Error(`unsupported fixed deb file mode: ${mode.toString(8)}`);
}

function parseDebContents(output) {
  const entries = [];
  for (const line of String(output || "").split(/\r?\n/)) {
    if (!line.trim()) continue;
    const match = /^(\S{10})\s+(\S+)\s+(\d+)\s+\S+\s+\S+\s+(.+)$/.exec(line);
    if (!match) throw new Error(`cannot parse dpkg-deb --contents line: ${line}`);
    let archivePath = match[4].split(/\s+->\s+|\s+link to\s+/, 1)[0];
    if (archivePath.startsWith("./")) archivePath = archivePath.slice(1);
    if (!archivePath.startsWith("/")) archivePath = `/${archivePath}`;
    entries.push({
      mode: match[1],
      ownerGroup: match[2],
      size: Number(match[3]),
      path: archivePath,
    });
  }
  return entries;
}

function checkedSpawn(spawn, command, args, options, label) {
  const result = spawn(command, args, options);
  if (result.error) throw result.error;
  if (result.status !== 0) {
    throw new Error(`${label} (${result.status}): ${result.stderr || ""}`);
  }
  return result;
}

function verifyLinuxDebFixedFiles({
  artifact,
  expectedFiles,
  spawn = spawnSync,
  exists = fs.existsSync,
  temporaryDirectory = os.tmpdir(),
} = {}) {
  if (!artifact) throw new Error("Linux deb artifact is required for fixed file verification");
  if (!expectedFiles || typeof expectedFiles !== "object" || Array.isArray(expectedFiles)) {
    throw new Error("expected Linux deb fixed files are required");
  }
  const artifactFingerprint = fingerprintLinuxDebArtifact(artifact);
  const dpkgDeb = ["/usr/bin/dpkg-deb", "/bin/dpkg-deb"].find(exists);
  if (!dpkgDeb) throw new Error("dpkg-deb is required to verify Linux package contents");
  const deterministicEnvironment = {
    ...process.env,
    LANG: "C",
    LC_ALL: "C",
    TZ: "UTC",
  };
  const contents = checkedSpawn(
    spawn,
    dpkgDeb,
    ["--contents", artifact],
    {
      cwd: APP_ROOT,
      encoding: "utf8",
      env: deterministicEnvironment,
      maxBuffer: 32 * 1024 * 1024,
      stdio: ["ignore", "pipe", "pipe"],
    },
    "cannot inspect deb contents",
  );
  const entries = parseDebContents(contents.stdout);
  const destinations = Object.keys(expectedFiles).sort();
  for (const destination of destinations) {
    normalizedDebDestination(destination);
    const expected = expectedFiles[destination];
    const matching = entries.filter((entry) => entry.path === destination);
    if (matching.length !== 1) {
      throw new Error(
        `deb fixed path must appear exactly once: ${destination} (found ${matching.length})`,
      );
    }
    const [entry] = matching;
    const expectedMode = symbolicRegularMode(expected.mode);
    if (entry.mode !== expectedMode) {
      throw new Error(
        `deb fixed path mode mismatch for ${destination}: expected ${expectedMode}, got ${entry.mode}`,
      );
    }
    if (entry.ownerGroup !== "root/root") {
      throw new Error(
        `deb fixed path owner mismatch for ${destination}: expected root/root, got ${entry.ownerGroup}`,
      );
    }
    if (entry.size !== expected.size) {
      throw new Error(
        `deb fixed path size mismatch for ${destination}: expected ${expected.size}, got ${entry.size}`,
      );
    }
  }

  const temporaryBase = fs.realpathSync(temporaryDirectory);
  const temporaryMetadata = fs.lstatSync(temporaryBase);
  if (!temporaryMetadata.isDirectory() || temporaryMetadata.isSymbolicLink()) {
    throw new Error(`deb verification temporary base is not a real directory: ${temporaryBase}`);
  }
  const extractionRoot = fs.mkdtempSync(path.join(temporaryBase, "pinvou-deb-verify-"));
  try {
    const extractionRelative = path.relative(temporaryBase, extractionRoot);
    if (
      !extractionRelative
      || extractionRelative === ".."
      || extractionRelative.startsWith(`..${path.sep}`)
      || path.isAbsolute(extractionRelative)
    ) {
      throw new Error(`deb verification temporary directory escaped its base: ${extractionRoot}`);
    }
    fs.chmodSync(extractionRoot, 0o700);
    checkedSpawn(
      spawn,
      dpkgDeb,
      ["--extract", artifact, extractionRoot],
      {
        cwd: APP_ROOT,
        encoding: "utf8",
        env: deterministicEnvironment,
        maxBuffer: 16 * 1024 * 1024,
        stdio: ["ignore", "pipe", "pipe"],
      },
      "cannot safely extract deb for fixed file verification",
    );
    for (const destination of destinations) {
      const expected = expectedFiles[destination];
      const extracted = path.join(extractionRoot, ...destination.slice(1).split("/"));
      const extractedRelative = path.relative(extractionRoot, extracted);
      if (
        !extractedRelative
        || extractedRelative === ".."
        || extractedRelative.startsWith(`..${path.sep}`)
        || path.isAbsolute(extractedRelative)
      ) {
        throw new Error(`extracted fixed deb path escaped temporary root: ${destination}`);
      }
      const metadata = regularSingleLink(extracted, "extracted fixed deb file");
      if (fs.realpathSync(extracted) !== extracted) {
        throw new Error(`extracted fixed deb path traversed a symlink: ${destination}`);
      }
      if ((metadata.mode & 0o7777) !== expected.mode) {
        throw new Error(`extracted fixed deb file mode mismatch for ${destination}`);
      }
      const bytes = fs.readFileSync(extracted);
      if (bytes.length !== expected.size || sha256(bytes) !== expected.sha256) {
        throw new Error(`extracted fixed deb file hash mismatch for ${destination}`);
      }
    }
  } finally {
    fs.rmSync(extractionRoot, { recursive: true, force: true, maxRetries: 3 });
  }
  if (!sameLinuxDebFingerprint(artifactFingerprint, fingerprintLinuxDebArtifact(artifact))) {
    throw new Error(`Linux deb artifact changed during fixed file verification: ${artifact}`);
  }
  return artifact;
}

function tauriCommandIndex(args) {
  return args.findIndex((argument) => argument === "build" || argument === "bundle");
}

function configSpecs(args) {
  const specs = [];
  for (let index = 0; index < args.length; index += 1) {
    if (args[index] === "--config" || args[index] === "-c") {
      if (!args[index + 1]) throw new Error("--config 缺少配置值");
      specs.push(args[index + 1]);
      index += 1;
    } else if (args[index].startsWith("--config=")) {
      specs.push(args[index].slice("--config=".length));
    }
  }
  return specs;
}

function windowsBundleTargets(args) {
  const explicit = [];
  for (let index = 0; index < args.length; index += 1) {
    const argument = args[index];
    if (argument === "--bundles" || argument === "-b") {
      if (!args[index + 1]) throw new Error(`${argument} 缺少 bundle 类型`);
      explicit.push(args[index + 1]);
      index += 1;
    } else if (argument.startsWith("--bundles=")) {
      explicit.push(argument.slice("--bundles=".length));
    }
  }
  if (explicit.length === 0 || explicit.includes("all")) return ["msi", "nsis"];
  return [...new Set(explicit.flatMap((value) => value.split(",")).filter(Boolean))];
}

function prepareTauriArgs(
  args,
  {
    platform = process.platform,
    architecture = process.arch,
    stageRuntime = stageWindowsRuntime,
    additionalConfigs = [],
  } = {},
) {
  const prepared = [...args];
  const commandIndex = tauriCommandIndex(prepared);
  if (commandIndex < 0) {
    // dev 不注入 packaging overlay。macOS 复用平台 overlay 保持原生顶栏一致；
    // Linux 只注入 dev overlay，让冷启动窗口等 React 首次提交后再显示，避开
    // Mutter/XWayland 首次映射期间视觉表面与输入表面短暂错位。
    const devIndex = prepared.indexOf("dev");
    const devConfig = platform === "darwin"
      ? platformConfigPath(platform)
      : platform === "linux"
        ? linuxStartupWindowConfigSpec()
        : null;
    if (devIndex >= 0 && devConfig) {
      // 与 build/bundle 保持相同优先级:自动平台配置在前,调用方显式
      // --config 在后,从而仍可有意覆盖平台默认值。
      prepared.splice(devIndex + 1, 0, "--config", devConfig);
    }
    return prepared;
  }

  const automaticConfigs = [platformConfigPath(platform)];
  if (platform === "linux") automaticConfigs.push(linuxStartupWindowConfigSpec());
  const architectureConfig = platformArchitectureConfigPath(platform, architecture);
  if (architectureConfig) automaticConfigs.push(architectureConfig);
  const stagedRuntime = stageRuntime({ platform });
  const runtimeConfig =
    typeof stagedRuntime === "string" ? stagedRuntime : stagedRuntime?.configPath;
  if (runtimeConfig) automaticConfigs.push(runtimeConfig);
  automaticConfigs.push(...additionalConfigs);
  const injected = automaticConfigs.flatMap((configPath) => ["--config", configPath]);
  // Automatic overlays must precede explicit signing/staging overlays so the
  // caller can intentionally override or remove inherited resource mappings.
  prepared.splice(commandIndex + 1, 0, ...injected);
  return prepared;
}

function runTauri(preparedArgs, spawn = spawnSync, environment = process.env) {
  const tauriCli = require.resolve("@tauri-apps/cli/tauri.js");
  const child = spawn(process.execPath, [tauriCli, ...preparedArgs], {
    cwd: APP_ROOT,
    env: { ...environment, [WRAPPER_ENV]: "1" },
    stdio: "inherit",
  });
  if (child.error) throw child.error;
  return child.status === null ? 1 : child.status;
}

function tauriRuntimeEnvironment(runtime, environment = process.env) {
  return runtime
    ? { ...environment, ORT_DYLIB_PATH: runtime.onnxRuntimeDylib }
    : environment;
}

function main() {
  const args = process.argv.slice(2);
  const validateOnly = args[0] === "--validate-only";
  if (validateOnly) args.shift();

  if (validateOnly) return;

  const isDev = args.includes("dev");
  const hasTauriBuildCommand = tauriCommandIndex(args) >= 0;
  const linuxDebBuild = hasTauriBuildCommand
    && process.platform === "linux"
    && linuxDebRequested(args);
  if (linuxDebBuild) setLinuxDebBuildUmask();
  const additionalConfigs = [];
  let linuxDebStage = null;
  // Windows 的 fastembed 使用动态 ONNX Runtime。正式包 staging 完整运行时并通过
  // resource overlay 携带 DLL；dev 只校验并展开 ONNX 组件，避免为 UI 开发准备无关工具。
  const windowsRuntime =
    hasTauriBuildCommand && process.platform === "win32"
      ? stageWindowsRuntime()
      : null;
  const windowsDevRuntime =
    isDev && process.platform === "win32" ? stageWindowsOnnxRuntime() : null;
  if (windowsRuntime && hasTauriBuildCommand) {
    stageWindowsInstaller({
      bundleTargets: windowsBundleTargets(args),
      runtime: windowsRuntime,
    });
  }
  const windowsBridgeOptions = windowsRuntime
    ? {
        nodeExecutable: windowsRuntime.nodeExecutable,
        npmExecPath: windowsRuntime.npmExecPath,
      }
    : undefined;
  if (isDev) {
    prepareCodexBridge();
    prepareWindowsCodexBridge();
  }
  if (hasTauriBuildCommand) {
    prepareCodexBridge();
    prepareWindowsCodexBridge(windowsBridgeOptions);
    prepareLinuxSupervisor({ tauriArgs: args });
    if (linuxDebBuild) linuxDebStage = stageLinuxDebFixedFiles();
    if (process.platform === "win32") {
      additionalConfigs.push(WINDOWS_BRIDGE_CONFIG_PATH);
    }
  }

  try {
    let preparedArgs = prepareTauriArgs(args, {
      additionalConfigs,
      stageRuntime: () => windowsRuntime,
    });
    if (linuxDebStage) {
      preparedArgs = appendFinalConfigSpecs(preparedArgs, [linuxDebStage.overlayPath]);
    }
    if (hasTauriBuildCommand) {
      const artifacts = writeEffectiveArtifacts(configSpecs(preparedArgs));
      if (linuxDebStage) {
        assertLinuxDebFixedFilesOverlay(artifacts.effectiveConfig, linuxDebStage.overlayFiles);
      }
      console.log(`[build] 有效 Tauri 配置: ${artifacts.effectiveConfigPath}`);
      console.log(
        `[build] 安装包资源清单: ${artifacts.resourceManifestPath} (${artifacts.resourceManifest.resourceFileCount} files)`,
      );
    }

    const requestedTarget = linuxDebBuild ? explicitTauriTarget(args) : null;
    const linuxDebTargetDirectory = linuxDebBuild
      ? path.join(
          APP_ROOT,
          "src-tauri",
          "target",
          ...(requestedTarget ? [requestedTarget] : []),
        )
      : null;
    const preBuildDebArtifacts = linuxDebBuild
      ? snapshotLinuxDebArtifacts({ targetDirectory: linuxDebTargetDirectory })
      : null;
    const tauriEnvironment = tauriRuntimeEnvironment(windowsRuntime || windowsDevRuntime);
    const exitCode = runTauri(preparedArgs, undefined, tauriEnvironment);
    if (
      exitCode === 0
      && hasTauriBuildCommand
      && linuxDebBuild
    ) {
      const artifact = verifyLinuxDebArchitecture({
        beforeArtifacts: preBuildDebArtifacts,
        targetDirectory: linuxDebTargetDirectory,
      });
      verifyLinuxDebFixedFiles({ artifact, expectedFiles: linuxDebStage.expectedFiles });
    }
    process.exitCode = exitCode;
  } finally {
    cleanupLinuxDebFixedFilesStage(linuxDebStage);
  }
}

if (require.main === module) {
  try {
    main();
  } catch (error) {
    console.error(`[build] ${error.message}`);
    process.exitCode = 1;
  }
}

module.exports = {
  LINUX_DEB_FIXED_FILE_MODES,
  appendFinalConfigSpecs,
  assertLinuxDebFixedFilesOverlay,
  cleanupLinuxDebFixedFilesStage,
  configSpecs,
  main,
  explicitTauriTarget,
  linuxDebRequested,
  nativeLinuxArchitecture,
  prepareCodexBridge,
  prepareLinuxSupervisor,
  prepareWindowsCodexBridge,
  setLinuxDebBuildUmask,
  snapshotLinuxDebArtifacts,
  stageLinuxDebFixedFiles,
  stageWindowsInstaller,
  stageWindowsOnnxRuntime,
  stageWindowsRuntime,
  prepareTauriArgs,
  runTauri,
  tauriRuntimeEnvironment,
  tauriCommandIndex,
  verifyElfArchitecture,
  verifyLinuxDebArchitecture,
  verifyLinuxDebFixedFiles,
  windowsBundleTargets,
};
