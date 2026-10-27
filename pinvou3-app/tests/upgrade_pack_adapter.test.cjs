// Orchestrator/file controls. Injected pack/signer outcomes are not native,
// Manifest-cryptography, NSIS or full actual upgrade-pack acceptance evidence.
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { createHash } = require('node:crypto');
const F = require('../scripts/upgrade-pack/files.cjs');
const { packageBuild, configuration, provenanceMaterial } = require('../scripts/upgrade-pack/adapter.cjs');
const { childEnvironment, runChild } = require('../scripts/upgrade-pack/process.cjs');
const { main } = require('../scripts/upgrade-pack/run.cjs');
const digest = (bytes) => createHash('sha256').update(bytes).digest('hex');

function fixture() {
  // Private small synthetic files only; deliberately retain all cases for review.
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'pinvou-t04-adapter-control-'));
  const out = path.join(directory, 'output');
  const file = (name, bytes) => {
    const filename = path.join(directory, name); fs.writeFileSync(filename, bytes, { flag: 'wx' });
    return { path: filename, size: Buffer.byteLength(bytes), sha256: digest(bytes) };
  };
  const inputs = ['finalInstaller', 'helper', 'launcher', 'helperArtifact'].map((role, i) => ({
    role, helperId: i === 3 ? 'helper-1' : null, name: `native/role-${i}`, file: file(`role-${i}`, `role-${i}`),
  }));
  const claimsBody = { scope: { targetKey: 'windows-x86_64-nsis-perMachine' }, appVersion: '0.10.13',
    incrementalPackages: [], finalInstaller: { sha256: inputs[0].file.sha256 },
    provenance: { sourceRevision: { value: 'a'.repeat(40) } } };
  const claims = file('claims', JSON.stringify(claimsBody));
  const statement = { _type: 'https://in-toto.io/Statement/v1', predicateType: 'https://slsa.dev/provenance/v1',
    subject: [{ name: 'sample.exe', digest: { sha256: inputs[0].file.sha256 } }],
    predicate: { buildDefinition: { resolvedDependencies: [{ digest: { gitCommit: 'a'.repeat(40) } }] } } };
  // Structurally signed fixture only; zero signature is intentionally not trusted.
  const envelope = { payloadType: 'application/vnd.in-toto+json',
    payload: Buffer.from(JSON.stringify(statement)).toString('base64'),
    signatures: [{ keyid: 'isolated-test', sig: Buffer.alloc(64).toString('base64') }] };
  const config = { version: 1, packTool: file('pack.exe', 'pack executable fixture'),
    manifestSigner: file('signer.exe', 'signer executable fixture'), claims,
    sbom: file('sbom', '{"format-fixture":true}'), provenance: file('dsse', JSON.stringify(envelope)),
    root: file('root', '{"independent-test-root":true}'), at: 1, inputs,
    limits: { InputBytes: 1024 * 1024, TotalInputBytes: 8 * 1024 * 1024, OutputBytes: 1024 * 1024,
      Archive: { ArchiveBytes: 1024 * 1024, FileBytes: 1024 * 1024, ExpandedBytes: 8 * 1024 * 1024,
        Entries: 100, CompressionRatio: 100, Nesting: 2, NestedMemoryBytes: 1024 * 1024, MetadataBytes: 1024 * 1024 },
      JSONL: { Bytes: 1024 * 1024, LineBytes: 1024 * 1024, Records: 1 },
      Native: { PE: { Bytes: 1024 * 1024, OptionalHeaderBytes: 4096, Sections: 100, Certificates: 100 },
        Resources: { SourceBytes: 1024 * 1024, MemoryBytes: 1024 * 1024 },
        ResourceTree: { Entries: 100, Depth: 8, NameUnits: 100 } } } };
  const configPath = path.join(directory, 'config.json'); fs.writeFileSync(configPath, JSON.stringify(config), { flag: 'wx' });
  const calls = [];
  async function execute(executable, args) {
    calls.push({ executable, args });
    const value = (flag) => args[args.indexOf(flag) + 1];
    if (args[0] === 'prepare') {
      const p = value('-out'); fs.mkdirSync(p);
      for (const [name, bytes] of [['FullPack.zip', 'synthetic-fullpack'], ['unsigned-package.json', fs.readFileSync(config.claims.path)], ['signing-input.bin', 'original signing input']]) fs.writeFileSync(path.join(p, name), bytes, { flag: 'wx' });
    } else if (args[0] === '--input') {
      assert.deepEqual(args.filter((_, i) => i % 2 === 0), ['--input', '--claims', '--output']);
      fs.writeFileSync(value('--output'), 'synthetic envelope; not crypto evidence', { flag: 'wx' });
    } else if (args[0] === 'assemble') {
      const a = value('-out'); fs.mkdirSync(a);
      for (const name of ['FullPack.zip', 'unsigned-package.json', 'signing-input.bin']) fs.copyFileSync(path.join(out, 'prepared', name), path.join(a, name));
      fs.copyFileSync(path.join(out, 'signed-package.json'), path.join(a, 'UpdatePackInfo.json'));
      fs.copyFileSync(path.join(out, 'sbom.json'), path.join(a, 'sbom.json'));
      fs.copyFileSync(path.join(out, 'provenance.jsonl'), path.join(a, 'provenance.jsonl'));
      fs.writeFileSync(path.join(a, 'UploadPack.zip'), 'independent synthetic upload');
    } else assert.equal(args[0], 'verify');
    return 0;
  }
  return { directory, out, config, configPath, claimsBody, envelope, calls, execute };
}
const options = (f, execute = f.execute) => ({ appVersion: '0.10.13', execute });
const completion = (f) => path.join(f.out, 'completion.json');

