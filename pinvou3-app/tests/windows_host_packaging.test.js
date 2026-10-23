const assert = require("node:assert/strict");
const test = require("node:test");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { pathToFileURL } = require("node:url");
const profileModule = require("../scripts/tauri/windows-host-profile.js");
const { canonicalBytes, validateSeed, loadBuildProfile, sha256, validateHostApplicationConfig } = profileModule;
const { relativeName, inventorySources, buildInstallationManifest, hashFile } = require("../scripts/tauri/windows-host-manifest.js");
const { writeNativeInventory } = require("../scripts/tauri/windows-host-native-inventory.js");
const { buildAppInventory } = require("../scripts/tauri/windows-host-inventory.js");
const { prepareHostBuild } = require("../scripts/tauri/windows-host-build.js");
const { TEMPLATE, nsisFileVersion, renderInclude, nativeTool, runBuildTool, verifyInstallerCRC, packageHostNSIS } = require("../scripts/tauri/windows-host-nsis.js");
const appRoot = path.resolve(__dirname, "..");
const repoRoot = path.dirname(appRoot);

function temporary(t) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "pinvou-host-packaging-test-"));
  t.after(() => {
    assert.equal(path.dirname(root), path.resolve(os.tmpdir()));
    assert.match(path.basename(root), /^pinvou-host-packaging-test-/u);
    fs.rmSync(root, { recursive: true });
  });
  return root;
}
test("source-structure guards retain identical main semantics for LF and CRLF", () => {
  const source = fs.readFileSync(path.join(appRoot, "scripts/tauri/build.js"), "utf8").replaceAll("\r\n", "\n");
  const pattern = /\nasync function main\(\) \{\n([\s\S]*?)\n\}\n/u;
  const lf = source.match(pattern);
  const crlf = source.replaceAll("\n", "\r\n").replaceAll("\r\n", "\n").match(pattern);
  assert.ok(lf); assert.ok(crlf); assert.equal(crlf[1], lf[1]);
});
async function profileFixture() {
  const { createFixtureSet } = await import(pathToFileURL(path.join(repoRoot,
    "specs/upgrade/v1/reference/tests/fixtures.mjs")).href);
  const rootBytes = Buffer.from(createFixtureSet().bytes.root);
  const seed = { hostSeedVersion: 1, profileKind: "lab", profileId: "contract-test",
    product: "pinvou", component: "app", targetKey: profileModule.TARGET,
    buildId: "b".repeat(64), appVersion: JSON.parse(fs.readFileSync(path.join(appRoot, "src-tauri/tauri.conf.json"))).version,
    layoutId: profileModule.LAYOUT, hostIpcVersion: 1, hostStateVersion: 1,
    originalRoot: { size: rootBytes.length, sha256: sha256(rootBytes) },
    approvedInitialPublisherPolicy: { policyId: "isolated-test-only", signerCertificateSha256: "c".repeat(64) } };
  return { seed, rootBytes, seedBytes: await canonicalBytes(seed) };
}

test("public Seed is canonical, closed, pinned and has an explicit host kind", async (t) => {
  const fixture = await profileFixture();
  const good = await validateSeed(fixture.seedBytes, fixture.rootBytes);
  assert.equal(good.seedSha256, sha256(fixture.seedBytes));
  for (const mutate of [
    (x) => { x.profileKind = "unknown"; }, (x) => { x.profileId = null; },
    (x) => { x.profileId = "../escape"; }, (x) => { x.extra = true; },
    (x) => { x.approvedInitialPublisherPolicy.policyId = null; },
    (x) => { x.originalRoot.sha256 = "d".repeat(64); },
    (x) => { x.hostStateVersion = 2; }, (x) => { x.appVersion = "01.2.3"; },
    (x) => { x.product = "another-product"; },
  ]) {
    const seed = JSON.parse(JSON.stringify(fixture.seed)); mutate(seed);
    await assert.rejects(validateSeed(await canonicalBytes(seed), fixture.rootBytes));
  }
  await assert.rejects(validateSeed(Buffer.concat([fixture.seedBytes, Buffer.from("\n")]), fixture.rootBytes));
  const duplicate = Buffer.from(fixture.seedBytes.toString().replace('{', '{"profileId":"first",'));
  await assert.rejects(validateSeed(duplicate, fixture.rootBytes));
  const changedRoot = Buffer.from(fixture.rootBytes); changedRoot[changedRoot.length - 2] ^= 1;
  await assert.rejects(validateSeed(fixture.seedBytes, changedRoot));
  const root = temporary(t);
  const seedPath = path.join(root, "seed.json"), rootPath = path.join(root, "root.json");
  fs.writeFileSync(seedPath, fixture.seedBytes); fs.writeFileSync(rootPath, fixture.rootBytes);
  assert.equal(await loadBuildProfile({ environment: {} }), null);
  await assert.rejects(loadBuildProfile({ environment: { PINVOU_HOST_BOOTSTRAP_SEED: seedPath } }));
  assert.equal((await loadBuildProfile({ environment: { PINVOU_HOST_BOOTSTRAP_SEED: seedPath,
    PINVOU_HOST_INITIAL_ROOT: rootPath } })).seed.profileId, fixture.seed.profileId);
});

