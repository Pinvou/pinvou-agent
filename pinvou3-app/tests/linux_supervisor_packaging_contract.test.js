const assert = require("node:assert/strict");
const crypto = require("node:crypto");
const childProcess = require("node:child_process");
const fs = require("node:fs");
const os = require("node:os");
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
  "/usr/lib/pinvou3/supervisor/pinvou-megabook-profile":
    "packaging/linux/deb/scripts/megabook-profile.sh",
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
assert.doesNotMatch(
  supervisorUnit,
  /^ProtectKernelModules=/m,
  "an unprivileged user unit must not trigger systemd 259's 218/CAPABILITIES failure",
);
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

const profileHelperPath = path.join(
  tauriRoot,
  "packaging/linux/deb/scripts/megabook-profile.sh",
);
const e2eHarnessPath = path.join(appRoot, "scripts/megabook-supervisor-e2e.sh");
const fixtureRoot = path.join(appRoot, "scripts/fixtures/megabook-supervisor-e2e");
const profileHelper = fs.readFileSync(profileHelperPath, "utf8");
const e2eHarness = fs.readFileSync(e2eHarnessPath, "utf8");
const debReadme = read("packaging/linux/deb/README.md");
const fixtureHash = (name) => crypto
  .createHash("sha256")
  .update(fs.readFileSync(path.join(fixtureRoot, name)))
  .digest("hex");
const fixtureMode = (name) => fs.statSync(path.join(fixtureRoot, name)).mode & 0o777;

assert.equal(files["/usr/lib/pinvou3/supervisor/pinvou-megabook-profile"],
  "packaging/linux/deb/scripts/megabook-profile.sh");
