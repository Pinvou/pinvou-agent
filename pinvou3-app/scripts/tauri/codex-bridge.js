const crypto = require("node:crypto");
const fs = require("node:fs");
const path = require("node:path");
const { spawnSync } = require("node:child_process");

const { APP_ROOT } = require("./platform-config.js");
const { npmInstallInvocation } = require("./npm-invocation.js");

const BRIDGE_PACKAGE_ROOT = path.join(APP_ROOT, "scripts", "codex-bridge-runtime");
const PACKAGE_JSON_PATH = path.join(BRIDGE_PACKAGE_ROOT, "package.json");
const LOCKFILE_PATH = path.join(BRIDGE_PACKAGE_ROOT, "package-lock.json");
const WINDOWS_RUNTIME_ROOT = path.join(
  APP_ROOT,
  "src-tauri",
  "target",
  "windows-runtime",
);
const WINDOWS_BRIDGE_ROOT = path.join(WINDOWS_RUNTIME_ROOT, "codex-bridge");
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
const CLAUDE_BRIDGE_ENTRYPOINT = path.join(
  "acp",
  "node_modules",
  "@agentclientprotocol",
  "claude-agent-acp",
  "dist",
  "index.js",
);
const CLAUDE_BRIDGE_PACKAGE_JSON = path.join(
  "acp",
  "node_modules",
  "@agentclientprotocol",
  "claude-agent-acp",
  "package.json",
);
const MARKER_NAME = "manifest.json";
const PREPARE_FORMAT_VERSION = 4;
const MINIMUM_NODE_MAJOR = 20;
const CODEX_PATH_SPAWN =
  'spawn(`"${codexPath}" app-server`, { shell: true, env: spawnEnv })';
const HIDDEN_CODEX_PATH_SPAWN =
  'spawn(`"${codexPath}" app-server`, { shell: true, env: spawnEnv, windowsHide: true })';
const BUNDLED_CODEX_SPAWN =
  'spawn(process.execPath, [bundledCodexPath, "app-server"], { env: spawnEnv })';
const HIDDEN_BUNDLED_CODEX_SPAWN =
  'spawn(process.execPath, [bundledCodexPath, "app-server"], { env: spawnEnv, windowsHide: true })';
const WINDOWS_NPM_CI_ARGS = [
  "ci",
  "--prefer-offline",
  "--no-audit",
  "--no-fund",
  "--omit=dev",
  "--include=optional",
  "--os=win32",
  "--cpu=x64",
];

function fileSha256(filePath) {
  return crypto.createHash("sha256").update(fs.readFileSync(filePath)).digest("hex");
}

function expectedCodexAcpVersion() {
  const packageJson = JSON.parse(fs.readFileSync(PACKAGE_JSON_PATH, "utf8"));
  return packageJson.dependencies["@agentclientprotocol/codex-acp"];
}

function expectedClaudeAcpVersion() {
  const packageJson = JSON.parse(fs.readFileSync(PACKAGE_JSON_PATH, "utf8"));
  return packageJson.dependencies["@agentclientprotocol/claude-agent-acp"];
}

function expectedMarker({ architecture = process.arch } = {}) {
  return {
    schema_version: PREPARE_FORMAT_VERSION,
    platform: "win32",
    arch: architecture,
    package_json_sha256: fileSha256(PACKAGE_JSON_PATH),
    lockfile_sha256: fileSha256(LOCKFILE_PATH),
  };
}

function hideWindowsChildProcesses(entrypointPath) {
  let source = fs.readFileSync(entrypointPath, "utf8");
  const replacements = [
    [CODEX_PATH_SPAWN, HIDDEN_CODEX_PATH_SPAWN],
    [BUNDLED_CODEX_SPAWN, HIDDEN_BUNDLED_CODEX_SPAWN],
  ];

  for (const [visibleSpawn, hiddenSpawn] of replacements) {
    if (source.includes(hiddenSpawn)) continue;
    if (!source.includes(visibleSpawn)) {
      throw new Error(
        "Windows ACP Bridge 子进程入口已变化，无法安全应用隐藏控制台补丁",
      );
    }
    source = source.replace(visibleSpawn, hiddenSpawn);
  }
  fs.writeFileSync(entrypointPath, source);
}

