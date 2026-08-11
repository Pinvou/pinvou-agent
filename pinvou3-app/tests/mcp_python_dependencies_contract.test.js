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
  fs.mkdirSync(sitePackages, { recursive: true });
  fs.mkdirSync(serverDirectory, { recursive: true });
  fs.writeFileSync(path.join(sitePackages, 'managed_dependency.py'), 'VALUE = "managed"\n');
  fs.writeFileSync(path.join(serverDirectory, 'sibling.py'), 'VALUE = "sibling"\n');
  const serverScript = path.join(serverDirectory, 'server.py');
  fs.writeFileSync(
    serverScript,
    [
      'import json, sys',
      'from managed_dependency import VALUE as dependency',
      'from sibling import VALUE as sibling',
      'print(json.dumps({"dependency": dependency, "sibling": sibling, "args": sys.argv[1:]}))',
      '',
    ].join('\n'),
  );
  const python = process.env.PINVOU3_TEST_PYTHON || (process.platform === 'win32' ? 'python' : 'python3');
  const result = childProcess.spawnSync(
    python,
    [runnerPath, sitePackages, serverScript, 'forwarded'],
    { encoding: 'utf8' },
  );
  assert.ifError(result.error);
  assert.strictEqual(result.status, 0, result.stderr);
  assert.deepStrictEqual(JSON.parse(result.stdout.trim()), {
    dependency: 'managed',
    sibling: 'sibling',
    args: ['forwarded'],
  });
} finally {
  fs.rmSync(runnerFixture, { recursive: true, force: true });
}

const runtimeBundle = read('src-tauri/src/features/runtime_bundle/platform/mod.rs');
assert.match(runtimeBundle, /python_dependency_runner\.py/, 'runner is not embedded into the runtime bundle');

const marketplace = read('src-tauri/src/features/marketplace/mod.rs');
assert.match(marketplace, /缺少 Windows Python 依赖锁/, 'Windows lock failure must be explicit');
assert.match(marketplace, /bundle_mcp_python_runner/, 'managed environment is not wired into MCP launch');
assert.match(marketplace, /依赖启动器缺失/, 'a missing managed runner must fail before registration');

console.log('mcp python dependency contract: ok');