assert.equal(fs.statSync(profileHelperPath).mode & 0o777, 0o755);
assert.equal(fs.statSync(e2eHarnessPath).mode & 0o777, 0o755);
if (process.platform === "linux") {
  for (const shellFile of [profileHelperPath, e2eHarnessPath]) {
    const checked = childProcess.spawnSync("/bin/dash", ["-n", shellFile], { encoding: "utf8" });
    assert.equal(checked.status, 0, `${shellFile} must parse as dash: ${checked.stderr}`);
  }
  const loaderParse = childProcess.spawnSync(
    "/usr/bin/python3",
    ["-c", "import ast,pathlib,sys; ast.parse(pathlib.Path(sys.argv[1]).read_text())",
      path.join(fixtureRoot, "memory-loader.py")],
    { encoding: "utf8", env: { ...process.env, PYTHONDONTWRITEBYTECODE: "1" } },
  );
  assert.equal(loaderParse.status, 0, `memory loader must parse: ${loaderParse.stderr}`);
  const e2ePythonBlocks = [...e2eHarness.matchAll(/<<'PY'\n([\s\S]*?)\nPY(?:\n|$)/g)]
    .map((match) => match[1]);
  const procStarttimeParser = e2ePythonBlocks.find((block) =>
    block.includes('raw.rfind(") ")') && block.includes("fields[19]"));
  assert.ok(procStarttimeParser, "E2E must carry the bounded /proc starttime parser");
  const procStarttimeCheck = childProcess.spawnSync(
    "/usr/bin/python3",
    ["-c", procStarttimeParser, String(process.pid)],
    { encoding: "utf8", env: { ...process.env, PYTHONDONTWRITEBYTECODE: "1" } },
  );
  assert.equal(procStarttimeCheck.status, 0, procStarttimeCheck.stderr);
  assert.match(procStarttimeCheck.stdout.trim(), /^[1-9][0-9]*$/);

  const helperEntry = profileHelper.indexOf('[ "$#" -eq 1 ] || fail');
  assert.ok(helperEntry > 0, "profile helper must retain one fixed public dispatch boundary");
  const helperLibrary = profileHelper.slice(0, helperEntry);
  const helperBehaviorRoot = fs.mkdtempSync(path.join(os.tmpdir(), "pinvou-profile-helper-"));
  try {
    fs.chmodSync(helperBehaviorRoot, 0o700);
    const helperBehavior = childProcess.spawnSync(
      "/bin/dash",
      ["-s", "--", helperBehaviorRoot,
        path.join(tauriRoot, "packaging/linux/deb/profiles/megabook-canary.conf")],
      {
        encoding: "utf8",
        env: {
          ...process.env,
          PYTHONHOME: "/tmp/pinvou-hostile-pythonhome",
          PYTHONPATH: "/tmp/pinvou-hostile-pythonpath",
        },
        input: `${helperLibrary}
test_root=$1
PROFILE_SOURCE=$2
home_dir=$test_root/home
profile_dir=$home_dir/.config/systemd/user/pinvou3-app.service.d
desktop_dir=$home_dir/.local/share/applications
state_dir=$home_dir/.local/state/pinvou3
/usr/bin/mkdir -p -m 0700 -- "$profile_dir" "$desktop_dir" "$state_dir"
/usr/bin/chmod 0775 -- "$home_dir/.config/systemd" "$home_dir/.config/systemd/user"
profile_target=$profile_dir/50-megabook-canary.conf
desktop_target=$desktop_dir/pinvou3-megabook-canary.desktop
legacy_marker_target=$state_dir/megabook-profile-v1.registered
installing_marker_target=$state_dir/megabook-profile-v2.installing
applied_marker_target=$state_dir/megabook-profile-v2.applied
profile_quarantine=$profile_dir/.pinvou-quarantine-profile-v2
desktop_quarantine=$desktop_dir/.pinvou-quarantine-desktop-v2
legacy_marker_quarantine=$state_dir/.pinvou-quarantine-marker-v1
installing_marker_quarantine=$state_dir/.pinvou-quarantine-marker-v2-installing
applied_marker_quarantine=$state_dir/.pinvou-quarantine-marker-v2-applied
profile_staging_dir=$profile_dir/.pinvou-profile-staging-v2
desktop_staging_dir=$desktop_dir/.pinvou-desktop-staging-v2
marker_staging_dir=$state_dir/.pinvou-marker-staging-v2

install_source_no_clobber \
  "$PROFILE_SOURCE" "$profile_target" "$profile_dir" .pinvou-profile "$PROFILE_SHA256"
[ "$(/usr/bin/stat -c %a:%h -- "$profile_target")" = 644:1 ] \
  || fail "behavior gate: published profile metadata mismatch"
[ "$(/usr/bin/sha256sum "$profile_target")" = \
  "$PROFILE_SHA256  $profile_target" ] \
  || fail "behavior gate: published profile hash mismatch"
[ ! -e "$profile_staging_dir" ] && [ ! -L "$profile_staging_dir" ] \
  || fail "behavior gate: profile staging namespace remains"

publish_marker_no_clobber installing "$installing_marker_target" "$INSTALLING_MARKER_SHA256"
[ "$(/usr/bin/stat -c %a:%h -- "$installing_marker_target")" = 600:1 ] \
  || fail "behavior gate: published marker metadata mismatch"
[ ! -e "$marker_staging_dir" ] && [ ! -L "$marker_staging_dir" ] \
  || fail "behavior gate: marker staging namespace remains"

quarantine_and_delete \
  "$profile_target" "$profile_quarantine" 644 "$PROFILE_SHA256" "$profile_dir"
quarantine_and_delete \
  "$installing_marker_target" "$installing_marker_quarantine" 600 \
  "$INSTALLING_MARKER_SHA256" "$state_dir"
[ ! -e "$profile_target" ] && [ ! -L "$profile_target" ] \
  || fail "behavior gate: profile target remains after quarantine deletion"
[ ! -e "$profile_quarantine" ] && [ ! -L "$profile_quarantine" ] \
  || fail "behavior gate: profile quarantine remains"
[ ! -e "$installing_marker_target" ] && [ ! -L "$installing_marker_target" ] \
  || fail "behavior gate: marker target remains after quarantine deletion"
[ ! -e "$installing_marker_quarantine" ] && [ ! -L "$installing_marker_quarantine" ] \
  || fail "behavior gate: marker quarantine remains"
/usr/bin/printf '%s\n' helper-behavior-pass
`,
      },
    );
    assert.equal(helperBehavior.status, 0,
      `real profile helper publication behavior failed: ${helperBehavior.stderr}`);
    assert.equal(helperBehavior.stdout.trim(), "helper-behavior-pass");
  } finally {
    fs.rmSync(helperBehaviorRoot, { recursive: true, force: true });
  }

  const payloadAttestation = e2ePythonBlocks.find((block) =>
    block.includes("pinvou-install-attestation-v1")
      && block.includes("fixed payload specification count changed"));
  const controlAttestation = e2ePythonBlocks.find((block) =>
    block.includes("fixed control md5sums attestation specification is invalid"));
  const controlArchiveAttestation = e2ePythonBlocks.find((block) =>
    block.includes("pinvou-control-members-v1")
      && block.includes("installed dpkg control database has an unexpected complete member set"));
  assert.ok(payloadAttestation, "E2E must carry the exact-deb payload attestation");
  assert.ok(controlAttestation, "E2E must carry the exact-deb control md5sums attestation");
  assert.ok(controlArchiveAttestation,
    "E2E must carry complete control-member and generated-list attestation");

  const archiveConsumerBoundary = controlArchiveAttestation.indexOf(
    "\ncontrol_records, field_records = stream_tar(",
  );
  assert.ok(archiveConsumerBoundary > 0,
    "E2E control archive consumers must remain independently testable");
  const archiveConsumerLibrary = controlArchiveAttestation.slice(0, archiveConsumerBoundary);
  const tarMemberCompatibilityGate = `${archiveConsumerLibrary}
import io


def archive_bytes(entries):
    buffer = io.BytesIO()
    with tarfile.open(fileobj=buffer, mode="w") as output:
        for name, kind, member_mode, raw in entries:
            member = tarfile.TarInfo(name)
            member.uid = 0
            member.gid = 0
            member.mode = member_mode
            if kind == "dir":
                member.type = tarfile.DIRTYPE
                member.size = 0
                output.addfile(member)
            else:
                member.size = len(raw)
                output.addfile(member, io.BytesIO(raw))
    return buffer.getvalue()


def consume(entries, consumer):
    with tarfile.open(fileobj=io.BytesIO(archive_bytes(entries)), mode="r:") as archive:
        return consumer(archive)


control_raw = b"Package: pinvou3\\nVersion: 0.0.0\\nArchitecture: amd64\\n\\n"
md5_raw = b"d41d8cd98f00b204e9800998ecf8427e  usr/bin/pinvou3-tauri\\n"
plain_control = [
    ("control", "file", 0o644, control_raw),
    ("md5sums", "file", 0o644, md5_raw),
    ("prerm", "file", 0o755, b"#!/bin/sh\\nexit 0\\n"),
]
dot_control = [
    ("./", "dir", 0o755, b""),
    ("./control", "file", 0o644, control_raw),
    ("./md5sums", "file", 0o644, md5_raw),
    ("./prerm", "file", 0o755, b"#!/bin/sh\\nexit 0\\n"),
]
plain_records, plain_fields = consume(plain_control, consume_control_archive)
dot_records, dot_fields = consume(dot_control, consume_control_archive)
if digest_control_members(plain_records) != digest_control_members(dot_records) \
        or digest_control_fields(plain_fields) != digest_control_fields(dot_fields):
    raise SystemExit("equivalent control tar relative forms produced different attestations")

plain_data = [
    ("usr", "dir", 0o755, b""),
    ("usr/bin", "dir", 0o755, b""),
    ("usr/bin/pinvou3-tauri", "file", 0o755, b"pinvou"),
]
dot_data = [
    ("./usr", "dir", 0o755, b""),
    ("./usr/bin", "dir", 0o755, b""),
    ("./usr/bin/pinvou3-tauri", "file", 0o755, b"pinvou"),
]
explicit_root_data = [
    (".", "dir", 0o755, b""),
    ("./usr", "dir", 0o755, b""),
    ("./usr/bin", "dir", 0o755, b""),
    ("./usr/bin/pinvou3-tauri", "file", 0o755, b"pinvou"),
]
late_root_data = [
    ("usr", "dir", 0o755, b""),
    ("usr/bin", "dir", 0o755, b""),
    ("./", "dir", 0o755, b""),
    ("usr/bin/pinvou3-tauri", "file", 0o755, b"pinvou"),
]
rootless_list = b"/usr\\n/usr/bin\\n/usr/bin/pinvou3-tauri\\n"
explicit_root_list = b"/.\\n/usr\\n/usr/bin\\n/usr/bin/pinvou3-tauri\\n"
late_root_list = b"/usr\\n/usr/bin\\n/.\\n/usr/bin/pinvou3-tauri\\n"
if consume(plain_data, consume_data_archive) != rootless_list \
        or consume(dot_data, consume_data_archive) != rootless_list \
        or consume(explicit_root_data, consume_data_archive) != explicit_root_list \
        or consume(late_root_data, consume_data_archive) != late_root_list:
    raise SystemExit("safe data tar relative/root forms produced an incorrect dpkg list")


def must_reject(entries, consumer, label):
    try:
        consume(entries, consumer)
    except SystemExit:
        return
    raise SystemExit(f"unsafe tar member was accepted: {label}")


for unsafe_name in (
    "/usr/bin/pinvou3-tauri",
    "../usr/bin/pinvou3-tauri",
    "usr/../bin/pinvou3-tauri",
    "usr//bin/pinvou3-tauri",
    "././usr/bin/pinvou3-tauri",
    "usr/./bin/pinvou3-tauri",
):
    must_reject([(unsafe_name, "file", 0o755, b"pinvou")], consume_data_archive, unsafe_name)
must_reject(
    [("usr/bin/pinvou3-tauri", "file", 0o755, b"one"),
     ("./usr/bin/pinvou3-tauri", "file", 0o755, b"two")],
    consume_data_archive,
    "normalized duplicate data path",
)
for unsafe_name in ("dir/prerm", "/prerm", "../prerm", "././prerm"):
    must_reject(
        plain_control + [(unsafe_name, "file", 0o755, b"#!/bin/sh\\nexit 0\\n")],
        consume_control_archive,
        unsafe_name,
    )
must_reject(
    [("control", "file", 0o644, control_raw),
     ("./control", "file", 0o644, control_raw),
     ("md5sums", "file", 0o644, md5_raw)],
    consume_control_archive,
    "normalized duplicate control member",
)
print("tar-member-compatibility-pass")
`;
  const tarMemberCompatibility = childProcess.spawnSync(
    "/usr/bin/python3",
    ["-I", "-c", tarMemberCompatibilityGate, "baseline", "/unused", "", "", ""],
    {
      encoding: "utf8",
      env: {
        ...process.env,
        PYTHONHOME: "/tmp/pinvou-hostile-pythonhome",
        PYTHONPATH: "/tmp/pinvou-hostile-pythonpath",
      },
    },
  );
  assert.equal(tarMemberCompatibility.status, 0, tarMemberCompatibility.stderr);
  assert.equal(tarMemberCompatibility.stdout.trim(), "tar-member-compatibility-pass");

  if (fs.existsSync("/usr/bin/dpkg-deb")) {
    const temporaryRoot = fs.mkdtempSync(path.join(os.tmpdir(), "pinvou-deb-attestation-"));
    try {
      const packageRoot = path.join(temporaryRoot, "root");
      const debPath = path.join(temporaryRoot, "pinvou3-test.deb");
      const fixedSpecs = [
        ["usr/bin/pinvou3-tauri", 0o755],
        ["usr/lib/pinvou3/supervisor/pinvou-supervisor", 0o755],
        ["usr/lib/pinvou3/supervisor/pinvou-megabook-profile", 0o755],
        ["usr/lib/systemd/user/pinvou3-supervisor.socket", 0o644],
        ["usr/lib/systemd/user/pinvou3-supervisor.service", 0o644],
        ["usr/lib/systemd/user/pinvou3-app.service", 0o644],
        ["usr/lib/systemd/user/pinvou-qwen3-asr.service.d/50-pinvou-supervisor.conf", 0o644],
        ["usr/share/pinvou3/supervisor/descriptors/pinvou-app-v1.json", 0o644],
        ["usr/share/pinvou3/supervisor/descriptors/pinvou-asr-v1.json", 0o644],
        ["usr/share/pinvou3/supervisor/profiles/megabook-canary.conf", 0o644],
        ["usr/share/pinvou3/supervisor/profiles/pinvou3-megabook-canary.desktop", 0o644],
        ["usr/share/applications/pinvou3.desktop", 0o644],
      ];
      fs.mkdirSync(path.join(packageRoot, "DEBIAN"), { recursive: true });
      fs.writeFileSync(path.join(packageRoot, "DEBIAN/control"), [
        "Package: pinvou3",
        "Version: 0.0.0",
        "Architecture: amd64",
        "Maintainer: Pinvou test",
        "Description: exact-deb attestation fixture",
        "",
      ].join("\n"));
      for (const controlScript of ["postinst", "prerm", "postrm"]) {
        const controlPath = path.join(packageRoot, "DEBIAN", controlScript);
        fs.writeFileSync(controlPath, `#!/bin/sh\n# ${controlScript} v1\nexit 0\n`, { mode: 0o755 });
        fs.chmodSync(controlPath, 0o755);
      }
      const md5sums = [];
      for (const [relative, mode] of fixedSpecs) {
        const absolute = path.join(packageRoot, relative);
        const bytes = Buffer.from(`fixture:${relative}\n`);
        fs.mkdirSync(path.dirname(absolute), { recursive: true });
        fs.writeFileSync(absolute, bytes, { mode });
        fs.chmodSync(absolute, mode);
        md5sums.push(`${crypto.createHash("md5").update(bytes).digest("hex")}  ${relative}`);
      }
      fs.writeFileSync(path.join(packageRoot, "DEBIAN/md5sums"), `${md5sums.join("\n")}\n`);
      const built = childProcess.spawnSync(
        "/usr/bin/dpkg-deb",
        ["--build", "--root-owner-group", packageRoot, debPath],
        { encoding: "utf8" },
      );
      assert.equal(built.status, 0, `synthetic deb build failed: ${built.stderr}`);

      const payloadArgs = fixedSpecs.map(([relative, mode]) =>
        `/${relative}:${mode.toString(8)}`);
      const payloadPositive = childProcess.spawnSync(
        "/usr/bin/python3",
        ["-c", payloadAttestation, "baseline", debPath, "", ...payloadArgs],
        { encoding: "utf8", env: { ...process.env, PYTHONDONTWRITEBYTECODE: "1" } },
      );
      assert.equal(payloadPositive.status, 0, payloadPositive.stderr);
      assert.match(payloadPositive.stdout.trim(), /^[0-9a-f]{64}$/);
      const wrongModeArgs = [...payloadArgs];
      wrongModeArgs[0] = "/usr/bin/pinvou3-tauri:644";
      const payloadWrongMode = childProcess.spawnSync(
        "/usr/bin/python3",
        ["-c", payloadAttestation, "baseline", debPath, "", ...wrongModeArgs],
        { encoding: "utf8", env: { ...process.env, PYTHONDONTWRITEBYTECODE: "1" } },
      );
      assert.notEqual(payloadWrongMode.status, 0, "wrong payload mode must fail closed");

      const controlPaths = fixedSpecs.map(([relative]) => relative);
      const controlPositive = childProcess.spawnSync(
        "/usr/bin/python3",
        ["-c", controlAttestation, "baseline", debPath, "", ...controlPaths],
        { encoding: "utf8", env: { ...process.env, PYTHONDONTWRITEBYTECODE: "1" } },
      );
      assert.equal(controlPositive.status, 0, controlPositive.stderr);
      assert.match(controlPositive.stdout.trim(), /^[0-9a-f]{64}$/);
      const missingControlPaths = [...controlPaths];
      missingControlPaths[0] = "usr/bin/not-pinvou";
      const controlMissing = childProcess.spawnSync(
        "/usr/bin/python3",
        ["-c", controlAttestation, "baseline", debPath, "", ...missingControlPaths],
        { encoding: "utf8", env: { ...process.env, PYTHONDONTWRITEBYTECODE: "1" } },
      );
      assert.notEqual(controlMissing.status, 0, "missing control checksum must fail closed");

      const controlArchivePositive = childProcess.spawnSync(
        "/usr/bin/python3",
        ["-c", controlArchiveAttestation, "baseline", debPath, "", "", ""],
        { encoding: "utf8", env: { ...process.env, PYTHONDONTWRITEBYTECODE: "1" } },
      );
      assert.equal(controlArchivePositive.status, 0, controlArchivePositive.stderr);
      assert.match(controlArchivePositive.stdout.trim(),
        /^[0-9a-f]{64}:[0-9a-f]{64}:[0-9a-f]{64}$/);

      const changedControlDeb = path.join(temporaryRoot, "pinvou3-changed-control.deb");
      const postinstPath = path.join(packageRoot, "DEBIAN/postinst");
      fs.writeFileSync(postinstPath, "#!/bin/sh\n# postinst changed\nexit 0\n", { mode: 0o755 });
      fs.chmodSync(postinstPath, 0o755);
      const changedControlBuild = childProcess.spawnSync(
        "/usr/bin/dpkg-deb",
        ["--build", "--root-owner-group", packageRoot, changedControlDeb],
        { encoding: "utf8" },
      );
      assert.equal(changedControlBuild.status, 0, changedControlBuild.stderr);
      const changedControl = childProcess.spawnSync(
        "/usr/bin/python3",
        ["-c", controlArchiveAttestation, "baseline", changedControlDeb, "", "", ""],
        { encoding: "utf8", env: { ...process.env, PYTHONDONTWRITEBYTECODE: "1" } },
      );
      assert.equal(changedControl.status, 0, changedControl.stderr);
      assert.notEqual(changedControl.stdout.trim().split(":")[0],
        controlArchivePositive.stdout.trim().split(":")[0],
        "changed postinst bytes must change the control-member attestation");

      const extraPreinstDeb = path.join(temporaryRoot, "pinvou3-extra-preinst.deb");
      const preinstPath = path.join(packageRoot, "DEBIAN/preinst");
      fs.writeFileSync(preinstPath, "#!/bin/sh\n# unexpected preinst\nexit 0\n", { mode: 0o755 });
      fs.chmodSync(preinstPath, 0o755);
      const extraPreinstBuild = childProcess.spawnSync(
        "/usr/bin/dpkg-deb",
        ["--build", "--root-owner-group", packageRoot, extraPreinstDeb],
        { encoding: "utf8" },
      );
      assert.equal(extraPreinstBuild.status, 0, extraPreinstBuild.stderr);
      const extraPreinst = childProcess.spawnSync(
        "/usr/bin/python3",
        ["-c", controlArchiveAttestation, "baseline", extraPreinstDeb, "", "", ""],
        { encoding: "utf8", env: { ...process.env, PYTHONDONTWRITEBYTECODE: "1" } },
      );
      assert.equal(extraPreinst.status, 0, extraPreinst.stderr);
      assert.notEqual(extraPreinst.stdout.trim().split(":")[0],
        changedControl.stdout.trim().split(":")[0],
        "an extra preinst member must change the complete control-member attestation");
    } finally {
      fs.rmSync(temporaryRoot, { recursive: true, force: true });
    }
  }
}

