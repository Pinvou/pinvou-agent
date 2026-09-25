const { spawnSync } = require("node:child_process");
const fs = require("node:fs");
const path = require("node:path");
const { writeEffectiveArtifacts } = require("./effective-config.js");
const {
  prepareCodexBridge,
  prepareWindowsCodexBridge,
  WINDOWS_BRIDGE_CONFIG_PATH,
} = require("./codex-bridge.js");
const {
  outputRoot: chromeDevtoolsMcpOutputRoot,
  prepareChromeDevtoolsMcp,
} = require("./chrome-devtools-mcp.js");
const {
  APP_ROOT,
  platformArchitectureConfigPath,
  platformConfigPath,
} = require("./platform-config.js");
const { linuxStartupWindowConfigSpec } = require("./startup-window-config.js");
const { prepareLinuxAsrRuntime } = require("./linux-asr-runtime.js");
const { prepareKnowledgeHost } = require("./knowledge-host.js");
const { WRAPPER_ENV } = require("./require-wrapper.js");
const { stageWindowsInstaller } = require("./windows-installer.js");
const {
  stageWindowsOnnxRuntime,
  stageWindowsRuntime,
} = require("./windows-runtime.js");

function tauriCommandIndex(args) {
  return args.findIndex((argument) => argument === "build" || argument === "bundle");
}

function configSpecs(args) {
  const specs = [];
  for (let index = 0; index < args.length; index += 1) {
    if (args[index] === "--config" || args[index] === "-c") {
      if (!args[index + 1]) throw new Error("--config 缺少配置值");
      specs.push(args[index + 1]);
      index += 1;
    } else if (args[index].startsWith("--config=")) {
      specs.push(args[index].slice("--config=".length));
    }
  }
  return specs;
}

function windowsBundleTargets(args) {
  const explicit = [];
  for (let index = 0; index < args.length; index += 1) {
    const argument = args[index];
    if (argument === "--bundles" || argument === "-b") {
      if (!args[index + 1]) throw new Error(`${argument} 缺少 bundle 类型`);
      explicit.push(args[index + 1]);
      index += 1;
    } else if (argument.startsWith("--bundles=")) {
      explicit.push(argument.slice("--bundles=".length));
    }
  }
  if (explicit.length === 0 || explicit.includes("all")) return ["msi", "nsis"];
  return [...new Set(explicit.flatMap((value) => value.split(",")).filter(Boolean))];
}

/**
 * Machine segment of a `--target` triple -> Node `process.arch` notation.
 *
 * When adding an architecture here, the reverse machine map in
 * `codex-bridge.js` must gain it too; `tests/tauri_effective_config.test.js`
 * derives assertions from both tables and turns red if only one changes.
 */
const TARGET_MACHINE_ARCHITECTURES = {
  aarch64: "arm64",
  amd64: "x64",
  arm64: "arm64",
  x86_64: "x64",
};

/**
 * Triples that deliberately fall back to the host architecture (darwin hosts
 * only). `universal-apple-darwin` builds x86_64 and aarch64 and merges them, so
 * it has no single target architecture; rejecting it as unknown would block
 * macOS packaging. On Linux this triple cannot produce anything, so it must
 * not become a way around the unknown-architecture failure.
 */
const HOST_FALLBACK_TARGET_TRIPLES = new Set(["universal-apple-darwin"]);

/**
 * Parse the target architecture (Node notation) from `--target <triple>`.
 *
 * Cross builds must pick overlays by the target architecture, not the host
 * one: otherwise `--target aarch64-unknown-linux-gnu` applies the x86_64
 * overlay, which references resources of the wrong architecture and misses
 * what the target overlay actually packages.
 *
 * Accepts `--target X` and `--target=X`; the last occurrence wins, matching
 * cargo's override semantics. Returns null without `--target`, so callers
 * fall back to `process.arch` and the native build is unchanged.
 *
 * An unknown machine segment (`riscv64gc-...`, `armv7-...`) must not return
 * null as well: the cross-build gate lets `!targetArch` through, so overlays,
 * the ASR runtime and the knowledge host would silently fall back to the host
 * architecture - exactly the "target main binary, host-architecture sidecars"
 * package the gate exists to prevent. That case throws. Two exceptions still
 * fall back, because neither is a cross build to an unknown architecture:
 * triples in HOST_FALLBACK_TARGET_TRIPLES (darwin hosts), and hosts whose own
 * arch is not in the table (a native riscv64 build, say), where we cannot tell
 * whether the build is cross at all. Only Linux hosts are gated: the sidecars
 * that are prepared for the host architecture exist only on that path.
 *
 * @param {string[]} args Full argument list passed to tauri.
 * @param {{platform?: string, hostArch?: string}} [options] Host platform and arch.
 * @returns {string|null} "arm64" / "x64", or null when falling back is allowed.
 * @throws {Error} A Linux host cross-builds to a machine segment not in the table.
 */
