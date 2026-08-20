const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");

const appRoot = path.resolve(__dirname, "..");
const tauriRoot = path.join(appRoot, "src-tauri");
const {
  explicitTauriTarget,
  nativeLinuxArchitecture,
  prepareLinuxSupervisor,
  verifyElfArchitecture,
  verifyLinuxDebArchitecture,
} = require("../scripts/tauri/build.js");

const read = (relative) => fs.readFileSync(path.join(tauriRoot, relative), "utf8");
const readJson = (relative) => JSON.parse(read(relative));
const unitValue = (unit, key) => {
  const match = unit.match(new RegExp(`^${key}=(.+)$`, "m"));
  assert.ok(match, `systemd unit must define ${key}`);
  return match[1];
};
const explicitSeconds = (value) => (/^\d+$/.test(value) ? `${value}s` : value);

const linux = readJson("config/platforms/linux/tauri.conf.json");
const files = linux.bundle.linux.deb.files;
const expectedFiles = {
  "/usr/lib/pinvou3/supervisor/pinvou-supervisor":
    "target/pinvou-supervisor/release/pinvou-supervisor",
  "/usr/lib/systemd/user/pinvou3-supervisor.socket":
    "packaging/linux/deb/systemd/pinvou3-supervisor.socket",
  "/usr/lib/systemd/user/pinvou3-supervisor.service":
    "packaging/linux/deb/systemd/pinvou3-supervisor.service",
  "/usr/lib/systemd/user/pinvou3-app.service":
    "packaging/linux/deb/systemd/pinvou3-app.service",
  "/usr/lib/systemd/user/pinvou-qwen3-asr.service.d/50-pinvou-supervisor.conf":
    "packaging/linux/deb/systemd/pinvou-qwen3-asr.service.d/50-pinvou-supervisor.conf",
  "/usr/share/pinvou3/supervisor/descriptors/pinvou-app-v1.json":
    "packaging/linux/descriptor/pinvou-app-v1.json",
  "/usr/share/pinvou3/supervisor/descriptors/pinvou-asr-v1.json":
    "packaging/linux/descriptor/pinvou-asr-v1.json",
  "/usr/share/pinvou3/supervisor/profiles/megabook-canary.conf":
    "packaging/linux/deb/profiles/megabook-canary.conf",
  "/usr/share/pinvou3/supervisor/profiles/pinvou3-megabook-canary.desktop":
    "packaging/linux/deb/profiles/pinvou3-megabook-canary.desktop",
};
for (const [destination, source] of Object.entries(expectedFiles)) {
  assert.equal(files[destination], source, `deb must install ${destination}`);
  if (!source.startsWith("target/")) {
    assert.ok(fs.existsSync(path.join(tauriRoot, source)), `deb source must exist: ${source}`);
  }
}

let buildInvocation = null;
let builtMode = null;
let verifiedElf = null;
const builtBinary = prepareLinuxSupervisor({
  platform: "linux",
  architecture: "x64",
  tauriArgs: ["build", "--target", "x86_64-unknown-linux-gnu"],
  spawn: (command, args, options) => {
    buildInvocation = { command, args, options };
    return { status: 0 };
  },
  exists: () => true,
  chmod: (_file, mode) => {
    builtMode = mode;
  },
  executable: () => true,
  verifyElf: (file, machine) => {
    verifiedElf = { file, machine };
  },
});
assert.equal(buildInvocation.command, "cargo");
assert.deepEqual(buildInvocation.args.slice(0, 3), ["build", "--release", "--locked"]);
assert.ok(buildInvocation.args.includes("--manifest-path"));
assert.ok(buildInvocation.args.includes("--target-dir"));
assert.match(builtBinary, /target[\/]pinvou-supervisor[\/]release[\/]pinvou-supervisor$/);
assert.equal(builtMode, 0o755, "deb companion source mode must not inherit a group-writable umask");
assert.deepEqual(verifiedElf, { file: builtBinary, machine: 62 });
assert.equal(explicitTauriTarget(["build", "--target=x86_64-unknown-linux-gnu"]), "x86_64-unknown-linux-gnu");
assert.throws(
  () => prepareLinuxSupervisor({
    platform: "linux",
    architecture: "x64",
    tauriArgs: ["build", "--target", "aarch64-unknown-linux-gnu"],
  }),
  /cross-target Linux packaging is refused/,
);
const elfHeader = Buffer.alloc(20);
elfHeader.set([0x7f, 0x45, 0x4c, 0x46, 2, 1]);
elfHeader.writeUInt16LE(62, 18);
assert.doesNotThrow(() => verifyElfArchitecture("fake", 62, () => elfHeader));
assert.throws(() => verifyElfArchitecture("fake", 183, () => elfHeader), /ELF machine mismatch/);
assert.equal(nativeLinuxArchitecture("arm64").debArchitecture, "arm64");

