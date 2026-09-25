const assert = require("node:assert/strict");
const crypto = require("node:crypto");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

const {
  BASE_CONFIG_PATH,
  buildResourceManifest,
  composeEffectiveConfig,
  mergeConfig,
} = require("../scripts/tauri/effective-config.js");
const {
  TARGET_MACHINE_ARCHITECTURES,
  chromeDevtoolsMcpEnvironment,
  configSpecs,
  prepareCodexBridge,
  prepareChromeDevtoolsMcpForPlatform,
  prepareWindowsCodexBridge,
  prepareTauriArgs,
  runTauri,
  tauriRuntimeEnvironment,
  tauriCommandIndex,
  supportsChromeDevtoolsMcp,
} = require("../scripts/tauri/build.js");
const {
  platformArchitectureConfigPath,
  platformConfigPath,
} = require("../scripts/tauri/platform-config.js");
const {
  linuxStartupWindowConfig,
  linuxStartupWindowConfigSpec,
} = require("../scripts/tauri/startup-window-config.js");
const { requireWrapper, WRAPPER_ENV } = require("../scripts/tauri/require-wrapper.js");
const { WINDOWS_BRIDGE_CONFIG_PATH } = require("../scripts/tauri/codex-bridge.js");
const {
  ADAPTED_RESPONSE_SHA256,
  ADAPTER_VERSION,
  GITKEEP,
  applyTargetIdAdapter,
  assertTargetIdAdapterIntegrity,
  expectedMarker,
  isPreparedRoot,
} = require("../scripts/tauri/chrome-devtools-mcp.js");

assert.equal(ADAPTER_VERSION, "pinvou-target-id-v1");
assert.equal(
  ADAPTED_RESPONSE_SHA256,
  "e08698ba25c72b304152da1de99005d2415b9034c7edd46615d942dac174e0a6",
);
const thirdPartyNotices = fs.readFileSync(
  path.join(__dirname, "..", "..", "THIRD_PARTY_NOTICES.md"),
  "utf8",
);
const chromeDevtoolsMcpNotice = thirdPartyNotices.match(
  /- chrome-devtools-mcp: Modified by Pinvou Agent during vendoring:[\s\S]*?(?=\n- |\n\n)/,
)?.[0];
assert.ok(chromeDevtoolsMcpNotice, "the chrome-devtools-mcp adapter must be disclosed");
for (const requiredNoticeText of [
  "build/src/McpResponse.js",
  "target_id",
  "conversation and tab ownership",
  "SHA-256",
]) {
  assert.ok(
    chromeDevtoolsMcpNotice.includes(requiredNoticeText),
    `the chrome-devtools-mcp notice must include ${requiredNoticeText}`,
  );
}
assert.deepEqual(
  fs.readFileSync(
    path.join(
      __dirname,
      "..",
      "src-tauri",
      "resources",
      "platforms",
      "windows",
      "chrome-devtools-mcp",
      ".gitkeep",
    ),
  ),
  Buffer.from(GITKEEP, "utf8"),
  "the tracked placeholder must remain byte-for-byte identical to the vendor rewrite",
);
{
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "pinvou-cdmcp-adapter-"));
  const sourceDir = path.join(root, "build", "src");
  const responsePath = path.join(sourceDir, "McpResponse.js");
  fs.mkdirSync(sourceDir, { recursive: true });
  const originalSource = [
    "function createStructuredPage(mcpPage) {",
    "    const entry = {",
    "        id: mcpPage.id,",
    "        url: mcpPage.pptrPage.url(),",
    "    };",
    "}",
  ].join("\n");
  const adaptedSource = originalSource.replace(
    "        id: mcpPage.id,\n        url: mcpPage.pptrPage.url(),",
    "        id: mcpPage.id,\n        target_id: mcpPage.pptrPage.target()._targetId,\n        url: mcpPage.pptrPage.url(),",
  );
  const fixtureSha256 = crypto.createHash("sha256").update(adaptedSource).digest("hex");
  fs.writeFileSync(responsePath, originalSource);
  applyTargetIdAdapter(root, { expectedSha256: fixtureSha256 });
  applyTargetIdAdapter(root, { expectedSha256: fixtureSha256 }); // idempotent rebuild
  const adapted = fs.readFileSync(responsePath, "utf8");
  assert.equal(adapted, adaptedSource);
  assert.equal(
    adapted.split("target_id: mcpPage.pptrPage.target()._targetId").length - 1,
    1,
  );
  assert.equal(
    assertTargetIdAdapterIntegrity(root, { expectedSha256: fixtureSha256 }),
    fixtureSha256,
  );

  const entry = path.join(sourceDir, "bin", "chrome-devtools-mcp.js");
  fs.mkdirSync(path.dirname(entry), { recursive: true });
  fs.writeFileSync(entry, "// fixture");
  fs.writeFileSync(path.join(root, "catalog-shim.json"), "{}");
  const fixtureMarker = expectedMarker(fixtureSha256);
  fs.writeFileSync(
    path.join(root, ".vendor-version.json"),
    JSON.stringify(fixtureMarker, null, 2),
  );
  assert.equal(isPreparedRoot(root, fixtureMarker), true);

  fs.appendFileSync(responsePath, "\n// unexpected mutation");
  assert.throws(
    () => assertTargetIdAdapterIntegrity(root, { expectedSha256: fixtureSha256 }),
    /SHA-256 mismatch/,
  );
  assert.equal(isPreparedRoot(root, fixtureMarker), false);

  fs.writeFileSync(responsePath, adaptedSource);
  fs.writeFileSync(
    path.join(root, ".vendor-version.json"),
    JSON.stringify({ ...fixtureMarker, responseSha256: "0".repeat(64) }, null, 2),
  );
  assert.equal(isPreparedRoot(root, fixtureMarker), false);

  fs.writeFileSync(responsePath, "// upstream drift");
  assert.throws(
    () => applyTargetIdAdapter(root, { expectedSha256: fixtureSha256 }),
    /adapter anchor state/,
  );
  fs.rmSync(root, { recursive: true, force: true });
}

