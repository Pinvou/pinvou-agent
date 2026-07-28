const crypto = require("node:crypto");
const fs = require("node:fs");
const path = require("node:path");
const { spawnSync } = require("node:child_process");

const { APP_ROOT } = require("./platform-config.js");
const { npmInstallInvocation } = require("./web-template.js");

const BRIDGE_PACKAGE_ROOT = path.join(APP_ROOT, "scripts", "codex-bridge-runtime");
const LOCKFILE_PATH = path.join(BRIDGE_PACKAGE_ROOT, "package-lock.json");
const WINDOWS_RUNTIME_ROOT = path.join(
  APP_ROOT,
  "src-tauri",
  "target",
  "windows-runtime",
);
const WINDOWS_BRIDGE_ROOT = path.join(
  WINDOWS_RUNTIME_ROOT,
  "codex-bridge",
);
const WINDOWS_NODE_ROOT = path.join(WINDOWS_RUNTIME_ROOT, "node");
const WINDOWS_NODE_EXECUTABLE = path.join(WINDOWS_NODE_ROOT, "node.exe");
const WINDOWS_BRIDGE_CONFIG_PATH = path.join(
  WINDOWS_RUNTIME_ROOT,
  "codex-bridge.tauri.conf.json",
);
const BRIDGE_ENTRYPOINT = path.join(
  "acp",
  "node_modules",
  "@agentclientprotocol",
  "codex-acp",
  "dist",
  "index.js",
);
const BRIDGE_PACKAGE_JSON = path.join(
  "acp",
  "node_modules",
  "@agentclientprotocol",
  "codex-acp",
  "package.json",
);
const MARKER_NAME = "manifest.json";
const PREPARE_FORMAT_VERSION = 2;
const WINDOWS_NODE_VERSION = "22.22.0";
const WINDOWS_NODE_ARCHIVE_NAME = `node-v${WINDOWS_NODE_VERSION}-win-x64.zip`;
const WINDOWS_NODE_ARCHIVE_SHA256 =
  "c97fa376d2becdc8863fcd3ca2dd9a83a9f3468ee7ccf7a6d076ec66a645c77a";
const WINDOWS_NODE_EXECUTABLE_SHA256 =
  "bae898add4643fcf890a83ad8ae56e20dce7e781cab161a53991ceba70c99ffb";
const WINDOWS_NODE_URLS = [
  `https://nodejs.org/dist/v${WINDOWS_NODE_VERSION}/${WINDOWS_NODE_ARCHIVE_NAME}`,
  `https://npmmirror.com/mirrors/node/v${WINDOWS_NODE_VERSION}/${WINDOWS_NODE_ARCHIVE_NAME}`,
];
const WINDOWS_NPM_CI_ARGS = [
  "ci",
  "--prefer-offline",
  "--no-audit",
  "--no-fund",
  "--omit=dev",
  "--omit=optional",
];

function expectedMarker({ architecture = process.arch } = {}) {
  const lockfile = fs.readFileSync(LOCKFILE_PATH);
  const packageJson = JSON.parse(
    fs.readFileSync(path.join(BRIDGE_PACKAGE_ROOT, "package.json"), "utf8"),
  );
  return {
    schema_version: PREPARE_FORMAT_VERSION,
    platform: "win32",
    arch: architecture,
    codex_acp_version: packageJson.dependencies["@agentclientprotocol/codex-acp"],
    lockfile_sha256: crypto.createHash("sha256").update(lockfile).digest("hex"),
    node_version: WINDOWS_NODE_VERSION,
    node_archive_sha256: WINDOWS_NODE_ARCHIVE_SHA256,
    node_executable_sha256: WINDOWS_NODE_EXECUTABLE_SHA256,
    node: "../node/node.exe",
    entrypoint: BRIDGE_ENTRYPOINT.replaceAll(path.sep, "/"),
    requires_managed_codex: true,
  };
}

function nonemptyFile(filePath) {
  try {
    return fs.statSync(filePath).isFile() && fs.statSync(filePath).size > 0;
  } catch {
    return false;
  }
}

function fileSha256(filePath) {
  return crypto.createHash("sha256").update(fs.readFileSync(filePath)).digest("hex");
}

function windowsBridgeOverlay() {
  return {
    bundle: {
      resources: {
        "target/windows-runtime/codex-bridge/": "runtime/codex-bridge",
        "target/windows-runtime/node/": "runtime/node",
      },
    },
  };
}

function writeWindowsBridgeOverlay() {
  fs.mkdirSync(path.dirname(WINDOWS_BRIDGE_CONFIG_PATH), { recursive: true });
  fs.writeFileSync(
    WINDOWS_BRIDGE_CONFIG_PATH,
    `${JSON.stringify(windowsBridgeOverlay(), null, 2)}\n`,
  );
}

