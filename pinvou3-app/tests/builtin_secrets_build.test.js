const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

const {
  loadBuiltinSecrets,
  parseEnvFile,
} = require("../scripts/tauri/builtin-secrets.js");
const { platformConfigPath } = require("../scripts/tauri/platform-config.js");

for (const platform of ["win32", "linux", "darwin"]) {
  const configPath = platformConfigPath(platform);
  assert.ok(fs.existsSync(configPath), `${platform} Tauri overlay must exist`);
  assert.doesNotThrow(() => JSON.parse(fs.readFileSync(configPath, "utf8")));
}

const parsed = parseEnvFile(`
# comment
export PINVOU3_BUILTIN_AMAP_KEY='amap-test'
PINVOU3_BUILTIN_IWENCAI_KEY="iwencai-test" # comment
export PINVOU3_BUILTIN_QCC_KEY=qcc-test
IGNORED=value
`);
assert.deepEqual(parsed, {
  PINVOU3_BUILTIN_AMAP_KEY: "amap-test",
  PINVOU3_BUILTIN_IWENCAI_KEY: "iwencai-test",
  PINVOU3_BUILTIN_QCC_KEY: "qcc-test",
});

const tempDir = fs.mkdtempSync(path.join(os.tmpdir(), "pinvou3-secrets-test-"));
const secretsPath = path.join(tempDir, ".builtin-secrets.env");
fs.writeFileSync(
  secretsPath,
  [
    "export PINVOU3_BUILTIN_AMAP_KEY='from-file-amap'",
    "export PINVOU3_BUILTIN_IWENCAI_KEY='from-file-iwencai'",
    "export PINVOU3_BUILTIN_QCC_KEY='from-file-qcc'",
    "",
  ].join("\n"),
  "utf8",
);

try {
  const environment = { PINVOU3_BUILTIN_AMAP_KEY: "from-environment-amap" };
  const result = loadBuiltinSecrets({ environment, secretsPath });
  assert.deepEqual(result.missing, []);
  assert.equal(environment.PINVOU3_BUILTIN_AMAP_KEY, "from-environment-amap");
  assert.equal(environment.PINVOU3_BUILTIN_IWENCAI_KEY, "from-file-iwencai");
  assert.equal(environment.PINVOU3_BUILTIN_QCC_KEY, "from-file-qcc");

  fs.writeFileSync(secretsPath, "PINVOU3_BUILTIN_AMAP_KEY='only-one'\n", "utf8");
  assert.throws(
    () => loadBuiltinSecrets({ environment: {}, secretsPath }),
    /PINVOU3_BUILTIN_IWENCAI_KEY/u,
  );
  const skipped = loadBuiltinSecrets({ environment: {}, secretsPath, allowMissing: true });
  assert.equal(skipped.missing.length, 2);
} finally {
  fs.rmSync(tempDir, { recursive: true, force: true });
}

console.log("builtin secrets build tests passed");