function targetArchitecture(
  args,
  { platform = process.platform, hostArch = process.arch } = {},
) {
  let triple = null;
  for (let i = 0; i < args.length; i += 1) {
    if (args[i] === "--target") {
      // A missing value, or a next token that is another flag, is reported as
      // such; otherwise `--bundles` would be parsed as the machine segment and
      // the error would point at a non-existent architecture problem.
      const value = args[i + 1];
      if (!value || value.startsWith("--")) {
        throw new Error(
          "--target must be followed by a target triple (e.g. aarch64-unknown-linux-gnu),"
            + ` got "${value ?? "(no next argument)"}".`,
        );
      }
      triple = value;
      i += 1;
    } else if (args[i].startsWith("--target=")) {
      const value = args[i].slice("--target=".length);
      if (!value) throw new Error("--target= must be followed by a target triple; the value is empty.");
      triple = value;
    }
  }
  if (!triple) return null;
  if (platform === "darwin" && HOST_FALLBACK_TARGET_TRIPLES.has(triple)) return null;
  const machine = triple.split("-")[0];
  const architecture = TARGET_MACHINE_ARCHITECTURES[machine];
  if (architecture) return architecture;
  const hostIsKnown = Object.values(TARGET_MACHINE_ARCHITECTURES).includes(hostArch);
  if (platform !== "linux" || !hostIsKnown) return null;
  throw new Error(
    `Cannot determine the target architecture of --target ${triple}: the machine segment`
      + ` "${machine}" is not in the architecture table. The host is ${hostArch}; falling`
      + " back to it would pick the wrong overlay and prepare the speech-recognition runtime"
      + " and the shared knowledge host for the host architecture, silently producing a"
      + " package whose main binary targets one architecture while its sidecars target"
      + " another. To support this architecture, extend all of the following (missing (2)"
      + " or (4) silently reintroduces a wrong package; missing (1), (3) or (5) fails the"
      + " build loudly): (1) TARGET_MACHINE_ARCHITECTURES in this file; (2)"
      + " ARCHITECTURE_CONFIG_NAMES in scripts/tauri/platform-config.js (without it"
      + " platformArchitectureConfigPath returns null and the whole architecture overlay is"
      + " skipped); (3) the matching architecture overlay under src-tauri/config/platforms/linux/;"
      + " (4) the Node arch -> machine map in scripts/tauri/codex-bridge.js (without it the"
      + " bridge script falls back to uname -m and packages a host-architecture Node);"
      + " (5) the target list in scripts/prepare-codex-bridge-runtime.sh. If the triple has"
      + " no single target architecture (like universal binaries, darwin hosts only), add it"
      + " to HOST_FALLBACK_TARGET_TRIPLES instead.",
  );
}

/**
 * Remove the resource declarations matched by `markers` from an overlay and
 * return the result as a replacement for that overlay.
 *
 * This is the single place that documents the approach; the wrappers and the
 * tests only point here.
 *
 * Problem: cross builds may skip preparing the ASR runtime or the shared
 * knowledge host, but the overlays still declare their resource paths.
 * `main()` passes `configSpecs(preparedArgs)` to `writeEffectiveArtifacts`
 * before running Tauri, and it checks that every effective resource exists,
 * so the build stops with a missing-resource error.
 *
 * Why replace instead of layering `{key: null}`: layering null is a valid and
 * shorter route (overlays merge as JSON Merge Patch, see `mergeConfig` in
 * `effective-config.js`, and the NSIS staging path relies on it). The one
 * reason for replacing is that the removal is filtered from the overlay's own
 * keys, so a future declaration of the same unpreparable kind is removed
 * automatically instead of needing a second key list here. The costs, stated
 * so they are not forgotten: markers match substrings, which is wider than an
 * exact key, and rewriting the whole overlay trades "missed a key" for
 * "dropped a key", which only the layer-by-layer comparison in
 * `tests/tauri_skip_knowledge_host_overlay.test.js` guards against.
 *
 * A reason that does NOT hold: a missed declaration does not fail late in the
 * bundle stage; the `writeEffectiveArtifacts` check fails within seconds,
 * before cargo starts, with the same message for either approach.
 *
 * The tracked config files are not edited instead, because every cross build
 * would then have to restore them, and `tests/tauri_platform_layout.test.js`
 * would stay red. `--config` accepts inline JSON as well as paths (see
 * `linuxStartupWindowConfigSpec`), so no temporary file needs cleaning up;
 * resource paths resolve against src-tauri either way.
 *
 * @param {string} configPath Absolute path of the overlay.
 * @param {string[]} markers Resource source substrings to remove.
 * @returns {string} Inline JSON after removal, or the path when nothing matched.
 */