let debInvocation = null;
const verifiedDeb = verifyLinuxDebArchitecture({
  platform: "linux",
  architecture: "x64",
  targetDirectory: "/tmp/fake-target",
  exists: (entry) => entry === "/tmp/fake-target/release/bundle/deb" || entry === "/usr/bin/dpkg-deb",
  readdir: () => ["pinvou3_0.8.3_amd64.deb"],
  stat: () => ({ mtimeMs: 1 }),
  spawn: (command, args, options) => {
    debInvocation = { command, args, options };
    return { status: 0, stdout: "amd64\n", stderr: "" };
  },
});
assert.equal(verifiedDeb, "/tmp/fake-target/release/bundle/deb/pinvou3_0.8.3_amd64.deb");
assert.equal(debInvocation.command, "/usr/bin/dpkg-deb");
assert.throws(
  () => verifyLinuxDebArchitecture({
    platform: "linux",
    architecture: "x64",
    targetDirectory: "/tmp/fake-target",
    exists: (entry) => entry === "/tmp/fake-target/release/bundle/deb" || entry === "/usr/bin/dpkg-deb",
    readdir: () => ["pinvou3.deb"],
    stat: () => ({ mtimeMs: 1 }),
    spawn: () => ({ status: 0, stdout: "arm64\n", stderr: "" }),
  }),
  /deb architecture mismatch/,
);
const buildWrapper = fs.readFileSync(path.join(appRoot, "scripts/tauri/build.js"), "utf8");
assert.match(
  buildWrapper,
  /if \(hasTauriBuildCommand\)[\s\S]*prepareLinuxSupervisor\(\{ tauriArgs: args \}\)[\s\S]*writeEffectiveArtifacts/,
  "Linux companion must exist before Tauri validates and bundles deb.files",
);
assert.equal(
  prepareLinuxSupervisor({ platform: "darwin", spawn: () => assert.fail("must not spawn") }),
  null,
);

const desktop = read("packaging/linux/deb/pinvou3.desktop");
const canaryDesktop = read("packaging/linux/deb/profiles/pinvou3-megabook-canary.desktop");
assert.match(desktop, /^Exec=\/usr\/bin\/pinvou3-tauri$/m);
assert.doesNotMatch(desktop, /pinvou-supervisor/);
assert.match(canaryDesktop, /^Exec=\/usr\/lib\/pinvou3\/supervisor\/pinvou-supervisor launch$/m);
assert.ok(
  Object.values(files).includes("packaging/linux/deb/profiles/pinvou3-megabook-canary.desktop"),
  "canary entry must ship as an inert profile asset, not replace the daily desktop entry",
);
assert.ok(
  !Object.keys(files).some(
    (destination) => destination.startsWith("/usr/share/applications/")
      && files[destination].includes("megabook-canary"),
  ),
  "the MegaBook-only launcher must not appear in a generic installation before profile activation",
);

