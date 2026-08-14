const assert = require('assert');
const childProcess = require('child_process');
const fs = require('fs');
const os = require('os');
const path = require('path');

const root = path.resolve(__dirname, '..');
const read = (relative) => fs.readFileSync(path.join(root, relative), 'utf8');
const manifest = (id) => JSON.parse(read(`resources/mcp-servers/${id}/manifest.json`));

const expected = new Map([
  ['gongwen', { imports: ['docx', 'lxml'], wheels: 3 }],
  ['pptx', { imports: ['pptx', 'lxml', 'PIL', 'xlsxwriter'], wheels: 5 }],
]);

const wheelByName = new Map();
for (const [id, contract] of expected) {
  const tool = manifest(id);
  const lock = tool.python_dependencies;
  assert.ok(lock, `${id}: python_dependencies is required`);
  assert.strictEqual(lock.schema_version, 1, `${id}: unsupported lock schema`);
  const target = lock.targets.find((item) => item.platform === 'windows-x64');
  assert.ok(target, `${id}: windows-x64 target is required`);
  assert.strictEqual(target.python, '3.13', `${id}: bundled Python ABI must be pinned`);
  assert.deepStrictEqual(target.imports, contract.imports, `${id}: import smoke list drifted`);
  assert.strictEqual(target.wheels.length, contract.wheels, `${id}: transitive wheel lock is incomplete`);

  for (const wheel of target.wheels) {
    assert.ok(wheel.name && wheel.version, `${id}: wheel identity is required`);
    assert.match(wheel.filename, /\.whl$/, `${id}: only wheel artifacts are allowed`);
    const url = new URL(wheel.url);
    assert.strictEqual(url.protocol, 'https:', `${id}: wheel URL must use HTTPS`);
    assert.strictEqual(url.hostname, 'files.pythonhosted.org', `${id}: wheel host is not trusted`);
    assert.strictEqual(path.posix.basename(url.pathname), wheel.filename, `${id}: URL filename mismatch`);
    assert.match(wheel.sha256, /^[0-9a-f]{64}$/, `${id}: SHA-256 must be pinned`);

    const prior = wheelByName.get(wheel.name);
    if (prior) {
      assert.deepStrictEqual(
        { version: wheel.version, sha256: wheel.sha256 },
        prior,
        `${wheel.name}: shared wheel cache identity drifted`,
      );
    } else {
      wheelByName.set(wheel.name, { version: wheel.version, sha256: wheel.sha256 });
    }
  }
}

const runnerPath = path.join(
  root,
  'src-tauri/resources/common/bundle/mcp-servers/python_dependency_runner.py',
);
assert.ok(fs.statSync(runnerPath).isFile(), 'managed Python MCP runner is missing');

