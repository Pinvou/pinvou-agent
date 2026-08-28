// Vendor the official chrome-devtools-mcp (Apache-2.0) as a self-contained build under
// `src-tauri/resources/platforms/<os>/chrome-devtools-mcp/`. The package resource overlay
// installs it under `runtime/chrome-devtools-mcp`. Vendoring requires network access at
// build time; the installed Pinvou application remains offline-capable.
//
// Idempotence: skip only when marker metadata and the adapted output hash match.
// Integrity: pin both the registry tarball SHA-512 and the adapted output SHA-256.
//
// Usage: node scripts/tauri/chrome-devtools-mcp.js
// build.js runs this automatically before dev/build/bundle. Run it manually to warm the
// vendored output before starting the project scripts.

const crypto = require("node:crypto");
const fs = require("node:fs");
const path = require("node:path");
const { spawnSync } = require("node:child_process");
const { APP_ROOT } = require("./platform-config.js");

const VERSION = "1.7.0";
// npm registry integrity for chrome-devtools-mcp@1.7.0 (SHA-512 base64 without `sha512-`).
const INTEGRITY_SHA512 =
  "6xFW7oiUxTxZuHcfyYBkKQtmttjCbfifKZMSEk5CV8H2FucvKweYiJr8CblddYHtYjA4C14K9VAs1r49906RBA==";
const TARBALL_URL = `https://registry.npmjs.org/chrome-devtools-mcp/-/chrome-devtools-mcp-${VERSION}.tgz`;
const MARKER_NAME = ".vendor-version.json";
const ADAPTER_VERSION = "pinvou-target-id-v1";
// SHA-256 of chrome-devtools-mcp@1.7.0 build/src/McpResponse.js after the
// guarded target_id adapter below has been applied. The tarball integrity
// anchors the upstream input; this digest anchors the exact rewritten output.
const ADAPTED_RESPONSE_SHA256 =
  "e08698ba25c72b304152da1de99005d2415b9034c7edd46615d942dac174e0a6";

const PLATFORM_DIR = { darwin: "macos", linux: "linux", win32: "windows" };

// The tracked .gitkeep is recreated after vendoring removes the whole directory. Its bytes
// must match the committed resources/platforms/*/chrome-devtools-mcp/.gitkeep; otherwise a
// vendor build always dirties the worktree and competing commits overwrite one another.
// Tauri resource layout tests (`tauri_platform_layout` and `tauri_effective_config`) use its
// presence as the checked-in resource anchor.
const GITKEEP = `This directory is populated at build time by pinvou3-app/scripts/tauri/chrome-devtools-mcp.js:
the official npm registry chrome-devtools-mcp package (Apache-2.0, pinned SHA-512),
its self-contained Rollup build/ output (about 13 MB, with a guarded target_id adapter),
+ catalog-shim.json (lazy-start shim response catalog),
+ .vendor-version.json, distributed through the package resource overlay as runtime/chrome-devtools-mcp.

This tracked .gitkeep preserves the resource directory for the Tauri layout tests
(tauri_platform_layout and tauri_effective_config); generated output is ignored by .gitignore.
npm run dev vendors the runtime automatically and injects its entry point into the development
process. Browser MCP is not guaranteed when Tauri is started outside the project scripts.
`;

function outputRoot(platform = process.platform) {
  const dir = PLATFORM_DIR[platform];
  if (!dir) throw new Error(`Unsupported platform: ${platform}`);
  return path.join(APP_ROOT, "src-tauri", "resources", "platforms", dir, "chrome-devtools-mcp");
}

function expectedMarker(responseSha256 = ADAPTED_RESPONSE_SHA256) {
  return {
    name: "chrome-devtools-mcp",
    version: VERSION,
    adapter: ADAPTER_VERSION,
    responseSha256,
  };
}

function sha256File(file) {
  return crypto.createHash("sha256").update(fs.readFileSync(file)).digest("hex");
}

function assertTargetIdAdapterIntegrity(
  root,
  { expectedSha256 = ADAPTED_RESPONSE_SHA256 } = {},
) {
  const responsePath = path.join(root, "build", "src", "McpResponse.js");
  const source = fs.readFileSync(responsePath, "utf8");
  const original = [
    "    const entry = {",
    "        id: mcpPage.id,",
    "        url: mcpPage.pptrPage.url(),",
  ].join("\n");
  const adapted = [
    "    const entry = {",
    "        id: mcpPage.id,",
    "        target_id: mcpPage.pptrPage.target()._targetId,",
    "        url: mcpPage.pptrPage.url(),",
  ].join("\n");
  const originalCount = source.split(original).length - 1;
  const adaptedCount = source.split(adapted).length - 1;
  if (adaptedCount !== 1 || originalCount !== 0) {
    throw new Error(
      `Unexpected final chrome-devtools-mcp targetId adapter state (original=${originalCount}, adapted=${adaptedCount})`,
    );
  }
  const actualSha256 = sha256File(responsePath);
  if (actualSha256 !== expectedSha256) {
    throw new Error(
      `chrome-devtools-mcp targetId adapter SHA-256 mismatch (expected ${expectedSha256.slice(0, 16)}…, actual ${actualSha256.slice(0, 16)}…)`,
    );
  }
  return actualSha256;
}

