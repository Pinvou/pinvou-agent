const assert = require("node:assert/strict");
const fs = require("node:fs");
const { spawn, spawnSync } = require("node:child_process");
const path = require("node:path");

assert.equal(process.platform, "darwin", "此冒烟测试必须在 macOS 原生 runner 执行");

const appRoot = path.resolve(__dirname, "..");
const runtimeRoot = path.join(
  appRoot,
  "src-tauri",
  "resources",
  "platforms",
  "macos",
  "codex-bridge",
);
const manifest = JSON.parse(
  fs.readFileSync(path.join(runtimeRoot, "manifest.json"), "utf8"),
);
const nodes = {
  arm64: path.join(runtimeRoot, "node", "darwin-arm64", "bin", "node"),
  x64: path.join(runtimeRoot, "node", "darwin-x64", "bin", "node"),
};
const npmCli = path.join(runtimeRoot, "node", "lib", "node_modules", "npm", "bin", "npm-cli.js");
const anthropicScope = path.join(runtimeRoot, "acp", "node_modules", "@anthropic-ai");
const codexBridgeEntrypoint = path.join(
  runtimeRoot,
  "acp",
  "node_modules",
  "@agentclientprotocol",
  "codex-acp",
  "dist",
  "index.js",
);
const claudeBridgeEntrypoint = path.join(
  runtimeRoot,
  "acp",
  "node_modules",
  "@agentclientprotocol",
  "claude-agent-acp",
  "dist",
  "index.js",
);

assert.equal(manifest.schema_version, 3);
assert.equal(manifest.platform, "darwin");
assert.equal(manifest.arch, "universal");
assert.equal(manifest.npm, "node/lib/node_modules/npm/bin/npm-cli.js");
assert.ok(fs.statSync(npmCli).size > 0, "连接器首次安装所需 npm CLI 必须随包存在");
for (const [arch, executable] of Object.entries(nodes)) {
  assert.ok(fs.statSync(executable).size > 0, `${arch} Node Runtime 必须存在且非空`);
  const lipoArch = arch === "x64" ? "x86_64" : "arm64";
  const result = spawnSync("/usr/bin/lipo", [executable, "-verify_arch", lipoArch]);
  assert.equal(result.status, 0, `${arch} Node Runtime 架构必须正确`);
}
// Claude Code 走系统安装（与 Codex/Kimi 一致），Bridge 不得携带 claude 平台原生二进制。
const leftoverClaudePackages = fs
  .readdirSync(anthropicScope)
  .filter((entry) => entry.startsWith("claude-agent-sdk-"));
assert.deepEqual(
  leftoverClaudePackages,
  [],
  `Bridge 中不得残留 Claude 平台二进制: ${leftoverClaudePackages.join(", ")}`,
);

const hostArch = process.arch === "arm64" ? "arm64" : "x64";
const hostNode = nodes[hostArch];
const nodeVersion = spawnSync(hostNode, ["--version"], {
  encoding: "utf8",
  timeout: 10_000,
});
assert.equal(nodeVersion.status, 0, nodeVersion.stderr);
assert.equal(nodeVersion.stdout.trim(), `v${manifest.node_version}`);
const npmVersion = spawnSync(hostNode, [npmCli, "--version"], {
  encoding: "utf8",
  timeout: 10_000,
});
assert.equal(npmVersion.status, 0, npmVersion.stderr);
assert.match(npmVersion.stdout.trim(), /^\d+\.\d+\.\d+$/);
const codexVersion = spawnSync(hostNode, [codexBridgeEntrypoint, "--version"], {
  encoding: "utf8",
  timeout: 10_000,
});
assert.equal(codexVersion.status, 0, codexVersion.stderr);
assert.equal(
  codexVersion.stdout.trim(),
  `@agentclientprotocol/codex-acp ${manifest.codex_acp_version}`,
);

function initializeClaudeBridge() {
  return new Promise((resolve, reject) => {
    const child = spawn(hostNode, [claudeBridgeEntrypoint], {
      cwd: runtimeRoot,
      env: process.env,
      stdio: ["pipe", "pipe", "pipe"],
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
      const lines = stdout.split(/\r?\n/);
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
          clientInfo: { name: "pinvou3-macos-smoke", version: "1.0.0" },
        },
      })}\n`,
    );
  });
}

initializeClaudeBridge()
  .then(() => {
    console.log("macOS universal Codex + Claude ACP Bridge Runtime: ok");
  })
  // eslint-disable-next-line unicorn/prefer-top-level-await -- smoke 脚本既有 async main() 结构
  .catch((error) => {
    console.error(error);
    process.exitCode = 1;
  });