const socketUnit = read("packaging/linux/deb/systemd/pinvou3-supervisor.socket");
const supervisorUnit = read("packaging/linux/deb/systemd/pinvou3-supervisor.service");
const appUnit = read("packaging/linux/deb/systemd/pinvou3-app.service");
const asrUnit = fs.readFileSync(
  path.join(appRoot, "..", "scripts/asr/pinvou-qwen3-asr.service"),
  "utf8",
);
const asrDropIn = read(
  "packaging/linux/deb/systemd/pinvou-qwen3-asr.service.d/50-pinvou-supervisor.conf",
);
const megaBookProfile = read("packaging/linux/deb/profiles/megabook-canary.conf");
assert.match(socketUnit, /^ListenStream=%t\/pinvou-supervisor\/control\.sock$/m);
assert.match(socketUnit, /^SocketMode=0600$/m);
assert.match(socketUnit, /^DirectoryMode=0700$/m);
assert.match(socketUnit, /^Service=pinvou3-supervisor\.service$/m);
assert.match(supervisorUnit, /^ExecStart=\/usr\/lib\/pinvou3\/supervisor\/pinvou-supervisor daemon$/m);
assert.match(supervisorUnit, /^StateDirectory=pinvou-supervisor$/m);
assert.match(supervisorUnit, /^Restart=on-failure$/m);
assert.doesNotMatch(supervisorUnit, /PartOf=pinvou3-app|ExecStart=.*(?:sh|bash|systemctl)/);
for (const [key, value] of [
  ["OOMPolicy", "kill"],
  ["KillMode", "control-group"],
  ["TasksMax", "512"],
  ["Restart", "on-failure"],
  ["RestartSec", "15s"],
  ["StartLimitIntervalSec", "300"],
  ["StartLimitBurst", "3"],
]) {
  assert.match(appUnit, new RegExp(`^${key}=${value.replace("%", "\\%")}$`, "m"));
}
assert.doesNotMatch(appUnit, /^Memory(?:High|Max|SwapMax)=/m);
assert.ok(
  !Object.keys(files).some((destination) => destination.includes("pinvou3-app.service.d")),
  "MegaBook 4G/8G profile must not be installed as a universal active drop-in",
);
assert.match(appUnit, /^ExecStopPost=-\/usr\/lib\/pinvou3\/supervisor\/pinvou-supervisor snapshot-app$/m);
for (const [key, value] of [
  ["MemoryHigh", "4G"],
  ["MemoryMax", "8G"],
  ["MemorySwapMax", "2G"],
  ["OOMPolicy", "kill"],
  ["KillMode", "control-group"],
  ["TasksMax", "512"],
]) {
  assert.match(megaBookProfile, new RegExp(`^${key}=${value}$`, "m"));
}
assert.match(megaBookProfile, /PINVOU_RESOURCE_PROFILE=megabook-canary-v1/);
for (const [key, value] of [
  ["MemoryHigh", "20%"],
  ["MemoryMax", "35%"],
  ["MemorySwapMax", "2G"],
  ["OOMPolicy", "kill"],
  ["KillMode", "control-group"],
  ["TasksMax", "128"],
]) {
  assert.match(asrDropIn, new RegExp(`^${key}=${value.replace("%", "\\%")}$`, "m"));
}
assert.match(asrDropIn, /^ExecStopPost=-\/usr\/lib\/pinvou3\/supervisor\/pinvou-supervisor snapshot-asr$/m);