function hostFixture(t) {
  const f = fixture();
  const tempRoot = fs.realpathSync(os.tmpdir());
  const owned = fs.realpathSync(f.directory);
  const relative = path.relative(tempRoot, owned);
  assert.ok(relative && !path.isAbsolute(relative) && relative !== '..' && !relative.startsWith(`..${path.sep}`));
  assert.ok(path.basename(owned).startsWith('pinvou-t04-adapter-control-'));
  t.after(() => fs.rmSync(owned, { recursive: true }));
  const source = path.join(f.directory, 'source'); fs.mkdirSync(source);
  fs.mkdirSync(path.join(source, 'app')); fs.mkdirSync(path.join(source, 'app', 'nested'));
  for (const [index, name] of ['installer.exe', 'pinvou-upgrade-helper.exe', 'pinvou-upgrade-launcher.exe'].entries()) {
    fs.copyFileSync(f.config.inputs[index].file.path, path.join(source, name));
  }
  fs.writeFileSync(path.join(source, 'app', 'pinvou3-tauri.exe'), 'actual app fixture');
  fs.writeFileSync(path.join(source, 'app', 'nested', 'empty.txt'), '');
  f.config.version = 2; f.config.sourceDirectory = source; delete f.config.inputs;
  fs.writeFileSync(f.configPath, JSON.stringify(f.config));
  return f;
}

