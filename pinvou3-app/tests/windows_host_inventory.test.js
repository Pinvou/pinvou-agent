const assert = require("node:assert/strict");
const test = require("node:test");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { canonicalBytes, parseCanonical, sha256 } = require("../scripts/tauri/windows-host-profile.js");
const { buildAppInventory, MAXIMUM_FILES, checkedSum } = require("../scripts/tauri/windows-host-inventory.js");
const { writeNativeInventory } = require("../scripts/tauri/windows-host-native-inventory.js");
const profile = { seed: { profileId: "inventory-test", buildId: "b".repeat(64) }, seedSha256: "a".repeat(64) };
const files = (count, length = 0) => Array.from({ length: count }, (_, index) => ({
  relativeName: `resource-${String(index).padStart(5,"0")}${"x".repeat(length)}`, size: index % 2, sha256: "c".repeat(64),
}));
function temporary(t) {
  const root=fs.mkdtempSync(path.join(os.tmpdir(),"pinvou-inventory-test-"));
  t.after(()=>{assert.equal(path.dirname(root),path.resolve(os.tmpdir()));assert.match(path.basename(root),/^pinvou-inventory-test-/u);fs.rmSync(root,{recursive:true});});
  return root;
}

test("signed pages preserve exact original bytes, safe totals, count boundaries and stable greedy order", async () => {
  for (const count of [1,4096,4097,MAXIMUM_FILES]) {
    const input=files(count), result=await buildAppInventory(profile,input);
    assert.equal(result.shards.length,Math.ceil(count/4096));
    assert.equal(result.appInventory.fileCount,count);
    assert.equal(result.appInventory.totalSize,Math.floor(count/2));
    let position=0;
    for(const [index,shard] of result.shards.entries()) {
      const value=await parseCanonical(shard.bytes);
      assert.deepEqual(JSON.parse(JSON.stringify(value)),{hostInventoryShardVersion:1,profileId:profile.seed.profileId,
        buildId:profile.seed.buildId,seedSha256:profile.seedSha256,index,files:input.slice(position,position+shard.files.length)});
      assert.deepEqual(result.appInventory.shards[index],{index,resourceId:34102+index,
        size:shard.bytes.length,sha256:sha256(shard.bytes),fileCount:shard.files.length});
      assert.ok(shard.bytes.length<=1048576);position+=shard.files.length;
    }
    assert.equal(position,count);
    const again=await buildAppInventory(profile,input);
    assert.deepEqual(again.appInventory,result.appInventory);
    assert.deepEqual(again.shards.map(s=>s.bytes),result.shards.map(s=>s.bytes));
  }
  await assert.rejects(buildAppInventory(profile,files(MAXIMUM_FILES+1)),/CAPACITY/u);
});

test("signed pages split at complete-wrapper byte limit and refuse a required ninth page", async () => {
  const input=files(2200,900), result=await buildAppInventory(profile,input);
  assert.equal(result.shards.length,3);
  for(let i=0;i<result.shards.length-1;i++) {
    const current=await parseCanonical(result.shards[i].bytes), next=result.shards[i+1].files[0];
    assert.ok(current.files.length<4096);
    assert.ok(Buffer.byteLength(JSON.stringify({...current,files:[...current.files,next]}))>1048576);
    await assert.rejects(canonicalBytes({...current,files:[...current.files,next]}),/JSON_INPUT_LIMIT/u);
  }
  const perPage=result.shards[0].files.length;
  const eight=await buildAppInventory(profile,files(perPage*8,900));
  assert.equal(eight.shards.length,8);
  await assert.rejects(buildAppInventory(profile,files(perPage*8+1,900)),/CAPACITY/u);
});

test("inventory rejects unsafe size sums, aliases/order, paths and open file shapes", async () => {
  assert.equal(checkedSum(9007199254740990,1),9007199254740991);
  assert.throws(()=>checkedSum(9007199254740991,1),/CAPACITY/u);
  for(const input of [[],files(1).map(x=>({...x,size:-1})),files(1).map(x=>({...x,size:0.5})),
    files(1).map(x=>({...x,size:9007199254740992})),files(1).map(x=>({...x,extra:true})),
    files(1).map(x=>({...x,relativeName:"../escape"})),[...files(1),...files(1)],files(2).reverse(),
    [{...files(1)[0],size:9007199254740991},{...files(2)[1],size:1}]]) {
    await assert.rejects(buildAppInventory(profile,input));
  }
});

test("native controls are independently bounded and preserve complete cross-block inputs", async (t) => {
  const directory=temporary(t), entries=files(10877).map(x=>({relativeName:x.relativeName,source:path.join(directory,"ordinary-file.txt")}));
  const result=await writeNativeInventory(directory,entries);
  const index=await parseCanonical(fs.readFileSync(result.indexPath));
  assert.deepEqual(JSON.parse(JSON.stringify(index)),{hostNativeInventoryVersion:1,fileCount:10877,blocks:result.blocks});
  const restored=[];
  for(const [position,reference] of result.blocks.entries()) {
    assert.equal(reference.index,position);
    assert.equal(reference.fileName,`native-inventory-${String(position).padStart(3,"0")}.json`);
    const bytes=fs.readFileSync(path.join(directory,reference.fileName)),block=await parseCanonical(bytes);
    assert.ok(bytes.length<=1048576);assert.ok(block.entries.length<=4096);
    assert.equal(reference.size,bytes.length);assert.equal(reference.sha256,sha256(bytes));
    assert.equal(reference.fileCount,block.entries.length);restored.push(...block.entries);
  }
  assert.deepEqual(JSON.parse(JSON.stringify(restored)),entries);
});

test("native source/path and aggregate overflow refuse before emitting any index or block", async (t) => {
  const root=temporary(t), short={relativeName:"ordinary",source:path.join(root,"ordinary")};
  for(const [index,entries] of [
    [{...short,source:path.join(root,"x".repeat(3073))}],
    [{...short,source:"relative-source"}],
    Array.from({length:7000},(_,i)=>({relativeName:`r${String(i).padStart(5,"0")}`,source:path.join(root,"x".repeat(2900))})),
  ].entries()) {
    const directory=path.join(root,String(index));fs.mkdirSync(directory);
    await assert.rejects(writeNativeInventory(directory,entries),/HOST_BUILD_NATIVE_/u);
    assert.deepEqual(fs.readdirSync(directory),[]);
  }
});
