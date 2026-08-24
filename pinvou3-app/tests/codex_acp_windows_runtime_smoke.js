const assert = require("node:assert/strict");
const { spawn, spawnSync } = require("node:child_process");
const fs = require("node:fs");
const path = require("node:path");

const {
  BRIDGE_ENTRYPOINT,
  CLAUDE_BRIDGE_ENTRYPOINT,
  expectedMarker,
  prepareWindowsCodexBridge,
  WINDOWS_BRIDGE_CONFIG_PATH,
  WINDOWS_BRIDGE_ROOT,
} = require("../scripts/tauri/codex-bridge.js");
const {
  buildResourceManifest,
  composeEffectiveConfig,
} = require("../scripts/tauri/effective-config.js");
const { platformConfigPath } = require("../scripts/tauri/platform-config.js");
const {
  WINDOWS_RUNTIME_CONFIG_PATH,
  describeWindowsRuntime,
} = require("../scripts/tauri/windows-runtime.js");

assert.equal(process.platform, "win32", "此冒烟测试必须在 Windows 原生 runner 执行");
assert.equal(process.arch, "x64", "Windows Codex Runtime 当前只支持 x64");

const stagedRuntime = fs.existsSync(WINDOWS_RUNTIME_CONFIG_PATH)
  ? describeWindowsRuntime()
  : null;
const nodeExecutable = stagedRuntime?.nodeExecutable ?? process.execPath;
prepareWindowsCodexBridge({
  nodeExecutable,
  npmExecPath: stagedRuntime?.npmExecPath,
});

const marker = expectedMarker({ architecture: "x64" });
const sourcePackage = JSON.parse(
  fs.readFileSync(
    path.join(__dirname, "..", "scripts", "codex-bridge-runtime", "package.json"),
    "utf8",
  ),
);
const bridgeVersion = spawnSync(
  nodeExecutable,
  [path.join(WINDOWS_BRIDGE_ROOT, BRIDGE_ENTRYPOINT), "--version"],
  {
    encoding: "utf8",
    env: { ...process.env, CODEX_PATH: "" },
  },
);
assert.equal(bridgeVersion.error, undefined);
assert.equal(bridgeVersion.status, 0, bridgeVersion.stderr);
assert.equal(
  bridgeVersion.stdout.trim(),
  `@agentclientprotocol/codex-acp ${sourcePackage.dependencies["@agentclientprotocol/codex-acp"]}`,
);

// Claude Code 走系统安装（与 Codex/Kimi 一致），Bridge 不得携带 claude 平台原生二进制。
const anthropicScope = path.join(
  WINDOWS_BRIDGE_ROOT,
  "acp",
  "node_modules",
  "@anthropic-ai",
);
const leftoverClaudePackages = fs
  .readdirSync(anthropicScope)
  .filter((entry) => entry.startsWith("claude-agent-sdk-"));
assert.deepEqual(
  leftoverClaudePackages,
  [],
  `Bridge 中不得残留 Claude 平台二进制: ${leftoverClaudePackages.join(", ")}`,
);

const { effectiveConfig } = composeEffectiveConfig([
  platformConfigPath("win32"),
  ...(stagedRuntime ? [stagedRuntime.configPath] : []),
  WINDOWS_BRIDGE_CONFIG_PATH,
]);
const manifest = buildResourceManifest(effectiveConfig, { platform: "win32" });
const destinations = new Set(manifest.files.map((file) => file.destination));
assert.ok(
  destinations.has(
    `runtime/codex-bridge/${BRIDGE_ENTRYPOINT.replaceAll(path.sep, "/")}`,
  ),
);
assert.ok(destinations.has("runtime/codex-bridge/manifest.json"));
if (stagedRuntime) {
  assert.ok(destinations.has("runtime/node/node.exe"));
  assert.ok(destinations.has("runtime/node/LICENSE"));
} else {
  assert.ok(
    [...destinations].every((destination) => !destination.startsWith("runtime/node/")),
    "Codex Bridge overlay must not package a second Node runtime",
  );
}

function initializeClaudeBridge() {
  return new Promise((resolve, reject) => {
    const entrypoint = path.join(WINDOWS_BRIDGE_ROOT, CLAUDE_BRIDGE_ENTRYPOINT);
    const child = spawn(nodeExecutable, [entrypoint], {
      cwd: WINDOWS_BRIDGE_ROOT,
      env: process.env,
      stdio: ["pipe", "pipe", "pipe"],
      windowsHide: true,
    });
    let settled = false;
    let stdout = "";
    let stderr = "";
    const timer = setTimeout(() => {
      finish(new Error(`Claude ACP initialize 超时: ${stderr.trim()}`));
    }, 15_000);

    function finish(error) {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      child.kill();
      if (error) reject(error);
      else resolve();
    }

    child.on("error", finish);
    child.on("exit", (code) => {
      if (!settled) {
        finish(new Error(`Claude ACP initialize 前退出: code=${code} stderr=${stderr.trim()}`));
      }
    });
    child.stderr.on("data", (chunk) => {
      stderr += chunk.toString();
    });
    child.stdout.on("data", (chunk) => {
      stdout += chunk.toString();
      const lines = stdout.split(/\r?\n/u);
      stdout = lines.pop() || "";
      for (const line of lines) {
        if (!line.trim()) continue;
        let response;
        try {
          response = JSON.parse(line);
        } catch {
          continue;
        }
        if (response.id !== 1) continue;
        if (response.error) {
          finish(new Error(`Claude ACP initialize 失败: ${JSON.stringify(response.error)}`));
          return;
        }
        try {
          assert.equal(response.result.protocolVersion, 1);
          assert.equal(typeof response.result.agentInfo?.name, "string");
        } catch (error) {
          finish(error);
          return;
        }
        finish();
        return;
      }
    });
    child.stdin.on("error", () => {});
    child.stdin.write(
      `${JSON.stringify({
        jsonrpc: "2.0",
        id: 1,
        method: "initialize",
        params: {
          protocolVersion: 1,
          clientCapabilities: {},
          clientInfo: { name: "pinvou3-windows-smoke", version: "1.0.0" },
        },
      })}\n`,
    );
  });
}

assert.equal(marker.platform, "win32");
initializeClaudeBridge()
  .then(() => {
    console.log("Windows Codex + Claude ACP Bridge existing-Node runtime: ok");
  })
  // eslint-disable-next-line unicorn/prefer-top-level-await -- smoke 脚本既有 async main() 结构
  .catch((error) => {
    console.error(error);
    process.exitCode = 1;
  });
