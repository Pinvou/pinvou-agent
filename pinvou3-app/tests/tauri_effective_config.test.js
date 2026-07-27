const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");

const {
  buildResourceManifest,
  composeEffectiveConfig,
  mergeConfig,
} = require("../scripts/tauri/effective-config.js");
const {
  configSpecs,
  prepareLinuxArm64Connectors,
  prepareLinuxCodexBridge,
  prepareWindowsCodexBridge,
  prepareTauriArgs,
  runTauri,
  tauriCommandIndex,
} = require("../scripts/tauri/build.js");
const {
  platformArchitectureConfigPath,
  platformConfigPath,
} = require("../scripts/tauri/platform-config.js");
const { requireWrapper, WRAPPER_ENV } = require("../scripts/tauri/require-wrapper.js");
const { WINDOWS_BRIDGE_CONFIG_PATH } = require("../scripts/tauri/codex-bridge.js");

let preparedBridge = null;
prepareLinuxCodexBridge({
  platform: "linux",
  spawn: (command, args, options) => {
    preparedBridge = { command, args, options };
    return { status: 0 };
  },
});
assert.match(preparedBridge.command, /prepare-codex-bridge-runtime\.sh$/);
assert.deepEqual(preparedBridge.args, []);
preparedBridge = null;
prepareLinuxCodexBridge({
  platform: "darwin",
  spawn: () => {
    throw new Error("macOS 不应准备 Linux Codex Bridge");
  },
});
assert.equal(preparedBridge, null);
assert.equal(
  prepareWindowsCodexBridge({ platform: "linux" }),
  false,
  "Linux 不应准备 Windows Codex Bridge",
);

assert.throws(() => requireWrapper({}), /禁止绕过平台 overlay/);
assert.doesNotThrow(() => requireWrapper({ [WRAPPER_ENV]: "1" }));

const buildArgs = prepareTauriArgs(
  ["--verbose", "build", "--bundles", "deb"],
  { platform: "linux" },
);
assert.equal(tauriCommandIndex(buildArgs), 1, "build command may follow global options");
assert.equal(configSpecs(buildArgs)[0], platformConfigPath("linux"));
const linuxArmArgs = prepareTauriArgs(
  ["build", "--bundles", "deb"],
  { platform: "linux", architecture: "arm64" },
);
assert.deepEqual(configSpecs(linuxArmArgs), [
  platformConfigPath("linux"),
]);

const explicitOverlay = "custom-signing.json";
const bundleArgs = prepareTauriArgs(
  ["bundle", "-c", explicitOverlay],
  { platform: "win32" },
);
assert.deepEqual(configSpecs(bundleArgs), [
  platformConfigPath("win32"),
  explicitOverlay,
]);
const windowsCodexArgs = prepareTauriArgs(
  ["build", "-c", explicitOverlay],
  {
    platform: "win32",
    additionalConfigs: [WINDOWS_BRIDGE_CONFIG_PATH],
  },
);
assert.deepEqual(configSpecs(windowsCodexArgs), [
  platformConfigPath("win32"),
  WINDOWS_BRIDGE_CONFIG_PATH,
  explicitOverlay,
]);
assert.deepEqual(
  prepareTauriArgs(["dev"], { platform: "linux" }),
  ["dev"],
  "dev must not receive packaging overlays",
);
const buildSource = fs.readFileSync(
  path.join(__dirname, "..", "scripts", "tauri", "build.js"),
  "utf8",
);
assert.match(
  buildSource,
  /if \(isDev\)[\s\S]*?prepareWindowsCodexBridge\(\)/,
  "Windows dev must prepare the ACP Bridge without packaging overlays",
);
let tauriInvocation = null;
assert.equal(
  runTauri(["--version"], (command, args, options) => {
    tauriInvocation = { command, args, options };
    return { status: 0 };
  }),
  0,
);
assert.equal(tauriInvocation.command, process.execPath);
assert.match(tauriInvocation.args[0], /@tauri-apps[\\/]cli[\\/]tauri\.js$/);
assert.equal(tauriInvocation.args[1], "--version");
assert.equal(tauriInvocation.options.env[WRAPPER_ENV], "1");

const linux = composeEffectiveConfig([platformConfigPath("linux")]).effectiveConfig;
assert.deepEqual(linux.bundle.targets, ["deb"]);
assert.match(linux.build.beforeBuildCommand, /require-wrapper\.js build/);
assert.match(
  linux.build.beforeBuildCommand,
  /npm run build:ui/,
  "release build must resolve Vite from the repository dependencies",
);
assert.doesNotMatch(
  linux.build.beforeBuildCommand,
  /&&\s+vite build/,
  "release build must not rely on a globally installed Vite binary",
);
assert.match(linux.build.beforeBundleCommand, /require-wrapper\.js bundle/);
assert.ok(linux.bundle.resources["resources/common/web-template/"]);
assert.equal(linux.bundle.resources["resources/platforms/linux/asr/"], "runtime/asr");
assert.equal(
  linux.bundle.resources["resources/platforms/linux/codex-bridge/"],
  "runtime/codex-bridge",
);
const linuxManifest = buildResourceManifest(linux, { platform: "linux" });
assert.ok(linuxManifest.resourceFileCount > 0);
assert.ok(linuxManifest.files.some((file) => file.destination.startsWith("web-template/")));
assert.ok(linuxManifest.files.some((file) => file.destination.startsWith("runtime/asr/")));
assert.ok(
  linuxManifest.files.some((file) => file.destination.startsWith("runtime/codex-bridge/")),
);

assert.equal(platformArchitectureConfigPath("linux", "arm64"), null);

const macos = composeEffectiveConfig([platformConfigPath("darwin")]).effectiveConfig;
assert.deepEqual(macos.bundle.targets, ["app", "dmg"]);
assert.ok(macos.bundle.resources["resources/common/web-template/"]);
assert.equal(
  macos.bundle.resources["resources/platforms/macos/aarch64/asr/"],
  undefined,
  "macOS system Speech must not bundle the legacy SenseVoice runtime",
);
const macosManifest = buildResourceManifest(macos, { platform: "darwin" });
assert.ok(
  !macosManifest.files.some((file) => file.destination.startsWith("runtime/asr/")),
  "macOS resource manifest must not contain a legacy ASR runtime",
);

const nullRemoval = mergeConfig(
  { bundle: { resources: { common: "common-target", runtime: "runtime-target" } } },
  { bundle: { resources: { runtime: null, staged: "" } } },
);
assert.deepEqual(nullRemoval.bundle.resources, { common: "common-target", staged: "" });

console.log("tauri effective config and installer resource manifest: ok");