assert.match(profileHelper, /usage: pinvou-megabook-profile <activate\|deactivate\|status>/);
assert.doesNotMatch(profileHelper, /\b(?:enable|start|stop)\b.*\$[2-9]|pinvou-qwen3-asr/);
assert.match(profileHelper, /APP_UNIT=pinvou3-app\.service/);
assert.match(profileHelper, /ownership marker appeared concurrently; refusing to overwrite it/);
assert.match(profileHelper, /owned file content changed/);
assert.match(profileHelper, /stop pinvou3-app\.service before changing its resource profile/);
assert.match(profileHelper, /\/usr\/bin\/ln -T --/);
assert.match(profileHelper, /\/usr\/bin\/mv -T -n --/);
assert.match(profileHelper, /os\.fsync\(fd\)/);
assert.match(profileHelper, /validate_owned_file_one_of[\s\S]*allowed_links[\s\S]*1:2/);
assert.match(profileHelper, /cleanup_staging_orphans/);
assert.match(profileHelper, /validate_effective_profile/);
assert.match(profileHelper, /DropInPaths[\s\S]*MemoryHigh[\s\S]*MemoryMax[\s\S]*MemorySwapMax/);
assert.equal((profileHelper.match(/\/usr\/bin\/python3/g) || []).length, 6,
  "helper must retain five Python calls plus one fixed-tool allowlist entry");