let preparedBridge = null;
prepareCodexBridge({
  platform: "linux",
  spawn: (command, args, options) => {
    preparedBridge = { command, args, options };
    return { status: 0 };
  },
});
assert.match(preparedBridge.command, /prepare-codex-bridge-runtime\.sh$/);
assert.deepEqual(preparedBridge.args, []);
preparedBridge = null;
prepareCodexBridge({
  platform: "darwin",
  spawn: (command, args, options) => {
    preparedBridge = { command, args, options };
    return { status: 0 };
  },
});
assert.match(preparedBridge.command, /prepare-codex-bridge-runtime\.sh$/);
assert.deepEqual(preparedBridge.args, []);
preparedBridge = null;
prepareCodexBridge({
  platform: "win32",
  spawn: () => {
    throw new Error("Windows 不应准备 Codex Bridge");
  },
});
assert.equal(preparedBridge, null);
assert.equal(
  prepareWindowsCodexBridge({ platform: "linux" }),
  false,
  "Linux 不应准备 Windows Codex Bridge",
);

// ---- Cross builds: targetArch -> PINVOU3_BRIDGE_TARGET_ARCH mapping and injection ----
//
// This chain is how cross builds prepare the bridge: build.js parses the Node
// notation targetArch from `--target`, codex-bridge.js maps it to a machine
// name in the environment, and the bridge script picks the Node distribution by
// that word. A reversed or missing mapping never fails the build; it silently
// prepares the wrong architecture, so every entry is pinned here.
function capturedBridgeSpawn(options) {
  let captured = null;
  prepareCodexBridge({
    ...options,
    spawn: (command, args, spawnOptions) => {
      captured = { command, args, options: spawnOptions };
      return { status: 0 };
    },
  });
  return captured;
}

