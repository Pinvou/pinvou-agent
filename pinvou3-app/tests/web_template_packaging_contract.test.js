const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { execFileSync } = require("node:child_process");
const {
  declaredPackageNames,
  expectedMarker,
  isPrepared,
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

const templateManifest = JSON.parse(
  fs.readFileSync(path.join(TEMPLATE_ROOT, "package.json"), "utf8"),
);
const declaredPackages = [
  ...new Set([
    ...Object.keys(templateManifest.dependencies || {}),
    ...Object.keys(templateManifest.devDependencies || {}),
  ]),
].sort();
assert.deepEqual(
  declaredPackageNames(),
  declaredPackages,
  "网页模板准备校验必须跟随直接依赖，不能保留已移除依赖的硬编码",
);
assert.ok(!declaredPackages.includes("esbuild"), "Vite 8 模板不再直接依赖 esbuild");

const fixtureRoot = fs.mkdtempSync(path.join(os.tmpdir(), "pinvou3-web-template-"));
const fixturePackageJson = path.join(fixtureRoot, "package.json");
const fixtureLockfile = path.join(fixtureRoot, "package-lock.json");
const fixtureNodeModules = path.join(fixtureRoot, "node_modules");
const fixtureMarker = path.join(fixtureNodeModules, ".pinvou-prepared.json");
const fixturePaths = {
  webTemplateRoot: fixtureRoot,
  packageJsonPath: fixturePackageJson,
  lockfilePath: fixtureLockfile,
  markerPath: fixtureMarker,
};
const markerForFixture = () => expectedMarker({
  platform: "test-platform",
  architecture: "test-architecture",
  ...fixturePaths,
});
const writeFixtureManifest = (manifest) => {
  fs.writeFileSync(fixturePackageJson, `${JSON.stringify(manifest, null, 2)}\n`);
};
const writeFixtureMarker = (marker) => {
  fs.mkdirSync(fixtureNodeModules, { recursive: true });
  fs.writeFileSync(fixtureMarker, `${JSON.stringify(marker, null, 2)}\n`);
};

try {
  fs.writeFileSync(fixtureLockfile, '{"lockfileVersion":3}\n');
  writeFixtureManifest({
    dependencies: {
      "@scope/runtime": "1.0.0",
      react: "19.0.0",
    },
    devDependencies: {
      react: "19.0.0",
      vite: "8.0.0",
    },
  });
  assert.deepEqual(
    declaredPackageNames(fixturePackageJson),
    ["@scope/runtime", "react", "vite"],
    "scoped 包和跨字段重复依赖必须正确解析并去重",
  );
  for (const packageName of declaredPackageNames(fixturePackageJson)) {
    fs.mkdirSync(path.join(fixtureNodeModules, packageName), { recursive: true });
  }
  const originalMarker = markerForFixture();
  assert.equal(originalMarker.format, 2, "package manifest 加入 marker 后必须提升格式版本");
  assert.ok(!declaredPackageNames(fixturePackageJson).includes("esbuild"));
  writeFixtureMarker(originalMarker);
  assert.equal(
    isPrepared(originalMarker, fixturePaths),
    true,
    "没有 esbuild 时，只要所有声明依赖目录存在就应判定为已准备",
  );

  fs.rmSync(path.join(fixtureNodeModules, "@scope", "runtime"), { recursive: true });
  assert.equal(
    isPrepared(originalMarker, fixturePaths),
    false,
    "缺少任一声明依赖目录时必须重新准备",
  );
  fs.mkdirSync(path.join(fixtureNodeModules, "@scope", "runtime"), { recursive: true });

  writeFixtureManifest({
    dependencies: {
      "@scope/runtime": "1.0.1",
      react: "19.0.0",
    },
    devDependencies: { vite: "8.0.0" },
  });
  assert.equal(
    isPrepared(originalMarker, fixturePaths),
    false,
    "package.json 修改后旧 marker 必须失效",
  );
  const manifestMarker = markerForFixture();
  writeFixtureMarker(manifestMarker);
  assert.equal(isPrepared(manifestMarker, fixturePaths), true);

  fs.writeFileSync(fixtureLockfile, '{"lockfileVersion":3,"changed":true}\n');
  assert.equal(
    isPrepared(manifestMarker, fixturePaths),
    false,
    "package-lock.json 修改后旧 marker 必须失效",
  );

  writeFixtureManifest({ dependencies: null });
  assert.deepEqual(
    declaredPackageNames(fixturePackageJson),
    [],
    "null dependencies 和缺失 devDependencies 必须按空字段处理",
  );
  writeFixtureManifest({ devDependencies: null });
  assert.deepEqual(
    declaredPackageNames(fixturePackageJson),
    [],
    "缺失 dependencies 和 null devDependencies 必须按空字段处理",
  );

  for (const fieldName of ["dependencies", "devDependencies"]) {
    for (const invalidValue of [[], "react", 1]) {
      writeFixtureManifest({ [fieldName]: invalidValue });
      assert.throws(
        () => declaredPackageNames(fixturePackageJson),
        new RegExp(`${fieldName} 必须是对象`),
        `${fieldName} 不得接受 ${typeof invalidValue} 类型`,
      );
    }
  }
  writeFixtureManifest({ dependencies: [] });
  assert.throws(
    () => isPrepared(originalMarker, fixturePaths),
    /dependencies 必须是对象/,
    "真实 isPrepared 路径必须明确报告非法 manifest 字段",
  );
} finally {
  fs.rmSync(fixtureRoot, { recursive: true, force: true });
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

assert.ok(!WINDOWS_NPM_CI_ARGS.includes("--omit=optional"));
assert.ok(WINDOWS_NPM_CI_ARGS.includes("--include=optional"));
assert.ok(WINDOWS_NPM_CI_ARGS.includes("--os=win32"));
assert.ok(WINDOWS_NPM_CI_ARGS.includes("--cpu=x64"));
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