test("certificate-optional Seed V2 is explicit, closed and preserves original Root pinning", async () => {
  const fixture = await profileFixture();
  const optional = structuredClone(fixture.seed);
  optional.hostSeedVersion = 2;
  delete optional.approvedInitialPublisherPolicy;
  optional.codeImagePolicy = { policyVersion: 1, certificate: "optional" };
  const bytes = await canonicalBytes(optional);
  const validated = await validateSeed(bytes, fixture.rootBytes);
  assert.deepEqual(profileModule.imageCertificatePolicy(validated.seed), { kind: "certificate-optional" });
  assert.deepEqual(validated.seedBytes, bytes);
  const legacy = await validateSeed(fixture.seedBytes, fixture.rootBytes);
  assert.deepEqual(legacy.seedBytes, fixture.seedBytes);
  assert.deepEqual(profileModule.imageCertificatePolicy(legacy.seed), {
    kind: "legacy-selected-signer", signerCertificateSha256: "c".repeat(64),
  });
  for (const mutate of [
    (x) => { x.hostSeedVersion = 1; },
    (x) => { x.hostSeedVersion = 3; },
    (x) => { delete x.codeImagePolicy; },
    (x) => { x.codeImagePolicy = null; },
    (x) => { x.approvedInitialPublisherPolicy = fixture.seed.approvedInitialPublisherPolicy; },
    (x) => { x.codeImagePolicy.extra = true; },
    (x) => { x.codeImagePolicy.policyVersion = 2; },
    (x) => { x.codeImagePolicy.certificate = "required"; },
    (x) => { x.originalRoot.sha256 = "e".repeat(64); },
  ]) {
    const changed = structuredClone(optional); mutate(changed);
    await assert.rejects(validateSeed(await canonicalBytes(changed), fixture.rootBytes));
  }
  const changedLegacy = structuredClone(fixture.seed);
  changedLegacy.codeImagePolicy = optional.codeImagePolicy;
  await assert.rejects(validateSeed(await canonicalBytes(changedLegacy), fixture.rootBytes));
  delete changedLegacy.codeImagePolicy;
  delete changedLegacy.approvedInitialPublisherPolicy;
  await assert.rejects(validateSeed(await canonicalBytes(changedLegacy), fixture.rootBytes));
});

test("optional certificate plans omit the signer while legacy and configured inputs stay strict", async (t) => {
  const root = temporary(t), fixture = await profileFixture();
  const optional = structuredClone(fixture.seed);
  optional.hostSeedVersion = 2;
  delete optional.approvedInitialPublisherPolicy;
  optional.codeImagePolicy = { policyVersion: 1, certificate: "optional" };
  const environment = {};
  for (const [key, file, bytes] of [
    ["PINVOU_HOST_BOOTSTRAP_SEED", "seed.json", await canonicalBytes(optional)],
    ["PINVOU_HOST_INITIAL_ROOT", "root.json", fixture.rootBytes],
    ["PINVOU_HOST_HELPER_BINARY", "helper.exe", "helper model only, never executed"],
    ["PINVOU_HOST_LAUNCHER_BINARY", "launcher.exe", "launcher model only, never executed"],
    ["PINVOU_HOST_MAKENSIS", "makensis.exe", "tool model only, never executed"],
  ]) { const target = path.join(root, file); fs.writeFileSync(target, bytes); environment[key] = target; }
  const options = { appRoot, environment, platform: "win32", architecture: "x64" };
  const args = ["build", "--bundles", "nsis"];
  assert.equal((await prepareHostBuild(args, options)).signer, null);
  environment.PINVOU_HOST_SIGNER_SCRIPT = "";
  await assert.rejects(prepareHostBuild(args, options));
  const signer = path.join(root, "sign.ps1");
  fs.writeFileSync(signer, "# explicit optional signer, never executed by this test");
  environment.PINVOU_HOST_SIGNER_SCRIPT = signer;
  assert.equal((await prepareHostBuild(args, options)).signer, signer);
  delete environment.PINVOU_HOST_SIGNER_SCRIPT;
  fs.writeFileSync(environment.PINVOU_HOST_BOOTSTRAP_SEED, fixture.seedBytes);
  await assert.rejects(prepareHostBuild(args, options));
  environment.PINVOU_HOST_SIGNER_SCRIPT = signer;
  assert.equal((await prepareHostBuild(args, options)).signer, signer);
});

test("enabled wrapper closes routes and preserves the default-off argument path", async (t) => {
  assert.equal(await prepareHostBuild(["build", "--bundles", "msi"], { appRoot, environment: {} }), null);
  const root = temporary(t), fixture = await profileFixture();
  const environment = {};
  for (const [key, file, bytes] of [
    ["PINVOU_HOST_BOOTSTRAP_SEED", "seed.json", fixture.seedBytes],
    ["PINVOU_HOST_INITIAL_ROOT", "root.json", fixture.rootBytes],
    ["PINVOU_HOST_HELPER_BINARY", "helper.exe", "helper"],
    ["PINVOU_HOST_LAUNCHER_BINARY", "launcher.exe", "launcher"],
    ["PINVOU_HOST_SIGNER_SCRIPT", "sign.ps1", "# explicit external build signer"],
    ["PINVOU_HOST_MAKENSIS", "makensis.exe", "compiler"],
  ]) { const target = path.join(root, file); fs.writeFileSync(target, bytes); environment[key] = target; }
  const options = { appRoot, environment, platform: "win32", architecture: "x64" };
  const plan = await prepareHostBuild(["build", "--bundles", "nsis"], options);
  assert.deepEqual(plan.args, ["build", "--target", "x86_64-pc-windows-msvc", "--features", "upgrade-host-lab", "--no-bundle"]);
  assert.match(plan.appBinary, /x86_64-pc-windows-msvc[\\/]release[\\/]pinvou3-tauri\.exe$/u);
  for (const args of [["build"], ["bundle", "--bundles", "nsis"], ["build", "--bundles", "msi"],
    ["build", "--bundles", "nsis", "--profile", "other"], ["build", "--bundles", "nsis", "--", "--anything"],
    ["build", "--bundles", "nsis", "--target", "i686-pc-windows-msvc"]]) {
    await assert.rejects(prepareHostBuild(args, options));
  }
  await assert.rejects(prepareHostBuild(["build", "--bundles", "nsis"], { ...options, platform: "linux" }));
});

