// Fresh private keys stay in this process; returned fixtures contain public evidence only.
import { readFileSync } from 'node:fs';
import { generateKeyPairSync } from 'node:crypto';
import { pathToFileURL } from 'node:url';
const base = pathToFileURL(process.argv[1]);
const { canonicalize } = await import(new URL('./canonical-json.mjs',base));
const { publicKeyDescriptor, signEnvelope, verifyRootSuccessor } = await import(new URL('./signatures.mjs',base));
const inventory = JSON.parse(readFileSync(new URL('../vectors/crypto/envelopes.json',base)));
const clone = structuredClone;
const rows = [];
const DAY = 86_400_000;
const context = (signed) => ({
  role: signed.role, product: signed.product,
  component: signed.component, scope: clone(signed.scope),
});
function pair() {
  const keys = generateKeyPairSync('ed25519');
  return { ...keys, descriptor: publicKeyDescriptor(keys.publicKey) };
}
function unique(pairs) {
  return [...new Map(pairs.map((item) => [item.descriptor.keyId, item])).values()];
}
const signedBytes = (signed, pairs) => Buffer.from(
  signEnvelope(signed, unique(pairs).map((item) => item.privateKey)),
);
function rotation(overlap = false) {
  const oldPairs = [pair(), pair(), pair()];
  const fresh = [pair(), pair(), pair()];
  const meta = [pair(), pair()];
  const nextPairs = overlap ? [oldPairs[0], fresh[1], fresh[2]] : fresh;
  const old = clone(inventory.trustedRoot);
  const next = clone(old);
  const now = old.issuedAt + 1000;
  for (const [root, pairs] of [[old, oldPairs], [next, nextPairs]]) {
    root.keys = unique([...pairs, ...meta]).map((item) => item.descriptor);
    for (const policy of root.roles) {
      policy.keyIds = (policy.role === 'root' ? pairs : [meta[0]])
        .map((item) => item.descriptor.keyId);
      policy.threshold = policy.role === 'root' ? 2 : 1;
    }
  }
  next.version = old.version + 1;
  next.issuedAt = now;
  next.expiresAt = now + 365 * DAY;
  return { old, next, now, oldPairs, nextPairs, meta,
    signers: unique([...oldPairs.slice(0, 2), ...nextPairs.slice(0, 2)]) };
}
function active(rotationCase, name = 'timestamp', mutate = () => {}) {
  const vector = inventory.vectors.find((item) => item.name === name);
  const signed = JSON.parse(Buffer.from(vector.envelopeBase64Url, 'base64url')).signed;
  if (Object.hasOwn(signed, 'signingKeyId')) {
    signed.signingKeyId = rotationCase.meta[0].descriptor.keyId;
  }
  mutate(signed);
  return { bytes: signedBytes(signed, [rotationCase.meta[0]]), expected: context(signed) };
}
function append(group, name, rotationCase, {
  signed = rotationCase.next, signers = rotationCase.signers,
  bytes = signedBytes(signed, signers), product = rotationCase.next.product,
  now = rotationCase.now, activeEnvelopes = [], activeRepeat = 1,
} = {}, code = null, rustCode = code) {
  // Compress only public fixture output. The actual reference receives every
  // repeated entry; Rust expands the same slice before calling its operation.
  const expanded = Array.from({ length: activeRepeat }, () => activeEnvelopes).flat();
  let actual = null;
  try {
    verifyRootSuccessor(bytes, {
      trustedRoot: rotationCase.old, product, now, activeEnvelopes: expanded,
    });
  } catch (error) {
    if (typeof error.code !== 'string') throw error;
    actual = error.code;
  }
  if (actual !== code) {
    throw new Error(`${group}/${name}: expected ${code}, actual ${actual}`);
  }
  rows.push({
    group, name, trustedRoot: rotationCase.old,
    expected: context({ ...rotationCase.next, product }), now,
    bytes: Buffer.from(bytes).toString('base64url'), activeRepeat,
    activeEnvelopes: activeEnvelopes.map((entry) => ({
      ...entry, bytes: Buffer.from(entry.bytes).toString('base64url'),
    })),
    accepted: actual === null, rustCode,
  });
}