const bridgeEnvFixture = { PINVOU_TEST_ENV: "kept" };
const hostBridgeSpawn = capturedBridgeSpawn({
  platform: "linux",
  env: bridgeEnvFixture,
});
// Non-cross builds fall back to uname -m: the rest of the environment passes
// through unchanged (deepEqual) but as a copy, because this path must drop a
// leftover PINVOU3_BRIDGE_TARGET_ARCH without touching the caller's object.
assert.deepEqual(
  hostBridgeSpawn.options.env,
  bridgeEnvFixture,
  "a non-cross build must pass the environment through so the script falls back to uname -m",
);
assert.notEqual(
  hostBridgeSpawn.options.env,
  bridgeEnvFixture,
  "the environment must be passed as a copy, never the caller's object",
);
assert.equal(
  hostBridgeSpawn.options.env.PINVOU3_BRIDGE_TARGET_ARCH,
  undefined,
  "without targetArch no target architecture may be injected",
);

const injectedBridgeMachines = new Set();
for (const [targetArch, machine] of [
  ["arm64", "aarch64"],
  ["x64", "x86_64"],
]) {
  for (const platform of ["linux", "darwin"]) {
    const crossBridgeSpawn = capturedBridgeSpawn({
      platform,
      targetArch,
      env: bridgeEnvFixture,
    });
    assert.equal(
      crossBridgeSpawn.options.env.PINVOU3_BRIDGE_TARGET_ARCH,
      machine,
      `cross building to ${targetArch} on ${platform} must inject ${machine}`,
    );
    assert.equal(
      crossBridgeSpawn.options.env.PINVOU_TEST_ENV,
      "kept",
      "injecting the target architecture must keep the rest of the environment",
    );
    injectedBridgeMachines.add(crossBridgeSpawn.options.env.PINVOU3_BRIDGE_TARGET_ARCH);
  }
}
assert.equal(
  bridgeEnvFixture.PINVOU3_BRIDGE_TARGET_ARCH,
  undefined,
  "injection must copy the environment, never mutate the caller's env",
);
// An arch the machine map does not know falls back to host behavior: the rest
// of the environment passes through (as a copy), and a leftover
// PINVOU3_BRIDGE_TARGET_ARCH from the caller's shell must be dropped. That
// variable is the script's manual switch; passing a leftover value through
// would prepare another architecture's Node for a non-cross build while the
// overlay still follows process.arch. The counter-example is loongarch64, a
// segment unlikely to ever be supported (riscv64 might, and would break this).
const leftoverBridgeEnv = { ...bridgeEnvFixture, PINVOU3_BRIDGE_TARGET_ARCH: "x86_64" };
const fallbackBridgeEnv = capturedBridgeSpawn({
  platform: "linux",
  targetArch: "loongarch64",
  env: leftoverBridgeEnv,
}).options.env;
assert.deepEqual(
  fallbackBridgeEnv,
  bridgeEnvFixture,
  "the fallback passes the rest through and must drop a leftover PINVOU3_BRIDGE_TARGET_ARCH",
);
assert.notEqual(
  fallbackBridgeEnv,
  leftoverBridgeEnv,
  "the fallback must also copy the environment",
);
assert.equal(
  leftoverBridgeEnv.PINVOU3_BRIDGE_TARGET_ARCH,
  "x86_64",
  "the leftover belongs to the caller: only the copy may be cleaned",
);

// Every injected word must be in the bridge script's target list, on both OSes.
//
// This pins the Darwin alias: triples spell macOS arm64 as aarch64
// (aarch64-apple-darwin) while macOS `uname -m` reports arm64. A script that
// only accepted `Darwin-arm64` would fail a manual single-architecture build on
// a darwin host; CI only uses universal-apple-darwin (nothing injected), so no
// other test would notice.
const bridgeScriptSource = fs.readFileSync(
  path.join(__dirname, "..", "scripts", "prepare-codex-bridge-runtime.sh"),
  "utf8",
);
const bridgeTargetDispatch = bridgeScriptSource.match(
  /case "\$OS_NAME-\$TARGET_MACHINE" in([\s\S]*?)\nesac/u,
)?.[1];
assert.ok(
  bridgeTargetDispatch,
  "the bridge script must keep its $OS_NAME-$TARGET_MACHINE target dispatch",
);
const acceptedBridgeTargets = new Set(
  (bridgeTargetDispatch.match(/^[ \t]*[A-Za-z0-9_|-]+\)/gmu) ?? []).flatMap((label) =>
    label.trim().slice(0, -1).split("|"),
  ),
);
for (const machine of injectedBridgeMachines) {
  for (const osName of ["Linux", "Darwin"]) {
    assert.ok(
      acceptedBridgeTargets.has(`${osName}-${machine}`),
      `the bridge script must accept ${osName}-${machine}, or cross builds fall into *)`,
    );
  }
}