test("actual composed app identity is bound before compilation and packaging", async (t) => {
  const { composeEffectiveConfig } = require("../scripts/tauri/effective-config.js");
  const { prepareTauriArgs, configSpecs } = require("../scripts/tauri/build.js");
  const fixture = await profileFixture();
  const profile = await validateSeed(fixture.seedBytes, fixture.rootBytes);
  const compose = (patch) => composeEffectiveConfig(configSpecs(prepareTauriArgs(
    ["build", "--target", "x86_64-pc-windows-msvc", "--no-bundle", "--config", JSON.stringify(patch)],
    { platform: "win32", architecture: "x64", stageRuntime: () => null },
  ))).effectiveConfig;
  // Tauri's omitted override uses this repository's actual Cargo main output,
  // independently of a presentation productName or a resource-only overlay.
  const cargo = fs.readFileSync(path.join(appRoot,"src-tauri/Cargo.toml"),"utf8");
  assert.match(cargo, /^default-run = "pinvou3-tauri"$/mu);
  assert.match(cargo, /\[\[bin\]\]\s+name = "pinvou3-tauri"\s+path = "src\/main.rs"/u);
  for (const patch of [{}, { mainBinaryName: null }, { mainBinaryName: "pinvou3-tauri" },
    { productName: "Independent presentation", bundle: { resources: { "test-file": "resources/test-file" } } }]) {
    const config = compose(patch);
    assert.doesNotThrow(() => validateHostApplicationConfig(profile,config));
    assert.equal(config.version,profile.seed.appVersion);
  }
  const root=temporary(t), stale=path.join(root,"pinvou3-tauri.exe");
  fs.writeFileSync(stale,"preexisting stale output must never be selected");
  let calls=0;
  for (const patch of [{version:"99.0.0"},{version:null},{mainBinaryName:"alternate-app"},
    {mainBinaryName:"pinvou3-tauri.exe"},{mainBinaryName:""}]) {
    const effectiveConfig=compose(patch);
    assert.throws(()=>validateHostApplicationConfig(profile,effectiveConfig),/HOST_BUILD_APPLICATION_IDENTITY_INVALID/u);
    await assert.rejects(packageHostNSIS({profile,effectiveConfig,appBinary:stale,
      run:async()=>{calls++;},native:async()=>{calls++;}}),/HOST_BUILD_APPLICATION_IDENTITY_INVALID/u);
  }
  assert.equal(calls,0);
  assert.equal(fs.readFileSync(stale,"utf8"),"preexisting stale output must never be selected");
  const source=fs.readFileSync(path.join(appRoot,"scripts/tauri/build.js"),"utf8").replaceAll("\r\n","\n");
  const body=source.match(/\nasync function main\(\) \{\n([\s\S]*?)\n\}\n/u)[1];
  const preliminary=body.indexOf("composeEffectiveConfig(configSpecs(identityArgs)).effectiveConfig");
  assert.ok(preliminary>body.indexOf("if (hostBuild) {"));
  assert.ok(preliminary<body.indexOf("await ensureWindowsRustToolchain()"));
  assert.ok(preliminary<body.indexOf("prepareKnowledgeHost("));
  const finalCheck=body.indexOf("validateHostApplicationConfig(hostBuild.profile, artifacts.effectiveConfig)");
  assert.ok(finalCheck>body.indexOf("artifacts = writeEffectiveArtifacts("));
  assert.ok(finalCheck<body.indexOf("await runTauri("));
});

test("complete inventory preserves empty files, UTF8 order and all exact resources", async (t) => {
  const root = temporary(t), fixture = await profileFixture();
  const profile = await validateSeed(fixture.seedBytes, fixture.rootBytes);
  const image = path.join(root, "image.exe"); fs.writeFileSync(image, "synthetic pure file, not a signed role");
  const resourceManifest = { files: [] };
  for (const name of ["empty", "\ue000", "\u{10000}"]) {
    const file = path.join(root, name);fs.writeFileSync(file, name === "empty" ? "" : name);
    resourceManifest.files.push({ source: name, destination: `resources/${name}`, kind: "file" });
  }
  let checked = 0;
  const result = await buildInstallationManifest({ profile, helper: image, launcher: image, appBinary: image,
    resourceManifest, tauriRoot: root, nativeCheck: async (entries) => { checked = entries.length; } });
  assert.equal(checked, 4); assert.equal(result.appFiles.length, 4);
  assert.equal(result.manifest.hostManifestVersion, 2);
  assert.equal(Object.hasOwn(result.manifest, "appFiles"), false);
  const empty = result.appFiles.find((x) => x.relativeName === "resources/empty");
  assert.equal(empty.size, 0); assert.equal(empty.sha256, sha256(Buffer.alloc(0)));
  assert.deepEqual(result.appFiles.map((x) => x.relativeName),
    ["pinvou3-tauri.exe", "resources/empty", "resources/\ue000", "resources/\u{10000}"]);
  assert.equal(result.manifest.app.sha256, result.appFiles[0].sha256);
  await assert.rejects(buildInstallationManifest({ profile, helper: image, launcher: image, appBinary: image,
    resourceManifest, tauriRoot: root }));
  const hardlink = path.join(root, "second-link.exe"); fs.linkSync(image, hardlink);
  await assert.rejects(hashFile(image)); fs.unlinkSync(hardlink);
  assert.equal((await hashFile(image)).size, fs.statSync(image).size);
  for (const bad of ["../x", "x\\y", "/x", "x:y", "CON.txt", "conin$.txt", "CONOUT$", "x/Com\u00b9.txt", "LPT\u00b2", "x/..", "x.", "x ", "a//b", "a\0b", "a\x7fb"]) {
    assert.throws(() => relativeName(bad));
  }
  for (const good of ["ordinary$.txt", "quote's.txt", "\u65e5\u672c\u8a9e/file", "literal${name}.txt"]) assert.equal(relativeName(good), good);
  assert.throws(() => inventorySources({ appBinary: image, tauriRoot: root,
    resourceManifest: { files: [{ source: "image.exe", destination: "PINVOU3-TAURI.EXE", kind: "file" }] } }));
  assert.throws(() => inventorySources({ appBinary: image, tauriRoot: root,
    resourceManifest: { files: [{ source: "image.exe", destination: "valid", kind: "symlink" }] } }));
  const maximumResources = Array.from({ length: 32767 }, (_, i) =>
    ({ source: "image.exe", destination: `resource-${i}`, kind: "file" }));
  assert.equal(inventorySources({ appBinary: image, tauriRoot: root,
    resourceManifest: { files: maximumResources } }).length, 32768);
  assert.throws(() => inventorySources({ appBinary: image, tauriRoot: root,
    resourceManifest: { files: [...maximumResources, { source: "image.exe", destination: "extra", kind: "file" }] } }));
});

