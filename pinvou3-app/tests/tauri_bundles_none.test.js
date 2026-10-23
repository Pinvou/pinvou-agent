// `--bundles none`: the wrapper-owned "compile without bundling" request.
//
// Background: tauri-bundler cannot bundle loongarch64 — its arch mapping
// (`binary_arch()`) has no LoongArch variant and panics on the
// loongarch64-unknown-linux-gnu triple (verified in the tauri-bundler source
// for tauri-bundler 2.10.4, the version inside the pinned
// @tauri-apps/cli 2.12.4). The loong64 release pipeline
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
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");

const {
  extractBundlesNone,
  prepareTauriArgs,
} = require("../scripts/tauri/build.js");

const BUILD_SCRIPT_SOURCE = fs.readFileSync(
  path.join(__dirname, "..", "scripts", "tauri", "build.js"),
  "utf8",
);

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
  // @tauri-apps/cli 2.12.4 rejects the value "none" at arg-parse time.
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

test("bundle subcommand rejects the request loudly instead of mis-splicing", () => {
  // `tauri bundle` has no --no-bundle; re-emitting it there would fail the
  // CLI's arg parse, and silently dropping the absorbed request would run a
  // FULL bundle instead of the requested skip. The wrapper surfaces its own
  // error with the reason (and `dev` keeps its absorb-only semantics —
  // covered by the test above — so this pin is what makes the gate
  // bundle-specific rather than build-or-nothing).
  assert.throws(
    () => prepareTauriArgs(
      ["bundle", "--target", "loongarch64-unknown-linux-gnu", "--bundles", "none"],
      LINUX_X64,
    ),
    /--bundles none is a wrapper-owned build request/u,
  );
});

test("the request may precede the subcommand (flag-first order)", () => {
  // The absorbed `--bundles none` tokens shift every index after them, so
  // the subcommand lookup must run on the filtered array: flag-first
  // orders used to read the wrong token and silently drop the request
  // (`build`) or bypass the bundle guard (`bundle`).
  const prepared = prepareTauriArgs(
    ["--bundles", "none", "build", "--target", "loongarch64-unknown-linux-gnu"],
    LINUX_X64,
  );
  assert.ok(prepared.includes("--no-bundle"), `missing --no-bundle: ${prepared}`);
  assert.ok(!prepared.includes("--bundles"), `--bundles leaked: ${prepared}`);
  assert.ok(!prepared.includes("none"), `the "none" value leaked: ${prepared}`);

  const equalsForm = prepareTauriArgs(
    ["--bundles=none", "build", "--target", "loongarch64-unknown-linux-gnu"],
    LINUX_X64,
  );
  assert.ok(equalsForm.includes("--no-bundle"), `missing --no-bundle: ${equalsForm}`);
});

test("bundle rejects the request in flag-first order too", () => {
  assert.throws(
    () => prepareTauriArgs(
      ["--bundles", "none", "bundle", "--target", "loongarch64-unknown-linux-gnu"],
      LINUX_X64,
    ),
    /--bundles none is a wrapper-owned build request/u,
  );
});

test("a real bundle type next to an absorbed none fails loudly", () => {
  // After absorption the leftover spelling would sit next to the injected
  // --no-bundle, where the CLI silently lets --no-bundle win — the same
  // silent degradation the `none,deb` value mix above refuses. Every
  // accepted spelling of the second request must fail here instead.
  for (const second of [
    ["--bundles", "deb"],
    ["-b", "deb"],
    ["--bundles=deb"],
    ["-b=deb"],
  ]) {
    assert.throws(
      () => prepareTauriArgs(
        ["build", "--target", "loongarch64-unknown-linux-gnu", "--bundles", "none", ...second],
        LINUX_X64,
      ),
      /conflicting --bundles requests/u,
      `subcommand-first with ${second} must fail loudly`,
    );
    assert.throws(
      () => prepareTauriArgs(
        ["--bundles", "none", ...second, "build", "--target", "loongarch64-unknown-linux-gnu"],
        LINUX_X64,
      ),
      /conflicting --bundles requests/u,
      `flag-first with ${second} must fail loudly`,
    );
  }
});

test("a space-separated bundle type following the absorbed none fails loudly", () => {
  // The CLI's `--bundles` is multi-value (`Vec<BundleFormat>` with
  // num_args(0..)), so `--bundles none deb` is ONE option with two values.
  // Absorbing only `none` would leave `deb` as a bare runner positional that
  // fails deep inside cargo with an "unexpected argument" error that never
  // mentions --bundles; the wrapper names the conflict itself, with the same
  // message as a second --bundles spelling.
  for (const bundleType of ["deb", "rpm", "appimage"]) {
    assert.throws(
      () => prepareTauriArgs(
        ["build", "--target", "loongarch64-unknown-linux-gnu", "--bundles", "none", bundleType],
        LINUX_X64,
      ),
      /conflicting --bundles requests/u,
      `subcommand-first with a bare ${bundleType} must fail loudly`,
    );
    assert.throws(
      () => prepareTauriArgs(
        ["--bundles", "none", bundleType, "build", "--target", "loongarch64-unknown-linux-gnu"],
        LINUX_X64,
      ),
      /conflicting --bundles requests/u,
      `flag-first with a bare ${bundleType} must fail loudly`,
    );
  }
  // The wrapper runs on every host while the CLI filters its possible values
  // per host platform, so the recognized set covers all platforms' words.
  for (const bundleType of ["msi", "nsis", "dmg", "app", "ios", "updater"]) {
    assert.throws(
      () => prepareTauriArgs(["build", "--bundles", "none", bundleType], LINUX_X64),
      /conflicting --bundles requests/u,
      `a bare ${bundleType} must fail loudly`,
    );
  }
});

