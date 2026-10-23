const assert = require("node:assert/strict");
const test = require("node:test");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { assertHostRoleImage } = require("../scripts/tauri/windows-host-role-pe.js");

// Data-only PE metadata, never executed/loaded/signed or called a real role.
function image({ flags = 0x800, normal = "kernel32.dll", delayed = "shell32.dll" } = {}) {
  const bytes = Buffer.alloc(0x1400), pe = 0x80, optional = pe + 24, section = optional + 240;
  bytes.write("MZ"); bytes.writeUInt32LE(pe, 60); bytes.write("PE\0\0", pe, "ascii");
  bytes.writeUInt16LE(0x8664, pe + 4); bytes.writeUInt16LE(1, pe + 6); bytes.writeUInt16LE(240, pe + 20);
  bytes.writeUInt16LE(0x20b, optional); bytes.writeUInt32LE(0x2000, optional + 56); bytes.writeUInt32LE(0x400, optional + 60);
  bytes.writeUInt32LE(16, optional + 108);
  bytes.writeUInt32LE(0x1000, section + 8); bytes.writeUInt32LE(0x1000, section + 12);
  bytes.writeUInt32LE(0x1000, section + 16); bytes.writeUInt32LE(0x400, section + 20);
  function directory(index, rva, size) { bytes.writeUInt32LE(rva, optional + 112 + index * 8); bytes.writeUInt32LE(size, optional + 116 + index * 8); }
  directory(10, 0x1100, 80); bytes.writeUInt32LE(80, 0x500); bytes.writeUInt16LE(flags, 0x500 + 78);
  if (normal !== null) { directory(1, 0x1200, 40); bytes.writeUInt32LE(0x1300, 0x600 + 12); bytes.write(normal + "\0", 0x700, "ascii"); }
  if (delayed !== null) { directory(13, 0x1400, 64); bytes.writeUInt32LE(1, 0x800); bytes.writeUInt32LE(0x1500, 0x804); bytes.write(delayed + "\0", 0x900, "ascii"); }
  return bytes;
}
function fixture(t, bytes = image()) {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), "pinvou-role-pe-"));
  const file = path.join(directory, "data-only.exe"); fs.writeFileSync(file, bytes);
  t.after(() => { assert.equal(path.dirname(directory), path.resolve(os.tmpdir())); assert.match(path.basename(directory), /^pinvou-role-pe-/u); fs.rmSync(directory, { recursive: true }); });
  return { file, directory };
}
function rejected(t, bytes, code = /HOST_BUILD_ROLE_IMAGE_INVALID/u) { const { file } = fixture(t, bytes); assert.throws(() => assertHostRoleImage(file), code); }

