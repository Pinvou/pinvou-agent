// Path orchestration only. Profile verification and packaging are isolated
// dependencies here; these tests claim no signing, PE or installer acceptance.
const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const vm = require("node:vm");

function fixture() {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "pinvou-host-build-target-"));
  const appRoot = path.join(root, "app");
  fs.mkdirSync(path.join(appRoot, "src-tauri"), { recursive: true });
  fs.writeFileSync(path.join(appRoot, "src-tauri/tauri.conf.json"), '{"version":"1.0.0"}');
  const environment = {};
  for (const [key, name] of Object.entries({ PINVOU_HOST_HELPER_BINARY: "helper.exe",
    PINVOU_HOST_LAUNCHER_BINARY: "launcher.exe", PINVOU_HOST_SIGNER_SCRIPT: "signer.ps1",
    PINVOU_HOST_MAKENSIS: "makensis.exe" })) {
    environment[key] = path.join(root, name);
    fs.writeFileSync(environment[key], "fixture-only-never-executed");
  }
  const calls = [];
  const module = { exports: {} };
  const source = path.resolve(__dirname, "../scripts/tauri/windows-host-build.js");
  vm.runInNewContext(fs.readFileSync(source, "utf8"), { module, process,
    console: { log() {} }, require(name) {
      if (name === "./windows-host-profile.js") return {
        loadBuildProfile: async () => ({ seed: { profileKind: "lab", profileId: "path-test-only", appVersion: "1.0.0" } }),
        // Opaque dependency for path tests; real policy validation lives in packaging tests.
        imageCertificatePolicy: () => ({ kind: "legacy-selected-signer" }),
        refuse() { throw new Error("HOST_BUILD_INPUT_INVALID"); },
      };
      if (name === "./windows-host-nsis.js") return {
        // Opaque numeric-version port; the real validator is covered separately.
        nsisFileVersion: value => { assert.equal(value, "1.0.0"); return "1.0.0.0"; },
        packageHostNSIS: async options => { calls.push(options); return { pathTestOnly: true }; },
      };
      if (name.startsWith("node:")) return require(name);
      throw new Error("UNEXPECTED_TEST_DEPENDENCY");
    },
  }, { filename: source });
  return { root, appRoot, environment, calls, ...module.exports,
    async prepare() { return this.prepareHostBuild(["build", "--bundles", "nsis"], {
      appRoot, environment, platform: "win32", architecture: "x64" }); },
    async finish(plan) { return this.finishHostBuild(plan, { resourceManifest: {}, effectiveConfig: {} },
      { appRoot, environment }); },
    close() {
      const actual = fs.realpathSync(root);
      assert.equal(path.dirname(actual), fs.realpathSync(os.tmpdir()));
      assert.ok(path.basename(actual).startsWith("pinvou-host-build-target-"));
      fs.rmSync(actual, { recursive: true, force: true });
    },
  };
}

test("default target keeps the original host packaging directory", async () => {
  const f = fixture();
  try {
    const plan = await f.prepare();
    assert.equal(plan.targetRoot, path.join(f.appRoot, "src-tauri/target"));
    await f.finish(plan);
    assert.equal(f.calls[0].outputParent, path.join(plan.targetRoot, "windows-host-packaging"));
    assert.ok(fs.statSync(f.calls[0].outputParent).isDirectory());
  } finally { f.close(); }
});

test("absolute Cargo target places packaging beside its actual application output", async () => {
  const f = fixture();
  try {
    f.environment.CARGO_TARGET_DIR = path.join(f.root, "absolute-cargo-target");
    const plan = await f.prepare();
    await f.finish(plan);
    assert.equal(f.calls[0].outputParent, path.join(f.environment.CARGO_TARGET_DIR, "windows-host-packaging"));
    assert.equal(plan.appBinary, path.join(plan.targetRoot, "x86_64-pc-windows-msvc/release/pinvou3-tauri.exe"));
    assert.equal(fs.existsSync(path.join(f.appRoot, "src-tauri/target/windows-host-packaging")), false);
  } finally { f.close(); }
});

test("relative Cargo target keeps preparation's existing src-tauri resolution", async () => {
  const f = fixture();
  try {
    f.environment.CARGO_TARGET_DIR = "../separate-cargo-target";
    const plan = await f.prepare();
    await f.finish(plan);
    assert.equal(f.calls[0].outputParent, path.join(f.appRoot, "separate-cargo-target/windows-host-packaging"));
  } finally { f.close(); }
});

test("later environment drift cannot relocate a prepared packaging operation", async () => {
  const f = fixture();
  try {
    const original = path.join(f.root, "original-target");
    const changed = path.join(f.root, "changed-target");
    f.environment.CARGO_TARGET_DIR = original;
    const plan = await f.prepare();
    f.environment.CARGO_TARGET_DIR = changed;
    await f.finish(plan);
    assert.equal(f.calls[0].outputParent, path.join(original, "windows-host-packaging"));
    assert.equal(fs.existsSync(changed), false);
  } finally { f.close(); }
});

test("retained application input outside target does not select the packaging root", async () => {
  const f = fixture();
  try {
    f.environment.CARGO_TARGET_DIR = path.join(f.root, "cargo-target");
    const plan = await f.prepare();
    const retained = path.join(f.root, "retained-app.exe");
    fs.writeFileSync(retained, "fixture-only-never-executed");
    await f.finish({ ...plan, appBinary: retained });
    assert.equal(f.calls[0].appBinary, retained);
    assert.equal(f.calls[0].outputParent, path.join(plan.targetRoot, "windows-host-packaging"));
  } finally { f.close(); }
});

for (const [label, value] of [["missing", undefined], ["relative", "target"]]) {
  test(`${label} prepared target root refuses before directory or packaging effects`, async () => {
    const f = fixture();
    try {
      const plan = await f.prepare();
      await assert.rejects(f.finish({ ...plan, targetRoot: value }), /HOST_BUILD_INPUT_INVALID/);
      assert.equal(f.calls.length, 0);
      assert.equal(fs.existsSync(path.join(f.appRoot, "src-tauri/target")), false);
    } finally { f.close(); }
  });
}