const runnerFixture = fs.mkdtempSync(path.join(os.tmpdir(), 'pinvou-python-runner-'));
try {
  const sitePackages = path.join(runnerFixture, 'managed site-packages');
  const serverDirectory = path.join(runnerFixture, 'server directory');
  const ambientDirectory = path.join(runnerFixture, 'ambient user site');
  const sentinel = path.join(runnerFixture, 'sitecustomize-ran');
  fs.mkdirSync(sitePackages, { recursive: true });
  fs.mkdirSync(serverDirectory, { recursive: true });
  fs.mkdirSync(ambientDirectory, { recursive: true });
  fs.writeFileSync(path.join(sitePackages, 'managed_dependency.py'), 'VALUE = "managed"\n');
  fs.writeFileSync(path.join(serverDirectory, 'sibling.py'), 'VALUE = "sibling"\n');
  fs.writeFileSync(path.join(ambientDirectory, 'ambient_dependency.py'), 'VALUE = "ambient"\n');
  fs.writeFileSync(
    path.join(ambientDirectory, 'sitecustomize.py'),
    `from pathlib import Path\nPath(${JSON.stringify(sentinel)}).write_text("executed")\n`,
  );
  const serverScript = path.join(serverDirectory, 'server.py');
  fs.writeFileSync(
    serverScript,
    [
      'import json, sys',
      'from managed_dependency import VALUE as dependency',
      'from sibling import VALUE as sibling',
      'try:',
      '    import ambient_dependency',
      '    ambient = ambient_dependency.VALUE',
      'except ModuleNotFoundError:',
      '    ambient = None',
      'print(json.dumps({"dependency": dependency, "sibling": sibling, "ambient": ambient, "args": sys.argv[1:]}))',
      '',
    ].join('\n'),
  );
  const python = process.env.PINVOU3_TEST_PYTHON || (process.platform === 'win32' ? 'python' : 'python3');
  const result = childProcess.spawnSync(
    python,
    ['-I', '-S', runnerPath, sitePackages, serverScript, 'forwarded'],
    {
      encoding: 'utf8',
      env: { ...process.env, PYTHONPATH: ambientDirectory, PYTHONUSERBASE: ambientDirectory },
    },
  );
  assert.ifError(result.error);
  assert.strictEqual(result.status, 0, result.stderr);
  assert.deepStrictEqual(JSON.parse(result.stdout.trim()), {
    dependency: 'managed',
    sibling: 'sibling',
    ambient: null,
    args: ['forwarded'],
  });
  assert.ok(!fs.existsSync(sentinel), 'ambient sitecustomize executed before the managed runner');
} finally {
  fs.rmSync(runnerFixture, { recursive: true, force: true });
}

const runtimeBundle = read('src-tauri/src/features/runtime_bundle/platform/mod.rs');
assert.match(runtimeBundle, /python_dependency_runner\.py/, 'runner is not embedded into the runtime bundle');
const repairIndex = runtimeBundle.indexOf('.repair_installed_python_tools()');
const refreshIndex = runtimeBundle.indexOf('self.refresh_mcp_python_commands()?');
assert.ok(repairIndex >= 0, 'legacy managed Python installs are not repaired at startup');
assert.ok(repairIndex < refreshIndex, 'legacy repair must finish before engines consume mcp.json');

const marketplace = read('src-tauri/src/features/marketplace/mod.rs');
assert.match(marketplace, /缺少 Windows Python 依赖锁/, 'Windows lock failure must be explicit');
assert.match(marketplace, /bundle_mcp_python_runner/, 'managed environment is not wired into MCP launch');
assert.match(marketplace, /依赖启动器缺失/, 'a missing managed runner must fail before registration');
assert.match(marketplace, /"-I"\.to_string\(\)/, 'managed runner must ignore ambient Python state');
assert.match(marketplace, /"-S"\.to_string\(\)/, 'managed runner must disable automatic site loading');
assert.match(marketplace, /MARKETPLACE_TRANSACTION_LOCK/, 'marketplace state mutations need one lock domain');
assert.match(marketplace, /state-transaction\.json/, 'cross-file updates need restart recovery');
assert.match(marketplace, /prune_from_committed_state/, 'environment pruning must use committed liveness');
assert.match(
  marketplace,
  /prune Python dependencies after repair failed/,
  'restart repair must not fail when only unused-cache cleanup is blocked',
);

const pythonDependencies = read('src-tauri/src/features/marketplace/python_dependencies.rs');
assert.match(pythonDependencies, /PartialFileCleanup/, 'partial wheel writes need RAII cleanup');
assert.match(
  pythonDependencies,
  /cleanup_stale_install_artifacts/,
  'restart and prune must recover stale install artifacts',
);
assert.match(
  pythonDependencies,
  /is_staging_environment_name/,
  'staging cleanup needs a strict name contract',
);
assert.match(
  pythonDependencies,
  /is_partial_wheel_name/,
  'partial wheel cleanup needs a strict name contract',
);

const pathsSource = read('src-tauri/src/platform/paths.rs');
assert.match(pathsSource, /managed_python_command/, 'locked dependencies need a dedicated Python resolver');
assert.match(pathsSource, /bundled_python_path/, 'Windows locked dependencies must use bundled Python');

console.log('mcp python dependency contract: ok');