assert.equal((profileHelper.match(/\/usr\/bin\/python3 -I (?:-|\-c)/g) || []).length, 5,
  "every helper Python call must ignore PYTHONPATH, PYTHONHOME, user site and cwd imports");
assert.match(profileHelper, /effective primary group is not provably user-private/);
assert.match(profileHelper, /effective primary gid is shared by another passwd identity/);
assert.match(profileHelper, /group-writable directory does not use the private primary group/);
for (const frozenLine of [
  "PROFILE_SOURCE=/usr/share/pinvou3/supervisor/profiles/megabook-canary.conf",
  "DESKTOP_SOURCE=/usr/share/pinvou3/supervisor/profiles/pinvou3-megabook-canary.desktop",
  "PROFILE_TARGET_SUFFIX=/.config/systemd/user/pinvou3-app.service.d/50-megabook-canary.conf",
  "DESKTOP_TARGET_SUFFIX=/.local/share/applications/pinvou3-megabook-canary.desktop",
  "LEGACY_MARKER_TARGET_SUFFIX=/.local/state/pinvou3/megabook-profile-v1.registered",
  "PROFILE_BYTES=351",
  "DESKTOP_BYTES=465",
  "PROFILE_SHA256=74cc705379e10f6626bb614118e66c080366e3bed907509a786d7692048e451c",
  "DESKTOP_SHA256=ddfe6a25920570d8992a9eb6c3d53bcc64404a6ce069e764e42383141e9a12a0",
]) {
  assert.ok(profileHelper.includes(frozenLine), `v1 cleanup ABI must retain: ${frozenLine}`);
}
for (const stagingName of [
  ".pinvou-profile-staging-v2",
  ".pinvou-desktop-staging-v2",
  ".pinvou-marker-staging-v2",
]) {
  assert.ok(profileHelper.includes(stagingName), `helper must reserve ${stagingName}`);
}
for (const property of [
  "LoadState",
  "FragmentPath",
  "DropInPaths",
  "MemoryAccounting",
  "MemoryHigh",
  "MemoryMax",
  "MemorySwapMax",
  "OOMPolicy",
  "KillMode",
  "TasksMax",
  "Restart",
  "RestartUSec",
  "StartLimitIntervalUSec",
  "StartLimitBurst",
  "Environment",
]) {
  assert.ok(profileHelper.includes(`\"${property}\"`),
    `effective profile validation must cover ${property}`);
}
for (const receiptFact of [
  'receipt.get("protocol_version") != 2',
  'receipt.get("target") != "pinvou_app"',
  'receipt.get("descriptor_revision") != "pinvou-app-descriptor-v1"',
  'receipt.get("expected_instance_generation") is not None',
  'receipt.get("action") != "status"',
  'receipt.get("outcome") != "reconciled"',
  'state not in ("inactive", "failed")',
]) {
  assert.ok(profileHelper.includes(receiptFact),
    `trusted Supervisor receipt validation must retain: ${receiptFact}`);
}
const legacyMarkerBytes = [
  "schema=pinvou-megabook-profile-v1",
  "profile_sha256=74cc705379e10f6626bb614118e66c080366e3bed907509a786d7692048e451c",
  "desktop_sha256=ddfe6a25920570d8992a9eb6c3d53bcc64404a6ce069e764e42383141e9a12a0",
  "",
].join("\n");
assert.equal(
  crypto.createHash("sha256").update(legacyMarkerBytes).digest("hex"),
  "5858fdf923bace7a8895b7a901f5ac16d798a97e7c15d8a361533329f9c605cc",
  "v1 marker bytes are a frozen cleanup ABI",
);
assert.match(profileHelper,
  /LEGACY_MARKER_SHA256=5858fdf923bace7a8895b7a901f5ac16d798a97e7c15d8a361533329f9c605cc/);
