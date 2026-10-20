// Public ephemeral QA fixtures; no private key or credential is persisted.
import {pathToFileURL} from 'node:url';
import {createHash} from 'node:crypto';
process.on('uncaughtException', error => {
  process.stderr.write('QA_FIXTURE_FAILED ' + error.name + '\n' + (error.stack ?? '').split('\n').filter(line => line.startsWith('    at file:')).join('\n') + '\n');
  process.exitCode = 1;
});
const text = value => Buffer.from(value).toString('utf8');
const base = pathToFileURL(process.argv[2]);
const {createFixtureSet, NOW} = await import(new URL('./tests/fixtures.mjs', base));
const {canonicalize} = await import(new URL('./canonical-json.mjs', base));
const {envelopeReference} = await import(new URL('./signatures.mjs', base));
const f = createFixtureSet();
const payload = Buffer.from(process.argv[3] ?? 'abc', 'utf8');
const identity = {size: payload.length, sha256: createHash('sha256').update(payload).digest('hex')};
f.metadata.package.fullPackage = {packageId: 'package-2', ...identity};
const packageBytes = text(f.sign(f.metadata.package));
for (const role of ['decision', 'download']) {
  const c = f.claims[role];
  for (const kind of ['endpointChain', 'hopChain']) {
    c.update[kind].package = identity;
    c.update[kind].packageManifest = envelopeReference(Buffer.from(packageBytes));
  }
}
f.claims.download.package = identity;
const scope = {product:'pinvou',component:'app',installId:'install-1',...f.claims.decision.scope};
const request = {protocolVersion:1,scope,decision:JSON.parse(text(f.sign(f.claims.decision))),downloadKey:'download-key-1',packageId:'package-2'};
const response = {protocolVersion:1,requestId:'qa-request',credential:JSON.parse(text(f.sign(f.claims.download))),packageId:'package-2',...identity,
  url:process.argv[4] ?? 'https://127.0.0.1:1/file',expiresAt:f.claims.download.exp,fileSourceClass:'isolated-qa'};
const root = text(canonicalize(f.metadata.root));
function alternate(kind) {
  const decision = structuredClone(f.claims.decision), download = structuredClone(f.claims.download);
  for (const c of [decision, download]) {
    c.iat += 1000; c.nbf += 1000; c.exp += 1000;
    c.telemetrySessionId = 'session-2'; c.decisionId = 'decision-2'; c.decisionRevision = 2; c.jti = 'future-' + c.role;
    if (kind === 'workflow') c.update.workflowId = 'workflow-2';
    if (kind === 'stage') c.update.originalStage.firstQualifiedAt += 1;
  }
  return {request:text(canonicalize({...request,downloadKey:'download-key-2',decision:JSON.parse(text(f.sign(decision)))})),
    response:text(canonicalize({...response,credential:JSON.parse(text(f.sign(download))),expiresAt:download.exp})),suppliedTimeMillis:NOW+1000};
}
const result = {root,rootSha256:createHash('sha256').update(root).digest('hex'),package:packageBytes,
  expected:text(canonicalize({role:'package',product:'pinvou',component:'app',scope:{targetKey:'linux-x86_64-deb-perMachine'}})),
  imported:{request:text(canonicalize(request)),response:text(canonicalize(response)),suppliedTimeMillis:NOW},payload:payload.toString('hex'),
  sessionImport:alternate('session'),workflowImport:alternate('workflow'),stageImport:alternate('stage')};
process.stdout.write(JSON.stringify(result));
