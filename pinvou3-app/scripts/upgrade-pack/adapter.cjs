// Project post-build composition of the existing offline upgrade-pack tool.
const fs = require('node:fs');
const path = require('node:path');
const { pathToFileURL } = require('node:url');
const F = require('./files.cjs');
const { HOST_PROFILE, validateHostLocations, snapshotHostSource } = require('./host-source.cjs');
const { childEnvironment, requireSuccess, runChild } = require('./process.cjs');
const WINDOWS = 'windows-x86_64-nsis-perMachine';
const METADATA = 1024 * 1024;

function numericObject(value, keys) {
  F.object(value, keys);
  for (const key of keys) F.positive(value[key]);
}
function windowsLimits(value) {
  F.object(value, ['InputBytes', 'TotalInputBytes', 'OutputBytes', 'Archive', 'JSONL', 'Native']);
  for (const key of ['InputBytes', 'TotalInputBytes', 'OutputBytes']) F.positive(value[key]);
  numericObject(value.Archive, ['ArchiveBytes', 'FileBytes', 'ExpandedBytes', 'Entries', 'CompressionRatio', 'Nesting', 'NestedMemoryBytes', 'MetadataBytes']);
  numericObject(value.JSONL, ['Bytes', 'LineBytes', 'Records']);
  F.object(value.Native, ['PE', 'Resources', 'ResourceTree']);
  numericObject(value.Native.PE, ['Bytes', 'OptionalHeaderBytes', 'Sections', 'Certificates']);
  numericObject(value.Native.Resources, ['SourceBytes', 'MemoryBytes']);
  numericObject(value.Native.ResourceTree, ['Entries', 'Depth', 'NameUnits']);
}

