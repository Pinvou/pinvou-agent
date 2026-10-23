// An independent, fixed NSIS profile. The original Tauri template is never patched.
const fs = require("node:fs");
const path = require("node:path");
const { spawn, execFile } = require("node:child_process");
const { canonicalBytes, codec, refuse, sha256, validateHostApplicationConfig, imageCertificatePolicy, hostNamespace } = require("./windows-host-profile.js");
const { buildInstallationManifest, hashFile, inventorySources, relativeName } = require("./windows-host-manifest.js");
const { MAXIMUM_FILES } = require("./windows-host-inventory.js");
const { writeNativeInventory } = require("./windows-host-native-inventory.js");
const { assertHostRoleImage } = require("./windows-host-role-pe.js");
const { snapshotCargoApp } = require("./windows-host-build-input.js");

const RESOURCE_SCRIPT = path.join(__dirname, "windows-host-resource.ps1");
const TEMPLATE = path.resolve(__dirname, "../../src-tauri/packaging/windows/nsis/host-installer.nsi");
const POWERSHELL = path.join(process.env.SystemRoot || "C:/Windows", "System32/WindowsPowerShell/v1.0/powershell.exe");

// The fixed numeric words are uint16. This NSIS adapter constraint does not
// change shared Root/protocol/app-version semantics.
function nsisFileVersion(value) {
  if (typeof value !== "string") refuse("HOST_BUILD_NSIS_VERSION_UNSUPPORTED");
  const parts = value.split(".");
  if (parts.length !== 3 || parts.some((part) => !/^(0|[1-9][0-9]*)$/u.test(part)
      || Number(part) > 65535 || String(Number(part)) !== part)) {
    refuse("HOST_BUILD_NSIS_VERSION_UNSUPPORTED");
  }
  return `${value}.0`;
}
function quoted(value) {
  if (typeof value !== "string" || /[\r\n\0]/u.test(value)) refuse();
  // `${...}` is NSIS preprocessor syntax even inside quoted data. This is a
  // build-directory restriction only; app resource names never enter NSIS code.
  if (value.includes("${")) refuse("HOST_BUILD_NSIS_PATH_UNSUPPORTED");
  return `"${value.replaceAll("$", "$$").replaceAll('"', '$\\"')}"`;
}
function executable(name) {
  if (!name || !path.isAbsolute(name) || !fs.statSync(name).isFile()) refuse("HOST_BUILD_TOOL_REQUIRED");
  return name;
}
function runBuildTool(command, args, { cwd, logFile, environment = process.env, recordOutput = true } = {}) {
  return new Promise((resolve, reject) => {
    const fd = fs.openSync(logFile, "a", 0o600);
    let child;
    try { child = spawn(executable(command), args, { cwd, env: environment,
      windowsHide: true, stdio: ["ignore", recordOutput ? fd : "ignore", recordOutput ? fd : "ignore"], shell: false }); }
    catch { fs.closeSync(fd); reject(new Error("HOST_BUILD_TOOL_FAILED")); return; }
    let failure = false;
    child.once("error", () => { failure = true; });
    // `close`, rather than exit, joins all inherited output streams before reuse.
    child.once("close", (code, signal) => {
      fs.closeSync(fd);
      if (failure || code !== 0 || signal) reject(new Error("HOST_BUILD_TOOL_FAILED"));
      else resolve();
    });
  });
}
function nativeTool(mode, parameters, options) {
  return runBuildTool(POWERSHELL, ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass",
    "-File", RESOURCE_SCRIPT, "-Mode", mode, ...parameters], options);
}
async function checkHelperProfile(image, profile) {
  assertHostRoleImage(image);
  const bytes = await new Promise((resolve, reject) => {
    execFile(image, ["--profile"], { windowsHide: true, encoding: "buffer", maxBuffer: 65536 },
      (error, stdout) => error ? reject(new Error("HOST_BUILD_COMPILED_PROFILE_INVALID")) : resolve(stdout));
  });
  let actual;
  try { actual = (await codec()).parseJson(bytes, 65536); } catch { refuse("HOST_BUILD_COMPILED_PROFILE_INVALID"); }
  const seed = profile.seed;
  const expected = { hostProfileVersion: 1, enabled: true, profileKind: seed.profileKind,
    profileId: seed.profileId, product: seed.product, component: seed.component, targetKey: seed.targetKey,
    buildId: seed.buildId, layoutId: seed.layoutId, seedSha256: profile.seedSha256, originalRoot: seed.originalRoot };
  if (!(await canonicalBytes(actual)).equals(await canonicalBytes(expected))) refuse("HOST_BUILD_COMPILED_PROFILE_INVALID");
}
function renderInclude({ profile, entries, installer, manifestPath, stubPath }) {
  const seed = profile.seed;
  const fileVersion = nsisFileVersion(seed.appVersion);
  const namespace = hostNamespace(seed);
  const packCommand = [quoted(POWERSHELL), "-NoProfile -NonInteractive -ExecutionPolicy Bypass -File",
    quoted(RESOURCE_SCRIPT), "-Mode Embed -Image", quoted(stubPath), "-Manifest", quoted(manifestPath)].join(" ");
  // The source helper owns every copy under its committed native intent, using
  // protected security descriptors at creation. Ordinary NSIS File inheritance
  // cannot supply that guarantee. The complete app tree remains in SOURCE/app.
  if (!entries.length || entries.length > MAXIMUM_FILES) refuse();
  for (const entry of entries) relativeName(entry.relativeName);
  return [
    `!define HOST_SEED_VERSION ${seed.hostSeedVersion}`,
    `!define HOST_SOURCE_LEAF ${quoted(namespace.sourceLeaf)}`,
    `!define HOST_CODE_LEAF ${quoted(namespace.codeLeaf)}`,
    `!define HOST_APP_LEAF ${quoted(namespace.appLeaf)}`,
    `!define HOST_SERVICE_NAME ${quoted(namespace.serviceName)}`,
    `!define HOST_DISPLAY_NAME ${quoted(namespace.displayName)}`,
    `!define HOST_PRODUCT_EN ${quoted(namespace.productNameEN)}`,
    `!define HOST_PRODUCT_ZH ${quoted(namespace.productNameZH)}`,
    `!define HOST_PRODUCT_JA ${quoted(namespace.productNameJA)}`,
    `!define HOST_PROFILE_ID ${quoted(seed.profileId)}`,
    `!define HOST_BUILD_ID ${quoted(seed.buildId)}`,
    `!define HOST_APP_VERSION ${quoted(seed.appVersion)}`,
    `!define HOST_FILE_VERSION ${quoted(fileVersion)}`,
    `!define HOST_APP_FILE_COUNT ${entries.length}`,
    `!define HOST_OUTFILE ${quoted(installer)}`,
    // Resource mutation is deliberately on the bare stub. makensis subsequently
    // appends its payload and calculates the real installer CRC; signing is last.
    `!packhdr ${quoted(stubPath)} '${packCommand.replaceAll("'", "$\\'")}'`,
    "",
  ].join("\n");
}