test("bounded data-only AMD64 metadata accepts exact system32 dependency flags", t => {
  const { file } = fixture(t);
  assert.deepEqual(assertHostRoleImage(file), { dependentLoadFlags: 0x800, imports: ["kernel32.dll"], delayImports: ["shell32.dll"] });
  const empty = fixture(t, image({ normal: null, delayed: null }));
  assert.deepEqual(assertHostRoleImage(empty.file).imports, []);
});
test("dependent-load flags must be exactly 0x800 without missing or additional bits", t => {
  for (const flags of [0, 0x200, 0xa00, 0xffff]) rejected(t, image({ flags }), /HOST_BUILD_ROLE_LOADER_UNSAFE/u);
});
test("ordinary and delay imports reject dynamic VC and UCRT families case-insensitively", t => {
  for (const name of ["VCRUNTIME140.dll", "vcruntime140_1d.dll", "MSVCP140_ATOMIC_WAIT.dll", "msvcr120.dll",
    "msvcrt20.dll", "msvcrt.dll", "msvcp_win.dll", "concrt140.dll", "vcomp140.dll", "vcamp140.dll", "vccorlib140.dll",
    "ucrtbase.dll", "ucrtbased.dll", "api-ms-win-crt-runtime-l1-1-0.dll"]) {
    rejected(t, image({ normal: name, delayed: null }), /HOST_BUILD_ROLE_RUNTIME_IMPORT/u);
    rejected(t, image({ normal: null, delayed: name }), /HOST_BUILD_ROLE_RUNTIME_IMPORT/u);
  }
});
test("headers, architecture, optional format and section bounds reject truncation/ambiguity", t => {
  for (const mutate of [
    b => b.fill(0, 0, 2), b => b.writeUInt32LE(0xffffffff, 60), b => b.writeUInt16LE(0x14c, 0x84),
    b => b.writeUInt16LE(0x10b, 0x98), b => b.writeUInt16LE(97, 0x86), b => b.writeUInt16LE(199, 0x94),
    b => b.writeUInt32LE(17, 0x98 + 108), b => b.writeUInt32LE(0xfffffff0, 0x188 + 20),
    b => b.writeUInt32LE(0xfffffff0, 0x188 + 16), b => b.writeUInt32LE(0x200, 0x188 + 12),
    b => b.writeUInt32LE(0x200, 0x188 + 20),
  ]) { const bytes=image(); mutate(bytes); rejected(t, bytes); }
  rejected(t, image().subarray(0, 0x501));
  const overlapping=image(); overlapping.writeUInt16LE(2,0x86); overlapping.copy(overlapping,0x188+40,0x188,0x188+40); rejected(t,overlapping);
});
test("load-config size and RVA cannot turn malformed or missing data into flags", t => {
  for (const mutate of [
    b => b.fill(0, 0x98+112+80, 0x98+112+88), b => b.writeUInt32LE(79,0x98+116+80),
    b => b.writeUInt32LE(79,0x500), b => b.writeUInt32LE(81,0x500),
    b => b.writeUInt32LE(0x1fff,0x98+112+80), b => b.writeUInt32LE(0,0x98+112+80),
  ]) { const bytes=image();mutate(bytes);rejected(t,bytes); }
});
test("ordinary/delay tables require valid descriptors, termination and bounded basenames", t => {
  for (const mutate of [
    b => b.writeUInt32LE(20,0x98+116+8), b => b.writeUInt32LE(32,0x98+116+13*8),
    b => b.writeUInt32LE(0,0x800), b => b.writeUInt32LE(3,0x800),
    b => b.writeUInt32LE(0x1fff,0x60c), b => b.fill(65,0x700,0x800),
    b => b.write("../wrong.dll\0",0x700,"ascii"), b => b.write("C:/wrong.dll\0",0x900,"ascii"),
    b => b.writeUInt32LE(0xfffffff0,0x98+116+8),
  ]) { const bytes=image();mutate(bytes);rejected(t,bytes); }
});
test("import descriptor work is bounded and a missing terminator never becomes empty",t=>{
  const bytes=Buffer.alloc(0x20400);image({normal:null,delayed:null}).copy(bytes);
  bytes.writeUInt32LE(0x21000,0x98+56);bytes.writeUInt32LE(0x20000,0x188+8);bytes.writeUInt32LE(0x20000,0x188+16);
  bytes.writeUInt32LE(0x1600,0x98+112+8);bytes.writeUInt32LE(4098*20,0x98+116+8);
  bytes.write("kernel32.dll\0",0x700,"ascii");
  for(let index=0;index<4097;index++)bytes.writeUInt32LE(0x1300,0xa00+index*20+12);
  rejected(t,bytes);
});
test("overlays do not trigger whole-image reads or a new artifact size cap", t => {
  const { file }=fixture(t);const handle=fs.openSync(file,"a");try{fs.writeSync(handle,Buffer.alloc(1024*1024));}finally{fs.closeSync(handle);}
  const original=fs.readSync, whole=fs.readFileSync;let total=0,maximum=0;
  fs.readSync=(fd,buffer,offset,length,position)=>{total+=length;maximum=Math.max(maximum,length);return original(fd,buffer,offset,length,position);};
  fs.readFileSync=(...args)=>{if(args[0]===file)throw new Error("whole image read forbidden");return whole(...args);};
  try{assert.equal(assertHostRoleImage(file).dependentLoadFlags,0x800);}finally{fs.readSync=original;fs.readFileSync=whole;}
  assert.ok(maximum<=4096);assert.ok(total<65536);
});
test("actual hardlinks refuse and original restored single-link file passes", t => {
  const { file,directory }=fixture(t),alias=path.join(directory,"alias.exe");
  fs.linkSync(file,alias);assert.throws(()=>assertHostRoleImage(file),/HOST_BUILD_ROLE_IMAGE_INVALID/u);
  fs.unlinkSync(alias);assert.equal(assertHostRoleImage(file).dependentLoadFlags,0x800);
});
test("actual file mutation during metadata read refuses", t => {
  const { file }=fixture(t),original=fs.readSync;let changed=false;
  fs.readSync=(...args)=>{const count=original(...args);if(!changed){changed=true;const fd=fs.openSync(file,"a");try{fs.writeSync(fd,Buffer.from([0]));}finally{fs.closeSync(fd);}}return count;};
  try{assert.throws(()=>assertHostRoleImage(file),/HOST_BUILD_ROLE_IMAGE_INVALID/u);}finally{fs.readSync=original;}
});
test("actual pathname replacement during metadata read refuses", t => {
  const {file,directory}=fixture(t),old=path.join(directory,"original.exe"),original=fs.readSync;let replaced=false;
  fs.readSync=(...args)=>{const count=original(...args);if(!replaced){replaced=true;fs.renameSync(file,old);fs.writeFileSync(file,image());}return count;};
  try{assert.throws(()=>assertHostRoleImage(file),/HOST_BUILD_ROLE_IMAGE_INVALID/u);}finally{fs.readSync=original;}
});
test("short native reads are completed without treating a partial structure as valid", t => {
  const {file}=fixture(t),original=fs.readSync;
  fs.readSync=(fd,buffer,offset,length,position)=>original(fd,buffer,offset,Math.min(length,3),position);
  try{assert.equal(assertHostRoleImage(file).dependentLoadFlags,0x800);}finally{fs.readSync=original;}
});
test("profile query refuses unsafe bytes before any attempted process execution", async t => {
  const { file }=fixture(t,image({flags:0})),child=require("node:child_process"),saved=child.execFile;
  const modulePath=require.resolve("../scripts/tauri/windows-host-nsis.js");let executions=0;
  child.execFile=(...args)=>{executions++;args.at(-1)(new Error("negative oracle: execution forbidden"));};
  delete require.cache[modulePath];
  try{const {checkHelperProfile}=require(modulePath);await assert.rejects(checkHelperProfile(file,{}),/HOST_BUILD_ROLE_LOADER_UNSAFE/u);assert.equal(executions,0);}
  finally{child.execFile=saved;delete require.cache[modulePath];}
});