assert.match(profileHelper, /megabook-profile-v1\.registered/);
assert.match(profileHelper, /megabook-profile-v2\.installing/);
assert.match(profileHelper, /megabook-profile-v2\.applied/);
assert.match(profileHelper, /phase=\$phase/);
for (const [phase, digest] of [
  ["installing", "02e599747fdf54301cf8f77227e4668f4e5a5112817b29a9cfe786c4613d98b5"],
  ["applied", "efd76b9543fcec1b362047e7b8b0fee91773811e35b2a3352224ae44dca7f6d3"],
]) {
  const markerBytes = [
    "schema=pinvou-megabook-profile-v2",
    `phase=${phase}`,
    "profile_sha256=74cc705379e10f6626bb614118e66c080366e3bed907509a786d7692048e451c",
    "desktop_sha256=ddfe6a25920570d8992a9eb6c3d53bcc64404a6ce069e764e42383141e9a12a0",
    "",
  ].join("\n");
  assert.equal(crypto.createHash("sha256").update(markerBytes).digest("hex"), digest);
  assert.ok(profileHelper.includes(digest), `v2 ${phase} marker hash must be pinned`);
}
assert.match(debReadme, /v1[\s\S]*(?:frozen|冻结)[\s\S]*v2/i);
const activateBody = profileHelper.slice(
  profileHelper.indexOf("activate_profile()"),
  profileHelper.indexOf("deactivate_profile()"),
);
assert.ok(activateBody.indexOf("require_app_inactive") < activateBody.indexOf("publish_marker_no_clobber"));
assert.ok(activateBody.lastIndexOf("daemon_reload") < activateBody.lastIndexOf("require_app_inactive"));
assert.ok(activateBody.lastIndexOf("require_app_inactive") < activateBody.indexOf("validate_effective_profile"));
assert.ok(activateBody.indexOf("validate_effective_profile")
  < activateBody.lastIndexOf("publish_marker_no_clobber applied"));