const CRC_TABLE = Array.from({ length: 256 }, (_, index) => {
  let value = index;
  for (let bit = 0; bit < 8; bit++) value = (value & 1) ? 0xedb88320 ^ (value >>> 1) : value >>> 1;
  return value >>> 0;
});
async function verifyInstallerCRC(file) {
  const handle = await fs.promises.open(file, "r");
  try {
    const size = (await handle.stat()).size;
    const block = Buffer.alloc(512);
    let offset = 0;
    let length;
    while (offset + 28 <= size) {
      const { bytesRead } = await handle.read(block, 0, 512, offset);
      if (bytesRead < 28) refuse("HOST_BUILD_NSIS_INVALID");
      if (block.readUInt32LE(4) === 0xdeadbeef && block.subarray(8, 20).toString("ascii") === "NullsoftInst") {
        const flags = block.readUInt32LE(0);
        length = block.readUInt32LE(24);
        if ((flags & ~15) || (flags & 4) || !(flags & 8) || length < 32
            || offset + length > size) refuse("HOST_BUILD_NSIS_CRC_INVALID");
        break;
      }
      offset += 512;
    }
    if (length === undefined || offset < 512) refuse("HOST_BUILD_NSIS_INVALID");
    // NSIS 3.11 fileform.c and build.cpp exclude the first 512 bytes (PE
    // checksum/security-directory fields), then stop at the stored CRC. A later
    // Authenticode certificate is outside that exact archive window.
    const end = offset + length - 4;
    let position = 512;
    let crc = 0xffffffff;
    const buffer = Buffer.alloc(65536);
    while (position < end) {
      const wanted = Math.min(buffer.length, end - position);
      const { bytesRead } = await handle.read(buffer, 0, wanted, position);
      if (bytesRead !== wanted) refuse();
      for (let i = 0; i < bytesRead; i++) crc = CRC_TABLE[(crc ^ buffer[i]) & 255] ^ (crc >>> 8);
      position += bytesRead;
    }
    const stored = Buffer.alloc(4);
    if ((await handle.read(stored, 0, 4, end)).bytesRead !== 4
        || ((crc ^ 0xffffffff) >>> 0) !== stored.readUInt32LE(0)) refuse("HOST_BUILD_NSIS_CRC_INVALID");
    return { firstHeaderOffset: offset, archiveBytes: length, crc32: stored.readUInt32LE(0) };
  } finally { await handle.close(); }
}

