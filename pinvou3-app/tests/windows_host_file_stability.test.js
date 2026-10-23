const test=require('node:test');
const assert=require('node:assert/strict');
const fs=require('node:fs'),os=require('node:os'),path=require('node:path');
const {createHash}=require('node:crypto');
const {hashFile}=require('../scripts/tauri/windows-host-manifest.js');

async function fixture(bytes,run){
  const root=fs.mkdtempSync(path.join(os.tmpdir(),'pinvou-file-stability-test-'));
  const file=path.join(root,'ordinary.txt');fs.writeFileSync(file,bytes);
  try{await run({root,file,bytes});}
  finally{
    assert.equal(path.dirname(path.resolve(root)),path.resolve(os.tmpdir()));
    assert.ok(path.basename(root).startsWith('pinvou-file-stability-test-'));
    fs.rmSync(root,{recursive:true});
  }
}
async function observeHandle(configure,run){
  const original=fs.promises.open;
  fs.promises.open=async function(...args){const handle=await original.apply(this,args);configure(handle);return handle;};
  try{await run();}finally{fs.promises.open=original;}
}
async function rejected(operation){
  try{await operation();}catch(error){return error;}
  assert.fail('Unstable file was accepted.');
}
function closedCause(error,file){
  assert.equal(error.message,'HOST_BUILD_INPUT_INVALID');
  const cause=error.cause;
  assert.deepEqual(Object.keys(cause).sort(),['delta','end','readSize','reason','start']);
  assert.equal(cause.reason,'HOST_BUILD_FILE_UNSTABLE');
  for(const name of ['start','end']){
    assert.deepEqual(Object.keys(cause[name]).sort(),['ctimeMs','mtimeMs','nlink','size']);
    assert.ok(Object.values(cause[name]).every(Number.isFinite));
    assert.ok(Object.isFrozen(cause[name]));
  }
  assert.deepEqual(Object.keys(cause.delta).sort(),['ctimeMs','mtimeMs','nlink','readSize','size']);
  assert.ok(Object.values(cause.delta).every(Number.isFinite));
  assert.equal(typeof cause.readSize,'number');
  assert.ok(Object.isFrozen(cause)&&Object.isFrozen(cause.delta));
  assert.equal(JSON.stringify(cause).includes(file),false);
  return cause;
}
for(const [label,bytes] of [['empty',Buffer.alloc(0)],['355-byte',Buffer.alloc(355,18)],['multiple-chunk',Buffer.alloc(200000,27)]]){
  test(`${label} stable file keeps its exact normal hash without diagnostics`,()=>fixture(bytes,async({file})=>{
    assert.deepEqual(await hashFile(file),{size:bytes.length,sha256:createHash('sha256').update(bytes).digest('hex')});
  }));
}
for(const field of ['size','mtimeMs','ctimeMs','nlink']){
  test(`end-read ${field} instability retains refusal and reports the exact numeric delta`,()=>fixture(Buffer.alloc(355,33),async({file})=>{
    let calls=0;
    await observeHandle(handle=>{
      const native=handle.stat.bind(handle);
      handle.stat=async(...args)=>{
        const value=await native(...args);calls++;
        if(calls===2)return Object.assign(Object.create(Object.getPrototypeOf(value)),value,{[field]:value[field]+17});
        return value;
      };
    },async()=>{
      const error=await rejected(()=>hashFile(file));const cause=closedCause(error,file);
      assert.equal(cause.delta[field],17);assert.equal(cause.readSize,355);assert.equal(cause.delta.readSize,0);
      for(const other of ['size','mtimeMs','ctimeMs','nlink'].filter(key=>key!==field))assert.equal(cause.delta[other],0);
    });
    assert.equal(calls,2);
  }));
}
test('actual truncation during streaming reports the short read and end size without accepting it',()=>fixture(Buffer.alloc(200000,45),async({file})=>{
  await observeHandle(handle=>{
    const native=handle.createReadStream.bind(handle);
    handle.createReadStream=(...args)=>{const stream=native(...args);stream.once('data',()=>fs.truncateSync(file,0));return stream;};
  },async()=>{
    const error=await rejected(()=>hashFile(file));const cause=closedCause(error,file);
    assert.equal(cause.start.size,200000);assert.equal(cause.end.size,0);
    assert.ok(cause.readSize>0&&cause.readSize<200000);assert.ok(cause.delta.readSize<0);
    assert.equal(cause.delta.size,-200000);
  });
}));
test('actual hard link added during streaming remains refused and is visible in the cause',()=>fixture(Buffer.alloc(200000,51),async({root,file})=>{
  await observeHandle(handle=>{
    const native=handle.createReadStream.bind(handle);
    handle.createReadStream=(...args)=>{const stream=native(...args);stream.once('data',()=>fs.linkSync(file,path.join(root,'alias.txt')));return stream;};
  },async()=>{
    const error=await rejected(()=>hashFile(file));const cause=closedCause(error,file);
    assert.equal(cause.start.nlink,1);assert.equal(cause.end.nlink,2);assert.equal(cause.delta.nlink,1);
  });
}));
test('existing hard links retain the original early error and do not gain an end-read cause',()=>fixture(Buffer.from('fixture'),async({root,file})=>{
  fs.linkSync(file,path.join(root,'alias.txt'));
  const error=await rejected(()=>hashFile(file));assert.equal(error.message,'HOST_BUILD_FILE_INVALID');assert.equal(error.cause,undefined);
}));
