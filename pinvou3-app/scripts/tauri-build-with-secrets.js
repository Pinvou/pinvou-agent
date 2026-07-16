const fs = require("node:fs");
const path = require("node:path");
const { spawnSync } = require("node:child_process");

const REQUIRED_SECRET_NAMES = [
  "PINVOU3_BUILTIN_AMAP_KEY",
  "PINVOU3_BUILTIN_IWENCAI_KEY",
  "PINVOU3_BUILTIN_QCC_KEY",
];

function parseEnvFile(content) {
  const values = {};

  for (const sourceLine of content.split(/\r?\n/u)) {
    let line = sourceLine.trim();
    if (!line || line.startsWith("#")) continue;
    if (line.startsWith("export ")) line = line.slice("export ".length).trimStart();

    const separator = line.indexOf("=");
    if (separator < 1) continue;

    const name = line.slice(0, separator).trim();
    if (!REQUIRED_SECRET_NAMES.includes(name)) continue;

    let raw = line.slice(separator + 1).trim();
    if (raw.startsWith("'") || raw.startsWith('"')) {
      const quote = raw[0];
      const closing = raw.lastIndexOf(quote);
      if (closing <= 0 || !/^\s*(?:#.*)?$/u.test(raw.slice(closing + 1))) {
        throw new Error(`${name} 的引号格式无效`);
      }
      raw = raw.slice(1, closing);
    } else {
      raw = raw.replace(/\s+#.*$/u, "").trim();
    }
    values[name] = raw;
  }

  return values;
}

function loadBuiltinSecrets({
  environment = process.env,
  secretsPath = path.resolve(__dirname, "..", "..", "scripts", ".builtin-secrets.env"),
  allowMissing = environment.PINVOU3_SKIP_BUILTIN_SECRETS === "1",
} = {}) {
  const fileValues = fs.existsSync(secretsPath)
    ? parseEnvFile(fs.readFileSync(secretsPath, "utf8").replace(/^\uFEFF/u, ""))
    : {};
  const loaded = [];
  const missing = [];

  for (const name of REQUIRED_SECRET_NAMES) {
    const existing = String(environment[name] || "").trim();
    const fromFile = String(fileValues[name] || "").trim();
    const value = existing || fromFile;
    if (!value) {
      missing.push(name);
      continue;
    }
    environment[name] = value;
    loaded.push(name);
  }

  if (missing.length > 0 && !allowMissing) {
    throw new Error(
      `Windows 发布构建缺少内置 MCP 密钥：${missing.join(", ")}。` +
        `请填写 ${secretsPath}，或仅在明确不需要内置额度时设置 PINVOU3_SKIP_BUILTIN_SECRETS=1。`,
    );
  }

  return { loaded, missing, secretsPath };
}

function main() {
  const args = process.argv.slice(2);
  const validateOnly = args[0] === "--validate-only";
  if (validateOnly) args.shift();

  const result = loadBuiltinSecrets();
  if (result.missing.length > 0) {
    console.warn(`[build] 已显式跳过 ${result.missing.length} 项内置 MCP 密钥。`);
  } else {
    console.log(`[build] 已加载并校验 ${result.loaded.length} 项内置 MCP 密钥。`);
  }
  if (validateOnly) return;

  const tauriCli = require.resolve("@tauri-apps/cli/tauri.js");
  const child = spawnSync(process.execPath, [tauriCli, ...args], {
    cwd: path.resolve(__dirname, ".."),
    env: process.env,
    stdio: "inherit",
  });
  if (child.error) throw child.error;
  process.exitCode = child.status === null ? 1 : child.status;
}

if (require.main === module) {
  try {
    main();
  } catch (error) {
    console.error(`[build] ${error.message}`);
    process.exitCode = 1;
  }
}

module.exports = { REQUIRED_SECRET_NAMES, loadBuiltinSecrets, parseEnvFile };