test("enabled source contains only fixed native gates and launcher presentation", () => {
  const source = fs.readFileSync(TEMPLATE, "utf8");
  assert.doesNotMatch(source, /\b(?:System|nsExec|nsDialogs|LangDLL|INetC|nsis_tauri_utils)::|InitPluginsDir|\$PLUGINSDIR|WriteUninstaller|taskkill|UninstallString.*Exec|^\s*(?:File|SetOutPath|CreateDirectory|RMDir)\s/im);
  assert.match(source, /CRCCheck force/u);
  assert.ok(source.indexOf("!insertmacro HostCall prepare-install") < source.indexOf("!insertmacro HostCall verify-files"));
  assert.ok(source.indexOf("!insertmacro HostCall start-service") < source.indexOf("CreateShortcut"));
  assert.match(source, /launcher\.exe/u); assert.match(source, /\/HOST_UNINSTALL/u);
  for (const language of ["1033", "2052", "1041"]) assert.match(source, new RegExp(`LangString HostFailure ${language}`));
});

test("local initial source entry is explicit and presentation is not business silent", async () => {
  const fixture = await profileFixture();
  const legacy = await validateSeed(fixture.seedBytes, fixture.rootBytes);
  const value = structuredClone(fixture.seed);
  value.hostSeedVersion = 2;
  delete value.approvedInitialPublisherPolicy;
  value.codeImagePolicy = { policyVersion: 1, certificate: "optional" };
  const local = await validateSeed(await canonicalBytes(value), fixture.rootBytes);
  const options = { entries: [{ relativeName: "pinvou3-tauri.exe" }], installer: path.resolve("installer.exe"),
    manifestPath: path.resolve("installation-manifest.json"), stubPath: path.resolve("header.tmp") };
  assert.match(renderInclude({ ...options, profile: legacy }), /!define HOST_SEED_VERSION 1/u);
  assert.match(renderInclude({ ...options, profile: local }), /!define HOST_SEED_VERSION 2/u);
  const source = fs.readFileSync(TEMPLATE, "utf8").replaceAll("\r\n", "\n");
  assert.match(source, /!if \$\{HOST_SEED_VERSION\} == 2\s+[\s\S]*?!insertmacro HostCall prepare-local-install\s+!else\s+!insertmacro HostCall prepare-install/u);
  assert.ok(source.indexOf("!insertmacro HostCall prepare-local-install") < source.indexOf("!insertmacro HostCall verify-files"));
  assert.match(source, /StrCmp \$1 "\/S" host_argument_silent/u);
  assert.match(source, /StrCmp \$1 "\/P" host_argument_passive/u);
  assert.doesNotMatch(source, /HostCall prepare-metadata-upgrade/u);
  // Source shape/closed routing only; actual parent, token, intent and SCM
  // acceptance need the real Windows cases, not these ordinary parsed values.
});

test("NSIS numeric versions map canonical uint16 words before native build effects", async (t) => {
  for (const [value, expected] of [["0.10.13", "0.10.13.0"], ["0.0.0", "0.0.0.0"],
    ["65535.65535.65535", "65535.65535.65535.0"]]) assert.equal(nsisFileVersion(value), expected);
  for (const value of [undefined, null, 13, ["0.10.13"], "01.2.3", "1.02.3", "1.2.03",
    "1.2", "1.2.3.0", "65536.0.0", "0.65536.0", "0.0.65536", "9007199254740993.0.0",
    "-1.2.3", "+1.2.3", " 1.2.3", "1.2.3\n", "1.2.3\r\n", "1.2.3\0", "1.2.$"])
    assert.throws(() => nsisFileVersion(value), /HOST_BUILD_NSIS_VERSION_UNSUPPORTED/u);
  const fixture = await profileFixture();
  const input = { profile: fixture, entries: [{ relativeName: "pinvou3-tauri.exe" }],
    installer: path.resolve("installer.exe"), manifestPath: path.resolve("manifest.json"), stubPath: path.resolve("stub.tmp") };
  const include = renderInclude(input);
  assert.ok(include.includes('!define HOST_APP_VERSION "0.10.13"'));
  assert.ok(include.includes('!define HOST_FILE_VERSION "0.10.13.0"'));
  assert.throws(() => renderInclude({ ...input, profile: { seed: { ...fixture.seed, appVersion: "65536.0.0" } } }), /HOST_BUILD_NSIS_VERSION_UNSUPPORTED/u);
  // The shared Seed grammar remains unchanged; this adapter's narrower physical
  // bound must reject before it looks for helper/compiler/signer inputs.
  const root = temporary(t), app = path.join(root, "app");
  fs.mkdirSync(path.join(app, "src-tauri"), { recursive: true });
  fs.writeFileSync(path.join(app, "src-tauri/tauri.conf.json"), JSON.stringify({ version: "65536.0.0" }));
  const seed = structuredClone(fixture.seed); seed.appVersion = "65536.0.0";
  const seedPath = path.join(root, "seed.json"), rootPath = path.join(root, "root.json");
  fs.writeFileSync(seedPath, await canonicalBytes(seed)); fs.writeFileSync(rootPath, fixture.rootBytes);
  await assert.rejects(prepareHostBuild(["build", "--bundles", "nsis"], { appRoot: app,
    platform: "win32", architecture: "x64", environment: { PINVOU_HOST_BOOTSTRAP_SEED: seedPath,
      PINVOU_HOST_INITIAL_ROOT: rootPath } }), /HOST_BUILD_NSIS_VERSION_UNSUPPORTED/u);
  const template = fs.readFileSync(TEMPLATE, "utf8").replaceAll("\r\n", "\n");
  assert.match(template, /^VIProductVersion "\$\{HOST_FILE_VERSION\}"$/mu);
  assert.match(template, /^VIFileVersion "\$\{HOST_FILE_VERSION\}"$/mu);
  for (const language of ["1033", "2052", "1041"]) for (const name of ["FileVersion", "ProductVersion"])
    assert.ok(template.includes(`VIAddVersionKey /LANG=${language} "${name}" "\${HOST_FILE_VERSION}"`));
});

