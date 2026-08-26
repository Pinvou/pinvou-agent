const assert = require("node:assert/strict");
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
  ADAPTER_VERSION,
  applyTargetIdAdapter,
} = require("../scripts/tauri/chrome-devtools-mcp.js");

assert.equal(ADAPTER_VERSION, "pinvou-target-id-v1");
{
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "pinvou-cdmcp-adapter-"));
  const sourceDir = path.join(root, "build", "src");
  const responsePath = path.join(sourceDir, "McpResponse.js");
  fs.mkdirSync(sourceDir, { recursive: true });
  fs.writeFileSync(
    responsePath,
    [
      "function createStructuredPage(mcpPage) {",
      "    const entry = {",
      "        id: mcpPage.id,",
      "        url: mcpPage.pptrPage.url(),",
      "    };",
      "}",
    ].join("\n"),
  );
  applyTargetIdAdapter(root);
  applyTargetIdAdapter(root); // 幂等：构建重试不能重复插入字段
  const adapted = fs.readFileSync(responsePath, "utf8");
  assert.equal(
    adapted.split("target_id: mcpPage.pptrPage.target()._targetId").length - 1,
    1,
  );
  fs.writeFileSync(responsePath, "// upstream drift");
  assert.throws(() => applyTargetIdAdapter(root), /adapter 锚点异常/);
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

assert.throws(() => requireWrapper({}), /禁止绕过平台 overlay/);
assert.doesNotThrow(() => requireWrapper({ [WRAPPER_ENV]: "1" }));

const linuxStartupOverlay = linuxStartupWindowConfigSpec();
const buildArgs = prepareTauriArgs(
  ["--verbose", "build", "--bundles", "deb"],
  { platform: "linux" },
);
assert.equal(tauriCommandIndex(buildArgs), 1, "build command may follow global options");
assert.deepEqual(configSpecs(buildArgs), [
  platformConfigPath("linux"),
  linuxStartupOverlay,
]);
const linuxArmArgs = prepareTauriArgs(
  ["build", "--bundles", "deb"],
  { platform: "linux", architecture: "arm64" },
);
assert.deepEqual(configSpecs(linuxArmArgs), [
  platformConfigPath("linux"),
  linuxStartupOverlay,
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

assert.equal(platformArchitectureConfigPath("linux", "arm64"), null);

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

const macosEntitlementsPath = path.join(
  __dirname,
  "..",
  "src-tauri",
  "packaging",
  "macos",
  "entitlements.plist",
);
const macosEntitlements = fs.readFileSync(macosEntitlementsPath);
const macosEntitlementsText = macosEntitlements.toString("utf8");
assert.ok(
  !macosEntitlements.subarray(0, 3).equals(Buffer.from([0xef, 0xbb, 0xbf])),
  "codesign entitlements must not contain a UTF-8 BOM",
);
assert.ok(
  !macosEntitlementsText.includes("\r") && !macosEntitlementsText.includes("<!--"),
  "Apple codesign requires canonical LF-only entitlements without XML comments",
);
assert.match(macosEntitlementsText, /<key>com\.apple\.security\.device\.audio-input<\/key>/);
const repoAttributes = fs.readFileSync(path.join(__dirname, "..", "..", ".gitattributes"), "utf8");
assert.match(
  repoAttributes,
  /^pinvou3-app\/src-tauri\/packaging\/macos\/entitlements\.plist text eol=lf$/m,
  "Windows checkouts must preserve the codesign entitlement file's LF line endings",
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