const r=rotation();
append('threshold','distinct-old-and-next-two-of-three',r);
append('threshold','old-only',r,{signers:r.oldPairs.slice(0,2)},'SIGNATURE_THRESHOLD_UNMET');
append('threshold','next-only',r,{signers:r.nextPairs.slice(0,2)},'SIGNATURE_THRESHOLD_UNMET');
append('threshold','one-from-each-is-not-two-per-policy',r,{signers:[r.oldPairs[0],r.nextPairs[0]]},'SIGNATURE_THRESHOLD_UNMET');
append('threshold','all-six-valid',r,{signers:[...r.oldPairs,...r.nextPairs]});
const overlap=rotation(true);
append('threshold','overlap-counts-in-both',overlap);
append('threshold','overlap-single-is-insufficient',overlap,{signers:[overlap.oldPairs[0]]},'SIGNATURE_THRESHOLD_UNMET');
append('threshold','descriptor-present-outside-root-policy',r,{signers:[...r.signers,r.meta[0]]},'SIGNING_KEY_NOT_AUTHORIZED');
const invalid=JSON.parse(signedBytes(r.next,[...r.oldPairs,...r.nextPairs]));invalid.signatures.at(-1).signature=Buffer.alloc(64).toString('base64url');
append('threshold','invalid-last-after-both-thresholds',r,{bytes:Buffer.from(canonicalize(invalid))},'SIGNATURE_INVALID');
const unknown=JSON.parse(signedBytes(r.next,r.signers));unknown.signatures.push({keyId:'f'.repeat(64),signature:Buffer.alloc(64).toString('base64url')});
append('threshold','unknown-last-after-thresholds',r,{bytes:Buffer.from(canonicalize(unknown))},'SIGNING_KEY_NOT_AUTHORIZED');
for(const [name,version]of[['same',1],['backward',1],['gap',3]]){
 const q=rotation();if(name==='backward'){q.old.version=2;q.next.version=1;}else q.next.version=version;
 append('continuity',name,q,{},'ROOT_CONTINUITY_INVALID');
}
const max=rotation();
max.old.version=Number.MAX_SAFE_INTEGER-1;
max.next.version=Number.MAX_SAFE_INTEGER;
append('continuity','last-safe-successor',max);
const past=rotation();
past.old.version=Number.MAX_SAFE_INTEGER;
past.next.version=Number.MAX_SAFE_INTEGER;
append('continuity','no-successor-after-max-safe',past,{},'ROOT_CONTINUITY_INVALID');
append('continuity','independently-wrong-product',r,{product:'another-product'},'CONTEXT_MISMATCH');
const oldProduct=rotation();
oldProduct.old.product='another-product';
append('continuity','old-product-does-not-match-next',oldProduct,{},'ROOT_CONTINUITY_INVALID');
const expired=rotation();
expired.old.expiresAt=expired.now-1;
append('windows','expired-old-is-history',expired);
const future=rotation();
future.old.issuedAt=future.now+1;
future.old.expiresAt=future.old.issuedAt+DAY;
append('windows','future-old-refuses',future,{},'TIME_WINDOW_INVALID');
const over=rotation();
over.old.expiresAt=over.old.issuedAt+366*DAY;
append('windows','old-original-window-still-bounded',over,{},'TIME_WINDOW_INVALID');
const nextExpired=rotation();
nextExpired.next.expiresAt=nextExpired.now;
nextExpired.next.issuedAt=nextExpired.now-1;
append('windows','expired-next-refuses',nextExpired,{},'TIME_WINDOW_CLOSED');
const nextFuture=rotation();
nextFuture.next.issuedAt=nextFuture.now+1;
nextFuture.next.expiresAt=nextFuture.now+DAY;
append('windows','future-next-refuses',nextFuture,{},'TIME_WINDOW_CLOSED');
const nextOver=rotation();
nextOver.next.expiresAt=nextOver.next.issuedAt+366*DAY;
append('windows','next-window-bounded',nextOver,{},'TIME_WINDOW_INVALID');
const nonmono=rotation();
nonmono.next.issuedAt=nonmono.old.issuedAt-1;
nonmono.next.expiresAt=nonmono.now+1;
append('windows','next-times-need-not-increase-over-old',nonmono);
const allActive=['timestamp','snapshot','target','release','package'].map(n=>active(r,n));
append('active','all-five-metadata-roles',r,{activeEnvelopes:allActive});
append('active','explicit-empty-set',r,{activeEnvelopes:[]});
append('active','repeated-entries-are-checked',r,{activeEnvelopes:[allActive[0],allActive[0]]});
append('active','no-aggregate-4096-entry-policy',r,{activeEnvelopes:[allActive[0]],activeRepeat:4097});
const retired=rotation();
retired.next.roles.find(p=>p.role==='timestamp').keyIds=[retired.meta[1].descriptor.keyId];
append('active','retired-active-key-refuses',retired,{activeEnvelopes:[active(retired)]},'SIGNING_KEY_NOT_AUTHORIZED');
const expiredActive=active(r,'timestamp',s=>{s.issuedAt=r.now-100*DAY;s.expiresAt=r.now-99*DAY;});
append('active','expired-active-still-preserves-key',r,{activeEnvelopes:[expiredActive]});
const badWindow=active(r,'timestamp',s=>{s.issuedAt=r.now+DAY;s.expiresAt=r.now;});
append('active','schema-valid-body-window-is-not-current-eligibility',r,{activeEnvelopes:[badWindow]});
append('active','credential-is-not-active-metadata',r,{activeEnvelopes:[active(r,'download')]},'ACTIVE_READ_SET_INVALID');
const activeRoot={bytes:signedBytes(r.next,r.signers),expected:context(r.next)};
append('active','root-is-not-active-component-metadata',r,{activeEnvelopes:[activeRoot]},'ACTIVE_READ_SET_INVALID');
const badLast={...allActive[0],bytes:Buffer.from(allActive[0].bytes)};
const badLastEnv=JSON.parse(badLast.bytes);
badLastEnv.signatures[0].signature=Buffer.alloc(64).toString('base64url');
badLast.bytes=Buffer.from(canonicalize(badLastEnv));
append('active','invalid-last-after-valid-read-set-prefix',r,{activeEnvelopes:[...allActive,badLast]},'SIGNATURE_INVALID');
const wrongScope={...allActive[0],expected:{...allActive[0].expected,scope:{...allActive[0].expected.scope,channelRevision:2}}};
append('active','independent-complete-active-scope',r,{activeEnvelopes:[wrongScope]},'CONTEXT_MISMATCH');
const incremental = active(r, 'package', (signed) => {
  signed.incrementalPackages = [{
    packageId: 'delta-2', size: 1, sha256: 'a'.repeat(64),
    baseVersion: '1.0.0', baseArtifactSha256: 'b'.repeat(64),
    resultArtifactSize: 123, resultArtifactSha256: 'a'.repeat(64),
    algorithm: 'test-delta', algorithmVersion: '1', format: 'test-delta',
  }];
});
append('active','active-package-does-not-run-production-body-semantics',r,{activeEnvelopes:[incremental]});
const extraComponent=rotation();
const second=active(extraComponent,'timestamp',s=>{s.component='second-component';});
const policy=clone(extraComponent.next.roles.find(p=>p.role==='timestamp'));policy.component='second-component';
extraComponent.next.roles.push(policy);
append('active','multiple-independent-components',extraComponent,{activeEnvelopes:[active(extraComponent),second]});
const unboundSecond={...second,expected:{...second.expected,component:'wrong-component'}};
append('active','bad-final-component-is-not-ignored',extraComponent,{activeEnvelopes:[active(extraComponent),unboundSecond]},'CONTEXT_MISMATCH');
const canonical=signedBytes(r.next,r.signers);
append('input','root-over-one-mib',r,{bytes:Buffer.concat([canonical,Buffer.alloc(1048577-canonical.length,32)])},'JSON_INPUT_LIMIT');
append('input','root-whitespace-noncanonical',r,{bytes:Buffer.concat([canonical,Buffer.from(' ')])},'ENVELOPE_NOT_CANONICAL');
append('input','root-duplicate-property',r,{bytes:Buffer.from(`{"signed":null,${canonical.toString().slice(1)}`)},'JSON_DUPLICATE_KEY');
for(const [name,value,code] of [
 ['string','a'.repeat(65537),'JSON_STRING_LIMIT'],['array',Array(4097).fill(0),'JSON_MEMBER_LIMIT'],
 ['object',Object.fromEntries(Array.from({length:4097},(_,i)=>[`k${i}`,0])),'JSON_MEMBER_LIMIT'],
 ['depth',Array.from({length:33}).reduce(v=>[v],0),'JSON_DEPTH_LIMIT']]){
 const e=JSON.parse(canonical);e.signed.unrecognized=value;
 append('input',`root-${name}-limit`,r,{bytes:Buffer.from(JSON.stringify(e))},code);
}
const schema=JSON.parse(canonical);
schema.signed.unrecognized=true;
append('input','root-closed-full-schema',r,{bytes:Buffer.from(canonicalize(schema))},'SCHEMA_INVALID','ENVELOPE_SCHEMA_INVALID');
const dup=JSON.parse(canonical);
dup.signatures.push(clone(dup.signatures[0]));
append('input','duplicate-signatures',r,{bytes:Buffer.from(canonicalize(dup))},'SIGNATURE_ORDER_INVALID');
const rev=JSON.parse(canonical);
rev.signatures.reverse();
append('input','unsorted-signatures',r,{bytes:Buffer.from(canonicalize(rev))},'SIGNATURE_ORDER_INVALID');
const largeActive={...allActive[0],bytes:Buffer.concat([allActive[0].bytes,Buffer.alloc(1048577-allActive[0].bytes.length,32)])};
append('input','active-metadata-retains-one-mib-profile',r,{activeEnvelopes:[largeActive]},'JSON_INPUT_LIMIT');
const cred=active(r,'download');
cred.bytes=Buffer.concat([cred.bytes,Buffer.alloc(65537-cred.bytes.length,32)]);
append('input','invalid-active-credential-retains-64k-profile',r,{activeEnvelopes:[cred]},'JSON_INPUT_LIMIT');