// Adding an architecture must extend build.js's table and codex-bridge.js's
// machine map together.
//
// The loop above hard-codes arm64->aarch64 / x64->x86_64 and pins the values;
// adding loongarch64 to TARGET_MACHINE_ARCHITECTURES without touching the
// machine map would keep it green. That is the hole: machine becomes
// undefined, nothing is injected, the script falls back to `uname -m` and the
// package ships a host-architecture bridge Node with no error. So both sides
// are derived from the sources here: every Node arch the table can produce,
// whether prepareCodexBridge really injects something for it, and whether the
// injected word is in the script's target list.
for (const targetArch of new Set(Object.values(TARGET_MACHINE_ARCHITECTURES))) {
  const injected = capturedBridgeSpawn({
    platform: "linux",
    targetArch,
    env: bridgeEnvFixture,
  }).options.env.PINVOU3_BRIDGE_TARGET_ARCH;
  assert.ok(
    injected,
    `build.js TARGET_MACHINE_ARCHITECTURES can produce ${targetArch}, but the machine map in`
      + " codex-bridge.js lacks it: cross builds would silently prepare a host-architecture"
      + " Node. Both tables must change together.",
  );
  for (const osName of ["Linux", "Darwin"]) {
    assert.ok(
      acceptedBridgeTargets.has(`${osName}-${injected}`),
      `${targetArch} injects ${injected}, which the bridge script does not accept on ${osName};`
        + " cross builds would fall into *)",
    );
  }
}

// One more derived link: every target architecture build.js can produce must
// have a Linux architecture overlay in platform-config.js. Otherwise
// platformArchitectureConfigPath returns null and prepareTauriArgs silently
// skips the whole architecture overlay - only this assertion would notice.
for (const targetArch of new Set(Object.values(TARGET_MACHINE_ARCHITECTURES))) {
  assert.ok(
    platformArchitectureConfigPath("linux", targetArch) !== null,
    `build.js TARGET_MACHINE_ARCHITECTURES can produce ${targetArch}, but platform-config.js`
      + " has no Linux architecture overlay for it: the overlay would be skipped silently."
      + " Both tables must change together.",
  );
}

assert.throws(() => requireWrapper({}), /禁止绕过平台 overlay/);
assert.doesNotThrow(() => requireWrapper({ [WRAPPER_ENV]: "1" }));

const linuxStartupOverlay = linuxStartupWindowConfigSpec();
const buildArgs = prepareTauriArgs(
  ["--verbose", "build", "--bundles", "deb"],
  { platform: "linux", architecture: "x64" },
);
assert.equal(tauriCommandIndex(buildArgs), 1, "build command may follow global options");
assert.deepEqual(configSpecs(buildArgs), [
  platformConfigPath("linux"),
  linuxStartupOverlay,
  platformArchitectureConfigPath("linux", "x64"),
]);
const linuxArmArgs = prepareTauriArgs(
  ["build", "--bundles", "deb"],
  { platform: "linux", architecture: "arm64" },
);
assert.deepEqual(configSpecs(linuxArmArgs), [
  platformConfigPath("linux"),
  linuxStartupOverlay,
  platformArchitectureConfigPath("linux", "arm64"),
]);