async function packageHostNSIS({ profile, appBinary, helper, launcher, resourceManifest,
  effectiveConfig, tauriRoot, outputParent, signer, makensis, environment = process.env,
  run = runBuildTool, native = nativeTool }) {
  validateHostApplicationConfig(profile, effectiveConfig);
  nsisFileVersion(profile.seed.appVersion); // Before any packaging effects.
  if (process.platform !== "win32") refuse("HOST_BUILD_PLATFORM_UNSUPPORTED");
  const certificatePolicy = imageCertificatePolicy(profile.seed);
  if (signer === undefined || signer === null) {
    if (certificatePolicy.kind === "legacy-selected-signer") refuse("HOST_BUILD_SIGNER_REQUIRED");
  } else if (typeof signer !== "string" || !path.isAbsolute(signer)
      || path.extname(signer).toLowerCase() !== ".ps1"
      || !fs.statSync(signer).isFile()) refuse("HOST_BUILD_SIGNER_REQUIRED");
  executable(makensis);
  if (effectiveConfig.bundle?.externalBin?.length || effectiveConfig.bundle?.fileAssociations?.length
      || effectiveConfig.plugins?.["deep-link"]) refuse("HOST_BUILD_MAPPING_UNSUPPORTED");
  assertHostRoleImage(helper);
  assertHostRoleImage(launcher);
  const declaredEntries = inventorySources({ appBinary, resourceManifest, tauriRoot });
  const root = fs.mkdtempSync(path.join(outputParent, "host-nsis-"));
  // Only the explicit Cargo output boundary accepts multiple source links.
  // Subsequent app inventory uses only this rehashed single-link snapshot.
  const appSnapshot = path.join(root, "cargo-app-input.exe");
  const inputEvidence = await snapshotCargoApp(appBinary, appSnapshot);
  fs.writeFileSync(path.join(root, "cargo-input-evidence.json"),
    `${JSON.stringify(inputEvidence, null, 2)}\n`, { flag: "wx", mode: 0o600 });
  const originalEntries = declaredEntries.map((entry) => entry.relativeName === "pinvou3-tauri.exe"
    ? { ...entry, source: appSnapshot } : entry);
  const logFile = path.join(root, "build-private.log");
  const options = { cwd: root, logFile, environment };
  const originalControlDirectory = path.join(root, "original-native-controls");
  fs.mkdirSync(originalControlDirectory);
  const originalControls = await writeNativeInventory(originalControlDirectory, originalEntries);
  await native("Inventory", ["-Inventory", originalControls.indexPath], options);
  const sign = async (role, image) => {
    const nativeRole = role === "helper" || role === "launcher";
    if (nativeRole) assertHostRoleImage(image);
    if (signer !== undefined && signer !== null) {
      // An explicitly configured signer failure remains a failure, never fallback.
      await run(POWERSHELL, ["-NoProfile", "-NonInteractive",
        "-ExecutionPolicy", "Bypass", "-File", signer, "-Role", role, "-Image", image], { ...options, recordOutput: false });
    }
    if (nativeRole) assertHostRoleImage(image);
  };
  const appRoot = path.join(root, "source", "app");
  fs.mkdirSync(appRoot, { recursive: true });
  const copied = [];
  for (const entry of originalEntries) {
    const observed = await hashFile(entry.source);
    const destination = path.join(appRoot, ...entry.relativeName.split("/"));
    fs.mkdirSync(path.dirname(destination), { recursive: true });
    fs.copyFileSync(entry.source, destination, fs.constants.COPYFILE_EXCL);
    if (JSON.stringify(await hashFile(destination)) !== JSON.stringify(observed)) refuse();
    copied.push({ ...entry, source: destination });
  }
  const helperCopy = path.join(root, "source", "pinvou-upgrade-helper.exe");
  const launcherCopy = path.join(root, "source", "pinvou-upgrade-launcher.exe");
  for (const [source, destination] of [[helper, helperCopy], [launcher, launcherCopy]]) {
    const before = await hashFile(source);
    fs.copyFileSync(source, destination, fs.constants.COPYFILE_EXCL);
    if (JSON.stringify(await hashFile(destination)) !== JSON.stringify(before)) refuse();
  }
  for (const [role, image] of [["helper", helperCopy], ["launcher", launcherCopy], ["app", path.join(appRoot, "pinvou3-tauri.exe")]]) {
    await sign(role, image);
    if (certificatePolicy.kind === "legacy-selected-signer") {
      await native("VerifyImage", ["-Image", image, "-ExpectedSigner",
        certificatePolicy.signerCertificateSha256], options);
    }
  }
  await checkHelperProfile(helperCopy, profile);
  const finalControlDirectory = path.join(root, "final-native-controls");
  fs.mkdirSync(finalControlDirectory);
  let finalControls;
  const actualResources = { files: copied.filter((x) => x.relativeName !== "pinvou3-tauri.exe")
    .map((x) => ({ source: path.relative(root, x.source), destination: x.relativeName, kind: "file" })) };
  const result = await buildInstallationManifest({ profile, helper: helperCopy, launcher: launcherCopy,
    appBinary: path.join(appRoot, "pinvou3-tauri.exe"), resourceManifest: actualResources, tauriRoot: root,
    nativeCheck: async (entries) => {
      finalControls = await writeNativeInventory(finalControlDirectory, entries);
      await native("Inventory", ["-Inventory", finalControls.indexPath], options);
    } });
  const manifestPath = path.join(root, "installation-manifest.json");
  fs.writeFileSync(manifestPath, result.bytes, { flag: "wx", mode: 0o600 });
  for (const shard of result.shards) {
    fs.writeFileSync(path.join(root, `installation-inventory-${shard.index}.json`), shard.bytes,
      { flag: "wx", mode: 0o600 });
  }
  const installer = path.join(root, "source", "installer.exe");
  const include = path.join(root, "host-build.generated.nsh");
  fs.writeFileSync(include, renderInclude({ profile, entries: result.entries, installer,
    manifestPath, stubPath: path.join(root, "header.tmp") }));
  await run(makensis, ["/NOCONFIG", "/V4", "/INPUTCHARSET", "UTF8", `/DPINVOU_HOST_BUILD_INCLUDE=${include}`, TEMPLATE], options);
  await native("Read", ["-Image", installer, "-Manifest", manifestPath], options);
  const crcBeforeSigning = await verifyInstallerCRC(installer);
  await sign("installer", installer);
  if (certificatePolicy.kind === "legacy-selected-signer") {
    await native("VerifyImage", ["-Image", installer, "-ExpectedSigner",
      certificatePolicy.signerCertificateSha256], options);
  }
  await native("Read", ["-Image", installer, "-Manifest", manifestPath], options);
  const crcAfterSigning = await verifyInstallerCRC(installer);
  if (JSON.stringify(crcBeforeSigning) !== JSON.stringify(crcAfterSigning)) refuse();
  // Rehash every final source role/resource after optional outer signing: a signer
  // or packaging step may not silently mutate bytes bound by the manifest.
  for (const [file, expected] of [[helperCopy, result.manifest.helper], [launcherCopy, result.manifest.launcher],
    ...result.entries.map((x, i) => [x.source, result.appFiles[i]])]) {
    const actual = await hashFile(file);
    if (actual.size !== expected.size || actual.sha256 !== expected.sha256) refuse();
  }
  const evidence = { hostBuildEvidenceVersion: 1, profileKind: profile.seed.profileKind, profileId: profile.seed.profileId,
    seedSha256: profile.seedSha256, manifestSha256: sha256(result.bytes),
    installer: await hashFile(installer), crc: crcAfterSigning,
    appInventory: result.manifest.appInventory,
    nativeControls: { original: originalControls.blocks, final: finalControls.blocks },
    initialTrustQualification: false, installationQualification: false };
  fs.writeFileSync(path.join(root, "build-evidence.json"), `${JSON.stringify(evidence, null, 2)}\n`, { mode: 0o600 });
  return { installer, sourceDirectory: path.dirname(installer), outputDirectory: root, manifestPath };
}

module.exports = { RESOURCE_SCRIPT, TEMPLATE, POWERSHELL, quoted, runBuildTool, nativeTool,
  checkHelperProfile, nsisFileVersion, renderInclude, verifyInstallerCRC, packageHostNSIS };
