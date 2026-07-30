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
const claudeExecutables = {
  arm64: path.join(
    runtimeRoot,
    "acp",
    "node_modules",
    "@anthropic-ai",
    "claude-agent-sdk-darwin-arm64",
    "claude",
  ),
  x64: path.join(
    runtimeRoot,
    "acp",
    "node_modules",
    "@anthropic-ai",
    "claude-agent-sdk-darwin-x64",
    "claude",
  ),
};
const claudeBridgeEntrypoint = path.join(
  runtimeRoot,
  "acp",
  "node_modules",
  "@agentclientprotocol",
  "claude-agent-acp",
  "dist",
  "index.js",
);

assert.equal(manifest.schema_version, 2);
assert.equal(manifest.platform, "darwin");
assert.equal(manifest.arch, "universal");
for (const [arch, executable] of Object.entries(nodes)) {
  assert.ok(fs.statSync(executable).size > 0, `${arch} Node Runtime 必须存在且非空`);
  const lipoArch = arch === "x64" ? "x86_64" : "arm64";
  const result = spawnSync("/usr/bin/lipo", [executable, "-verify_arch", lipoArch]);
  assert.equal(result.status, 0, `${arch} Node Runtime 架构必须正确`);
}
for (const [arch, executable] of Object.entries(claudeExecutables)) {
  assert.ok(fs.statSync(executable).size > 0, `${arch} Claude Runtime 必须存在且非空`);
  const lipoArch = arch === "x64" ? "x86_64" : "arm64";
  const result = spawnSync("/usr/bin/lipo", [executable, "-verify_arch", lipoArch]);
  assert.equal(result.status, 0, `${arch} Claude Runtime 架构必须正确`);
}

const hostArch = process.arch === "arm64" ? "arm64" : "x64";
const hostNode = nodes[hostArch];
const hostClaude = claudeExecutables[hostArch];
const nodeVersion = spawnSync(hostNode, ["--version"], {
  encoding: "utf8",
  timeout: 10_000,
});
assert.equal(nodeVersion.status, 0, nodeVersion.stderr);
assert.equal(nodeVersion.stdout.trim(), `v${manifest.node_version}`);
const claudeVersion = spawnSync(hostClaude, ["--version"], {
  encoding: "utf8",
  timeout: 10_000,
});
assert.equal(claudeVersion.status, 0, claudeVersion.stderr);
assert.notEqual(`${claudeVersion.stdout}${claudeVersion.stderr}`.trim(), "");

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
  .catch((error) => {
    console.error(error);
    process.exitCode = 1;
  });
