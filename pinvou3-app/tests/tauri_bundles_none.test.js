// `--bundles none`: the wrapper-owned "compile without bundling" request.
//
// Background: tauri-bundler cannot bundle loongarch64 — its arch mapping
// (`binary_arch()`) has no LoongArch variant and panics on the
// loongarch64-unknown-linux-gnu triple (verified in the tauri-bundler source
// for tauri-bundler 2.9.4, the version inside the pinned
// @tauri-apps/cli 2.11.4). The loong64 release pipeline
// (scripts/ci-build-linux-loongarch64.sh) therefore runs the full build.js
// pipeline with `--bundles none` and assembles the deb itself via
// scripts/package-deb-loongarch64.sh.
//
// The CLI itself rejects the value at arg-parse time ("error: invalid value
// 'none'"; possible values on Linux are deb/rpm/appimage) — its own
// equivalent is `--no-bundle`. These tests pin the translation contract:
// `--bundles none` reaches the CLI as `--no-bundle` (and nothing else
// changes), real bundle types are untouched, and a `none` mixed with real
// types is never silently absorbed.
const assert = require("node:assert/strict");
const test = require("node:test");

const {
  extractBundlesNone,
  prepareTauriArgs,
} = require("../scripts/tauri/build.js");

const LINUX_X64 = { platform: "linux", architecture: "x64", stageRuntime: () => null };

const LOONG64_BUILD = [
  "build",
  "--target",
  "loongarch64-unknown-linux-gnu",
  "--bundles",
  "none",
];

test("bundles none on the loongarch64 target compiles without the bundler", () => {
  const prepared = prepareTauriArgs(LOONG64_BUILD, LINUX_X64);
  // The CLI's own skip flag is present: the compiled binary is left at the
  // cargo target dir (target/<triple>/release/) instead of a bundle.
  assert.ok(prepared.includes("--no-bundle"), `missing --no-bundle: ${prepared}`);
  // The wrapper-owned spelling never reaches the CLI: the pinned
  // @tauri-apps/cli 2.11.4 rejects the value "none" at arg-parse time.
  assert.ok(!prepared.includes("--bundles"), `--bundles leaked: ${prepared}`);
  assert.ok(!prepared.includes("none"), `the "none" value leaked: ${prepared}`);
  // The full pipeline is unaffected: the same overlays a bundling build gets
  // (platform + loong64 architecture + startup window) are still injected.
  const configs = [];
  for (let i = 0; i < prepared.length; i += 1) {
    if (prepared[i] === "--config" && prepared[i + 1]) configs.push(prepared[i + 1]);
  }
  assert.ok(
    configs.some((spec) => spec.includes("loongarch64")),
    `the loong64 architecture overlay must still be injected: ${configs}`,
  );
});

test("--no-bundle is spliced before the value-taking --config pairs", () => {
  // `--no-bundle` is flag-like and position-independent for the CLI, but it
  // must never land between a `--config` and its value.
  const prepared = prepareTauriArgs(LOONG64_BUILD, LINUX_X64);
  const firstConfig = prepared.indexOf("--config");
  assert.ok(firstConfig > 0, "expected injected --config overlays");
  assert.ok(
    prepared.indexOf("--no-bundle") < firstConfig,
    `--no-bundle must precede the first --config pair: ${prepared}`,
  );
});

test("all accepted spellings request no bundling", () => {
  for (const request of [
    ["--bundles", "none"],
    ["-b", "none"],
    ["--bundles=none"],
    ["-b=none"],
  ]) {
    const prepared = prepareTauriArgs(["build", ...request], LINUX_X64);
    assert.ok(
      prepared.includes("--no-bundle"),
      `${request} must translate to --no-bundle: ${prepared}`,
    );
    assert.ok(!prepared.includes("--bundles"), `${request} leaked: ${prepared}`);
    assert.ok(!prepared.includes("none"), `${request} value leaked: ${prepared}`);
  }
});

test("extractBundlesNone keeps unrelated args and order intact", () => {
  const { args, noBundle } = extractBundlesNone([
    "build",
    "--target",
    "loongarch64-unknown-linux-gnu",
    "--bundles",
    "none",
    "--",
    "--frozen",
  ]);
  assert.equal(noBundle, true);
  assert.deepEqual(args, ["build", "--target", "loongarch64-unknown-linux-gnu", "--", "--frozen"]);
});

test("a none mixed with real bundle types is not silently absorbed", () => {
  // `--bundles none,deb` stays in the args and fails at the CLI's value
  // validation, exactly like any other unknown bundle type — absorbing it
  // would silently downgrade a typo to a no-bundle build.
  const { args, noBundle } = extractBundlesNone(["build", "--bundles", "none,deb"]);
  assert.equal(noBundle, false);
  assert.deepEqual(args, ["build", "--bundles", "none,deb"]);
});

test("real bundle types are untouched (x86_64 deb build unchanged)", () => {
  const prepared = prepareTauriArgs(
    ["build", "--target", "x86_64-unknown-linux-gnu", "--bundles", "deb"],
    LINUX_X64,
  );
  assert.ok(!prepared.includes("--no-bundle"), `no-bundle leaked: ${prepared}`);
  const bundlesIndex = prepared.indexOf("--bundles");
  assert.notEqual(bundlesIndex, -1, "--bundles must survive");
  assert.equal(prepared[bundlesIndex + 1], "deb");
});

test("real bundle types are untouched (macOS dmg build unchanged)", () => {
  const prepared = prepareTauriArgs(
    ["build", "--target", "universal-apple-darwin", "--bundles", "dmg"],
    { platform: "darwin", architecture: "arm64", stageRuntime: () => null },
  );
  assert.ok(!prepared.includes("--no-bundle"), `no-bundle leaked: ${prepared}`);
  const bundlesIndex = prepared.indexOf("--bundles");
  assert.notEqual(bundlesIndex, -1, "--bundles must survive");
  assert.equal(prepared[bundlesIndex + 1], "dmg");
});

test("no bundling is meaningful on Windows too, and does not stage installers", () => {
  // stageWindowsInstaller returns null unless "nsis" is requested, so the
  // wrapper-owned spelling must not be treated as a Windows bundle type.
  const prepared = prepareTauriArgs(
    ["build", "--bundles", "none"],
    { platform: "win32", architecture: "x64", stageRuntime: () => null },
  );
  assert.ok(prepared.includes("--no-bundle"), `missing --no-bundle: ${prepared}`);
  assert.ok(!prepared.includes("--bundles"), `--bundles leaked: ${prepared}`);
});

test("dev absorbs the request without injecting the build-only flag", () => {
  // `tauri dev` never bundles, so stripping IS the semantics; injecting
  // --no-bundle would fail the CLI's arg parse for the dev command.
  const prepared = prepareTauriArgs(["dev", "--bundles", "none"], LINUX_X64);
  assert.ok(!prepared.includes("--no-bundle"), `--no-bundle leaked into dev: ${prepared}`);
  assert.ok(!prepared.includes("--bundles"), `--bundles leaked into dev: ${prepared}`);
  assert.ok(!prepared.includes("none"), `the "none" value leaked into dev: ${prepared}`);
});

test("repeated none requests are absorbed as one", () => {
  const { args, noBundle } = extractBundlesNone(["build", "-b", "none", "--bundles=none"]);
  assert.equal(noBundle, true);
  assert.deepEqual(args, ["build"]);
  const prepared = prepareTauriArgs(
    ["build", "--bundles", "none", "--bundles=none"],
    LINUX_X64,
  );
  assert.equal(prepared.filter((arg) => arg === "--no-bundle").length, 1);
});