function configWithoutResources(configPath, markers) {
  let raw;
  try {
    raw = JSON.parse(fs.readFileSync(configPath, "utf8"));
  } catch (error) {
    // A bare SyntaxError does not say which file failed, and these paths are
    // assembled per platform/architecture; keep the original as the cause.
    throw new Error(`Failed to parse config ${configPath}: ${error.message}`, { cause: error });
  }
  const resources = raw?.bundle?.resources;
  if (!resources || typeof resources !== "object") return configPath;
  const kept = Object.fromEntries(
    Object.entries(resources).filter(
      ([source]) => !markers.some((marker) => source.includes(marker)),
    ),
  );
  if (Object.keys(kept).length === Object.keys(resources).length) return configPath;
  return JSON.stringify({ ...raw, bundle: { ...raw.bundle, resources: kept } });
}

/**
 * Drop the ASR resource declaration from the architecture overlay when the
 * ASR runtime build is skipped: `target/linux-asr-runtime/<machine>/sense-voice-main`
 * does not exist then. See `configWithoutResources` for the approach.
 * @param {string} configPath Absolute path of the architecture overlay.
 * @returns {string} Inline JSON after removal, or the path when nothing matched.
 */
function architectureConfigWithoutAsr(configPath) {
  return configWithoutResources(configPath, ["linux-asr-runtime"]);
}

/**
 * Drop the knowledge-host resource declaration from the platform overlay
 * when the shared knowledge host is skipped.
 *
 * The `knowledge-host/` declaration lives in the platform overlay
 * (`config/platforms/linux/tauri.conf.json`); architecture overlays only
 * declare ASR. Skipping the build without removing the declaration fails
 * silently either way: a clean tree packages an empty directory (no server),
 * and a dirty tree packages the server left by an earlier host-architecture
 * build into the other architecture's package. See `configWithoutResources`
 * for the approach.
 * @param {string} configPath Absolute path of the platform overlay.
 * @returns {string} Inline JSON after removal, or the path when nothing matched.
 */
function platformConfigWithoutKnowledgeHost(configPath) {
  return configWithoutResources(configPath, ["knowledge-host"]);
}

/**
 * When the target architecture differs from the host, check that every step
 * that can only prepare host-architecture artifacts has been dealt with.
 *
 * This is a hard failure, not a warning, because the symptom is a silently
 * wrong package rather than a build error:
 *   - without `PINVOU3_SKIP_LINUX_ASR`, `build-sensevoice-runtime.sh` asserts
 *     `requested_arch == host_arch` and fails, which is at least visible;
 *   - without `PINVOU3_SKIP_KNOWLEDGE_HOST`, `cargo build` runs without
 *     `--target` and the host-architecture server is packaged into the other
 *     architecture's deb with no warning at all.
 *
 * The codex bridge is not on this list: it prepares the target architecture
 * (`PINVOU3_BRIDGE_TARGET_ARCH` in `prepare-codex-bridge-runtime.sh`), so a
 * cross build produces a correct package without skipping it.
 *
 * @param {object} options Target/host architecture and the two skip switches.
 * @returns {string[]} Descriptions of the violations; empty when the build is fine.
 */
