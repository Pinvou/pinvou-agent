import { pathToFileURL } from 'node:url';
const source=pathToFileURL(process.argv[1]);
const { envelopeReference, assertReference }=await import(source);
const { parseJson, canonicalize }=await import(new URL('./canonical-json.mjs',source));
const { validateSchema }=await import(new URL('./schema-registry.mjs',source));
const { checkReferenceRoles }=await import(new URL('./semantics.mjs',source));
function checked(reference) {
  // Exercise the original common reference Schema through its real use site.
  // No checkSemantics or cryptographic verification of this shape-only wrapper.
  validateSchema('metadata/timestamp',{signed:{protocolVersion:1,role:'timestamp',
    product:'pinvou',component:'app',scope:{},version:1,issuedAt:0,expiresAt:0,snapshot:reference},
    signatures:[{keyId:'0'.repeat(64),signature:'A'.repeat(86)}]});
  checkReferenceRoles(reference);
  return reference;
}
const chunks=[];for await(const chunk of process.stdin)chunks.push(chunk);
const results=JSON.parse(Buffer.concat(chunks).toString('utf8')).map((sample)=>{
  try {
    const bytes=Buffer.from(sample.input,'base64');
    if(sample.mode==='shape')return Buffer.from(canonicalize(checked(parseJson(bytes)))).toString('base64');
    const actual=checked(envelopeReference(bytes));
    if(sample.mode==='derive')return Buffer.from(canonicalize(actual)).toString('base64');
    const expected=checked(parseJson(Buffer.from(sample.expected,'base64')));
    try { assertReference(expected,bytes); return true; }
    catch(error) { if(error.code==='REFERENCE_MISMATCH')return false;throw error; }
  } catch { return null; }
});
process.stdout.write(JSON.stringify(results));
