#!/usr/bin/env node
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");

const appRoot = path.join(__dirname, "..");
const repoRoot = path.join(appRoot, "..");
const read = (relativePath) =>
  fs.readFileSync(path.join(repoRoot, relativePath), "utf8");

// The asr/ directory exclusion and the runtime/asr target ban are asserted by
// tauri_effective_config.test.js at both the effective-config and
// resource-manifest layers (stronger); not repeated here.

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
const macRelease = read(".github/workflows/release-packages.yml");
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
for (const [name, source] of [
  ["mac-build.yml", macBuild],
  ["release-packages.yml", macRelease],
]) {
  assert.match(
    source,
    /MACOSX_DEPLOYMENT_TARGET:\s*"11\.0"/,
    `${name} must match the declared macOS 11 minimum`,
  );
}

const infoPlist = read("pinvou3-app/src-tauri/packaging/macos/Info.plist");
assert.match(infoPlist, /NSSpeechRecognitionUsageDescription/);
assert.match(
  infoPlist,
  /<key>NSLocalNetworkUsageDescription<\/key>\s*<string>[^<\s][^<]*<\/string>/,
  "local network access must declare a non-empty privacy purpose",
);
assert.match(
  infoPlist,
  /Apple Speech 服务/,
  "privacy prompt must disclose possible Apple Speech service processing",
);
assert.match(
  verifyScript,
  /for usage_key in[^\n]*NSLocalNetworkUsageDescription/,
  "macOS verification must require the local network privacy purpose",
);
assert.match(
  verifyScript,
  /BUNDLED_INFO_PLIST[\s\S]*?NSLocalNetworkUsageDescription/,
  "macOS verification must inspect the bundled local network privacy purpose",
);

const requiredPrivacyKeys = [
  "NSMicrophoneUsageDescription",
  "NSSpeechRecognitionUsageDescription",
  "NSLocalNetworkUsageDescription",
];
for (const locale of ["en", "zh-Hans", "ja"]) {
  const strings = read(
    `pinvou3-app/src-tauri/resources/platforms/macos/infoplist/${locale}.lproj/InfoPlist.strings`,
  );
  for (const key of requiredPrivacyKeys) {
    assert.match(
      strings,
      new RegExp(`^${key}\\s*=\\s*"[^"\\r\\n]+";`, "m"),
      `${locale} InfoPlist.strings must localize ${key}`,
    );
  }
}

const macPlatform = read(
  "pinvou3-app/src-tauri/src/features/voice/platform/macos.rs",
);
assert.match(macPlatform, /pub fn asr_model_exists\(\) -> bool \{[\s\S]*?\btrue\b/);
assert.match(
  macPlatform,
  /pub fn asr_tool_exists\(\) -> bool \{[\s\S]*?\btrue\b/,
  "macOS system Speech must not be treated as an installable dependency",
);
assert.doesNotMatch(
  macPlatform,
  /pub fn asr_tool_exists\(\) -> bool \{[\s\S]*?speech_available\(\)/,
  "transient default-locale availability must not block voice recording",
);
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