function crossBuildViolations({
  platform,
  targetArch,
  hostArch,
  skipLinuxAsr,
  skipKnowledgeHost,
}) {
  if (platform !== "linux" || !targetArch || targetArch === hostArch) return [];
  const problems = [];
  if (!skipLinuxAsr) {
    problems.push(
      "the SenseVoice speech-recognition runtime can only be built natively on the target"
        + " architecture; set PINVOU3_SKIP_LINUX_ASR=1 for cross builds",
    );
  }
  if (!skipKnowledgeHost) {
    problems.push(
      "the shared knowledge host build ignores --target and only produces host-architecture"
        + " binaries; set PINVOU3_SKIP_KNOWLEDGE_HOST=1 for cross builds",
    );
  }
  return problems;
}

function prepareTauriArgs(
  args,
  {
    platform = process.platform,
    architecture = process.arch,
    stageRuntime = stageWindowsRuntime,
    additionalConfigs = [],
    // Normally both skip switches arrive as parameters: `main()` reads the
    // environment once and the gate, the preparation steps and the overlay
    // removal here all use that one result. Reading the variables again here
    // could yield a different answer (the environment changed, or a refactor
    // moved a read after a step that edits env), and a mismatch means either
    // "declaration removed but the artifact still built" or "artifact skipped
    // but still declared" - the latter fails the bundle with a missing
    // resource. The defaults only serve callers that bypass `main()` (tests,
    // other scripts requiring this module).
    skipLinuxAsr = process.env.PINVOU3_SKIP_LINUX_ASR === "1",
    skipKnowledgeHost = process.env.PINVOU3_SKIP_KNOWLEDGE_HOST === "1",
  } = {},
) {
  const prepared = [...args];
  const commandIndex = tauriCommandIndex(prepared);
  if (commandIndex < 0) {
    // dev 不注入 packaging overlay。macOS 复用平台 overlay 保持原生顶栏一致；
    // Linux 只注入 dev overlay，让冷启动窗口等 React 首次提交后再显示，避开
    // Mutter/XWayland 首次映射期间视觉表面与输入表面短暂错位。
    const devIndex = prepared.indexOf("dev");
    const devConfig = platform === "darwin"
      ? platformConfigPath(platform)
      : platform === "linux"
        ? linuxStartupWindowConfigSpec()
        : null;
    const automaticConfigs = [devConfig, ...additionalConfigs].filter(Boolean);
    if (devIndex >= 0 && automaticConfigs.length > 0) {
      // 与 build/bundle 保持相同优先级:自动平台配置在前,调用方显式
      // --config 在后,从而仍可有意覆盖平台默认值。
      const injected = automaticConfigs.flatMap((configPath) => ["--config", configPath]);
      prepared.splice(devIndex + 1, 0, ...injected);
    }
    return prepared;
  }

  // Platform overlay. Skipping the shared knowledge host also removes its
  // resource declaration, which lives in this platform layer.
  const platformConfig = platformConfigPath(platform);
  const automaticConfigs = [
    skipKnowledgeHost && platform === "linux"
      ? platformConfigWithoutKnowledgeHost(platformConfig)
      : platformConfig,
  ];
  if (platform === "linux") automaticConfigs.push(linuxStartupWindowConfigSpec());
  // Cross builds pick the architecture overlay by --target, not by the host.
  // The host platform/arch are passed so targetArchitecture can tell a cross
  // build to an unknown machine (rejected) from a native one (fallback).
  const effectiveArchitecture = targetArchitecture(prepared, {
    platform,
    hostArch: architecture,
  }) || architecture;
  const architectureConfig = platformArchitectureConfigPath(platform, effectiveArchitecture);
  if (architectureConfig) {
    // The Linux guard mirrors the knowledge-host one: the ASR replacement only
    // targets Linux `target/linux-asr-runtime/...` declarations. Without it
    // macOS/Windows overlays would be rewritten too; harmless today only
    // because the marker happens not to match there.
    automaticConfigs.push(
      skipLinuxAsr && platform === "linux"
        ? architectureConfigWithoutAsr(architectureConfig)
        : architectureConfig,
    );
  }
  const stagedRuntime = stageRuntime({ platform });
  const runtimeConfig =
    typeof stagedRuntime === "string" ? stagedRuntime : stagedRuntime?.configPath;
  if (runtimeConfig) automaticConfigs.push(runtimeConfig);
  automaticConfigs.push(...additionalConfigs);
  const injected = automaticConfigs.flatMap((configPath) => ["--config", configPath]);
  // Automatic overlays must precede explicit signing/staging overlays so the
  // caller can intentionally override or remove inherited resource mappings.
  prepared.splice(commandIndex + 1, 0, ...injected);
  return prepared;
}