test("actual NSIS stub resource and CRC; SDK ordinal alias refusal", {
  skip: process.platform !== "win32" || !process.env.PINVOU_HOST_TEST_NSIS,
}, async (t) => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "pinvou-host-native-$'\u65e5\u672c\u8a9e-"));
  t.diagnostic(`Retained build-only native artifacts: ${root}`);
  const fixture = await profileFixture();
  // Copy the exact build adapters to this legal metacharacter/Unicode directory
  // so both !packhdr's tool path and its inputs exercise actual argument parsing.
  for (const name of ["windows-host-profile.js","windows-host-manifest.js","windows-host-inventory.js","windows-host-native-inventory.js","windows-host-nsis.js","windows-host-role-pe.js","windows-host-build-input.js","windows-host-resource.ps1"]) {
    fs.copyFileSync(path.join(appRoot,"scripts/tauri",name),path.join(root,name));
  }
  const special = require(path.join(root,"windows-host-nsis.js"));
  const source = path.join(root, "file with space.txt"); fs.writeFileSync(source, "compiler-only fixture");
  const options = { cwd: root, logFile: path.join(root, "private.log") };
  const profile=await validateSeed(fixture.seedBytes,fixture.rootBytes);
  const result=await buildInstallationManifest({profile,helper:source,launcher:source,appBinary:source,
    resourceManifest:{files:[]},tauriRoot:root,nativeCheck:async()=>{}});
  const manifestPath = path.join(root, "resource.json");fs.writeFileSync(manifestPath,result.bytes);
  for(const shard of result.shards)fs.writeFileSync(path.join(root,`installation-inventory-${shard.index}.json`),shard.bytes);
  const installer = path.join(root, "not-for-execution.exe"), include = path.join(root, "build.nsh");
  const input = { profile: fixture, entries: [{ source, relativeName: "file $'\u65e5\u672c\u8a9e.txt" }],
    installer, manifestPath, stubPath: path.join(root, "stub.tmp") };
  assert.throws(()=>special.renderInclude({...input,manifestPath:path.join(root,"${unsafe}.json")}));
  fs.writeFileSync(include, special.renderInclude({ profile: fixture, entries: [{ source, relativeName: "file $'\u65e5\u672c\u8a9e.txt" }],
    installer, manifestPath, stubPath: path.join(root, "stub.tmp") }));
  await runBuildTool(process.env.PINVOU_HOST_TEST_NSIS, ["/NOCONFIG", "/V4", "/INPUTCHARSET", "UTF8", `/DPINVOU_HOST_BUILD_INCLUDE=${include}`, TEMPLATE], options);
  // Read the actual compiler output's numeric resource, never run the image.
  const versionProbe = path.join(root, "read-fixed-version.ps1");
  fs.writeFileSync(versionProbe, `param([string]$Image,[int]$Major,[int]$Minor,[int]$Patch)
$ErrorActionPreference='Stop'
$v=[Diagnostics.FileVersionInfo]::GetVersionInfo($Image)
if ($v.FileMajorPart -ne $Major -or $v.FileMinorPart -ne $Minor -or $v.FileBuildPart -ne $Patch -or $v.FilePrivatePart -ne 0 -or $v.ProductMajorPart -ne $Major -or $v.ProductMinorPart -ne $Minor -or $v.ProductBuildPart -ne $Patch -or $v.ProductPrivatePart -ne 0) { throw 'FIXED_VERSION_MISMATCH' }
$expected="$Major.$Minor.$Patch.0"
if ($v.FileVersion -ne $expected -or $v.ProductVersion -ne $expected) { throw 'VERSION_STRING_MISMATCH' }
`);
  const [major, minor, patch] = fixture.seed.appVersion.split(".");
  const observeVersion = (expectedMajor) => runBuildTool(special.POWERSHELL,
    ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", versionProbe, "-Image", installer,
      "-Major", String(expectedMajor), "-Minor", minor, "-Patch", patch], options);
  await observeVersion(major);
  await assert.rejects(observeVersion((Number(major) + 1) % 65536));
  await observeVersion(major);
  await special.nativeTool("Read", ["-Image", installer, "-Manifest", manifestPath], options);
  await assert.rejects(nativeTool("Embed", ["-Image", installer, "-Manifest", manifestPath], options));
  await nativeTool("Read", ["-Image", installer, "-Manifest", manifestPath], options);
  const valid = await verifyInstallerCRC(installer);assert.ok(valid.firstHeaderOffset > 512);
  const changed = path.join(root, "corrupt.exe");fs.copyFileSync(installer, changed);
  const fd=fs.openSync(changed,"r+");const byte=Buffer.alloc(1);fs.readSync(fd,byte,0,1,valid.firstHeaderOffset+30);byte[0]^=1;fs.writeSync(fd,byte,0,1,valid.firstHeaderOffset+30);fs.closeSync(fd);
  await assert.rejects(verifyInstallerCRC(changed));
  const wrong=path.join(root,"wrong.json");fs.writeFileSync(wrong,'{"compilerOnly":false}');
  await assert.rejects(nativeTool("Read",["-Image",installer,"-Manifest",wrong],options));
  let controlNumber=0;
  const inventory=async(items)=>{
    const directory=path.join(root,`controls-${controlNumber++}`);fs.mkdirSync(directory);
    return (await writeNativeInventory(directory,items)).indexPath;
  };
  let inventoryPath=await inventory([{relativeName:"\u00c4.txt",source},{relativeName:"different.txt",source}]);
  await nativeTool("Inventory",["-Inventory",inventoryPath],options);
  inventoryPath=await inventory([{relativeName:"\u00c4.txt",source},{relativeName:"\u00e4.txt",source}]);
  await assert.rejects(nativeTool("Inventory",["-Inventory",inventoryPath],options));
  const alternate=path.join(root,"alternate-language.exe");fs.copyFileSync(installer,alternate);
  const alter=path.join(root,"alter-resource.ps1");
  fs.writeFileSync(alter, `param([string]$Image,[string]$Manifest)\n$ErrorActionPreference='Stop'\nAdd-Type @'\nusing System;using System.Runtime.InteropServices;\npublic static class ResourceFault {\n[DllImport("kernel32.dll",CharSet=CharSet.Unicode)]static extern IntPtr BeginUpdateResourceW(string p,bool d);\n[DllImport("kernel32.dll")]static extern bool UpdateResourceW(IntPtr h,IntPtr t,IntPtr n,ushort l,byte[] b,uint s);\n[DllImport("kernel32.dll")]static extern bool EndUpdateResourceW(IntPtr h,bool d);\npublic static void Add(string p,byte[] b){IntPtr h=BeginUpdateResourceW(p,false);if(h==IntPtr.Zero)throw new Exception();if(!UpdateResourceW(h,(IntPtr)10,(IntPtr)34101,1033,b,(uint)b.Length)){EndUpdateResourceW(h,true);throw new Exception();}if(!EndUpdateResourceW(h,false))throw new Exception();}}\n'@\n[ResourceFault]::Add($Image,[IO.File]::ReadAllBytes($Manifest))\n`);
  const { POWERSHELL } = require("../scripts/tauri/windows-host-nsis.js");
  await runBuildTool(POWERSHELL,["-NoProfile","-NonInteractive","-File",alter,"-Image",alternate,"-Manifest",manifestPath],options);
  await assert.rejects(nativeTool("Read",["-Image",alternate,"-Manifest",manifestPath],options));
  fs.copyFileSync(installer,alternate);
  await nativeTool("Read",["-Image",alternate,"-Manifest",manifestPath],options);
  inventoryPath=await inventory([{relativeName:"Dir",source},{relativeName:"dir/file",source}]);
  await assert.rejects(nativeTool("Inventory",["-Inventory",inventoryPath],options));
  for (const names of [["a","a-","a/file"],["A","a-","a/file"],["\u00c4","\u00c4-","\u00e4/file"]]) {
    const positive=[names[0],names[1],"b/file"];
    inventoryPath=await inventory(positive.map((relativeName)=>({relativeName,source})));
    await nativeTool("Inventory",["-Inventory",inventoryPath],options);
    inventoryPath=await inventory(names.map((relativeName)=>({relativeName,source})));
    await assert.rejects(nativeTool("Inventory",["-Inventory",inventoryPath],options));
    inventoryPath=await inventory(positive.map((relativeName)=>({relativeName,source})));
    await nativeTool("Inventory",["-Inventory",inventoryPath],options);
  }
  // None of these compiler outputs is executed or treated as a trusted image.
});

