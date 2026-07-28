const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

const {
  codexAcpInheritsSettings,
  patchCodexAcpInheritSettings,
} = require("../scripts/tauri/codex-acp-patch.js");

const linuxBuildScript = fs.readFileSync(
  path.join(__dirname, "..", "scripts", "prepare-codex-bridge-runtime.sh"),
  "utf8",
);
assert.match(
  linuxBuildScript,
  /"\$NODE_DIST_ROOT\/bin\/node" "\$PATCH_SCRIPT"[\s\S]*?codex-acp\/dist\/index\.js/,
  "Linux packages must patch the installed Codex ACP bridge",
);
assert.match(
  linuxBuildScript,
  /"\$node" "\$PATCH_SCRIPT" --check "\$entry"/,
  "Linux packages must reject a cached bridge that does not inherit Codex settings",
);

const fixture = `var AgentMode = class _AgentMode {
  static ReadOnly = new _AgentMode(
    "read-only"
  );
  static AgentFullAccess = new _AgentMode(
    "agent-full-access"
  );
  static DEFAULT_AGENT_MODE = _AgentMode.Agent;
  static all() {
    return [_AgentMode.ReadOnly, _AgentMode.Agent, _AgentMode.AgentFullAccess];
  }
};
async function sendPrompt(agentMode, additionalDirectories) {
  return await this.codexClient.runTurn({
      threadId: "thread",
      approvalPolicy: agentMode.approvalPolicy,
      sandboxPolicy: addAdditionalDirectoriesToSandboxPolicy(agentMode.sandboxPolicy, additionalDirectories),
      summary: "auto"
    });
}
`;

const tempRoot = fs.mkdtempSync(path.join(os.tmpdir(), "pinvou-codex-config-"));
try {
  const entrypoint = path.join(tempRoot, "index.js");
  fs.writeFileSync(entrypoint, fixture);

  assert.equal(patchCodexAcpInheritSettings(entrypoint), true);
  const patched = fs.readFileSync(entrypoint, "utf8");
  assert.equal(codexAcpInheritsSettings(patched), true);
  assert.match(patched, /static DEFAULT_AGENT_MODE = _AgentMode\.Inherit/);
  assert.match(patched, /Use approval and sandbox defaults from Codex config\.toml/);
  assert.match(
    patched,
    /agentMode\.approvalPolicy === null \? \{\} : \{ approvalPolicy:/,
    "inherit mode must omit turn-level approvalPolicy",
  );
  assert.match(
    patched,
    /agentMode\.sandboxPolicy === null \? \{\} : \{/,
    "inherit mode must omit turn-level sandboxPolicy",
  );
  assert.equal(
    patchCodexAcpInheritSettings(entrypoint),
    false,
    "the packaged runtime patch must be idempotent",
  );

  const unknownEntrypoint = path.join(tempRoot, "unknown.js");
  fs.writeFileSync(unknownEntrypoint, "console.log('changed upstream');\n");
  assert.throws(
    () => patchCodexAcpInheritSettings(unknownEntrypoint),
    /入口已变化/,
    "an upstream bundle layout change must fail closed instead of shipping an unpatched bridge",
  );
} finally {
  fs.rmSync(tempRoot, { recursive: true, force: true });
}

console.log("✓ Codex ACP defaults to inherited system approval and sandbox settings");