function runTauri(preparedArgs, spawn = spawnSync, environment = process.env) {
  const tauriCli = require.resolve("@tauri-apps/cli/tauri.js");
  const child = spawn(process.execPath, [tauriCli, ...preparedArgs], {
    cwd: APP_ROOT,
    env: { ...environment, [WRAPPER_ENV]: "1" },
    stdio: "inherit",
  });
  if (child.error) throw child.error;
  return child.status === null ? 1 : child.status;
}

function tauriRuntimeEnvironment(runtime, environment = process.env) {
  return runtime
    ? { ...environment, ORT_DYLIB_PATH: runtime.onnxRuntimeDylib }
    : environment;
}

function supportsChromeDevtoolsMcp(platform = process.platform) {
  return platform === "win32";
}

// Returns the vendor preparation result; the real prepare step downloads the
// vendored tarball asynchronously, so callers must await it to keep staging
// ahead of the resource manifest computation and the Tauri bundle step.
function prepareChromeDevtoolsMcpForPlatform({
  platform = process.platform,
  prepare = prepareChromeDevtoolsMcp,
} = {}) {
  if (!supportsChromeDevtoolsMcp(platform)) return false;
  return prepare({ platform });
}

function withoutChromeDevtoolsMcpOverride(environment) {
  if (!Object.prototype.hasOwnProperty.call(environment, "PINVOU3_CDMCP_BIN")) {
    return environment;
  }
  const sanitized = { ...environment };
  delete sanitized.PINVOU3_CDMCP_BIN;
  return sanitized;
}

function chromeDevtoolsMcpEnvironment(
  development,
  environment = process.env,
  platform = process.platform,
) {
  // Only the Windows WebView2 host currently exposes the app-owned CDP endpoint.
  // Never leak a caller-provided MCP binary override into packaged builds or into
  // the currently inactive WKWebView/WebKitGTK substrate.
  if (!development || !supportsChromeDevtoolsMcp(platform)) {
    return withoutChromeDevtoolsMcpOverride(environment);
  }
  return {
    ...environment,
    PINVOU3_CDMCP_BIN: path.join(
      chromeDevtoolsMcpOutputRoot(platform),
      "build",
      "src",
      "bin",
      "chrome-devtools-mcp.js",
    ),
  };
}

