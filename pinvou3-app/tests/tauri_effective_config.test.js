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
  prepareTauriArgs,
  tauriCommandIndex,
} = require("../scripts/tauri/build.js");
const {
  APP_ROOT,
  platformConfigPath,
} = require("../scripts/tauri/platform-config.js");
const { requireWrapper, WRAPPER_ENV } = require("../scripts/tauri/require-wrapper.js");

const noRuntime = () => null;

assert.throws(() => requireWrapper({}), /禁止绕过平台 overlay/);
assert.doesNotThrow(() => requireWrapper({ [WRAPPER_ENV]: "1" }));

const buildArgs = prepareTauriArgs(
  ["--verbose", "build", "--bundles", "deb"],
  { platform: "linux", stageRuntime: noRuntime },
);
assert.equal(tauriCommandIndex(buildArgs), 1, "build command may follow global options");
assert.equal(configSpecs(buildArgs)[0], platformConfigPath("linux"));

const explicitOverlay = "src-tauri/config/platforms/windows/signing.wosign.conf.json";
const bundleArgs = prepareTauriArgs(
  ["bundle", "-c", explicitOverlay],
  { platform: "win32", stageRuntime: () => "runtime.generated.json" },
);
assert.deepEqual(configSpecs(bundleArgs), [
  platformConfigPath("win32"),
  "runtime.generated.json",
  explicitOverlay,
]);
assert.deepEqual(
  prepareTauriArgs(["dev"], { platform: "linux", stageRuntime: noRuntime }),
  ["dev"],
  "dev must not receive packaging overlays",
);

const linux = composeEffectiveConfig([platformConfigPath("linux")]).effectiveConfig;
assert.deepEqual(linux.bundle.targets, ["deb"]);
assert.match(linux.build.beforeBuildCommand, /require-wrapper\.js build/);
assert.match(linux.build.beforeBundleCommand, /require-wrapper\.js bundle/);
assert.ok(linux.bundle.resources["resources/common/web-template/"]);
assert.equal(linux.bundle.resources["resources/platforms/linux/asr/"], "runtime/asr");
const linuxManifest = buildResourceManifest(linux, { platform: "linux" });
assert.ok(linuxManifest.resourceFileCount > 0);
assert.ok(linuxManifest.files.some((file) => file.destination.startsWith("web-template/")));
assert.ok(linuxManifest.files.some((file) => file.destination.startsWith("runtime/asr/")));

const macos = composeEffectiveConfig([platformConfigPath("darwin")]).effectiveConfig;
assert.deepEqual(macos.bundle.targets, ["dmg"]);
assert.ok(macos.bundle.resources["resources/common/web-template/"]);

const nullRemoval = mergeConfig(
  { bundle: { resources: { common: "common-target", runtime: "runtime-target" } } },
  { bundle: { resources: { runtime: null, staged: "" } } },
);
assert.deepEqual(nullRemoval.bundle.resources, { common: "common-target", staged: "" });

// The private runtime stays optional in Linux/default CI. When it has been
// staged by a Windows build, validate the exact generated overlay as well.
const generatedRuntimeConfig = path.join(
  APP_ROOT,
  "src-tauri",
  "target",
  "windows-runtime",
  "tauri.generated.conf.json",
);
if (fs.existsSync(generatedRuntimeConfig)) {
  const windows = composeEffectiveConfig([
    platformConfigPath("win32"),
    generatedRuntimeConfig,
  ]).effectiveConfig;
  const windowsManifest = buildResourceManifest(windows, { platform: "win32" });
  const destinations = windowsManifest.files.map((file) => file.destination);
  for (const requiredPrefix of [
    "runtime/7zip/",
    "runtime/asr/",
    "runtime/node/",
    "runtime/onnxruntime/",
    "runtime/pandoc/",
    "runtime/poppler/",
    "runtime/python/",
    "runtime/tesseract/",
  ]) {
    assert.ok(
      destinations.some((destination) => destination.startsWith(requiredPrefix)),
      `staged Windows resource manifest must contain ${requiredPrefix}`,
    );
  }
}

console.log("tauri effective config and installer resource manifest: ok");