test("actual image trust uses the verifier's native module despite inherited search paths", {
  skip: process.platform !== "win32" || !process.env.PINVOU_HOST_TEST_SIGNED_IMAGE,
}, async (t) => {
  const image = process.env.PINVOU_HOST_TEST_SIGNED_IMAGE;
  const signer = process.env.PINVOU_HOST_TEST_SIGNER_SHA256;
  assert.ok(path.isAbsolute(image));
  assert.match(signer || "", /^[0-9a-f]{64}$/u);
  const before = await hashFile(image, { allowEmpty: false });
  const root = temporary(t), modules = path.join(root, "empty-module-search");
  fs.mkdirSync(modules);
  const options = { cwd: root, logFile: path.join(root, "native-image.log"),
    environment: { ...process.env, PSModulePath: modules } };
  const verify = (file, expected) => nativeTool("VerifyImage", ["-Image", file, "-ExpectedSigner", expected], options);
  await verify(image, signer);
  const wrong = (signer[0] === "0" ? "1" : "0") + signer.slice(1);
  await assert.rejects(verify(image, wrong));
  const unsigned = path.join(root, "unsigned-not-for-execution.exe");
  fs.writeFileSync(unsigned, "ordinary unsigned input, never executed");
  await assert.rejects(verify(unsigned, signer));
  await verify(image, signer);
  assert.deepEqual(await hashFile(image, { allowEmpty: false }), before);
  // Real native Valid + exact leaf, then real refusals and restored positive.
  // No signing, trust-store changes, verifier injection or image execution.
});

