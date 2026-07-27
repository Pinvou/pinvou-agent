const crypto = require("node:crypto");
const fs = require("node:fs");
const path = require("node:path");
const { spawnSync } = require("node:child_process");

const { APP_ROOT } = require("./platform-config.js");
const { npmInstallInvocation } = require("./web-template.js");

const BRIDGE_PACKAGE_ROOT = path.join(APP_ROOT, "scripts", "codex-bridge-runtime");
const LOCKFILE_PATH = path.join(BRIDGE_PACKAGE_ROOT, "package-lock.json");
const WINDOWS_BRIDGE_ROOT = path.join(
  APP_ROOT,
  "src-tauri",
  "target",
  "windows-runtime",
  "codex-bridge",
);
const WINDOWS_BRIDGE_CONFIG_PATH = path.join(
  APP_ROOT,
  "src-tauri",
  "target",
  "windows-runtime",
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
const PREPARE_FORMAT_VERSION = 1;
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

function windowsBridgeOverlay() {
  return {
    bundle: {
      resources: {
        "target/windows-runtime/codex-bridge/": "runtime/codex-bridge",
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
      nonemptyFile(path.join(outputRoot, BRIDGE_ENTRYPOINT))
    );
  } catch {
    return false;
  }
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

  const stagingRoot = `${WINDOWS_BRIDGE_ROOT}.tmp-${process.pid}-${Date.now()}`;
  const acpRoot = path.join(stagingRoot, "acp");
  fs.rmSync(stagingRoot, { recursive: true, force: true });
  fs.mkdirSync(acpRoot, { recursive: true });

  try {
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
    const result = spawn(invocation.command, invocation.args, {
      cwd: acpRoot,
      env: environment,
      stdio: "inherit",
    });
    if (result.error) throw result.error;
    if (result.status !== 0) {
      throw new Error(`Windows ACP Bridge 安装失败，npm ci 退出码：${result.status}`);
    }

    fs.rmSync(path.join(acpRoot, "package.json"), { force: true });
    fs.rmSync(path.join(acpRoot, "package-lock.json"), { force: true });
    fs.writeFileSync(
      path.join(stagingRoot, MARKER_NAME),
      `${JSON.stringify(expected, null, 2)}\n`,
    );
    if (!isPrepared(expected, stagingRoot)) {
      throw new Error("Windows ACP Bridge 准备后校验失败");
    }

    fs.rmSync(WINDOWS_BRIDGE_ROOT, { recursive: true, force: true });
    fs.renameSync(stagingRoot, WINDOWS_BRIDGE_ROOT);
    writeWindowsBridgeOverlay();
    console.log(`[codex-bridge] Windows ACP Bridge ready: ${WINDOWS_BRIDGE_ROOT}`);
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
  WINDOWS_NPM_CI_ARGS,
  expectedMarker,
  isPrepared,
  main,
  prepareLinuxCodexBridge,
  preparePlatformCodexBridge,
  prepareWindowsCodexBridge,
  windowsBridgeOverlay,
};
