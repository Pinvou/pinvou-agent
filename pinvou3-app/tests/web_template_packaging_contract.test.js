const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const { execFileSync } = require("node:child_process");
const {
  npmInstallInvocation,
  npmInvocation,
} = require("../scripts/tauri/web-template.js");
const {
  WINDOWS_NPM_CI_ARGS,
  expectedMarker: expectedBridgeMarker,
  hideWindowsChildProcesses,
} = require("../scripts/tauri/codex-bridge.js");

const APP_ROOT = path.resolve(__dirname, "..");
const REPO_ROOT = path.resolve(APP_ROOT, "..");
const TEMPLATE_ROOT = path.join(
  APP_ROOT,
  "src-tauri",
  "resources",
  "common",
  "web-template",
);

for (const required of ["package.json", "package-lock.json"]) {
  assert.ok(
    fs.existsSync(path.join(TEMPLATE_ROOT, required)),
    `网页模板必须保留 ${required}`,
  );
}

const trackedNodeModules = execFileSync(
  "git",
  ["ls-files", "--", ":(glob)**/web-template/node_modules/**"],
  { cwd: REPO_ROOT, encoding: "utf8" },
).trim();
assert.equal(
  trackedNodeModules,
  "",
  "源码仓库不得跟踪网页模板 node_modules",
);

const buildSource = fs.readFileSync(
  path.join(APP_ROOT, "scripts", "tauri", "build.js"),
  "utf8",
);
const prepareIndex = buildSource.indexOf("prepareWebTemplate();");
const manifestIndex = buildSource.indexOf("writeEffectiveArtifacts(");
assert.ok(prepareIndex >= 0, "统一 Tauri wrapper 必须准备网页模板依赖");
assert.ok(
  manifestIndex > prepareIndex,
  "必须先准备平台依赖，再生成安装包资源清单",
);

const prepareSource = fs.readFileSync(
  path.join(APP_ROOT, "scripts", "tauri", "web-template.js"),
  "utf8",
);
assert.match(prepareSource, /package-lock\.json/);
assert.match(prepareSource, /"ci"/);
assert.match(prepareSource, /platform/);
assert.match(prepareSource, /architecture/);

assert.deepEqual(
  npmInvocation({
    platform: "win32",
    env: { ComSpec: "C:\\Windows\\System32\\cmd.exe" },
  }),
  {
    command: "C:\\Windows\\System32\\cmd.exe",
    args: [
      "/d",
      "/s",
      "/c",
      "npm.cmd ci --prefer-offline --no-audit --no-fund",
    ],
  },
  "Windows 必须经 cmd.exe 调用 npm.cmd，避免 Node spawnSync EINVAL",
);
assert.deepEqual(
  npmInvocation({ platform: "linux" }),
  {
    command: "npm",
    args: ["ci", "--prefer-offline", "--no-audit", "--no-fund"],
  },
  "非 Windows 平台继续直接调用 npm",
);

assert.ok(WINDOWS_NPM_CI_ARGS.includes("--omit=optional"));
assert.deepEqual(Object.keys(expectedBridgeMarker({ architecture: "x64" })), [
  "schema_version",
  "platform",
  "arch",
  "package_json_sha256",
  "lockfile_sha256",
]);
const bridgePatchRoot = fs.mkdtempSync(
  path.join(require("node:os").tmpdir(), "pinvou3-codex-bridge-"),
);
try {
  const entrypoint = path.join(bridgePatchRoot, "index.js");
  fs.writeFileSync(
    entrypoint,
    [
      'spawn(`"${codexPath}" app-server`, { shell: true, env: spawnEnv })',
      'spawn(process.execPath, [bundledCodexPath, "app-server"], { env: spawnEnv })',
    ].join("\n"),
  );
  hideWindowsChildProcesses(entrypoint);
  const patched = fs.readFileSync(entrypoint, "utf8");
  assert.match(patched, /shell: true, env: spawnEnv, windowsHide: true/);
  assert.match(patched, /bundledCodexPath[\s\S]*?windowsHide: true/);
} finally {
  fs.rmSync(bridgePatchRoot, { recursive: true, force: true });
}
const packageJson = JSON.parse(fs.readFileSync(path.join(APP_ROOT, "package.json"), "utf8"));
assert.equal(
  packageJson.scripts["prepare:codex-bridge"],
  "node scripts/tauri/codex-bridge.js",
);
assert.deepEqual(
  npmInstallInvocation({
    platform: "win32",
    environment: { npm_execpath: "C:\\nodejs\\node_modules\\npm\\bin\\npm-cli.js" },
    nodeExecutable: "C:\\nodejs\\node.exe",
    npmArgs: ["ci", "--omit=optional"],
  }),
  {
    command: "C:\\nodejs\\node.exe",
    args: [
      "C:\\nodejs\\node_modules\\npm\\bin\\npm-cli.js",
      "ci",
      "--omit=optional",
    ],
  },
  "Windows 依赖准备应支持传入受控的 npm ci 参数",
);
assert.throws(
  () => npmInstallInvocation({ platform: "win32", npmArgs: ["ci", "& whoami"] }),
  /不受支持的字符/,
  "Windows command interpreter must reject injectable npm arguments",
);

console.log("web template packaging contract ok");