function packageFixture(t) {
  const {directory}=fixture(t),output=path.join(directory,"output");fs.mkdirSync(output);
  const put=(name,bytes)=>{const file=path.join(directory,name);fs.writeFileSync(file,bytes);return file;};
  return {profile:{seed:{hostSeedVersion:1,appVersion:"0.10.13",
    approvedInitialPublisherPolicy:{policyId:"role-model-only",signerCertificateSha256:"c".repeat(64)}}},
    effectiveConfig:{version:"0.10.13"},
    appBinary:put("ordinary-app-input.exe","negative-orchestration-fixture-never-executed"),
    helper:put("helper.exe",image()),launcher:put("launcher.exe",image()),signer:put("sign.ps1","# never executed"),
    makensis:put("makensis.exe","never executed"),tauriRoot:directory,resourceManifest:{files:[]},outputParent:output};
}
const windowsOnly={skip:process.platform!=="win32"};
test("unsafe original roles refuse before packaging directory or tool side effects",windowsOnly,async t=>{
  const plan=packageFixture(t);fs.writeFileSync(plan.helper,image({flags:0}));let tools=0;
  const {packageHostNSIS}=require("../scripts/tauri/windows-host-nsis.js");
  await assert.rejects(packageHostNSIS({...plan,run:async()=>{tools++;},native:async()=>{tools++;}}),/HOST_BUILD_ROLE_LOADER_UNSAFE/u);
  assert.equal(tools,0);assert.deepEqual(fs.readdirSync(plan.outputParent),[]);
});
test("a role changed after preflight refuses before its signer invocation",windowsOnly,async t=>{
  const plan=packageFixture(t);let signs=0,native=0;
  const {packageHostNSIS}=require("../scripts/tauri/windows-host-nsis.js");
  await assert.rejects(packageHostNSIS({...plan,run:async()=>{signs++;},native:async()=>{
    native++;fs.writeFileSync(plan.helper,image({flags:0}));
  }}),/HOST_BUILD_ROLE_LOADER_UNSAFE/u);
  assert.equal(signs,0);assert.equal(native,1);
});
test("a signer that changes role loader metadata is refused before native trust or execution",windowsOnly,async t=>{
  const plan=packageFixture(t);let signs=0,native=0;
  const {packageHostNSIS}=require("../scripts/tauri/windows-host-nsis.js");
  await assert.rejects(packageHostNSIS({...plan,run:async(command,args)=>{
    signs++;assert.equal(args[args.indexOf("-Role")+1],"helper");fs.writeFileSync(args[args.indexOf("-Image")+1],image({flags:0}));
  },native:async()=>{native++;}}),/HOST_BUILD_ROLE_LOADER_UNSAFE/u);
  assert.equal(signs,1);assert.equal(native,1);
});