test('fixed Windows host source is snapped and passed through the real adapter entry', async (t) => {
  const f = hostFixture(t);
  await packageBuild(f.configPath, f.out, options(f, async (exe, args, opts) => {
    if (args[0] === 'prepare') {
      const input = JSON.parse(fs.readFileSync(args[args.indexOf('-config') + 1], 'utf8'));
      assert.equal(input.sourceProfile, 'pinvou3-windows-host-source.v1');
      assert.equal(input.sourceDirectory, 'host-source');
      assert.deepEqual(input.inputs, []);
      assert.equal(fs.readFileSync(path.join(f.out, input.sourceDirectory, 'app', 'pinvou3-tauri.exe'), 'utf8'), 'actual app fixture');
      assert.equal(fs.statSync(path.join(f.out, input.sourceDirectory, 'app', 'nested', 'empty.txt')).size, 0);
    }
    return f.execute(exe, args, opts);
  }));
  assert.deepEqual(f.calls.map((call) => call.args[0]), ['prepare', '--input', 'assemble', 'verify']);
  const receipt = JSON.parse(fs.readFileSync(completion(f)));
  assert.equal(receipt.version, 2);
  assert.equal(receipt.hostSource.profile, 'pinvou3-windows-host-source.v1');
  assert.equal(receipt.hostSource.fileCount, 5);
  const names = ['installer.exe', 'pinvou-upgrade-helper.exe', 'pinvou-upgrade-launcher.exe',
    'app/nested/empty.txt', 'app/pinvou3-tauri.exe'];
  const records = names.map((name) => {
    const bytes = fs.readFileSync(path.join(f.config.sourceDirectory, ...name.split('/')));
    return `${JSON.stringify([name, bytes.length, digest(bytes)])}\n`;
  }).join('');
  assert.equal(receipt.hostSource.membersSha256, digest(records));
  assert.equal(receipt.dsseAuthenticated, false);
  assert.equal(receipt.currentQualificationChecked, false);
});

test('fixed Windows host source changes and missing roles refuse before completion', async (t) => {
  for (const fault of ['missing-helper', 'source-drift', 'new-entry', 'empty-drift', 'snapshot-drift']) await t.test(fault, async (t) => {
    const f = hostFixture(t);
    if (fault === 'missing-helper') fs.renameSync(path.join(f.config.sourceDirectory, 'pinvou-upgrade-helper.exe'), path.join(f.directory, 'retained-helper'));
    await assert.rejects(packageBuild(f.configPath, f.out, options(f, async (exe, args, opts) => {
      const code = await f.execute(exe, args, opts);
      if (args[0] === '--input' && fault === 'source-drift') fs.appendFileSync(path.join(f.config.sourceDirectory, 'app', 'pinvou3-tauri.exe'), 'changed');
      if (args[0] === '--input' && fault === 'new-entry') fs.writeFileSync(path.join(f.config.sourceDirectory, 'app', 'new.txt'), 'changed');
      if (args[0] === '--input' && fault === 'empty-drift') fs.writeFileSync(path.join(f.config.sourceDirectory, 'app', 'nested', 'empty.txt'), 'changed');
      if (args[0] === '--input' && fault === 'snapshot-drift') fs.appendFileSync(path.join(f.out, 'host-source', 'app', 'pinvou3-tauri.exe'), 'changed');
      return code;
    })));
    assert.equal(fs.existsSync(completion(f)), false);
  });
});

test('host source overlapping output is rejected before mutating source', async (t) => {
  const f = hostFixture(t);
  const outputInsideSource = path.join(f.config.sourceDirectory, 'app', 'output');
  await assert.rejects(packageBuild(f.configPath, outputInsideSource, options(f)), /T04_SOURCE_DIRECTORY/u);
  assert.equal(fs.existsSync(outputInsideSource), false);
  assert.equal(f.calls.length, 0);
});

test('host source refuses same-byte replacement between hashing and copying', async (t) => {
  const f = hostFixture(t);
  const target = path.join(f.config.sourceDirectory, 'app', 'pinvou3-tauri.exe');
  const originalHashReference = F.hashReference;
  let replaced = false;
  F.hashReference = (...args) => {
    const ref = originalHashReference(...args);
    if (args[0] === target && !replaced) {
      replaced = true;
      const bytes = fs.readFileSync(target);
      fs.renameSync(target, path.join(f.directory, 'original-app'));
      fs.writeFileSync(target, bytes, { flag: 'wx' });
    }
    return ref;
  };
  t.after(() => { F.hashReference = originalHashReference; });
  await assert.rejects(packageBuild(f.configPath, f.out, options(f)), /T04_SOURCE_CHANGED/u);
  assert.equal(replaced, true);
  assert.equal(f.calls.length, 0);
  assert.equal(fs.existsSync(completion(f)), false);
});