test("actual multi-page resources, canonical framing and cross-control-block native refusals", {
  skip: process.platform !== "win32" || !process.env.PINVOU_HOST_TEST_NSIS,
}, async (t) => {
  const root=fs.mkdtempSync(path.join(os.tmpdir(),"pinvou-host-pages-native-"));
  t.diagnostic(`Retained build-only page controls: ${root}`);
  const fixture=await profileFixture(),profile=await validateSeed(fixture.seedBytes,fixture.rootBytes);
  const ordinary=path.join(root,"ordinary-not-a-role.txt");fs.writeFileSync(ordinary,"compile-only page validation");
  const base=await buildInstallationManifest({profile,helper:ordinary,launcher:ordinary,appBinary:ordinary,
    resourceManifest:{files:[]},tauriRoot:root,nativeCheck:async()=>{}});
  const main=base.appFiles[0],empty=sha256(Buffer.alloc(0));
  const files=[main,...Array.from({length:8192},(_,i)=>({relativeName:`z-${String(i).padStart(5,"0")}`,size:0,sha256:empty}))];
  const pages=await buildAppInventory(profile,files);
  const original={...base.manifest,appInventory:pages.appInventory};
  const options={cwd:root,logFile:path.join(root,"private-native-pages.log")};
  const stub=path.join(path.dirname(process.env.PINVOU_HOST_TEST_NSIS),"Stubs/zlib-x86-unicode");
  let sequence=0;
  async function bundle(manifest=original,shards=pages.shards.map(x=>x.bytes)) {
    const directory=path.join(root,`case-${sequence++}`);fs.mkdirSync(directory);
    const manifestPath=path.join(directory,"installation-manifest.json");
    fs.writeFileSync(manifestPath,Buffer.isBuffer(manifest)?manifest:await canonicalBytes(manifest));
    shards.forEach((bytes,index)=>{if(bytes!==null)fs.writeFileSync(path.join(directory,`installation-inventory-${index}.json`),bytes);});
    const image=path.join(directory,"bare-stub-not-for-execution.exe");fs.copyFileSync(stub,image);
    return {directory,manifestPath,image};
  }
  async function embed(value) { return nativeTool("Embed",["-Image",value.image,"-Manifest",value.manifestPath],options); }
  async function rejectUnchanged(value) {
    const before=await hashFile(value.image);await assert.rejects(embed(value));assert.deepEqual(await hashFile(value.image),before);
  }
  const positive=await bundle();await embed(positive);
  await nativeTool("Read",["-Image",positive.image,"-Manifest",positive.manifestPath],options);
  const resourceFault=path.join(root,"resource-faults.ps1");
  fs.writeFileSync(resourceFault,`param([string]$Directory,[string]$Image)\n$ErrorActionPreference='Stop'\nAdd-Type @'\nusing System;using System.IO;using System.Runtime.InteropServices;\npublic static class PageFault {\n[DllImport("kernel32.dll",CharSet=CharSet.Unicode)]static extern IntPtr BeginUpdateResourceW(string p,bool d);\n[DllImport("kernel32.dll")]static extern bool UpdateResourceW(IntPtr h,IntPtr t,IntPtr n,ushort l,byte[] b,uint s);\n[DllImport("kernel32.dll")]static extern bool EndUpdateResourceW(IntPtr h,bool d);\nstatic void Put(IntPtr h,int id,ushort lang,byte[] b){if(!UpdateResourceW(h,(IntPtr)10,(IntPtr)id,lang,b,b==null?0:(uint)b.Length))throw new Exception();}\npublic static void Make(string source,string directory,string mode){string target=Path.Combine(directory,mode+".exe");File.Copy(source,target);IntPtr h=BeginUpdateResourceW(target,false);if(h==IntPtr.Zero)throw new Exception();bool ended=false;try{byte[] a=File.ReadAllBytes(Path.Combine(directory,"installation-inventory-0.json")),b=File.ReadAllBytes(Path.Combine(directory,"installation-inventory-1.json"));switch(mode){case "unused":Put(h,34109,0,a);break;case "language":Put(h,34102,1033,a);break;case "missing":Put(h,34102,0,null);break;case "swapped":Put(h,34102,0,b);Put(h,34103,0,a);break;case "tampered":a[a.Length-2]^=1;Put(h,34102,0,a);break;default:throw new Exception();}ended=true;if(!EndUpdateResourceW(h,false))throw new Exception();}finally{if(!ended)EndUpdateResourceW(h,true);}}}\n'@\nforeach($mode in @('unused','language','missing','swapped','tampered')){[PageFault]::Make($Image,$Directory,$mode)}\n`);
  const { POWERSHELL }=require("../scripts/tauri/windows-host-nsis.js");
  await runBuildTool(POWERSHELL,["-NoProfile","-NonInteractive","-File",resourceFault,
    "-Directory",positive.directory,"-Image",positive.image],options);
  for(const mode of ["unused","language","missing","swapped","tampered"]) {
    await assert.rejects(nativeTool("Read",["-Image",path.join(positive.directory,`${mode}.exe`),"-Manifest",positive.manifestPath],options));
  }
  await nativeTool("Read",["-Image",positive.image,"-Manifest",positive.manifestPath],options);
  for(const mutate of [
    x=>{x.hostManifestVersion=1;},x=>{x.appInventory.fileCount++;},x=>{x.appInventory.totalSize++;},
    x=>{x.appInventory.shards[1].index=0;},x=>{x.appInventory.shards[1].resourceId=34102;},
    x=>{x.appInventory.shards[0].fileCount--;},
  ]) { const manifest=structuredClone(original);mutate(manifest);await rejectUnchanged(await bundle(manifest)); }
  const missing=pages.shards.map(x=>x.bytes);missing[1]=null;await rejectUnchanged(await bundle(original,missing));
  const swapped=pages.shards.map(x=>x.bytes);[swapped[0],swapped[1]]=[swapped[1],swapped[0]];
  await rejectUnchanged(await bundle(original,swapped));
  for(const mutate of [x=>{x.profileId="wrong-scope";},x=>{x.index=1;},x=>{x.files.reverse();}]) {
    const shards=pages.shards.map(x=>x.bytes),value=JSON.parse(shards[0]);mutate(value);
    shards[0]=await canonicalBytes(value);const manifest=structuredClone(original);
    manifest.appInventory.shards[0].size=shards[0].length;manifest.appInventory.shards[0].sha256=sha256(shards[0]);
    await rejectUnchanged(await bundle(manifest,shards));
  }
  for(const bytes of [Buffer.concat([await canonicalBytes(original),Buffer.from("\n")]),
    Buffer.from((await canonicalBytes(original)).toString().replace('{','{"app":null,')),
    Buffer.from((await canonicalBytes(original)).toString().replace('"lab"','"\\u006cab"'))]) {
    await rejectUnchanged(await bundle(bytes));
  }
  // Native comparison must cover the whole block aggregate, not each block in
  // isolation. All sources here are the same ordinary test file, not host roles.
  let controlSequence=0;
  async function controls(names) {
    const directory=path.join(root,`controls-${controlSequence++}`);fs.mkdirSync(directory);
    const result=await writeNativeInventory(directory,names.map(relativeName=>({relativeName,source:ordinary})));
    assert.equal(result.blocks.length,2);return result.indexPath;
  }
  const names=["A",...Array.from({length:4095},(_,i)=>`B-${String(i).padStart(5,"0")}`),"other"];
  let index=await controls(names);await nativeTool("Inventory",["-Inventory",index],options);
  for(const last of ["a","a/child"]) {
    index=await controls([...names.slice(0,-1),last]);await assert.rejects(nativeTool("Inventory",["-Inventory",index],options));
  }
  index=await controls(names);await nativeTool("Inventory",["-Inventory",index],options);
  const restored=await bundle();await embed(restored);
  await nativeTool("Read",["-Image",restored.image,"-Manifest",restored.manifestPath],options);
  // No produced image is signed, launched, installed or accepted as a role.
});