test("a bundle type following the equals-form absorbed none fails loudly", () => {
  // Same conflict as the space form: `--bundles=none deb` absorbs the `none`
  // request and leaves `deb` as a bare runner positional (the CLI's usage is
  // `tauri build [OPTIONS] [ARGS]...`, so the bare word passes arg-parse and
  // only dies deep in the build without naming --bundles). Both spellings and
  // both flag orders must be named loudly.
  for (const spelling of ["--bundles=none", "-b=none"]) {
    assert.throws(
      () => prepareTauriArgs(
        ["build", "--target", "loongarch64-unknown-linux-gnu", spelling, "deb"],
        LINUX_X64,
      ),
      /conflicting --bundles requests/u,
      `${spelling} followed by deb must fail loudly (subcommand-first)`,
    );
    assert.throws(
      () => prepareTauriArgs(
        [spelling, "deb", "build", "--target", "loongarch64-unknown-linux-gnu"],
        LINUX_X64,
      ),
      /conflicting --bundles requests/u,
      `${spelling} followed by deb must fail loudly (flag-first)`,
    );
  }
  // A `=`-form `none` with no following bundle-type word stays valid.
  const prepared = prepareTauriArgs(["build", "--bundles=none"], LINUX_X64);
  assert.ok(prepared.includes("--no-bundle"), `missing --no-bundle: ${prepared}`);
});

test("a non-bundle token after the absorbed none is not a conflict", () => {
  // The follower check must fire only on the CLI's bundle-type words: the
  // `--` runner-argument passthrough and flag values keep working.
  const prepared = prepareTauriArgs(
    ["build", "--bundles", "none", "--", "--frozen"],
    LINUX_X64,
  );
  assert.ok(prepared.includes("--no-bundle"), `missing --no-bundle: ${prepared}`);
  assert.deepEqual(prepared.slice(-2), ["--", "--frozen"]);
});

test("dev keeps absorb-only semantics with a bare bundle type after none", () => {
  // dev never bundles and gets no --no-bundle, so the contradiction the build
  // path rejects cannot arise there either; the follower passes through
  // untouched, exactly like a second --bundles spelling on dev (pinned above).
  const prepared = prepareTauriArgs(["dev", "--bundles", "none", "deb"], LINUX_X64);
  assert.ok(!prepared.includes("--no-bundle"), `--no-bundle leaked into dev: ${prepared}`);
  assert.ok(!prepared.includes("--bundles"), `--bundles leaked into dev: ${prepared}`);
  assert.ok(!prepared.includes("none"), `the "none" value leaked into dev: ${prepared}`);
  assert.ok(prepared.includes("deb"), `the follower must pass through on dev: ${prepared}`);
});

test("dev keeps its absorb-only semantics even with a second bundles spelling", () => {
  // dev never bundles and gets no --no-bundle, so the contradiction the
  // build path rejects cannot arise there; the pass-through spelling stays
  // with the CLI exactly as before this gate existed.
  const prepared = prepareTauriArgs(
    ["dev", "--bundles", "none", "--bundles", "deb"],
    LINUX_X64,
  );
  assert.ok(!prepared.includes("--no-bundle"), `--no-bundle leaked into dev: ${prepared}`);
  const bundlesIndex = prepared.indexOf("--bundles");
  assert.notEqual(bundlesIndex, -1, "the pass-through --bundles must survive on dev");
  assert.equal(prepared[bundlesIndex + 1], "deb");
});

test("repeated none spellings alone stay contradictory-free", () => {
  const prepared = prepareTauriArgs(
    ["--bundles", "none", "build", "--bundles=none", "--target", "loongarch64-unknown-linux-gnu"],
    LINUX_X64,
  );
  assert.equal(prepared.filter((arg) => arg === "--no-bundle").length, 1);
});

test("main() validates the request before any preparation work runs", () => {
  // The never-succeedable shapes are knowable from argv alone, while the
  // sidecar preparation takes minutes: pin the fail-fast call ordering so a
  // refactor cannot silently move the validation back behind the prep steps.
  const mainBody = BUILD_SCRIPT_SOURCE.match(/\nasync function main\(\) \{\n([\s\S]*?)\n\}\n/u);
  assert.ok(mainBody, "build.js must keep its main() entry point");
  const planAt = mainBody[1].indexOf("bundlesNonePlan(args)");
  assert.ok(planAt >= 0, "main() must run bundlesNonePlan(args) for fail-fast validation");
  for (const step of ["prepareLinuxAsrRuntime(", "prepareCodexBridge(", "prepareKnowledgeHost("]) {
    const stepAt = mainBody[1].indexOf(step);
    assert.ok(
      stepAt >= 0 && planAt < stepAt,
      `bundlesNonePlan must run before ${step} so an impossible request fails fast`,
    );
  }
});