test('host source rejects a hard-linked app member and empty native role', async (t) => {
  for (const fault of ['hard-link', 'empty-installer']) await t.test(fault, async (t) => {
    const f = hostFixture(t);
    if (fault === 'hard-link') {
      fs.linkSync(path.join(f.config.sourceDirectory, 'app', 'pinvou3-tauri.exe'),
        path.join(f.config.sourceDirectory, 'app', 'linked.exe'));
    } else fs.writeFileSync(path.join(f.config.sourceDirectory, 'installer.exe'), '');
    await assert.rejects(packageBuild(f.configPath, f.out, options(f)));
    assert.equal(f.calls.length, 0);
    assert.equal(fs.existsSync(completion(f)), false);
  });
});

test('host source enforces the exact aggregate input boundary', async (t) => {
  for (const delta of [0, -1]) await t.test(`limit ${delta}`, async (t) => {
    const f = hostFixture(t);
    const materials = ['claims', 'sbom', 'provenance', 'root'].reduce((sum, key) => sum + f.config[key].size, 0);
    const names = ['installer.exe', 'pinvou-upgrade-helper.exe', 'pinvou-upgrade-launcher.exe',
      'app/nested/empty.txt', 'app/pinvou3-tauri.exe'];
    const bytes = names.reduce((sum, name) => sum + fs.statSync(path.join(f.config.sourceDirectory, ...name.split('/'))).size, 0);
    f.config.limits.TotalInputBytes = materials + bytes + delta;
    fs.writeFileSync(f.configPath, JSON.stringify(f.config));
    if (delta === 0) {
      await packageBuild(f.configPath, f.out, options(f));
      assert.equal(fs.existsSync(completion(f)), true);
    } else {
      await assert.rejects(packageBuild(f.configPath, f.out, options(f)), /T04_CAPACITY/u);
      assert.equal(f.calls.length, 0);
      assert.equal(fs.existsSync(completion(f)), false);
    }
  });
});

test('host source profile rejects ambiguous version and extra inputs', (t) => {
  const common = hostFixture(t).config;
  assert.doesNotThrow(() => configuration(common));
  for (const altered of [{ ...common, version: 3 }, { ...common, inputs: [] }, { ...common, sourceDirectory: 'relative' }]) {
    assert.throws(() => configuration(altered));
  }
});