assert.ok(activateBody.lastIndexOf("publish_marker_no_clobber applied")
  < activateBody.lastIndexOf("status_profile"));
const deactivateBody = profileHelper.slice(
  profileHelper.indexOf("deactivate_profile()"),
  profileHelper.indexOf("[ \"$#\" -eq 1 ]"),
);
assert.ok(deactivateBody.indexOf("require_app_inactive") < deactivateBody.indexOf("profile_quarantine"));
assert.ok(deactivateBody.indexOf("desktop_quarantine") < deactivateBody.indexOf("daemon_reload"));
assert.ok(deactivateBody.indexOf("daemon_reload") < deactivateBody.lastIndexOf("require_app_inactive"));
assert.ok(deactivateBody.lastIndexOf("require_app_inactive")
  < deactivateBody.lastIndexOf("applied_marker_quarantine"));
const quarantineBody = profileHelper.slice(
  profileHelper.indexOf("quarantine_and_delete()"),
  profileHelper.indexOf("require_no_quarantines()"),
);
assert.ok(quarantineBody.indexOf("/usr/bin/mv -T -n --")
  < quarantineBody.indexOf("validate_fixed_public_file \"$quarantine\""));
assert.ok(quarantineBody.indexOf("validate_fixed_public_file \"$quarantine\"")
  < quarantineBody.lastIndexOf("/usr/bin/rm -- \"$quarantine\""));