async function originalJSON() {
  return import(pathToFileURL(path.resolve(__dirname, '../../..', 'specs/upgrade/v1/reference/canonical-json.mjs')).href);
}
function configuration(value) {
  const common = ['version', 'packTool', 'manifestSigner', 'claims', 'sbom', 'provenance', 'root', 'at', 'limits'];
  if (value?.version === 1) F.object(value, [...common, 'inputs']);
  else if (value?.version === 2) F.object(value, [...common, 'sourceDirectory']);
  else F.refuse('T04_CONFIGURATION');
  if (!Number.isSafeInteger(value.at) || value.at < 0) F.refuse('T04_CONFIGURATION');
  for (const key of ['packTool', 'manifestSigner', 'claims', 'sbom', 'provenance', 'root']) F.reference(value[key]);
  for (const key of ['packTool', 'manifestSigner']) {
    if (!value[key].path.toLowerCase().endsWith('.exe')) F.refuse('T04_EXECUTABLE');
  }
  if (value.version === 2) {
    if (typeof value.sourceDirectory !== 'string' || !path.isAbsolute(value.sourceDirectory)) F.refuse('T04_SOURCE_DIRECTORY');
    windowsLimits(value.limits);
    return value;
  }
  if (!Array.isArray(value.inputs) || value.inputs.length < 4 || value.inputs.length > 4096) F.refuse('T04_INPUTS');
  const names = new Set(); const roles = new Set();
  for (const input of value.inputs) {
    F.object(input, ['role', 'helperId', 'name', 'file']); F.reference(input.file);
    if (!['finalInstaller', 'helper', 'launcher', 'helperArtifact'].includes(input.role)
        || (input.role === 'helperArtifact' ? typeof input.helperId !== 'string' || !input.helperId : input.helperId !== null)
        || typeof input.name !== 'string' || !input.name || names.has(input.name)) F.refuse('T04_INPUTS');
    const role = `${input.role}:${input.helperId || ''}`;
    if (roles.has(role)) F.refuse('T04_INPUTS');
    roles.add(role); names.add(input.name);
  }
  for (const role of ['finalInstaller:', 'helper:', 'launcher:']) if (!roles.has(role)) F.refuse('T04_INPUTS');
  // This Windows adapter accepts only its used existing Go capacity structures.
  // No credentials/unknown fields can be serialized into generated config.
  windowsLimits(value.limits);
  return value;
}
function base64(value, exactLength) {
  if (typeof value !== 'string' || value.length === 0) F.refuse('T04_DSSE_MATERIAL');
  const bytes = Buffer.from(value, 'base64');
  if (bytes.toString('base64') !== value || (exactLength !== undefined && bytes.length !== exactLength)) F.refuse('T04_DSSE_MATERIAL');
  return bytes;
}
function provenanceMaterial(bytes, claims, parseJson) {
  // One signed envelope, structurally consistent with the existing T10 profile.
  // This does NOT authenticate the key, DSSE signature or current policy.
  const envelope = parseJson(bytes, METADATA);
  F.object(envelope, ['payloadType', 'payload', 'signatures']);
  if (envelope.payloadType !== 'application/vnd.in-toto+json'
      || !Array.isArray(envelope.signatures) || envelope.signatures.length !== 1) F.refuse('T04_DSSE_MATERIAL');
  F.object(envelope.signatures[0], ['keyid', 'sig']);
  if (typeof envelope.signatures[0].keyid !== 'string' || !envelope.signatures[0].keyid) F.refuse('T04_DSSE_MATERIAL');
  base64(envelope.signatures[0].sig, 64);
  const statement = parseJson(base64(envelope.payload), METADATA);
  if (statement?._type !== 'https://in-toto.io/Statement/v1'
      || statement.predicateType !== 'https://slsa.dev/provenance/v1'
      || !Array.isArray(statement.subject) || statement.subject.length !== 1
      || statement.subject[0]?.digest?.sha256 !== claims.finalInstaller?.sha256) F.refuse('T04_DSSE_BINDING');
  const dependencies = statement.predicate?.buildDefinition?.resolvedDependencies;
  if (!Array.isArray(dependencies) || dependencies.length !== 1
      || dependencies[0]?.digest?.gitCommit !== claims.provenance?.sourceRevision?.value) F.refuse('T04_DSSE_BINDING');
}
async function packageBuild(configPath, outputDirectory, { appVersion, signal, execute = runChild } = {}) {
  F.checkSignal(signal);
  if (typeof appVersion !== 'string' || !/^\d+\.\d+\.\d+$/u.test(appVersion)
      || !path.isAbsolute(configPath) || !path.isAbsolute(outputDirectory)) F.refuse('T04_ARGUMENTS');
  const { parseJson, canonicalize } = await originalJSON();
  const originalConfig = F.hashReference(configPath, METADATA, signal);
  const originalConfigIdentity = F.inspect(originalConfig, METADATA, { signal }).identity;
  const config = configuration(parseJson(F.readBounded(configPath, METADATA, signal)));
  F.inspect(originalConfig, METADATA, { signal });
  let total = 0;
  const materials = [config.claims, config.sbom, config.provenance, config.root,
    ...(config.version === 1 ? config.inputs.map((i) => i.file) : [])];
  for (const ref of materials) {
    if (ref.size > config.limits.InputBytes || ref.size > config.limits.TotalInputBytes - total) F.refuse('T04_CAPACITY');
    total += ref.size;
  }
  const claims = parseJson(F.readBounded(config.claims.path, METADATA, signal));
  if (claims?.scope?.targetKey !== WINDOWS || claims.appVersion !== appVersion
      || !Array.isArray(claims.incrementalPackages) || claims.incrementalPackages.length !== 0) F.refuse('T04_CLAIMS');
  F.inspect(config.claims, METADATA, { signal });
  provenanceMaterial(F.readBounded(config.provenance.path, Math.min(METADATA, config.limits.InputBytes), signal), claims, parseJson);
  F.inspect(config.provenance, Math.min(METADATA, config.limits.InputBytes), { signal });
  if (config.version === 2) validateHostLocations(config.sourceDirectory, outputDirectory);
  F.ordinary(path.dirname(outputDirectory), true);
  fs.mkdirSync(outputDirectory, { mode: 0o700 }); // New leaf only; never recursive/overwrite.
  const directory = F.directoryIdentity(F.ordinary(outputDirectory, true));
  const temp = path.join(outputDirectory, 'child-temp'); fs.mkdirSync(temp, { mode: 0o700 });
  const sources = []; const snapshots = [];
  function remember(ref, allowEmpty = false) {
    snapshots.push({ ...F.inspect(ref, Math.max(1, ref.size), { signal, allowEmpty }), allowEmpty });
    return ref;
  }
  function snapshot(ref, name, maximum, allowEmpty = false, expectedIdentity) {
    F.sameDirectory(outputDirectory, directory);
    const destination = path.join(outputDirectory, name);
    const source = F.inspect(ref, maximum, { destination, signal, allowEmpty });
    if (expectedIdentity !== undefined && source.identity !== expectedIdentity) F.refuse('T04_SOURCE_CHANGED');
    sources.push({ ...source, allowEmpty });
    remember({ ...ref, path: destination }, allowEmpty);
    return name;
  }
  const local = {
    claims: snapshot(config.claims, 'claims.json', METADATA),
    sbom: snapshot(config.sbom, 'sbom.json', config.limits.InputBytes),
    provenance: snapshot(config.provenance, 'provenance.jsonl', Math.min(METADATA, config.limits.InputBytes)),
    inputs: config.version === 1 ? config.inputs.map((input, i) => ({ role: input.role, helperId: input.helperId || '', name: input.name,
      path: snapshot(input.file, `native-${i}.bin`, config.limits.InputBytes) })) : [],
    limits: config.limits,
  };
  let hostSource;
  if (config.version === 2) {
    hostSource = snapshotHostSource(config.sourceDirectory, outputDirectory, config.limits, snapshot, signal, total);
    local.sourceProfile = HOST_PROFILE;
    local.sourceDirectory = 'host-source';
  }
  const rootPath = path.join(outputDirectory, snapshot(config.root, 'root.json', METADATA));
  const packPath = path.join(outputDirectory, snapshot(config.packTool, 'upgrade-pack.exe', config.packTool.size));
  const signerPath = path.join(outputDirectory, snapshot(config.manifestSigner, 'manifest-signer.exe', config.manifestSigner.size));
  const inputPath = path.join(outputDirectory, 'pack-input.json');
  F.writeExclusive(inputPath, Buffer.from(canonicalize(local)));
  remember(F.hashReference(inputPath, METADATA, signal));
  function recheck() {
    F.checkSignal(signal); F.sameDirectory(outputDirectory, directory);
    if (F.inspect(originalConfig, METADATA, { signal }).identity !== originalConfigIdentity) F.refuse('T04_SOURCE_CHANGED');
    for (const source of [...sources, ...snapshots]) {
      const ref = { path: source.path, size: source.size, sha256: source.sha256 };
      const current = F.inspect(ref, Math.max(1, source.size), { signal, allowEmpty: source.allowEmpty });
      if (current.identity !== source.identity) F.refuse('T04_SOURCE_CHANGED');
    }
    hostSource?.recheckDirectories();
  }
  const options = { cwd: outputDirectory, env: childEnvironment(temp), signal };
  async function stage(executable, args) {
    recheck();
    await requireSuccess(executable, args, options, execute);
    recheck();
  }
  const prepared = path.join(outputDirectory, 'prepared');
  const assembled = path.join(outputDirectory, 'assembled');
  const manifest = path.join(outputDirectory, 'signed-package.json');
  await stage(packPath, ['prepare', '-config', inputPath, '-out', prepared]);
  const preparedRefs = {};
  for (const name of ['unsigned-package.json', 'signing-input.bin', 'FullPack.zip']) {
    preparedRefs[name] = remember(F.hashReference(path.join(prepared, name), name.endsWith('.zip') ? config.limits.OutputBytes : METADATA, signal));
  }
  if (fs.existsSync(manifest)) F.refuse('T04_OUTPUT_EXISTS');
  await stage(signerPath, ['--input', path.join(prepared, 'signing-input.bin'),
    '--claims', path.join(prepared, 'unsigned-package.json'), '--output', manifest]);
  const manifestRef = remember(F.hashReference(manifest, METADATA, signal));
  const verifyArgs = ['-config', inputPath, '-manifest', manifest, '-root', rootPath,
    '-root-sha256', config.root.sha256, '-at', String(config.at)];
  await stage(packPath, ['assemble', ...verifyArgs, '-out', assembled]);
  await stage(packPath, ['verify', ...verifyArgs, '-upload', path.join(assembled, 'UploadPack.zip')]);
  const outputs = {};
  for (const name of ['FullPack.zip', 'UploadPack.zip', 'UpdatePackInfo.json', 'unsigned-package.json', 'signing-input.bin', 'sbom.json', 'provenance.jsonl']) {
    const maximum = name.endsWith('.zip') ? config.limits.OutputBytes : (name === 'sbom.json' ? config.limits.InputBytes : METADATA);
    const ref = F.hashReference(path.join(assembled, name), maximum, signal);
    remember(ref);
    outputs[name] = { size: ref.size, sha256: ref.sha256 };
  }
  for (const name of Object.keys(preparedRefs)) {
    if (outputs[name].sha256 !== preparedRefs[name].sha256 || outputs[name].size !== preparedRefs[name].size) F.refuse('T04_OUTPUT_BINDING');
  }
  if (outputs['UpdatePackInfo.json'].sha256 !== manifestRef.sha256 || outputs['UpdatePackInfo.json'].size !== manifestRef.size
      || outputs['sbom.json'].sha256 !== config.sbom.sha256
      || outputs['sbom.json'].size !== config.sbom.size
      || outputs['provenance.jsonl'].sha256 !== config.provenance.sha256
      || outputs['provenance.jsonl'].size !== config.provenance.size) F.refuse('T04_OUTPUT_BINDING');
  recheck();
  const receipt = { version: config.version, targetKey: WINDOWS, appVersion, config: originalConfig,
    packTool: config.packTool, manifestSigner: config.manifestSigner, offlineRoot: config.root,
    offlineVerificationAt: config.at, inputMaterials: materials, outputs,
    offlineManifestVerifiedByPackTool: true, dsseAuthenticated: false,
    currentQualificationChecked: false, nativeInstallationTested: false };
  if (hostSource) receipt.hostSource = { profile: HOST_PROFILE, fileCount: hostSource.fileCount,
    totalBytes: hostSource.totalBytes, membersSha256: hostSource.membersSha256 };
  F.publishCompletion(outputDirectory, Buffer.from(canonicalize(receipt)), directory);
  // No fallible operation after completion publication.
}
module.exports = { WINDOWS, windowsLimits, configuration, provenanceMaterial, packageBuild };