test("production opt-in matches the canonical Seed and refuses missing public inputs", async (t) => {
  const fixture = await profileFixture(), root = temporary(t);
  const seedPath = path.join(root, "production-seed.json"), rootPath = path.join(root, "root.json");
  const seed = { ...fixture.seed, profileKind: "production" };
  const bytes = await canonicalBytes(seed);
  const profile = await validateSeed(bytes, fixture.rootBytes);
  assert.deepEqual(profile.seedBytes, bytes);
  fs.writeFileSync(seedPath, bytes); fs.writeFileSync(rootPath, fixture.rootBytes);
  const environment = { PINVOU_HOST_BOOTSTRAP_SEED: seedPath, PINVOU_HOST_INITIAL_ROOT: rootPath };
  await assert.rejects(loadBuildProfile({ environment }), /HOST_BUILD_PROFILE_KIND_MISMATCH/u);
  environment.PINVOU_HOST_PROFILE_KIND = "production";
  assert.equal((await loadBuildProfile({ environment })).seed.profileKind, "production");
  fs.writeFileSync(seedPath, fixture.seedBytes);
  await assert.rejects(loadBuildProfile({ environment }), /HOST_BUILD_PROFILE_KIND_MISMATCH/u);
  await assert.rejects(loadBuildProfile({ environment: { PINVOU_HOST_PROFILE_KIND: "production" } }), /HOST_BUILD_INPUT_PAIR_REQUIRED/u);
  for (const kind of ["", "Production", "unknown"]) {
    await assert.rejects(loadBuildProfile({ environment: { ...environment, PINVOU_HOST_PROFILE_KIND: kind } }), /HOST_BUILD_PROFILE_KIND_INVALID/u);
  }
  await assert.rejects(validateSeed(await canonicalBytes({ ...seed, profileId: "app-fault-test" }), fixture.rootBytes));
});

test("production packaging selects only its matching host feature and fixed installation namespace", async (t) => {
  const fixture = await profileFixture(), root = temporary(t);
  const seed = { ...fixture.seed, profileKind: "production" };
  const environment = { PINVOU_HOST_PROFILE_KIND: "production" };
  for (const [key, file, bytes] of [
    ["PINVOU_HOST_BOOTSTRAP_SEED", "seed.json", await canonicalBytes(seed)],
    ["PINVOU_HOST_INITIAL_ROOT", "root.json", fixture.rootBytes],
    ["PINVOU_HOST_HELPER_BINARY", "helper.exe", "not executed"],
    ["PINVOU_HOST_LAUNCHER_BINARY", "launcher.exe", "not executed"],
    ["PINVOU_HOST_SIGNER_SCRIPT", "sign.ps1", "# not executed"],
    ["PINVOU_HOST_MAKENSIS", "makensis.exe", "not executed"],
  ]) { const target = path.join(root, file); fs.writeFileSync(target, bytes); environment[key] = target; }
  const options = { appRoot, environment, platform: "win32", architecture: "x64" };
  const args = ["build", "--bundles", "nsis"];
  const plan = await prepareHostBuild([...args, "--features", "local-embed"], options);
  assert.ok(plan.args.includes("local-embed,upgrade-host-production"));
  for (const features of ["upgrade-host-lab", "native-acceptance-hooks", "pinvou-upgrade-host/native-acceptance-hooks"]) {
    await assert.rejects(prepareHostBuild([...args, "--features", features], options));
  }
  fs.writeFileSync(environment.PINVOU_HOST_BOOTSTRAP_SEED, fixture.seedBytes);
  delete environment.PINVOU_HOST_PROFILE_KIND;
  await assert.rejects(prepareHostBuild([...args, "--features", "upgrade-host-production"], options));
  const productionNamespace = profileModule.hostNamespace(seed);
  const labNamespace = profileModule.hostNamespace(fixture.seed);
  assert.deepEqual([productionNamespace.sourceLeaf, productionNamespace.codeLeaf, productionNamespace.appLeaf,
    productionNamespace.serviceName], ["UpdaterBootstrapSource", "Updater", "App", "PinvouUpgrade-contract-test"]);
  assert.deepEqual([labNamespace.sourceLeaf, labNamespace.codeLeaf, labNamespace.appLeaf,
    labNamespace.serviceName], ["UpdaterBootstrapSourceLab", "UpdaterLab", "AppLab", "PinvouUpgradeLab-contract-test"]);
  const include = renderInclude({ profile: { seed }, entries: [{ relativeName: "pinvou3-tauri.exe" }],
    installer: path.resolve("installer.exe"), manifestPath: path.resolve("manifest.json"), stubPath: path.resolve("stub.tmp") });
  for (const [key, value] of [["SOURCE_LEAF", productionNamespace.sourceLeaf], ["CODE_LEAF", productionNamespace.codeLeaf],
    ["APP_LEAF", productionNamespace.appLeaf], ["SERVICE_NAME", productionNamespace.serviceName]]) {
    assert.ok(include.includes(`!define HOST_${key} "${value}"`));
  }
  const template = fs.readFileSync(TEMPLATE, "utf8");
  assert.ok(template.includes('Uninstall\\${HOST_SERVICE_NAME}'));
  assert.doesNotMatch(template, /UpdaterBootstrapSourceLab|UpdaterLab|AppLab|PinvouUpgradeLab/u);
  const resourceSource = fs.readFileSync(path.join(appRoot, "scripts/tauri/windows-host-resource.ps1"), "utf8");
  assert.ok(resourceSource.includes("$manifestValue.profileKind -cnotin @('lab','production')"));
});