const fixtureContracts = {
  "memory-loader.py": [0o755, "e740b1c6632b2cdd10158fed72c4760720c80956e6c93ba5ae19c929b9800cde"],
  "90-memory-high.conf": [0o644, "13c32ca901b5e45411fcf597b21373817b3d3893c8fc875035a5928c1dd35d47"],
  "90-memory-max.conf": [0o644, "fd6a23395c235e5344e8fc9d346403c0413738832afa2e34bc41a86f0e541e08"],
  "go-high.marker": [0o644, "6319b41e829ccc8fd69446d15ecbae665ef60942acd56458ea690abd3d5e8c30"],
  "go-max.marker": [0o644, "9fbddcd505d602fe6292020d95b9618486cacfb304f5ba30729ac0794d91da63"],
};
for (const [name, [mode, digest]] of Object.entries(fixtureContracts)) {
  assert.equal(fixtureMode(name), mode, `${name} mode must be fixed`);
  assert.equal(fixtureHash(name), digest, `${name} hash must be fixed`);
  assert.equal(mode & 0o022, 0, `${name} must not be group/other writable`);
}
assert.match(fs.readFileSync(path.join(fixtureRoot, "90-memory-high.conf"), "utf8"),
  /^ExecStartPost=\/usr\/bin\/python3 %t\/pinvou-megabook-e2e\/memory-loader\.py high$/m);
assert.match(fs.readFileSync(path.join(fixtureRoot, "90-memory-max.conf"), "utf8"),
  /^ExecStartPost=\/usr\/bin\/python3 %t\/pinvou-megabook-e2e\/memory-loader\.py max$/m);