function windowsChildProcessesHidden(entrypointPath) {
  try {
    const source = fs.readFileSync(entrypointPath, "utf8");
    return (
      source.includes(HIDDEN_CODEX_PATH_SPAWN) &&
      source.includes(HIDDEN_BUNDLED_CODEX_SPAWN)
    );
  } catch {
    return false;
  }
}

function nonemptyFile(filePath) {
  try {
    const stat = fs.statSync(filePath);
    return stat.isFile() && stat.size > 0;
  } catch {
    return false;
  }
}

function packageDirectories(root, scope, prefix) {
  const scopeRoot = path.join(root, "acp", "node_modules", scope);
  try {
    return fs
      .readdirSync(scopeRoot, { withFileTypes: true })
      .filter((entry) => entry.isDirectory() && entry.name.startsWith(prefix))
      .map((entry) => path.join(scopeRoot, entry.name));
  } catch {
    return [];
  }
}

function pruneWindowsPlatformPackages(root) {
  for (const packageRoot of packageDirectories(root, "@openai", "codex-")) {
    fs.rmSync(packageRoot, { recursive: true, force: true });
  }
  // Claude Code 走系统安装（与 Codex/Kimi 一致），适配器通过
  // CLAUDE_CODE_EXECUTABLE / PATH 中的 claude 启动，不随包携带 claude.exe
  //（单个原生二进制约 140MB）。
  for (const packageRoot of packageDirectories(
    root,
    "@anthropic-ai",
    "claude-agent-sdk-",
  )) {
    fs.rmSync(packageRoot, { recursive: true, force: true });
  }
}