async function main() {
  const args = process.argv.slice(2);
  const validateOnly = args[0] === "--validate-only";
  if (validateOnly) args.shift();

  if (validateOnly) return;

  const isDev = args.includes("dev");
  const hasTauriBuildCommand = tauriCommandIndex(args) >= 0;
  // The two skip switches are read exactly once, here: the gate, the
  // preparation steps and the overlay removal (`prepareTauriArgs`) all
  // receive this one result, however many awaits lie in between. See the
  // parameter comment in `prepareTauriArgs` for what a mismatch costs.
  const skipLinuxAsr = process.env.PINVOU3_SKIP_LINUX_ASR === "1";
  const skipKnowledgeHost = process.env.PINVOU3_SKIP_KNOWLEDGE_HOST === "1";
  const additionalConfigs = [];
  // Windows 的 fastembed 使用动态 ONNX Runtime。正式包 staging 完整运行时并通过
  // resource overlay 携带 DLL；dev 只校验并展开 ONNX 组件，避免为 UI 开发准备无关工具。
  const windowsRuntime =
    hasTauriBuildCommand && process.platform === "win32"
      ? stageWindowsRuntime()
      : null;
  const windowsDevRuntime =
    isDev && process.platform === "win32" ? stageWindowsOnnxRuntime() : null;
  if (windowsRuntime && hasTauriBuildCommand) {
    stageWindowsInstaller({
      bundleTargets: windowsBundleTargets(args),
      runtime: windowsRuntime,
    });
  }
  const windowsBridgeOptions = windowsRuntime
    ? {
        nodeExecutable: windowsRuntime.nodeExecutable,
        npmExecPath: windowsRuntime.npmExecPath,
      }
    : undefined;
  if (isDev) {
    prepareCodexBridge();
    // Tauri dev does not apply the package resource overlay, so the development process
    // points directly to the verified workspace vendor entry. Only Windows WebView2 exposes
    // app-owned CDP. Linux uses BrowserCore/WebKitWebDriver and macOS product capability is
    // currently disabled; neither platform may prepare or fall back to external Chrome.
    await prepareChromeDevtoolsMcpForPlatform();
    prepareWindowsCodexBridge();
    const developmentHost = prepareKnowledgeHost({ development: true });
    if (developmentHost?.configSpec) additionalConfigs.push(developmentHost.configSpec);
  }
  if (hasTauriBuildCommand) {
    const hostArch = process.arch;
    // An unknown machine segment throws here (see `targetArchitecture`)
    // instead of reaching the gate, which lets a null target through.
    const targetArch = targetArchitecture(args);

    // Cross-build consistency gate: fail now rather than ship a package
    // with sidecars of the wrong architecture.
    const violations = crossBuildViolations({
      platform: process.platform,
      targetArch,
      hostArch,
      skipLinuxAsr,
      skipKnowledgeHost,
    });
    if (violations.length > 0) {
      const detail = violations.map((line) => `  - ${line}`).join("\n");
      throw new Error(
        `Cross build (host ${hostArch} -> target ${targetArch}) is not fully configured:\n${detail}`,
      );
    }

    // SenseVoice can only be built natively on the target architecture
    // (build-sensevoice-runtime.sh requires requested_arch == host_arch and
    // links the host glibc), so cross builds skip it explicitly; the overlay
    // declaration is removed in prepareTauriArgs.
    if (!skipLinuxAsr) prepareLinuxAsrRuntime();
    // The bridge prepares the target architecture: it compiles nothing, it
    // only downloads, verifies and unpacks the official Node tarball, so a
    // cross build can produce a correct package. Without a target the script
    // falls back to uname -m and the native build is unchanged.
    prepareCodexBridge(targetArch ? { targetArch } : {});
    await prepareChromeDevtoolsMcpForPlatform();
    prepareWindowsCodexBridge(windowsBridgeOptions);
    // The shared knowledge host builds host-architecture binaries (cargo build
    // without --target), which would be wrong inside another architecture's
    // package; same class of problem as SenseVoice.
    if (!skipKnowledgeHost) prepareKnowledgeHost();
    if (process.platform === "win32") {
      additionalConfigs.push(WINDOWS_BRIDGE_CONFIG_PATH);
    }
  }

  const preparedArgs = prepareTauriArgs(args, {
    additionalConfigs,
    stageRuntime: () => windowsRuntime,
    // Pass the values computed above instead of letting prepareTauriArgs read
    // the environment again: removing a declaration and skipping its
    // preparation must be the same decision.
    skipLinuxAsr,
    skipKnowledgeHost,
  });
  if (hasTauriBuildCommand) {
    const artifacts = writeEffectiveArtifacts(configSpecs(preparedArgs));
    console.log(`[build] 有效 Tauri 配置: ${artifacts.effectiveConfigPath}`);
    console.log(
      `[build] 安装包资源清单: ${artifacts.resourceManifestPath} (${artifacts.resourceManifest.resourceFileCount} files)`,
    );
  }

  const tauriEnvironment = chromeDevtoolsMcpEnvironment(
    isDev,
    tauriRuntimeEnvironment(windowsRuntime || windowsDevRuntime),
  );
  process.exitCode = runTauri(preparedArgs, undefined, tauriEnvironment);
}

if (require.main === module) {
  void runBuild();
}

async function runBuild() {
  try {
    await main();
  } catch (error) {
    console.error(`[build] ${error.message}`);
    process.exitCode = 1;
  }
}

module.exports = {
  // Exported for tests: the tables that must change together when adding an
  // architecture are checked by assertions derived from this one.
  TARGET_MACHINE_ARCHITECTURES,
  architectureConfigWithoutAsr,
  configWithoutResources,
  configSpecs,
  crossBuildViolations,
  platformConfigWithoutKnowledgeHost,
  targetArchitecture,
  chromeDevtoolsMcpEnvironment,
  main,
  prepareChromeDevtoolsMcp,
  prepareChromeDevtoolsMcpForPlatform,
  prepareCodexBridge,
  prepareKnowledgeHost,
  prepareLinuxAsrRuntime,
  prepareWindowsCodexBridge,
  stageWindowsInstaller,
  stageWindowsOnnxRuntime,
  stageWindowsRuntime,
  prepareTauriArgs,
  runTauri,
  supportsChromeDevtoolsMcp,
  tauriRuntimeEnvironment,
  tauriCommandIndex,
  windowsBundleTargets,
};