// These pass the closed JSON Schema but fail Root descriptor/policy consistency.
const badDescriptor = rotation();
badDescriptor.next.keys[0].publicKey = badDescriptor.meta[0].descriptor.publicKey;
append('structure', 'descriptor-id-must-match-actual-key', badDescriptor, {},
  'KEY_ID_INVALID', 'ROOT_DESCRIPTOR_INVALID');
const duplicatePolicy = rotation();
duplicatePolicy.next.roles.push(clone(duplicatePolicy.next.roles[0]));
append('structure', 'duplicate-root-role-policy', duplicatePolicy, {},
  'ROLE_POLICY_DUPLICATE', 'ROOT_POLICY_INVALID');
const missingKey = rotation();
missingKey.next.roles[0].keyIds[0] = 'f'.repeat(64);
append('structure', 'policy-key-must-be-declared', missingKey, {},
  'ROLE_THRESHOLD_INVALID', 'ROOT_POLICY_INVALID');
const weakThreshold = rotation();
weakThreshold.next.roles[0].threshold = 1;
append('structure', 'root-threshold-remains-at-least-two', weakThreshold, {},
  'ROOT_THRESHOLD_INVALID', 'ROOT_POLICY_INVALID');
const missingPolicy = rotation();
missingPolicy.next.roles = missingPolicy.next.roles.filter((policy) => policy.role !== 'root');
append('structure', 'exact-root-policy-required', missingPolicy, {},
  'ROOT_POLICY_MISSING', 'ROOT_POLICY_INVALID');
console.log(JSON.stringify(rows));