/**
 * Upstream list_pages structuredContent contains only MCP's numeric pageId. The application
 * host tracks conversation and tab ownership by Chromium targetId. Without this join key,
 * a restarted MCP process could only guess pages by URL and might act across conversations.
 *
 * The adapter strictly matches one source fragment from version 1.7.0. A version upgrade or
 * upstream structural change fails the vendor build and requires review. It performs no
 * fuzzy replacement and does not change tool behavior.
 */
function applyTargetIdAdapter(
  root,
  { expectedSha256 = ADAPTED_RESPONSE_SHA256 } = {},
) {
  const responsePath = path.join(root, "build", "src", "McpResponse.js");
  const source = fs.readFileSync(responsePath, "utf8");
  const original = [
    "    const entry = {",
    "        id: mcpPage.id,",
    "        url: mcpPage.pptrPage.url(),",
  ].join("\n");
  const adapted = [
    "    const entry = {",
    "        id: mcpPage.id,",
    "        target_id: mcpPage.pptrPage.target()._targetId,",
    "        url: mcpPage.pptrPage.url(),",
  ].join("\n");
  const originalCount = source.split(original).length - 1;
  const adaptedCount = source.split(adapted).length - 1;
  if (adaptedCount === 1 && originalCount === 0) {
    return assertTargetIdAdapterIntegrity(root, { expectedSha256 });
  }
  if (originalCount !== 1 || adaptedCount !== 0) {
    throw new Error(
      `Unexpected chrome-devtools-mcp targetId adapter anchor state (original=${originalCount}, adapted=${adaptedCount})`,
    );
  }
  fs.writeFileSync(responsePath, source.split(original).join(adapted));
  return assertTargetIdAdapterIntegrity(root, { expectedSha256 });
}

// Windows PATH order may resolve `tar` to Git for Windows' GNU tar, which reads the
// drive letter of `D:\...` as an rsh remote host and fails with "Cannot connect to D:
// resolve failed". Windows 10 1803+ ships a bsdtar at System32 that handles local
// drive paths natively, so resolve it explicitly instead of relying on PATH.
function systemTarCommand({ platform = process.platform, systemRoot = process.env.SystemRoot } = {}) {
  if (platform !== "win32") return "tar";
  const bsdtar = path.join(systemRoot || "C:\\Windows", "System32", "tar.exe");
  return fs.existsSync(bsdtar) ? bsdtar : "tar";
}

function isPreparedRoot(root, marker = expectedMarker()) {
  try {
    const actual = JSON.parse(fs.readFileSync(path.join(root, MARKER_NAME), "utf8"));
    if (JSON.stringify(actual) !== JSON.stringify(marker)) return false;
    if (sha256File(path.join(root, "build", "src", "McpResponse.js")) !== marker.responseSha256) {
      return false;
    }
    // A complete self-contained build has its main entry point, lazy-start shim catalog,
    // and exact final adapter hash. Packaged output has no node_modules.
    return (
      fs.existsSync(path.join(root, "build", "src", "bin", "chrome-devtools-mcp.js")) &&
      fs.existsSync(path.join(root, "catalog-shim.json"))
    );
  } catch {
    return false;
  }
}

function isPrepared(platform = process.platform) {
  return isPreparedRoot(outputRoot(platform));
}

function run(cmd, args, { cwd, inherit = false, input, env } = {}) {
  const result = spawnSync(cmd, args, {
    cwd,
    input,
    env,
    stdio: inherit ? "inherit" : ["pipe", "pipe", "pipe"],
    maxBuffer: 32 * 1024 * 1024,
    timeout: 60000,
    encoding: input != null ? "utf8" : undefined,
  });
  if (result.error) throw result.error;
  if (result.status !== 0) {
    const err = String(result.stderr || result.stdout || "").trim();
    throw new Error(`${cmd} ${args.join(" ")} failed with status=${result.status}${err ? `: ${err.slice(0, 500)}` : ""}`);
  }
  return result;
}