function isPrepared(
  expected = expectedMarker(),
  outputRoot = WINDOWS_BRIDGE_ROOT,
  nodeRoot = WINDOWS_NODE_ROOT,
) {
  try {
    const actual = JSON.parse(
      fs.readFileSync(path.join(outputRoot, MARKER_NAME), "utf8"),
    );
    const packageJson = JSON.parse(
      fs.readFileSync(path.join(outputRoot, BRIDGE_PACKAGE_JSON), "utf8"),
    );
    return (
      JSON.stringify(actual) === JSON.stringify(expected) &&
      packageJson.version === expected.codex_acp_version &&
      nonemptyFile(path.join(outputRoot, BRIDGE_ENTRYPOINT)) &&
      nonemptyFile(path.join(nodeRoot, "node.exe")) &&
      fileSha256(path.join(nodeRoot, "node.exe")) ===
        expected.node_executable_sha256
    );
  } catch {
    return false;
  }
}

function checkedSpawn(spawn, command, args, options, label) {
  const result = spawn(command, args, options);
  if (result.error) throw result.error;
  if (result.status !== 0) {
    throw new Error(`${label}失败，退出码：${result.status ?? "unknown"}`);
  }
  return result;
}

// curl/tar 使用 Windows 内置 System32 副本的绝对路径，避免 PATH 劫持
// （CodeQL js/shell-command-injection-from-environment）。
function windowsSystemTool(name) {
  const systemRoot = process.env.SystemRoot || "C:\\Windows";
  return path.join(systemRoot, "System32", name);
}

function downloadWindowsNode(archivePath, {
  environment,
  spawn,
} = {}) {
  let lastError = null;
  for (const url of WINDOWS_NODE_URLS) {
    fs.rmSync(archivePath, { force: true });
    const result = spawn(
      windowsSystemTool("curl.exe"),
      [
        "--fail",
        "--location",
        "--retry",
        "2",
        "--connect-timeout",
        "15",
        url,
        "--output",
        archivePath,
      ],
      {
        env: environment,
        stdio: "inherit",
      },
    );
    if (!result.error && result.status === 0) return;
    lastError = result.error || new Error(`curl.exe 退出码：${result.status ?? "unknown"}`);
  }
  throw new Error(`下载 Windows Node.js Runtime 失败：${lastError?.message || "未知错误"}`);
}

function prepareWindowsNode(stagingRoot, {
  environment,
  spawn,
} = {}) {
  const archivePath = path.join(stagingRoot, WINDOWS_NODE_ARCHIVE_NAME);
  const extractedRoot = path.join(stagingRoot, "node-extracted");
  const distributionRoot = path.join(
    extractedRoot,
    `node-v${WINDOWS_NODE_VERSION}-win-x64`,
  );
  const nodeRoot = path.join(stagingRoot, "node");

  console.log(`[codex-bridge] 准备 Windows Node.js ${WINDOWS_NODE_VERSION} Runtime`);
  downloadWindowsNode(archivePath, { environment, spawn });
  const archiveSha256 = fileSha256(archivePath);
  if (archiveSha256 !== WINDOWS_NODE_ARCHIVE_SHA256) {
    throw new Error(
      `Windows Node.js Runtime 完整性校验失败：expected=${WINDOWS_NODE_ARCHIVE_SHA256} actual=${archiveSha256}`,
    );
  }

  fs.mkdirSync(extractedRoot, { recursive: true });
  checkedSpawn(
    spawn,
    windowsSystemTool("tar.exe"),
    ["-xf", archivePath, "-C", extractedRoot],
    { env: environment, stdio: "inherit" },
    "解压 Windows Node.js Runtime",
  );

  fs.mkdirSync(nodeRoot, { recursive: true });
  fs.copyFileSync(path.join(distributionRoot, "node.exe"), path.join(nodeRoot, "node.exe"));
  fs.copyFileSync(path.join(distributionRoot, "LICENSE"), path.join(nodeRoot, "LICENSE"));
  if (fileSha256(path.join(nodeRoot, "node.exe")) !== WINDOWS_NODE_EXECUTABLE_SHA256) {
    throw new Error("Windows node.exe 完整性校验失败");
  }

  const versionResult = checkedSpawn(
    spawn,
    path.join(nodeRoot, "node.exe"),
    ["--version"],
    { env: environment, encoding: "utf8" },
    "验证 Windows Node.js Runtime",
  );
  if (String(versionResult.stdout || "").trim() !== `v${WINDOWS_NODE_VERSION}`) {
    throw new Error(
      `Windows Node.js Runtime 版本不匹配：${String(versionResult.stdout || "").trim()}`,
    );
  }

  fs.rmSync(archivePath, { force: true });
  fs.rmSync(extractedRoot, { recursive: true, force: true });
  return nodeRoot;
}