// These data-only files exercise the real bounded PE parser and copy pipeline.
// No binary/tool is executed; the helper exec port is forbidden explicitly.
test("no-signer copies still reject a launcher changed after original role validation",windowsOnly,async t=>{
  for(const [name,bad,code] of [
    ["dynamic-crt",image({normal:"vcruntime140.dll"}),/HOST_BUILD_ROLE_RUNTIME_IMPORT/u],
    ["invalid-pe",Buffer.from("not a PE; never executed"),/HOST_BUILD_ROLE_IMAGE_INVALID/u],
  ]) await t.test(name,async t=>{
    const plan=packageFixture(t);
    plan.profile.seed.hostSeedVersion=2;
    delete plan.profile.seed.approvedInitialPublisherPolicy;
    plan.profile.seed.codeImagePolicy={policyVersion:1,certificate:"optional"};
    delete plan.signer;
    assert.equal(assertHostRoleImage(plan.launcher).dependentLoadFlags,0x800);
    const child=require("node:child_process"),savedExec=child.execFile;
    const modulePath=require.resolve("../scripts/tauri/windows-host-nsis.js"),savedModule=require.cache[modulePath];
    let helperExecutions=0,toolExecutions=0,nativeCalls=0;
    child.execFile=(...args)=>{helperExecutions++;args.at(-1)(new Error("negative oracle: helper execution forbidden"));};
    delete require.cache[modulePath];
    try {
      const {packageHostNSIS}=require(modulePath);
      await assert.rejects(packageHostNSIS({...plan,
        run:async()=>{toolExecutions++;throw new Error("negative oracle: tool execution forbidden");},
        native:async mode=>{
          nativeCalls++;assert.equal(mode,"Inventory");assert.equal(nativeCalls,1);
          // Deterministic change after the actual source role checks, before
          // hash/copy. A late copy must not inherit the earlier validation.
          fs.writeFileSync(plan.launcher,bad);
        },
      }),code);
      assert.equal(nativeCalls,1);assert.equal(toolExecutions,0);assert.equal(helperExecutions,0);
      const builds=fs.readdirSync(plan.outputParent);assert.equal(builds.length,1);
      const copied=path.join(plan.outputParent,builds[0],"source/pinvou-upgrade-launcher.exe");
      assert.deepEqual(fs.readFileSync(copied),bad);
    } finally {
      child.execFile=savedExec;
      if(savedModule)require.cache[modulePath]=savedModule;else delete require.cache[modulePath];
    }
  });
});