// Capture the MCP handshake and tool catalog in catalog-shim.json. During lazy startup,
// browser-wrapper uses it to answer the Engine's initialize and tools/list calls directly.
// chrome-devtools-mcp registers tools statically, so enumeration needs no browser connection.
// The server exits at stdin EOF, allowing spawnSync to supply the complete exchange at once.
function captureCatalog(entry, root) {
  const lines = [
    JSON.stringify({
      jsonrpc: "2.0",
      id: 1,
      method: "initialize",
      params: {
        protocolVersion: "2024-11-05",
        capabilities: {},
        clientInfo: { name: "pinvou-vendor", version: "0" },
      },
    }),
    JSON.stringify({ jsonrpc: "2.0", method: "notifications/initialized" }),
    JSON.stringify({ jsonrpc: "2.0", id: 2, method: "tools/list", params: {} }),
    "",
  ].join("\n");
  const result = run(
    process.execPath,
    [entry, "--no-usage-statistics", "--no-performance-crux"],
    {
      cwd: root,
      input: lines,
      env: { ...process.env, CHROME_DEVTOOLS_MCP_NO_UPDATE_CHECKS: "1", CI: "1" },
    },
  );
  let initializeResult = null;
  let toolsListResult = null;
  for (const line of String(result.stdout || "").split("\n")) {
    if (!line.trim()) continue;
    let msg;
    try {
      msg = JSON.parse(line);
    } catch {
      continue; // Ignore non-protocol output; stdout should normally be protocol-only.
    }
    if (msg.id === 1 && msg.result) initializeResult = msg.result;
    if (msg.id === 2 && msg.result) toolsListResult = msg.result;
  }
  if (!initializeResult || !toolsListResult || !Array.isArray(toolsListResult.tools)) {
    throw new Error("Failed to capture the MCP catalog: initialize or tools/list response missing");
  }
  if (toolsListResult.nextCursor) {
    throw new Error("Failed to capture the MCP catalog: paginated tools/list is not supported");
  }
  const catalog = { initializeResult, toolsListResult };
  fs.writeFileSync(path.join(root, "catalog-shim.json"), JSON.stringify(catalog));
  console.log(`[chrome-devtools-mcp] catalog: ${toolsListResult.tools.length} tools`);
}

function prepareChromeDevtoolsMcp({ platform = process.platform } = {}) {
  if (isPrepared(platform)) return false;

  const root = outputRoot(platform);
  const stagingRoot = path.join(APP_ROOT, "target", "chrome-devtools-mcp-staging", `${platform}-${process.pid}`);
  const tarball = path.join(stagingRoot, `chrome-devtools-mcp-${VERSION}.tgz`);

  fs.mkdirSync(stagingRoot, { recursive: true });
  console.log(`[chrome-devtools-mcp] vendor ${VERSION} → ${root}`);
  try {
    // 1) Download synchronously. curl is available on all three build platforms; retries
    //    keep release builds from failing immediately on a weak connection.
    run("curl", ["-fsSL", "--retry", "3", "--retry-delay", "2", "--retry-all-errors", "-o", tarball, TARBALL_URL], { cwd: stagingRoot });
    // 2) Verify SHA-512 integrity.
    const hash = crypto.createHash("sha512").update(fs.readFileSync(tarball)).digest("base64");
    if (hash !== INTEGRITY_SHA512) {
      throw new Error(
        `chrome-devtools-mcp@${VERSION} SHA-512 mismatch (expected ${INTEGRITY_SHA512.slice(0, 16)}…, actual ${hash.slice(0, 16)}…)`,
      );
    }
    // 3) Extract with tar (bsdtar on macOS/Linux and the System32 bsdtar on Windows;
    //    see systemTarCommand for why PATH order must not decide on Windows).
    run(systemTarCommand(), ["-xzf", tarball, "-C", stagingRoot], { cwd: stagingRoot });
    const unpacked = path.join(stagingRoot, "package");
    if (!fs.existsSync(unpacked)) {
      throw new Error("Extracted tarball is missing package/; the upstream layout changed");
    }
    // 4) Move the prepared package into place.
    fs.rmSync(root, { recursive: true, force: true });
    fs.mkdirSync(path.dirname(root), { recursive: true });
    fs.renameSync(unpacked, root);
    // 4.5) Recreate the tracked .gitkeep removed with the old directory. CI resource-layout
    //      tests depend on this directory existing in a clean checkout.
    fs.writeFileSync(path.join(root, ".gitkeep"), GITKEEP);
    // 5) Apply the guarded minimal adapter, then smoke-test the upstream entry point.
    applyTargetIdAdapter(root);
    // 5.5) Self-contained smoke test: run --help with the current Node.js installation and
    //      no installed dependencies to prove that the runtime works offline.
    const entry = path.join(root, "build", "src", "bin", "chrome-devtools-mcp.js");
    if (!fs.existsSync(entry)) throw new Error("Extracted package is missing build/src/bin/chrome-devtools-mcp.js");
    run(process.execPath, [entry, "--help"], { cwd: root });
    // 5.6) Capture initialize/tools/list as the browser-wrapper lazy-start shim source.
    captureCatalog(entry, root);
    // 6) marker
    fs.writeFileSync(path.join(root, MARKER_NAME), JSON.stringify(expectedMarker(), null, 2));
    console.log(`[chrome-devtools-mcp] ready: ${root}`);
    return true;
  } finally {
    fs.rmSync(stagingRoot, { recursive: true, force: true });
  }
}

module.exports = {
  ADAPTED_RESPONSE_SHA256,
  ADAPTER_VERSION,
  GITKEEP,
  applyTargetIdAdapter,
  assertTargetIdAdapterIntegrity,
  expectedMarker,
  prepareChromeDevtoolsMcp,
  isPrepared,
  isPreparedRoot,
  outputRoot,
  systemTarCommand,
};

if (require.main === module) {
  try {
    prepareChromeDevtoolsMcp();
  } catch (error) {
    console.error(`[chrome-devtools-mcp] ${error.message}`);
    process.exitCode = 1;
  }
}