function prepareWindowsCodexBridge({
  platform = process.platform,
  architecture = process.arch,
  environment = process.env,
  nodeExecutable = process.execPath,
  spawn = spawnSync,
} = {}) {
  if (platform !== "win32") return false;
  if (architecture !== "x64") {
    throw new Error(`Windows Codex ACP Bridge 暂不支持 ${architecture} 架构`);
  }

  const expected = expectedMarker({ architecture });
  if (isPrepared(expected)) {
    writeWindowsBridgeOverlay();
    console.log(`[codex-bridge] 复用 Windows/${architecture} Bridge`);
    return false;
  }

  const stagingRoot = path.join(
    WINDOWS_RUNTIME_ROOT,
    `.codex-bridge.tmp-${process.pid}-${Date.now()}`,
  );
  const bridgeRoot = path.join(stagingRoot, "codex-bridge");
  const acpRoot = path.join(bridgeRoot, "acp");
  fs.rmSync(stagingRoot, { recursive: true, force: true });
  fs.mkdirSync(acpRoot, { recursive: true });

  try {
    const nodeRoot = prepareWindowsNode(stagingRoot, {
      environment,
      spawn,
    });
    for (const fileName of ["package.json", "package-lock.json"]) {
      fs.copyFileSync(
        path.join(BRIDGE_PACKAGE_ROOT, fileName),
        path.join(acpRoot, fileName),
      );
    }

    const invocation = npmInstallInvocation({
      platform,
      environment,
      nodeExecutable,
      npmArgs: WINDOWS_NPM_CI_ARGS,
    });
    console.log("[codex-bridge] 从锁文件准备 Windows ACP Bridge");
    checkedSpawn(
      spawn,
      invocation.command,
      invocation.args,
      {
        cwd: acpRoot,
        env: environment,
        stdio: "inherit",
      },
      "安装 Windows ACP Bridge",
    );

    fs.rmSync(path.join(acpRoot, "package.json"), { force: true });
    fs.rmSync(path.join(acpRoot, "package-lock.json"), { force: true });
    fs.writeFileSync(
      path.join(bridgeRoot, MARKER_NAME),
      `${JSON.stringify(expected, null, 2)}\n`,
    );
    if (!isPrepared(expected, bridgeRoot, nodeRoot)) {
      throw new Error("Windows ACP Bridge 与 Node Runtime 准备后校验失败");
    }

    fs.rmSync(WINDOWS_BRIDGE_ROOT, { recursive: true, force: true });
    fs.rmSync(WINDOWS_NODE_ROOT, { recursive: true, force: true });
    fs.renameSync(bridgeRoot, WINDOWS_BRIDGE_ROOT);
    fs.renameSync(nodeRoot, WINDOWS_NODE_ROOT);
    writeWindowsBridgeOverlay();
    console.log(
      `[codex-bridge] Windows ACP Bridge + Node ready: ${WINDOWS_BRIDGE_ROOT}`,
    );
    return true;
  } finally {
    fs.rmSync(stagingRoot, { recursive: true, force: true });
  }
}

function prepareLinuxCodexBridge({
  platform = process.platform,
  spawn = spawnSync,
} = {}) {
  if (platform !== "linux") return false;
  const script = path.join(APP_ROOT, "scripts", "prepare-codex-bridge-runtime.sh");
  const result = spawn(script, [], {
    cwd: APP_ROOT,
    stdio: "inherit",
  });
  if (result.error) throw result.error;
  if (result.status !== 0) {
    throw new Error(`准备 Codex ACP Bridge 失败，退出码：${result.status ?? "unknown"}`);
  }
  return true;
}

function preparePlatformCodexBridge(options = {}) {
  const platform = options.platform ?? process.platform;
  if (platform === "linux") {
    return prepareLinuxCodexBridge({ ...options, platform });
  }
  if (platform === "win32") {
    return prepareWindowsCodexBridge({ ...options, platform });
  }
  return false;
}

function main() {
  preparePlatformCodexBridge();
}

if (require.main === module) {
  try {
    main();
  } catch (error) {
    console.error(`[codex-bridge] ${error.message}`);
    process.exitCode = 1;
  }
}

module.exports = {
  BRIDGE_ENTRYPOINT,
  BRIDGE_PACKAGE_ROOT,
  LOCKFILE_PATH,
  WINDOWS_BRIDGE_ROOT,
  WINDOWS_BRIDGE_CONFIG_PATH,
  WINDOWS_NODE_ARCHIVE_SHA256,
  WINDOWS_NODE_EXECUTABLE,
  WINDOWS_NODE_EXECUTABLE_SHA256,
  WINDOWS_NODE_ROOT,
  WINDOWS_NODE_VERSION,
  WINDOWS_NPM_CI_ARGS,
  expectedMarker,
  isPrepared,
  main,
  prepareLinuxCodexBridge,
  preparePlatformCodexBridge,
  prepareWindowsCodexBridge,
  windowsBridgeOverlay,
};