const explicitOverlay = "custom-signing.json";
const windowsRuntimeOverlay = "target/windows-runtime/tauri.generated.conf.json";
const bundleArgs = prepareTauriArgs(
  ["bundle", "-c", explicitOverlay],
  { platform: "win32", stageRuntime: () => null },
);
assert.deepEqual(configSpecs(bundleArgs), [
  platformConfigPath("win32"),
  explicitOverlay,
]);
const windowsCodexArgs = prepareTauriArgs(
  ["build", "-c", explicitOverlay],
  {
    platform: "win32",
    stageRuntime: () => ({ configPath: windowsRuntimeOverlay }),
    additionalConfigs: [WINDOWS_BRIDGE_CONFIG_PATH],
  },
);
assert.deepEqual(configSpecs(windowsCodexArgs), [
  platformConfigPath("win32"),
  windowsRuntimeOverlay,
  WINDOWS_BRIDGE_CONFIG_PATH,
  explicitOverlay,
]);
assert.deepEqual(
  prepareTauriArgs(["dev"], { platform: "linux" }),
  ["dev", "--config", linuxStartupOverlay],
  "Linux dev must hide the main window until the first React commit",
);
assert.deepEqual(
  prepareTauriArgs(["dev"], { platform: "win32" }),
  ["dev"],
  "Windows dev must not receive packaging overlays",
);
assert.deepEqual(
  configSpecs(prepareTauriArgs(["dev", "-c", explicitOverlay], { platform: "linux" })),
  [linuxStartupOverlay, explicitOverlay],
  "explicit Linux dev overlays must override the automatic cold-start overlay",
);
const linuxKnowledgeHostDevOverlay = JSON.stringify({
  bundle: { resources: { "target/knowledge-host-dev/": "runtime/knowledge-host" } },
});
assert.deepEqual(
  configSpecs(prepareTauriArgs(["dev", "-c", explicitOverlay], {
    platform: "linux",
    additionalConfigs: [linuxKnowledgeHostDevOverlay],
  })),
  [linuxStartupOverlay, linuxKnowledgeHostDevOverlay, explicitOverlay],
  "Linux dev host resources must be injected before caller overrides",
);
assert.deepEqual(
  prepareTauriArgs(["dev"], { platform: "darwin" }),
  ["dev", "--config", platformConfigPath("darwin")],
  "macOS dev must receive the platform overlay (native titlebar) to match packaged output",
);
assert.deepEqual(
  prepareTauriArgs(["dev", "--features", "browser-macos-preview"], { platform: "darwin" }),
  [
    "dev",
    "--config",
    platformConfigPath("darwin"),
    "--features",
    "browser-macos-preview",
  ],
  "the isolated macOS BrowserCore preview feature must reach the Tauri Cargo build unchanged",
);
assert.deepEqual(
  prepareTauriArgs(
    ["build", "--features", "browser-macos-preview", "--target", "universal-apple-darwin"],
    { platform: "darwin" },
  ),
  [
    "build",
    "--config",
    platformConfigPath("darwin"),
    "--features",
    "browser-macos-preview",
    "--target",
    "universal-apple-darwin",
  ],
  "preview packaging must remain an explicit opt-in instead of changing normal macOS builds",
);
assert.deepEqual(
  configSpecs(prepareTauriArgs(["dev", "-c", explicitOverlay], { platform: "darwin" })),
  [platformConfigPath("darwin"), explicitOverlay],
  "explicit macOS dev overlays must override the automatic platform overlay",
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
// Pin the order, not the exact line: preparation must happen before the
// resource manifest is written, or the bundle fails with a missing resource.
assert.ok(
  buildSource.includes("prepareLinuxAsrRuntime()"),
  "Linux packaging must still prepare the SenseVoice runtime",
);
assert.ok(
  buildSource.indexOf("prepareLinuxAsrRuntime()") < buildSource.indexOf("writeEffectiveArtifacts("),
  "Linux packaging must prepare the architecture-specific SenseVoice runtime before manifest generation",
);
// Cross builds must be able to skip ASR (SenseVoice only builds natively).
assert.match(buildSource, /skipLinuxAsr/u, "the ASR step must stay skippable for cross builds");
// The mapping tests above prove that a given targetArch injects the right
// word; this pins the other end of the wiring: the parsed target architecture
// is handed to the bridge preparation. Loose on purpose (targetArch anywhere in
// the call) so equivalent refactors pass while a broken chain fails.
assert.match(
  buildSource,
  /prepareCodexBridge\([^)]*targetArch/u,
  "the bridge must be prepared for the parsed target architecture, or cross builds silently package a host-architecture Node",
);
const preparedBrowserPlatforms = [];
for (const platform of ["win32", "darwin", "linux"]) {
  const result = prepareChromeDevtoolsMcpForPlatform({
    platform,
    prepare: (options) => {
      preparedBrowserPlatforms.push(options.platform);
      return "prepared";
    },
  });
  assert.equal(
    result,
    platform === "win32" ? "prepared" : false,
    `${platform} must follow its declared Chrome MCP packaging capability`,
  );
  assert.equal(supportsChromeDevtoolsMcp(platform), platform === "win32");
}
assert.deepEqual(
  preparedBrowserPlatforms,
  ["win32"],
  "only the Windows WebView2 backend may prepare chrome-devtools-mcp",
);
const browserDevEnvironment = chromeDevtoolsMcpEnvironment(
  true,
  { PINVOU_TEST_ENV: "kept" },
  "win32",
);
assert.equal(browserDevEnvironment.PINVOU_TEST_ENV, "kept");
assert.match(
  browserDevEnvironment.PINVOU3_CDMCP_BIN,
  /resources[\\/]platforms[\\/]windows[\\/]chrome-devtools-mcp[\\/]build[\\/]src[\\/]bin[\\/]chrome-devtools-mcp\.js$/,
);
for (const platform of ["darwin", "linux"]) {
  const displayOnlyEnvironment = chromeDevtoolsMcpEnvironment(
    true,
    { PINVOU_TEST_ENV: "kept", PINVOU3_CDMCP_BIN: "stale-external-entry" },
    platform,
  );
  assert.deepEqual(
    displayOnlyEnvironment,
    { PINVOU_TEST_ENV: "kept" },
    `${platform} inactive browser substrate must neither inject nor inherit a Chrome MCP entry`,
  );
}
assert.deepEqual(
  chromeDevtoolsMcpEnvironment(
    false,
    { PINVOU_TEST_ENV: "kept", PINVOU3_CDMCP_BIN: "stale-external-entry" },
    "win32",
  ),
  { PINVOU_TEST_ENV: "kept" },
  "packaged Windows builds must resolve chrome-devtools-mcp only from the app resource directory",
);
let tauriInvocation = null;
const tauriEnvironment = { PINVOU_TEST_ENV: "kept" };
assert.equal(
  runTauri(["--version"], (command, args, options) => {
    tauriInvocation = { command, args, options };
    return { status: 0 };
  }, tauriEnvironment),
  0,
);
assert.equal(tauriInvocation.command, process.execPath);
assert.match(tauriInvocation.args[0], /@tauri-apps[\\/]cli[\\/]tauri\.js$/);
assert.equal(tauriInvocation.args[1], "--version");
assert.equal(tauriInvocation.options.env[WRAPPER_ENV], "1");
assert.equal(tauriInvocation.options.env.PINVOU_TEST_ENV, "kept");
const ortEnvironment = tauriRuntimeEnvironment(
  { onnxRuntimeDylib: "C:\\runtime\\onnxruntime.dll" },
  tauriEnvironment,
);
assert.equal(ortEnvironment.PINVOU_TEST_ENV, "kept");
assert.equal(ortEnvironment.ORT_DYLIB_PATH, "C:\\runtime\\onnxruntime.dll");

const linux = composeEffectiveConfig([
  platformConfigPath("linux"),
  linuxStartupOverlay,
]).effectiveConfig;
assert.deepEqual(linux.bundle.targets, ["deb"]);
assert.equal(linux.app.windows[0].visible, false);
assert.match(linux.app.windows[0].url, /[?&]startupWindow=hidden(?:&|$)/);
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
assert.equal(linux.bundle.resources["resources/platforms/linux/asr/"], "runtime/asr");
assert.equal(
  linux.bundle.resources["resources/platforms/linux/knowledge-host/"],
  "runtime/knowledge-host",
);
assert.equal(
  linux.bundle.resources["resources/platforms/linux/codex-bridge/"],
  "runtime/codex-bridge",
);
assert.equal(
  linux.bundle.resources["resources/platforms/linux/chrome-devtools-mcp/"],
  undefined,
  "Linux BrowserCore must not package the Windows-only chrome-devtools-mcp backend",
);
assert.ok(
  linux.bundle.linux.deb.depends.includes("webkit2gtk-driver"),
  "Linux BrowserCore packages must install the WebKitGTK WebDriver backend",
);
const linuxManifest = buildResourceManifest(linux, { platform: "linux" });
assert.ok(linuxManifest.resourceFileCount > 0);
assert.ok(linuxManifest.files.some((file) => file.destination.startsWith("runtime/asr/")));
assert.ok(
  linuxManifest.files.some((file) => file.destination.startsWith("runtime/codex-bridge/")),
);
assert.ok(
  !linuxManifest.files.some((file) => file.destination.startsWith("runtime/chrome-devtools-mcp/")),
  "Linux resource manifest must exclude chrome-devtools-mcp",
);

assert.match(
  platformArchitectureConfigPath("linux", "x64").replaceAll("\\", "/"),
  /platforms\/linux\/x86_64\/tauri\.conf\.json$/u,
);
assert.match(
  platformArchitectureConfigPath("linux", "arm64").replaceAll("\\", "/"),
  /platforms\/linux\/aarch64\/tauri\.conf\.json$/u,
);

const linuxDev = composeEffectiveConfig([linuxStartupOverlay]).effectiveConfig;
assert.equal(linuxDev.app.windows[0].visible, false);
assert.match(linuxDev.app.windows[0].url, /[?&]startupWindow=hidden(?:&|$)/);
const baseMainWindow = composeEffectiveConfig([]).effectiveConfig.app.windows[0];
for (const [label, config] of [["packaging", linux], ["dev", linuxDev]]) {
  const mainWindow = { ...config.app.windows[0], visible: undefined };
  mainWindow.url = mainWindow.url.replace("&startupWindow=hidden", "");
  assert.deepEqual(
    mainWindow,
    { ...baseMainWindow, visible: undefined },
    `Linux ${label} must only override the main-window cold-start controls`,
  );
}

const generatedFromChangedBase = linuxStartupWindowConfig({
  readFile: () => JSON.stringify({
    app: { windows: [{ label: "main", url: "index.html", width: 1234 }] },
  }),
});
assert.equal(generatedFromChangedBase.app.windows[0].width, 1234);
assert.equal(generatedFromChangedBase.app.windows[0].visible, false);
assert.equal(
  generatedFromChangedBase.app.windows[0].url,
  "index.html?startupWindow=hidden",
  "Linux startup overlay must derive window properties from the base config",
);

const macos = composeEffectiveConfig([platformConfigPath("darwin")]).effectiveConfig;
assert.deepEqual(macos.bundle.targets, ["app", "dmg"]);
assert.equal(
  macos.bundle.resources["resources/platforms/macos/codex-bridge/"],
  "runtime/codex-bridge",
);
assert.equal(
  macos.bundle.resources["resources/platforms/macos/infoplist/"],
  "./",
  "macOS must bundle localized privacy purpose strings",
);
assert.equal(
  macos.bundle.resources["resources/platforms/macos/aarch64/asr/"],
  undefined,
  "macOS system Speech must not bundle the legacy SenseVoice runtime",
);
assert.equal(
  macos.bundle.resources["resources/platforms/macos/chrome-devtools-mcp/"],
  undefined,
  "macOS BrowserCore packages must not carry chrome-devtools-mcp",
);
const macosManifest = buildResourceManifest(macos, { platform: "darwin" });
assert.ok(
  macosManifest.files.some((file) => file.destination.startsWith("runtime/codex-bridge/")),
  "macOS resource manifest must contain the Codex ACP Bridge runtime",
);
for (const locale of ["en", "zh-Hans", "ja"]) {
  assert.ok(
    macosManifest.files.some(
      (file) => file.destination === `${locale}.lproj/InfoPlist.strings`,
    ),
    `macOS resource manifest must contain ${locale} privacy purpose strings`,
  );
}
assert.ok(
  !macosManifest.files.some((file) => file.destination.startsWith("runtime/asr/")),
  "macOS resource manifest must not contain a legacy ASR runtime",
);
assert.ok(
  !macosManifest.files.some((file) => file.destination.startsWith("runtime/chrome-devtools-mcp/")),
  "macOS resource manifest must exclude chrome-devtools-mcp",
);

const windows = composeEffectiveConfig([platformConfigPath("win32")]).effectiveConfig;
assert.equal(
  windows.bundle.resources["resources/platforms/windows/chrome-devtools-mcp/"],
  "runtime/chrome-devtools-mcp",
  "Windows must package the adapter used by the app-owned WebView2 CDP endpoint",
);
const windowsManifest = buildResourceManifest(windows, { platform: "win32" });
assert.ok(
  windowsManifest.files.some((file) => file.destination.startsWith("runtime/chrome-devtools-mcp/")),
  "Windows resource manifest must contain chrome-devtools-mcp",
);

const runtimeBundleExtraction = fs.readFileSync(
  path.join(
    __dirname,
    "..",
    "src-tauri",
    "src",
    "features",
    "runtime_bundle",
    "platform",
    "extraction.rs",
  ),
  "utf8",
);
assert.match(
  runtimeBundleExtraction,
  /#\[cfg\(any\(target_os = "linux", target_os = "macos"\)\)\][\s\S]*?fn browser_mcp_entry_for_session[\s\S]*?@pinvou\/browser-core/,
  "Linux and macOS must register the unified Pinvou BrowserCore wrapper",
);
assert.match(
  runtimeBundleExtraction,
  /#\[cfg\(target_os = "linux"\)\]\s*find_webkit_webdriver\(\)\?;/,
  "Linux BrowserCore must keep its WebKitWebDriver runtime gate",
);
assert.match(
  runtimeBundleExtraction,
  /#\[cfg\(target_os = "windows"\)\]\s*fn browser_mcp_entry_for_session[\s\S]*?PINVOU3_CDMCP_BIN/,
  "the chrome-devtools-mcp environment override must remain inside the Windows-only path",
);

// macOS 主窗口走系统原生红绿灯顶栏(titleBarStyle=Overlay),前端据此隐藏自绘三键。
// --config overlay 按 JSON Merge Patch 合并,windows 数组整体替换,因此 overlay 必须
// 携带完整窗口定义。按基础数组动态生成期望值,确保新增窗口或新增字段也会触发
// 防漂移失败,而不是依赖容易漏项的固定字段清单。
const baseWindows = JSON.parse(fs.readFileSync(BASE_CONFIG_PATH, "utf8")).app.windows;
const expectedMacosWindows = baseWindows.map((window) => (
  window.label === "main"
    ? {
        ...window,
        decorations: true,
        titleBarStyle: "Overlay",
        hiddenTitle: true,
        trafficLightPosition: { x: 12, y: 20 },
      }
    : window
));
assert.deepEqual(
  macos.app.windows,
  expectedMacosWindows,
  "macOS overlay 必须完整同步基础窗口数组,且只覆盖主窗口的原生顶栏字段",
);

const nullRemoval = mergeConfig(
  { bundle: { resources: { common: "common-target", runtime: "runtime-target" } } },
  { bundle: { resources: { runtime: null, staged: "" } } },
);
assert.deepEqual(nullRemoval.bundle.resources, { common: "common-target", staged: "" });

console.log("tauri effective config and installer resource manifest: ok");
