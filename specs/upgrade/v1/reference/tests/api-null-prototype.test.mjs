import test from 'node:test';
import assert from 'node:assert/strict';
import { assertApiContract, assertApiShape } from '../api-registry.mjs';
import { canonicalize, parseJson } from '../canonical-json.mjs';

const reference = { recordKind:'transaction', recordId:'record-1', revision:1, sha256:'a'.repeat(64) };
const operation = 'execution-review-install';
function response(readSet = [reference, {...reference,recordId:'record-2'}]) {
  return { protocolVersion:1, requestId:'request-1', transactionId:'transaction-1',
    transactionRevision:0, reviewedAt:0, currentReadSet:structuredClone(readSet),
    verifiedObjectSha256:'a'.repeat(64) };
}
test('all three execution-review contracts accept distinct multi-object read sets and retain input identity', () => {
  for(const purpose of ['preinstall','install','activate']) {
    const value = response();
    assertApiShape('execution-review-'+purpose+'-response', value);
    assert.equal(assertApiContract('execution-review-'+purpose,'response',value),value);
  }
});
test('strict null-prototype input remains unchanged after Schema validation', () => {
  const value = parseJson(canonicalize(response()));
  assert.equal(Object.getPrototypeOf(value),null);
  assert.equal(Object.getPrototypeOf(value.currentReadSet[0]),null);
  const before = canonicalize(value);
  assert.equal(assertApiContract(operation,'response',value),value);
  assert.equal(Object.getPrototypeOf(value),null);
  assert.equal(Object.getPrototypeOf(value.currentReadSet[0]),null);
  assert.deepEqual(canonicalize(value),before);
});
test('read-set uniqueness uses complete JSON equality without inventing record identity uniqueness', () => {
  for(const second of [reference,
    {sha256:reference.sha256,revision:1,recordId:reference.recordId,recordKind:reference.recordKind}]) {
    assert.throws(()=>assertApiContract(operation,'response',response([reference,second])),{code:'API_CONTRACT_INVALID'});
  }
  assert.throws(()=>assertApiContract(operation,'response',response([
    {...reference,revision:0},{...reference,revision:-0}
  ])),{code:'API_CONTRACT_INVALID'});
  for(const second of [{...reference,revision:2},{...reference,sha256:'b'.repeat(64)}]) {
    const value=response([reference,second]);
    assert.equal(assertApiContract(operation,'response',value),value);
  }
});
test('prototype-like data is rejected by the closed Schema without pollution or hooks', () => {
  assert.equal(Object.hasOwn(Object.prototype,'pinvouPrototypeProbe'),false);
  for(const key of ['__proto__','constructor','valueOf']) {
    const value=response();
    Object.defineProperty(value.currentReadSet[0],key,{
      value:{pinvouPrototypeProbe:true},enumerable:true,writable:true,configurable:true
    });
    assert.throws(()=>assertApiContract(operation,'response',value),{code:'API_CONTRACT_INVALID'});
    assert.equal(Object.hasOwn(Object.prototype,'pinvouPrototypeProbe'),false);
  }
  let getterCalls=0;
  const value=response();
  Object.defineProperty(value,'extra',{enumerable:true,get(){getterCalls++;return 1;}});
  assert.throws(()=>assertApiContract(operation,'response',value),{code:'JSON_INVALID_VALUE'});
  assert.equal(getterCalls,0);
  let hookCalls=0;
  const withHook=response();withHook.toJSON=()=>{hookCalls++;return {};};
  assert.throws(()=>assertApiContract(operation,'response',withHook),{code:'JSON_INVALID_VALUE'});
  assert.equal(hookCalls,0);
});
test('strict byte, string, member and depth limits still precede the Schema adapter', () => {
  const string=response();string.currentReadSet[0].recordId='x'.repeat(65537);
  assert.throws(()=>assertApiContract(operation,'response',string),{code:'JSON_STRING_LIMIT'});
  const members=response(Array.from({length:4097},(_,index)=>({...reference,recordId:'record-'+index})));
  assert.throws(()=>assertApiContract(operation,'response',members),{code:'JSON_MEMBER_LIMIT'});
  const bytes=response();for(let i=0;i<18;i++)bytes['extra'+i]='x'.repeat(60000);
  assert.throws(()=>assertApiContract(operation,'response',bytes),{code:'JSON_INPUT_LIMIT'});
  const depth=response();let child={};depth.extra=child;
  for(let i=0;i<33;i++){child.next={};child=child.next;}
  assert.throws(()=>assertApiContract(operation,'response',depth),{code:'JSON_DEPTH_LIMIT'});
});
