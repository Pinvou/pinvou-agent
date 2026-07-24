#!/usr/bin/env node
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");

const appRoot = path.join(__dirname, "..");
const repoRoot = path.join(appRoot, "..");
const read = (relativePath) =>
  fs.readFileSync(path.join(repoRoot, relativePath), "utf8");

const macConfig = JSON.parse(
  read("pinvou3-app/src-tauri/config/platforms/macos/tauri.conf.json"),
);
const macResources = macConfig.bundle.resources || {};
assert.equal(
  macResources["resources/platforms/macos/aarch64/asr/"],
  undefined,
  "macOS must not bundle the legacy SenseVoice runtime",
);
assert.ok(
  !Object.values(macResources).some((destination) => destination === "runtime/asr"),
  "macOS resources must not target runtime/asr",
);

const legacyAsrDir = path.join(
  appRoot,
  "src-tauri",
  "resources",
  "platforms",
  "macos",
  "aarch64",
  "asr",
);
assert.ok(
  !fs.existsSync(legacyAsrDir) ||
    !fs.readdirSync(legacyAsrDir).some((name) => name.includes("sense-voice")),
  "legacy macOS SenseVoice artifacts must be removed",
);

const releaseScript = read("scripts/release-macos.sh");
const verifyScript = read("scripts/run-mac-verify.sh");
for (const [name, source] of [
  ["release-macos.sh", releaseScript],
  ["run-mac-verify.sh", verifyScript],
]) {
  assert.doesNotMatch(source, /sense-voice-darwin-arm64/i, `${name} is stale`);
  assert.match(
    source,
    /Contents\/MacOS\/pinvou3-tauri/,
    `${name} must verify the real Tauri main binary`,
  );
}
assert.match(
  releaseScript,
  /if \[ ! -f "\$APP_BIN" \]; then[\s\S]*?exit 1[\s\S]*?fi/,
  "release must fail when the main binary is missing",
);

const macBuild = read(".github/workflows/mac-build.yml");
const bundleStart = macBuild.indexOf("- name: Tauri bundle smoke");
const verifyStart = macBuild.indexOf("- name: Verify 脚本", bundleStart);
assert.ok(bundleStart >= 0 && verifyStart > bundleStart, "macOS bundle steps must exist");
const bundleStep = macBuild.slice(bundleStart, verifyStart);
assert.match(bundleStep, /--target universal-apple-darwin/);
assert.doesNotMatch(
  bundleStep,
  /continue-on-error:\s*true/,
  "Universal bundle smoke must fail the main mac-build job",
);

const infoPlist = read("pinvou3-app/src-tauri/packaging/macos/Info.plist");
assert.match(infoPlist, /NSSpeechRecognitionUsageDescription/);
assert.match(
  infoPlist,
  /Apple Speech 服务/,
  "privacy prompt must disclose possible Apple Speech service processing",
);

const macPlatform = read(
  "pinvou3-app/src-tauri/src/features/voice/platform/macos.rs",
);
assert.match(macPlatform, /pub fn asr_model_exists\(\) -> bool \{[\s\S]*?\btrue\b/);
assert.match(
  macPlatform,
  /pub fn asr_bundled_runtime_status\(\) -> Option<bool> \{[\s\S]*?Some\(asr_tool_exists\(\)\)/,
);
assert.match(
  macPlatform,
  /pub fn asr_dependency_installable\(\) -> bool \{[\s\S]*?\bfalse\b/,
);

const linuxPlatform = read(
  "pinvou3-app/src-tauri/src/features/voice/platform/linux.rs",
);
assert.match(
  linuxPlatform,
  /voice_asr::engine_path\(\)\.is_file\(\)[\s\S]*?voice_asr::model_path\(\)\.is_file\(\)/,
  "Linux native recognition must only use the managed engine/model",
);

const windowsPlatform = read(
  "pinvou3-app/src-tauri/src/features/voice/platform/windows.rs",
);
assert.match(
  windowsPlatform,
  /pub fn recognize_native\([\s\S]*?\) -> Option<Result<String, String>> \{\s*None\s*\}/,
  "Windows bundled ASR must stay on the CLI path",
);

const voiceCommand = read("pinvou3-app/src-tauri/src/app/commands/voice.rs");
for (const envName of [
  "PINVOU3_ASR_CMD",
  "PINVOU3_DEEPSPEECH2_CMD",
  "PADDLESPEECH_BIN",
]) {
  assert.match(
    voiceCommand,
    new RegExp(`std::env::var\\("${envName}"\\)`),
    `${envName} must enable explicit CLI fallback`,
  );
}

console.log("macOS phase 2 packaging and ASR contracts: ok");