test('post-build orchestrator completes four fixed stages and preserves qualification boundary', async () => {
  const f = fixture(); await packageBuild(f.configPath, f.out, options(f));
  assert.deepEqual(f.calls.map((c) => c.args[0]), ['prepare', '--input', 'assemble', 'verify']);
  const receipt = JSON.parse(fs.readFileSync(completion(f)));
  assert.notEqual(receipt.outputs['FullPack.zip'].sha256, receipt.outputs['UploadPack.zip'].sha256);
  assert.equal(receipt.outputs['provenance.jsonl'].sha256, f.config.provenance.sha256);
  assert.equal(receipt.offlineRoot.sha256, f.config.root.sha256);
  assert.equal(receipt.dsseAuthenticated, false); assert.equal(receipt.currentQualificationChecked, false);
  assert.equal(receipt.nativeInstallationTested, false);
  assert.equal(fs.statSync(completion(f)).nlink, 2);
});
test('signer failure, changed original input and changed same-byte snapshot cannot publish', async (t) => {
  for (const fault of ['signer-failed', 'original-drift', 'snapshot-rebind']) await t.test(fault, async () => {
    const f = fixture();
    await assert.rejects(packageBuild(f.configPath, f.out, options(f, async (exe, args, opts) => {
      const result = await f.execute(exe, args, opts);
      if (args[0] === '--input') {
        if (fault === 'signer-failed') return 1;
        const target = fault === 'original-drift' ? f.config.inputs[1].file.path : path.join(f.out, 'native-1.bin');
        if (fault === 'original-drift') fs.appendFileSync(target, 'changed');
        else { const raw = fs.readFileSync(target); fs.renameSync(target, `${target}.retained`); fs.writeFileSync(target, raw, { flag: 'wx' }); }
      }
      return result;
    })));
    assert.equal(fs.existsSync(completion(f)), false);
    assert.equal(f.calls.some((c) => c.args[0] === 'assemble'), false);
  });
});
test('closed configuration rejects extra credentials, missing role, unknown role and aliases', () => {
  for (const change of [(c) => { c.token = 'not-a-real-secret'; },
    (c) => { c.inputs.pop(); c.inputs[2].role = 'helper'; },
    (c) => { c.inputs[0].role = 'app'; }, (c) => { c.PackTool = c.packTool; },
    (c) => { c.packTool.sha256 = [c.packTool.sha256]; },
    (c) => { c.limits.Native.Resources.providerToken = 'test'; }]) {
    const f = fixture(); change(f.config); assert.throws(() => configuration(f.config));
  }
});
test('duplicate raw configuration keys fail before an output directory or child exists', async () => {
  const f = fixture();
  const raw = fs.readFileSync(f.configPath, 'utf8').replace('"version":1', '"version":1,"version":1');
  fs.writeFileSync(f.configPath, raw);
  await assert.rejects(packageBuild(f.configPath, f.out, options(f)));
  assert.equal(fs.existsSync(f.out), false); assert.equal(f.calls.length, 0);
});
test('DSSE material controls do not accept JSON-only fixture or wrong installer/source', async () => {
  const { parseJson } = await import('../../specs/upgrade/v1/reference/canonical-json.mjs');
  const f = fixture();
  assert.doesNotThrow(() => provenanceMaterial(Buffer.from(JSON.stringify(f.envelope)), f.claimsBody, parseJson));
  assert.throws(() => provenanceMaterial(Buffer.from('{"isolated-format-fixture":true}'), f.claimsBody, parseJson));
  for (const field of ['installer', 'source']) {
    const changed = structuredClone(f.claimsBody);
    if (field === 'installer') changed.finalInstaller.sha256 = 'f'.repeat(64);
    else changed.provenance.sourceRevision.value = 'b'.repeat(40);
    assert.throws(() => provenanceMaterial(Buffer.from(JSON.stringify(f.envelope)), changed, parseJson));
  }
});
test('streaming accepts many short reads, rejects oversize before open and real hardlinks', () => {
  const f = fixture(); const ref = f.config.inputs[0].file;
  const read = fs.readSync; let reads = 0;
  try {
    fs.readSync = (fd, buffer, offset, length, position) => { reads++; return read(fd, buffer, offset, Math.min(length, 1), position); };
    F.inspect(ref, ref.size); assert.ok(reads > ref.size);
  } finally { fs.readSync = read; }
  const open = fs.openSync; let opened = false;
  try {
    fs.openSync = (...args) => { opened = true; return open(...args); };
    assert.throws(() => F.inspect(ref, ref.size - 1)); assert.equal(opened, false);
  } finally { fs.openSync = open; }
  fs.linkSync(ref.path, path.join(f.directory, 'retained-link'));
  assert.throws(() => F.inspect(ref, ref.size), /T04_FILE_TYPE/u);
});
test('streaming detects real growth and completion collision/Sync failure stays incomplete', () => {
  const f = fixture(); const ref = f.config.inputs[0].file;
  const read = fs.readSync; let first = true;
  try {
    fs.readSync = (...args) => { const n = read(...args); if (first) { first = false; fs.appendFileSync(ref.path, 'growth'); } return n; };
    assert.throws(() => F.inspect(ref, ref.size), /T04_GROWTH/u);
  } finally { fs.readSync = read; }
  for (const fault of ['collision', 'sync']) {
    const directory = path.join(f.directory, fault); fs.mkdirSync(directory);
    const identity = F.directoryIdentity(F.ordinary(directory, true));
    const filename = path.join(directory, 'completion.json');
    if (fault === 'collision') fs.writeFileSync(filename, 'retained original');
    const sync = fs.fsyncSync;
    try {
      if (fault === 'sync') fs.fsyncSync = () => { throw new Error('controlled Sync failure'); };
      assert.throws(() => F.publishCompletion(directory, Buffer.from('new'), identity));
    } finally { fs.fsyncSync = sync; }
    if (fault === 'collision') assert.equal(fs.readFileSync(filename, 'utf8'), 'retained original');
    else assert.equal(fs.existsSync(filename), false);
  }
});
test('controlled premature EOF, hash mismatch and close failure cannot publish', () => {
  const f = fixture(); const ref = f.config.inputs[0].file;
  assert.throws(() => F.inspect({ ...ref, sha256: '0'.repeat(64) }, ref.size), /T04_HASH/u);
  const read = fs.readSync;
  try { fs.readSync = () => 0; assert.throws(() => F.inspect(ref, ref.size), /T04_READ/u); }
  finally { fs.readSync = read; }
  const directory = path.join(f.directory, 'controlled-close-failure'); fs.mkdirSync(directory);
  const identity = F.directoryIdentity(F.ordinary(directory, true));
  const close = fs.closeSync; let attempts = 0;
  try {
    // Close the real dummy fd, then inject a reported close error. This is a
    // controlled error path, not a claim of an actual OS handle-close failure.
    fs.closeSync = (fd) => { attempts++; close(fd); throw new Error('controlled close failure'); };
    assert.throws(() => F.publishCompletion(directory, Buffer.from('new'), identity));
  } finally { fs.closeSync = close; }
  assert.equal(attempts, 1); assert.equal(fs.existsSync(path.join(directory, 'completion.json')), false);
});
test('assemble/verify refusal and cancellation after signing cannot be hidden by later stages', async (t) => {
  for (const fault of ['assemble', 'verify', 'cancel-after-sign']) await t.test(fault, async () => {
    const f = fixture(); const controller = new AbortController();
    await assert.rejects(packageBuild(f.configPath, f.out, { ...options(f, async (exe, args, opts) => {
      if (args[0] === fault) return 1;
      const code = await f.execute(exe, args, opts);
      if (args[0] === '--input' && fault === 'cancel-after-sign') controller.abort();
      return code;
    }), signal: controller.signal }));
    assert.equal(fs.existsSync(completion(f)), false);
    if (fault === 'cancel-after-sign') assert.equal(f.calls.length, 2);
  });
});
test('closed child environment excludes provider variables and Node injection', () => {
  const env = childEnvironment('private-temp', { SystemRoot: 'C:\\Windows', NODE_OPTIONS: '--require unwanted',
    NODE_PATH: 'unwanted', TOKEN: 'test-value', PATH: 'unwanted', UPGRADE_TEST_DATABASE: 'unused' });
  assert.deepEqual(Object.keys(env).sort(), ['PATH', 'SystemRoot', 'TEMP', 'TMP']);
});
test('actual direct child nonzero and cancellation never become a successful stage', async () => {
  const cwd = fs.mkdtempSync(path.join(os.tmpdir(), 'pinvou-t04-child-control-'));
  assert.equal(await runChild(process.execPath, ['-e', 'process.exit(3)'], { cwd, env: {} }), 3);
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), 100);
  try { await assert.rejects(runChild(process.execPath, ['-e', 'setInterval(()=>{},1000)'], { cwd, env: {}, signal: controller.signal }), /T04_CANCELLED/u); }
  finally { clearTimeout(timer); }
});
test('pre-aborted operation and unsupported platform create no output', async () => {
  const f = fixture(); const controller = new AbortController(); controller.abort();
  await assert.rejects(packageBuild(f.configPath, f.out, { ...options(f), signal: controller.signal }), /T04_CANCELLED/u);
  assert.equal(f.calls.length, 0); assert.equal(fs.existsSync(f.out), false);
  await assert.rejects(main(['--config', f.configPath, '--out', f.out], { platform: 'linux' }), /T04_ARGUMENTS/u);
});