assert.doesNotMatch(e2eHarness, /systemctl[^\n]*\battach\b|SIGSTOP|SIGCONT|\/bin\/kill|\/usr\/bin\/kill/);
assert.doesNotMatch(e2eHarness, /^\s*sudo\b/m);
assert.match(e2eHarness, /publish_private_staged_file[\s\S]*\/usr\/bin\/ln -T --/);
assert.match(e2eHarness, /validate_user_file_links[\s\S]*fsync_directory/);
assert.match(e2eHarness, /transaction_residue_absent/);
for (const stagingName of [
  ".pinvou-profile-staging-v2",
  ".pinvou-desktop-staging-v2",
  ".pinvou-marker-staging-v2",
]) {
  assert.ok(e2eHarness.includes(stagingName),
    `E2E cleanup and purge must account for ${stagingName}`);
}
assert.match(e2eHarness, /payload_attestation\(\)/);
assert.match(e2eHarness, /pinvou-install-attestation-v1/);
assert.match(e2eHarness, /control_md5sums_attestation\(\)/);
assert.match(e2eHarness, /control_archive_attestation\(\)/);
assert.equal((e2eHarness.match(/\/usr\/bin\/python3/g) || []).length, 30,
  "E2E must retain 29 isolated Python calls plus one fixed-tool allowlist entry");
assert.equal((e2eHarness.match(/\/usr\/bin\/python3 -I (?:-|\-c)/g) || []).length, 29,
  "every E2E Python decision boundary must ignore ambient Python import state");
assert.match(e2eHarness, /deb_control_md5sums_sha256/);
assert.match(e2eHarness, /deb_control_members_sha256/);
assert.match(e2eHarness, /deb_control_fields_sha256/);
assert.match(e2eHarness, /deb_generated_list_sha256/);
assert.match(e2eHarness, /schema=pinvou-megabook-e2e-v3/);
assert.match(e2eHarness, /does not reconstruct or claim equality of the original archive compression/);
assert.match(e2eHarness, /\/var\/lib\/dpkg\/info\/pinvou3\.md5sums/);
assert.match(e2eHarness, /\/usr\/bin\/dpkg --verify pinvou3/);
assert.match(e2eHarness, /fixed payload specification count changed/);
assert.match(e2eHarness, /installed payload does not match the exact baseline deb/);
assert.match(debReadme, /control archive's complete[\s\S]*control\s+`md5sums` bytes/);
assert.match(debReadme, /`dpkg --verify pinvou3`/);
assert.match(e2eHarness, /megabook-profile-v2\.installing/);
assert.match(e2eHarness, /megabook-profile-v2\.applied/);
assert.match(e2eHarness, /\*\[!A-Za-z0-9\._-\]\*\).*unsafe character/);
assert.match(e2eHarness, /"\$SUPERVISOR" launch[\s\S]*send_fixed_launch "\$request_id"/);
assert.match(e2eHarness, /send_same_uid_asr_stop_negative/);
const stableAsrStopBody = e2eHarness.slice(
  e2eHarness.indexOf("wait_for_stable_asr_stop()"),
  e2eHarness.indexOf("wait_for_app_generation_change()"),
);
assert.match(stableAsrStopBody, /ActiveState[\s\S]*inactive/);
assert.match(stableAsrStopBody, /MainPID[\s\S]*= 0/);
assert.match(stableAsrStopBody, /\[ -z "\$\(unit_property "\$ASR_UNIT" InvocationID/);
assert.doesNotMatch(stableAsrStopBody, /expected_generation/);
assert.match(e2eHarness, /memory\.oom\.group/);
assert.match(e2eHarness, /raw\.rfind\("\) "\)[\s\S]*fields\[19\]/);
assert.match(e2eHarness, /pid_identity_retired/);
assert.doesNotMatch(e2eHarness, /\[ ! -e "\/proc\/\$max_(?:loader|main|webkit)_pid" \]/);
assert.match(e2eHarness, /wait_for_webkit_in_app_cgroup/);
assert.match(e2eHarness, /host:supervisor-app[\s\S]*governable[\s\S]*supportedActions/);
assert.match(e2eHarness,
  /owners\[work_id\] = \(work\.get\("owner"\), work\.get\("kind"\), work_generation\)/);
assert.match(e2eHarness,
  /work_identity != \("host:supervisor-asr", "asr_cgroup", generation\)/);
assert.match(e2eHarness, /claim_asserted[\s\S]*causationId[\s\S]*evidenceEventIds/);
assert.match(e2eHarness,
  /host_work_directive_dispatch_recorded[\s\S]*dispatch_sequence < ack_sequence < reconcile_sequence/);
assert.match(e2eHarness, /--show-cursor[\s\S]*--after-cursor/);
for (const counter of ["high", "max", "oom", "oom_kill", "oom_group_kill"]) {
  assert.ok(e2eHarness.includes(`\"${counter}\"`), `OOM evidence must include ${counter}`);
}
assert.match(e2eHarness, /prepare-purge\|verify-purged/);
assert.ok(e2eHarness.indexOf("fixed_stop_app") < e2eHarness.indexOf("remove_e2e_assets"));
assert.equal(
  fs.readdirSync(fixtureRoot).some((name) => name === "__pycache__" || name.endsWith(".pyc")),
  false,
  "fixture source must not contain Python bytecode",
);

console.log("linux supervisor packaging contract: ok");
