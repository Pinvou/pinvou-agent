const fs = require("node:fs");

const INHERIT_MODE = `  static Inherit = new _AgentMode(
    "inherit",
    "Codex settings",
    "Use approval and sandbox defaults from Codex config.toml.",
    null,
    null,
    "inherit"
  );
`;

const READ_ONLY_ANCHOR = "  static ReadOnly = new _AgentMode(\n";
const DEFAULT_MODE_SOURCE = "  static DEFAULT_AGENT_MODE = _AgentMode.Agent;";
const DEFAULT_MODE_PATCHED = "  static DEFAULT_AGENT_MODE = _AgentMode.Inherit;";
const MODE_LIST_SOURCE =
  "    return [_AgentMode.ReadOnly, _AgentMode.Agent, _AgentMode.AgentFullAccess];";
const MODE_LIST_PATCHED =
  "    return [_AgentMode.Inherit, _AgentMode.ReadOnly, _AgentMode.Agent, _AgentMode.AgentFullAccess];";
const TURN_POLICY_SOURCE = `      approvalPolicy: agentMode.approvalPolicy,
      sandboxPolicy: addAdditionalDirectoriesToSandboxPolicy(agentMode.sandboxPolicy, additionalDirectories),`;
const TURN_POLICY_PATCHED = `      ...(agentMode.approvalPolicy === null ? {} : { approvalPolicy: agentMode.approvalPolicy }),
      ...(agentMode.sandboxPolicy === null ? {} : {
        sandboxPolicy: addAdditionalDirectoriesToSandboxPolicy(agentMode.sandboxPolicy, additionalDirectories)
      }),`;

const PATCH_MARKERS = [
  "  static Inherit = new _AgentMode(",
  DEFAULT_MODE_PATCHED,
  MODE_LIST_PATCHED,
  "...(agentMode.approvalPolicy === null ? {} : { approvalPolicy: agentMode.approvalPolicy })",
  "...(agentMode.sandboxPolicy === null ? {} : {",
];

function codexAcpInheritsSettings(source) {
  return PATCH_MARKERS.every(marker => source.includes(marker));
}

function replaceExactlyOnce(source, before, after, label) {
  const first = source.indexOf(before);
  if (first < 0 || source.indexOf(before, first + before.length) >= 0) {
    throw new Error(`Codex ACP ${label} 入口已变化，无法安全应用配置继承补丁`);
  }
  return `${source.slice(0, first)}${after}${source.slice(first + before.length)}`;
}

function patchCodexAcpInheritSettings(entrypointPath) {
  let source = fs.readFileSync(entrypointPath, "utf8");
  if (codexAcpInheritsSettings(source)) return false;
  if (PATCH_MARKERS.some(marker => source.includes(marker))) {
    throw new Error("Codex ACP 配置继承补丁不完整，拒绝继续打包");
  }

  source = replaceExactlyOnce(
    source,
    READ_ONLY_ANCHOR,
    `${INHERIT_MODE}${READ_ONLY_ANCHOR}`,
    "mode",
  );
  source = replaceExactlyOnce(
    source,
    DEFAULT_MODE_SOURCE,
    DEFAULT_MODE_PATCHED,
    "default mode",
  );
  source = replaceExactlyOnce(source, MODE_LIST_SOURCE, MODE_LIST_PATCHED, "mode list");
  source = replaceExactlyOnce(
    source,
    TURN_POLICY_SOURCE,
    TURN_POLICY_PATCHED,
    "turn policy",
  );
  if (!codexAcpInheritsSettings(source)) {
    throw new Error("Codex ACP 配置继承补丁应用后校验失败");
  }
  fs.writeFileSync(entrypointPath, source);
  return true;
}

function main() {
  const args = process.argv.slice(2);
  const checkOnly = args[0] === "--check";
  const entrypointPath = checkOnly ? args[1] : args[0];
  if (!entrypointPath) {
    throw new Error("缺少 Codex ACP entrypoint 路径");
  }
  if (checkOnly) {
    if (!codexAcpInheritsSettings(fs.readFileSync(entrypointPath, "utf8"))) {
      throw new Error("Codex ACP 尚未应用系统配置继承补丁");
    }
    return;
  }
  patchCodexAcpInheritSettings(entrypointPath);
}

if (require.main === module) {
  try {
    main();
  } catch (error) {
    console.error(`[codex-acp-patch] ${error.message}`);
    process.exitCode = 1;
  }
}

module.exports = {
  codexAcpInheritsSettings,
  patchCodexAcpInheritSettings,
};