const appDescriptor = readJson("packaging/linux/descriptor/pinvou-app-v1.json");
const asrDescriptor = readJson("packaging/linux/descriptor/pinvou-asr-v1.json");
assert.deepEqual(appDescriptor.allowedActions, ["status", "launch"]);
assert.deepEqual(asrDescriptor.allowedActions, ["status", "stop"]);
assert.equal(appDescriptor.descriptorRevision, "pinvou-app-descriptor-v1");
assert.equal(asrDescriptor.descriptorRevision, "pinvou-asr-descriptor-v1");
assert.equal(appDescriptor.unit, "pinvou3-app.service");
assert.equal(asrDescriptor.unit, "pinvou-qwen3-asr.service");
assert.equal(
  appDescriptor.resourcePolicyOwner,
  "base app unit plus explicit MegaBook canary deployment drop-in",
);
assert.equal(
  asrDescriptor.resourcePolicyOwner,
  "ASR base unit plus pinvou-supervisor package drop-in",
);
assert.equal(appDescriptor.resourcePolicy.memoryHigh, "4G");
assert.equal(appDescriptor.resourcePolicy.memoryMax, "8G");
assert.equal(appDescriptor.resourcePolicy.memorySwapMax, "2G");
assert.equal(asrDescriptor.resourcePolicy.memoryMax, "35%");
assert.equal(asrDescriptor.resourcePolicy.memorySwapMax, "2G");
for (const [descriptor, unit] of [
  [appDescriptor, appUnit],
  [asrDescriptor, asrUnit],
]) {
  assert.equal(descriptor.resourcePolicy.restart, unitValue(unit, "Restart"));
  assert.equal(
    descriptor.resourcePolicy.restartSec,
    explicitSeconds(unitValue(unit, "RestartSec")),
  );
  assert.equal(
    descriptor.resourcePolicy.startLimitIntervalSec,
    explicitSeconds(unitValue(unit, "StartLimitIntervalSec")),
  );
  assert.equal(
    descriptor.resourcePolicy.startLimitBurst,
    Number(unitValue(unit, "StartLimitBurst")),
  );
}
assert.match(asrDescriptor.fragmentSuffix, /pinvou-qwen3-asr\.service$/);
assert.match(asrDescriptor.executableSuffix, /runtime\/bin\/python$/);

const protocol = read("crates/host-supervisor-protocol/src/lib.rs");
const daemon = read("packaging/linux/supervisor/src/lib.rs");
assert.match(protocol, /serde\(deny_unknown_fields\)/);
assert.match(protocol, /enum ManagedHostWork[\s\S]*PinvouApp,[\s\S]*PinvouAsr,/);
assert.doesNotMatch(protocol, /\b(?:pid|unit|command|shell|systemctl)\s*:/i);
assert.match(daemon, /libc::SO_PEERCRED/);
assert.match(daemon, /MAX_STORED_REQUESTS: usize = 65_536/);
assert.match(daemon, /pending directive is not reconciled; action was not replayed/);
assert.match(daemon, /control-v1\.jsonl/);
assert.match(daemon, /observations-v1\.jsonl/);
assert.match(daemon, /control caller PID is not pinvou3-app\.service MainPID/);
assert.match(daemon, /FD_CLOEXEC/);
for (const property of [
  "Restart",
  "RestartUSec",
  "StartLimitIntervalUSec",
  "StartLimitBurst",
]) {
  assert.match(daemon, new RegExp(`--property=[^"\\n]*${property}`));
}
assert.doesNotMatch(daemon, /Command::new\((?:request|unit|command|target)/);

const postinst = read("packaging/linux/deb/scripts/postinst.sh");
const postrm = read("packaging/linux/deb/scripts/postrm.sh");
assert.match(postinst, /\/run\/user\/\*/);
assert.match(postinst, /"\$operation" pinvou3-supervisor\.socket/);
assert.doesNotMatch(postinst, /--global|systemctl[^\n]*\benable\b|\/home\/|user-dirs\.dirs|XDG_DESKTOP_DIR|cp\s/);
assert.doesNotMatch(postinst, /NOPASSWD|sudo\s|command\s+-v/);
assert.equal(
  linux.bundle.linux.deb.postRemoveScript,
  "packaging/linux/deb/scripts/postrm.sh",
);
assert.match(postrm, /"\$systemctl_bin" --user daemon-reload/);
for (const script of [postinst, read("packaging/linux/deb/scripts/prerm.sh"), postrm]) {
  assert.doesNotMatch(script, /\/home\/\*|user-dirs\.dirs|\.\s+"?\$[^\n]*user-dirs/);
  assert.doesNotMatch(script, /^\s*(?:\.|source)\s+/m, "maintainer scripts must not source user configuration");
  assert.doesNotMatch(script, /(?:^|\s)ln\s+(?:-[^\s]+\s+)*/m, "maintainer scripts must not create enable-style symlinks");
  assert.doesNotMatch(script, /(?:\$HOME|\$\{HOME\}|~\/)/, "root maintainer scripts must not resolve a user home");
}

console.log("linux supervisor packaging contract: ok");
