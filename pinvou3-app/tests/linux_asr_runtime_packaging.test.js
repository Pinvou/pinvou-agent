const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const { spawnSync } = require("node:child_process");

const {
  BUILD_SCRIPT,
  linuxAsrRuntimeOutput,
  prepareLinuxAsrRuntime,
} = require("../scripts/tauri/linux-asr-runtime.js");

const appRoot = path.resolve(__dirname, "..");
const repoRoot = path.resolve(appRoot, "..");
const trackedRuntime = "pinvou3-app/src-tauri/resources/platforms/linux/asr/sense-voice-main";
const senseVoiceLicense = path.join(
  appRoot,
  "src-tauri/resources/platforms/linux/asr/LICENSE-SenseVoice.cpp",
);

const sourceLock = fs.readFileSync(
  path.join(repoRoot, "scripts/asr/sensevoice-source.env"),
  "utf8",
);
assert.match(
  sourceLock,
  /^SENSEVOICE_SOURCE_COMMIT=[0-9a-f]{40}$/mu,
  "SenseVoice source must be pinned to an immutable commit",
);

// THIRD_PARTY_NOTICES.md names the pinned commit; keep it in lockstep with
// the source lock so a pin advance cannot leave the notice stale.
const pinnedCommit = sourceLock.match(/^SENSEVOICE_SOURCE_COMMIT=([0-9a-f]{40})$/mu)[1];
const senseVoiceNotice = fs
  .readFileSync(path.join(repoRoot, "THIRD_PARTY_NOTICES.md"), "utf8")
  .split("\n")
  .find((line) => line.startsWith("| SenseVoice.cpp |"));
assert.ok(senseVoiceNotice, "THIRD_PARTY_NOTICES.md must list SenseVoice.cpp");
assert.ok(
  senseVoiceNotice.includes(pinnedCommit),
  "THIRD_PARTY_NOTICES.md must name the SenseVoice.cpp commit pinned in sensevoice-source.env",
);

const trackedCheck = spawnSync(
  "git",
  ["ls-files", "--error-unmatch", trackedRuntime],
  { cwd: repoRoot, encoding: "utf8" },
);
assert.equal(
  trackedCheck.status,
  1,
  "precompiled SenseVoice ELF must not be stored in Git",
);
assert.match(fs.readFileSync(senseVoiceLicense, "utf8"), /Copyright \(c\) 2024 lovemefan/u);

const x64 = linuxAsrRuntimeOutput("x64");
assert.equal(x64.directory, "x86_64");
assert.match(x64.binaryPath.replaceAll("\\", "/"), /target\/linux-asr-runtime\/x86_64\/sense-voice-main$/u);
const arm64 = linuxAsrRuntimeOutput("arm64");
assert.equal(arm64.directory, "aarch64");
assert.match(arm64.binaryPath.replaceAll("\\", "/"), /target\/linux-asr-runtime\/aarch64\/sense-voice-main$/u);
assert.throws(() => linuxAsrRuntimeOutput("ia32"), /暂不支持 ia32/);

let invocation = null;
const prepared = prepareLinuxAsrRuntime({
  platform: "linux",
  architecture: "x64",
  environment: { PINVOU_TEST_ENV: "kept" },
  spawn: (command, args, options) => {
    invocation = { command, args, options };
    return { status: 0 };
  },
});
assert.equal(invocation.command, "bash");
assert.deepEqual(invocation.args, [
  BUILD_SCRIPT,
  "--output",
  x64.binaryPath,
  "--arch",
  "x86_64",
]);
assert.equal(invocation.options.cwd, repoRoot);
assert.equal(invocation.options.env.PINVOU_TEST_ENV, "kept");
assert.equal(prepared.binaryPath, x64.binaryPath);
assert.equal(
  prepareLinuxAsrRuntime({
    platform: "darwin",
    spawn: () => { throw new Error("non-Linux must not build SenseVoice"); },
  }),
  null,
);
assert.throws(
  () => prepareLinuxAsrRuntime({
    platform: "linux",
    architecture: "arm64",
    spawn: () => ({ status: 9 }),
  }),
  /退出码：9/,
);

// The shared Linux overlay is used for both architectures, so it maps the
// architecture-neutral ASR files one by one. Mapping the whole asr/ directory
// would also ship any stray local sense-voice-main (the gitignored legacy
// location) into both debs, regardless of its architecture.
const sharedLinuxConfig = JSON.parse(
  fs.readFileSync(path.join(appRoot, "src-tauri/config/platforms/linux/tauri.conf.json"), "utf8"),
);
const sharedAsrSources = Object.keys(sharedLinuxConfig.bundle.resources)
  .filter((source) => source.startsWith("resources/platforms/linux/asr/"))
  .sort((left, right) => left.localeCompare(right));
assert.deepEqual(sharedAsrSources, [
  "resources/platforms/linux/asr/LICENSE-SenseVoice.cpp",
  "resources/platforms/linux/asr/pinvou3-asr-shim.py",
]);

for (const [architecture, directory] of [["x64", "x86_64"], ["arm64", "aarch64"]]) {
  const configPath = path.join(
    appRoot,
    "src-tauri/config/platforms/linux",
    directory,
    "tauri.conf.json",
  );
  const config = JSON.parse(fs.readFileSync(configPath, "utf8"));
  assert.equal(
    config.bundle.resources[`target/linux-asr-runtime/${directory}/sense-voice-main`],
    "runtime/asr/sense-voice-main",
    `${architecture} overlay must stage its own SenseVoice ELF`,
  );
}

console.log("Linux SenseVoice runtime packaging contract: ok");