function platformPackagesValid(root) {
  const codexPackages = packageDirectories(root, "@openai", "codex-");
  const claudePackages = packageDirectories(
    root,
    "@anthropic-ai",
    "claude-agent-sdk-",
  );
  return codexPackages.length === 0 && claudePackages.length === 0;
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

function isPrepared(expected = expectedMarker(), outputRoot = WINDOWS_BRIDGE_ROOT) {
  try {
    const actual = JSON.parse(
      fs.readFileSync(path.join(outputRoot, MARKER_NAME), "utf8"),
    );
    const packageJson = JSON.parse(
      fs.readFileSync(path.join(outputRoot, BRIDGE_PACKAGE_JSON), "utf8"),
    );
    const claudePackageJson = JSON.parse(
      fs.readFileSync(path.join(outputRoot, CLAUDE_BRIDGE_PACKAGE_JSON), "utf8"),
    );
    return (
      JSON.stringify(actual) === JSON.stringify(expected) &&
      packageJson.version === expectedCodexAcpVersion() &&
      claudePackageJson.version === expectedClaudeAcpVersion() &&
      nonemptyFile(path.join(outputRoot, BRIDGE_ENTRYPOINT)) &&
      nonemptyFile(path.join(outputRoot, CLAUDE_BRIDGE_ENTRYPOINT)) &&
      platformPackagesValid(outputRoot) &&
      windowsChildProcessesHidden(path.join(outputRoot, BRIDGE_ENTRYPOINT))
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

function validateNodeRuntime(nodeExecutable, { environment, spawn } = {}) {
  if (!nonemptyFile(nodeExecutable)) {
    throw new Error(`Codex Bridge 缺少可用 Node：${nodeExecutable}`);
  }
  const result = checkedSpawn(
    spawn,
    nodeExecutable,
    ["--version"],
    { env: environment, encoding: "utf8" },
    "验证 Codex Bridge Node",
  );
  const version = String(result.stdout || "").trim();
  const major = /^v(\d+)\./u.exec(version)?.[1];
  if (!major || Number(major) < MINIMUM_NODE_MAJOR) {
    throw new Error(
      `Codex Bridge Node 版本不受支持：${version || "unknown"}，需要 Node ${MINIMUM_NODE_MAJOR}+`,
    );
  }
  return version;
}

function prepareWindowsCodexBridge({
  platform = process.platform,
  architecture = process.arch,
  environment = process.env,
  nodeExecutable = process.execPath,
  npmExecPath,
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
    const nodeVersion = validateNodeRuntime(nodeExecutable, { environment, spawn });
    const installEnvironment = npmExecPath
      ? { ...environment, npm_execpath: npmExecPath }
      : environment;
    if (npmExecPath && !nonemptyFile(npmExecPath)) {
      throw new Error(`Codex Bridge 缺少可用 npm CLI：${npmExecPath}`);
    }

    for (const fileName of ["package.json", "package-lock.json"]) {
      fs.copyFileSync(
        path.join(BRIDGE_PACKAGE_ROOT, fileName),
        path.join(acpRoot, fileName),
      );
    }

    const invocation = npmInstallInvocation({
      platform,
      environment: installEnvironment,
      nodeExecutable,
      npmArgs: WINDOWS_NPM_CI_ARGS,
    });
    console.log(`[codex-bridge] 使用现有 ${nodeVersion} 从锁文件准备 Windows ACP Bridge`);
    checkedSpawn(
      spawn,
      invocation.command,
      invocation.args,
      {
        cwd: acpRoot,
        env: installEnvironment,
        stdio: "inherit",
      },
      "安装 Windows ACP Bridge",
    );

    pruneWindowsPlatformPackages(bridgeRoot);
    hideWindowsChildProcesses(path.join(bridgeRoot, BRIDGE_ENTRYPOINT));
    fs.rmSync(path.join(acpRoot, "package.json"), { force: true });
    fs.rmSync(path.join(acpRoot, "package-lock.json"), { force: true });
    fs.writeFileSync(
      path.join(bridgeRoot, MARKER_NAME),
      `${JSON.stringify(expected, null, 2)}\n`,
    );
    if (!isPrepared(expected, bridgeRoot)) {
      throw new Error("Windows ACP Bridge 准备后校验失败");
    }

    fs.rmSync(WINDOWS_BRIDGE_ROOT, { recursive: true, force: true });
    fs.renameSync(bridgeRoot, WINDOWS_BRIDGE_ROOT);
    writeWindowsBridgeOverlay();
    console.log(`[codex-bridge] Windows ACP Bridge ready: ${WINDOWS_BRIDGE_ROOT}`);
    return true;
  } finally {
    fs.rmSync(stagingRoot, { recursive: true, force: true });
  }
}

function prepareCodexBridge({
  platform = process.platform,
  spawn = spawnSync,
} = {}) {
  if (platform !== "linux" && platform !== "darwin") return false;
  const script = path.join(APP_ROOT, "scripts", "prepare-codex-bridge-runtime.sh");
  const result = spawn(script, [], {
    cwd: APP_ROOT,
    stdio: "inherit",
  });
  if (result.error) throw result.error;
  if (result.status !== 0) {
    throw new Error(
      `准备 Codex ACP Bridge 失败，退出码：${result.status ?? "unknown"}`,
    );
  }
  return true;
}

function preparePlatformCodexBridge(options = {}) {
  const platform = options.platform ?? process.platform;
  if (platform === "linux" || platform === "darwin") {
    return prepareCodexBridge({ ...options, platform });
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
  CLAUDE_BRIDGE_ENTRYPOINT,
  LOCKFILE_PATH,
  MINIMUM_NODE_MAJOR,
  WINDOWS_BRIDGE_ROOT,
  WINDOWS_BRIDGE_CONFIG_PATH,
  WINDOWS_NPM_CI_ARGS,
  expectedMarker,
  hideWindowsChildProcesses,
  isPrepared,
  main,
  prepareCodexBridge,
  preparePlatformCodexBridge,
  prepareWindowsCodexBridge,
  validateNodeRuntime,
  windowsBridgeOverlay,
};
