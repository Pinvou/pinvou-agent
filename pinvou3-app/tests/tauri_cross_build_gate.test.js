// Cross-build consistency gate and the two "drop the resource declaration"
// helpers.
//
// Root cause of everything pinned here: some preparation steps only produce
// host-architecture artifacts. In a cross build they either produce the wrong
// architecture or nothing at all, and neither case fails the build - the
// package is silently wrong until a user hits the feature on a device. So this
// file pins four things: the gate blocks, `main()` really uses the gate and the
// skip switches, the helpers remove what they should, and nothing else.
//
// (Why the helpers replace the whole overlay instead of layering null is
// argued once, in the `configWithoutResources` doc comment in build.js.)
const assert = require("node:assert/strict");
const test = require("node:test");

const {
  architectureConfigWithoutAsr,
  configWithoutResources,
  crossBuildViolations,
  main,
  platformConfigWithoutKnowledgeHost,
  targetArchitecture,
} = require("../scripts/tauri/build.js");

const cross = (over = {}) => ({
  platform: "linux",
  targetArch: "arm64",
  hostArch: "x64",
  skipLinuxAsr: false,
  skipKnowledgeHost: false,
  ...over,
});

test("cross build with neither switch set reports both violations", () => {
  assert.equal(crossBuildViolations(cross()).length, 2);
});

test("cross build that skips ASR but not the knowledge host is still rejected", () => {
  const violations = crossBuildViolations(cross({ skipLinuxAsr: true }));
  assert.equal(violations.length, 1);
  assert.ok(
    violations[0].includes("PINVOU3_SKIP_KNOWLEDGE_HOST"),
    `the message must name the switch to set: ${violations[0]}`,
  );
});

test("cross build passes only with both switches set", () => {
  assert.deepEqual(
    crossBuildViolations(cross({ skipLinuxAsr: true, skipKnowledgeHost: true })),
    [],
  );
});

test("non-cross builds are never blocked", () => {
  // Same architecture: both steps are correct natively; blocking would break
  // the default path.
  assert.deepEqual(crossBuildViolations(cross({ targetArch: "x64" })), []);
  // No --target: the caller falls back to process.arch, a native build too.
  assert.deepEqual(crossBuildViolations(cross({ targetArch: null })), []);
  // Non-Linux: neither step is on that packaging path.
  assert.deepEqual(crossBuildViolations(cross({ platform: "win32" })), []);
  assert.deepEqual(crossBuildViolations(cross({ platform: "darwin" })), []);
});

test("targetArchitecture accepts both --target spellings", () => {
  assert.equal(targetArchitecture(["build", "--target", "aarch64-unknown-linux-gnu"]), "arm64");
  assert.equal(targetArchitecture(["build", "--target=x86_64-unknown-linux-gnu"]), "x64");
  // Without --target it is a native build: null keeps process.arch.
  assert.equal(targetArchitecture(["build"]), null);
});

test("a --target without a value is reported as such, not as an unknown architecture", () => {
  // `--target --bundles deb` used to parse "--bundles" as the triple and
  // complain about an unknown machine segment, pointing at a non-existent
  // architecture problem instead of the missing value.
  for (const args of [
    ["build", "--target"],
    ["build", "--target", "--bundles", "deb"],
    ["build", "--target="],
  ]) {
    assert.throws(
      // Rejected while parsing, whatever the host is, so no options here.
      () => targetArchitecture(args),
      (error) => {
        assert.match(error.message, /--target/u, `must name the argument: ${error.message}`);
        assert.ok(
          !error.message.includes("architecture table"),
          `must not disguise itself as an unsupported architecture: ${error.message}`,
        );
        return true;
      },
    );
  }
});

// An unknown machine segment used to return null as well. The gate lets
// `!targetArch` through, so overlays, ASR and the knowledge host would fall
// back to the host architecture - the "target main binary, host sidecars"
// package the gate exists to prevent, entering through another door. It now
// throws, and the tests below pin when a build really counts as cross.
const LINUX_X64 = { platform: "linux", hostArch: "x64" };

test("a Linux cross build to an unknown machine segment fails instead of falling back", () => {
  // Counter-examples use segments unlikely to ever be supported; riscv64 is a
  // plausible future target and would turn this test red on its own.
  for (const triple of ["loongarch64-unknown-linux-gnu", "armv7-unknown-linux-gnueabihf"]) {
    assert.throws(
      () => targetArchitecture(["build", `--target=${triple}`], LINUX_X64),
      (error) => {
        // The message says which triple, why, and how to add support.
        assert.ok(error.message.includes(triple), `must name the triple: ${error.message}`);
        assert.ok(
          error.message.includes(triple.split("-")[0]),
          `must name the unknown machine segment: ${error.message}`,
        );
        assert.ok(
          error.message.includes("TARGET_MACHINE_ARCHITECTURES"),
          `must point at the table to extend: ${error.message}`,
        );
        return true;
      },
    );
  }
});

test("universal-apple-darwin keeps falling back to the host architecture", () => {
  // Universal binaries build both architectures and have no single target;
  // rejecting them would block macOS packaging.
  assert.equal(
    targetArchitecture(
      ["build", "--target=universal-apple-darwin"],
      { platform: "darwin", hostArch: "arm64" },
    ),
    null,
  );
  // The allowance is darwin-only: on Linux cargo cannot build macOS output, so
  // the triple must not bypass the unknown-architecture failure.
  assert.throws(
    () => targetArchitecture(["build", "--target=universal-apple-darwin"], LINUX_X64),
    /universal/,
  );
});

test("the unknown-architecture failure is limited to real cross builds", () => {
  const unknown = ["build", "--target=loongarch64-unknown-linux-gnu"];
  // Non-Linux hosts: the host-only sidecars are not on those packaging paths.
  assert.equal(targetArchitecture(unknown, { platform: "win32", hostArch: "x64" }), null);
  assert.equal(targetArchitecture(unknown, { platform: "darwin", hostArch: "arm64" }), null);
  // A host arch outside the table: building loongarch64 on loongarch64 is
  // native, and there is no way to tell whether the build is cross.
  assert.equal(
    targetArchitecture(unknown, { platform: "linux", hostArch: "loongarch64" }),
    null,
  );
  // Without --target the machine segment is never examined.
  assert.equal(targetArchitecture(["build"], LINUX_X64), null);
});

// ---- Where the switches are read ----

const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

const BUILD_SCRIPT_SOURCE = fs.readFileSync(
  path.join(__dirname, "..", "scripts", "tauri", "build.js"),
  "utf8",
);

test("main() computes both skip switches once and passes them down", () => {
  // The gate and the overlay removal must share one decision. Reading the
  // environment separately in main() and prepareTauriArgs, with awaits in
  // between, could yield two answers: "declaration removed but artifact still
  // built", or "artifact skipped but still declared", which fails the bundle
  // with a missing resource.
  const call = BUILD_SCRIPT_SOURCE.match(
    /const preparedArgs = prepareTauriArgs\(args, \{[\s\S]*?\n {2}\}\);/u,
  );
  assert.ok(call, "main() must assemble the Tauri arguments through prepareTauriArgs");
  assert.match(call[0], /\bskipLinuxAsr,/u, "main() must pass the computed skipLinuxAsr down");
  assert.match(
    call[0],
    /\bskipKnowledgeHost,/u,
    "main() must pass the computed skipKnowledgeHost down",
  );
  for (const name of ["PINVOU3_SKIP_LINUX_ASR", "PINVOU3_SKIP_KNOWLEDGE_HOST"]) {
    assert.equal(
      BUILD_SCRIPT_SOURCE.split(`process.env.${name}`).length - 1,
      2,
      `${name} may only be read twice: once at the main() entry and once as the`
        + " prepareTauriArgs default for callers that bypass main(). Any other read"
        + " is a source of divergence.",
    );
  }
});

// ---- main() orchestration ----
//
// Everything above tests pure functions. They can all be right while main()
// is wired wrong and the package is still silently broken: removing the whole
// `if (violations.length > 0) throw` block, or making
// `if (!skipKnowledgeHost) prepareKnowledgeHost()` unconditional, used to keep
// every test green. The tests below pin both: the gate through the real
// `main()`, the preparation guards through source structure.

/**
 * Run the real `main()` as a given host platform/arch and restore globals.
 *
 * Calling `main()` in-process is safe because only pure computation precedes
 * the gate on the Linux, non-dev path (platform checks, Windows-only
 * branches, `targetArchitecture`): nothing is spawned or written before the
 * gate throws. Scenarios past the gate cannot be tested this way - they would
 * download Node tarballs and run cargo for tens of minutes.
 *
 * `process.platform` / `process.arch` can only be changed globally since
 * `main()` reads them directly. node --test runs each test file in its own
 * process, so restoring them in `finally` is enough.
 * @param {string[]} argv Command-line arguments (without node and script).
 * @param {{platform: string, arch: string, env?: Record<string, string|undefined>}} host Host.
 * @returns {Promise<number>} Whatever `main()` returns.
 */
async function runMainAs(argv, { platform, arch, env = {} }) {
  const savedArgv = process.argv;
  const savedPlatform = Object.getOwnPropertyDescriptor(process, "platform");
  const savedArch = Object.getOwnPropertyDescriptor(process, "arch");
  const savedEnv = Object.fromEntries(
    Object.keys(env).map((name) => [name, process.env[name]]),
  );
  try {
    process.argv = [process.execPath, "build.js", ...argv];
    Object.defineProperty(process, "platform", { value: platform, configurable: true });
    Object.defineProperty(process, "arch", { value: arch, configurable: true });
    for (const [name, value] of Object.entries(env)) {
      if (value === undefined) delete process.env[name];
      else process.env[name] = value;
    }
    return await main();
  } finally {
    process.argv = savedArgv;
    Object.defineProperty(process, "platform", savedPlatform);
    Object.defineProperty(process, "arch", savedArch);
    for (const [name, value] of Object.entries(savedEnv)) {
      if (value === undefined) delete process.env[name];
      else process.env[name] = value;
    }
  }
}

// The faked host arch is deliberately the opposite of the real machine, and
// the target triple is the real machine's architecture:
//   1. target != host, so this is a cross build the gate must block;
//   2. if the gate were removed, `prepareLinuxAsrRuntime()` would run
//      `scripts/asr/build-sensevoice-runtime.sh` for the faked architecture,
//      whose first action is the `requested_arch == host_arch` assertion
//      (before any download or compile). It exits within milliseconds with a
//      different error, so the test still fails without hanging CI. Hard-coding
//      x64 would make an x86_64 machine really compile SenseVoice instead.
const REAL_MACHINE_IS_ARM = process.arch === "arm64";
const FAKE_HOST_ARCH = REAL_MACHINE_IS_ARM ? "x64" : "arm64";
const CROSS_ARGV = [
  "build",
  "--target",
  REAL_MACHINE_IS_ARM ? "aarch64-unknown-linux-gnu" : "x86_64-unknown-linux-gnu",
];
const CROSS_TARGET_ARCH = REAL_MACHINE_IS_ARM ? "arm64" : "x64";

test("main() really stops at the gate instead of discarding the violations", async () => {
  // Without the throw block this run would pass the gate and reach
  // prepareLinuxAsrRuntime() (which fails fast with another error, see above),
  // never producing the messages asserted here.
  await assert.rejects(
    () => runMainAs(CROSS_ARGV, {
      platform: "linux",
      arch: FAKE_HOST_ARCH,
      env: { PINVOU3_SKIP_LINUX_ASR: undefined, PINVOU3_SKIP_KNOWLEDGE_HOST: undefined },
    }),
    (error) => {
      assert.match(
        error.message,
        /Cross build/u,
        `must say the cross build is incomplete: ${error.message}`,
      );
      assert.match(error.message, /PINVOU3_SKIP_LINUX_ASR/u, error.message);
      assert.match(error.message, /PINVOU3_SKIP_KNOWLEDGE_HOST/u, error.message);
      // Both host and target must appear, or the error does not say which build.
      assert.ok(error.message.includes(FAKE_HOST_ARCH), error.message);
      assert.ok(error.message.includes(CROSS_TARGET_ARCH), error.message);
      return true;
    },
  );
});

test("main() feeds the switches into the gate: one switch still blocks, naming only the other", async () => {
  // Proves that the two `process.env.PINVOU3_SKIP_*` reads in main() reach the
  // gate. The direction matters: setting the ASR switch instead would, without
  // the gate, skip ASR and reach `prepareCodexBridge()`, which downloads Node
  // and hangs; leaving ASR unset hits the SenseVoice architecture assertion and
  // exits at once.
  await assert.rejects(
    () => runMainAs(CROSS_ARGV, {
      platform: "linux",
      arch: FAKE_HOST_ARCH,
      env: { PINVOU3_SKIP_LINUX_ASR: undefined, PINVOU3_SKIP_KNOWLEDGE_HOST: "1" },
    }),
    (error) => {
      assert.match(error.message, /PINVOU3_SKIP_LINUX_ASR/u, error.message);
      assert.ok(
        !error.message.includes("PINVOU3_SKIP_KNOWLEDGE_HOST"),
        `a switch that is already set must not be reported: ${error.message}`,
      );
      return true;
    },
  );
});

// The "do not block native builds" side cannot be tested this way: once the
// gate passes, main() really downloads Node and runs cargo. The pure
// `crossBuildViolations` tests above cover it.

test("the gate and both preparation guards stay in main() and never become unconditional", () => {
  // Removing or weakening any of these three silently breaks the package: no
  // gate lets the cross build continue, and an unconditional
  // `prepareKnowledgeHost()` makes `cargo build` (no --target) produce a
  // host-architecture server that is packaged for the other architecture with
  // no warning at all.
  //
  // Exercising the preparation steps for real would run them, so this falls
  // back to source-structure assertions, like the "switches read twice" test.
  // They require the guard on the same line as the call: equivalent refactors
  // (renames, reordering) pass, an unconditional call fails.
  const mainBody = BUILD_SCRIPT_SOURCE.match(/\nasync function main\(\) \{\n([\s\S]*?)\n\}\n/u);
  assert.ok(mainBody, "build.js must keep its main() entry point");

  // The behavioral tests above already pin the gate; pinning its structure too
  // makes "delete the whole block" fail on an assertion with a clear message.
  assert.match(
    mainBody[1],
    /if \(violations\.length > 0\) \{[\s\S]*?throw new Error\(/u,
    "main() must throw when there are violations; computing them without using them is no gate",
  );

  for (const [step, guard] of [
    ["prepareLinuxAsrRuntime", "!skipLinuxAsr"],
    ["prepareKnowledgeHost", "!skipKnowledgeHost"],
  ]) {
    // The dev call `prepareKnowledgeHost({ development: true })` is outside the
    // gate's scope (it never enters a package); only the packaging call counts.
    const packagingCalls = mainBody[1]
      .split("\n")
      .filter((line) => line.includes(`${step}(`) && !line.includes("development"));
    assert.equal(
      packagingCalls.length,
      1,
      `main() must call ${step}() exactly once on the packaging path, found ${packagingCalls.length}`,
    );
    assert.ok(
      packagingCalls[0].includes(`if (${guard})`),
      `${step}() must be guarded by ${guard}, or a cross build silently prepares`
        + ` host-architecture artifacts: ${packagingCalls[0].trim()}`,
    );
  }
});

// ---- Dropping resource declarations ----

// One temporary directory for the whole file, removed afterwards.
const TEMP_ROOT = fs.mkdtempSync(path.join(os.tmpdir(), "xbuild-"));
test.after(() => fs.rmSync(TEMP_ROOT, { recursive: true, force: true }));

let tempConfigSeq = 0;

/**
 * Write a temporary overlay and return its absolute path.
 * @param {object|string} content Config content (object or raw text).
 * @param {string} [name] File name; only affects the name shown in errors.
 * @returns {string} Absolute path of the config file.
 */
function writeTempConfig(content, name = "tauri.conf.json") {
  // One sub-directory per file: `configWithoutResources` errors include the
  // file name, and same-named files side by side would make them ambiguous.
  const dir = path.join(TEMP_ROOT, `c${(tempConfigSeq += 1)}`);
  fs.mkdirSync(dir);
  const file = path.join(dir, name);
  fs.writeFileSync(file, typeof content === "string" ? content : JSON.stringify(content));
  return file;
}

function writeConfig(resources) {
  return writeTempConfig({ bundle: { resources, targets: ["deb"] } });
}

test("removes the ASR declaration and keeps everything else", () => {
  const file = writeConfig({
    "target/linux-asr-runtime/aarch64/sense-voice-main": "runtime/asr/sense-voice-main",
    "resources/platforms/linux/codex-bridge/": "runtime/codex-bridge",
  });
  const out = JSON.parse(architectureConfigWithoutAsr(file));
  assert.deepEqual(Object.keys(out.bundle.resources), ["resources/platforms/linux/codex-bridge/"]);
  assert.deepEqual(out.bundle.targets, ["deb"], "other bundle fields must survive");
});

test("removes the knowledge-host declaration", () => {
  const file = writeConfig({
    "resources/platforms/linux/knowledge-host/": "runtime/knowledge-host",
    "resources/platforms/linux/codex-bridge/": "runtime/codex-bridge",
  });
  const out = JSON.parse(platformConfigWithoutKnowledgeHost(file));
  assert.deepEqual(Object.keys(out.bundle.resources), ["resources/platforms/linux/codex-bridge/"]);
});

test("returns the path unchanged when nothing matches", () => {
  const file = writeConfig({ "resources/platforms/linux/codex-bridge/": "runtime/codex-bridge" });
  assert.equal(platformConfigWithoutKnowledgeHost(file), file);
  assert.equal(architectureConfigWithoutAsr(file), file);
});

test("tolerates a config without bundle.resources", () => {
  const file = writeTempConfig({ productName: "x" }, "c.json");
  assert.equal(configWithoutResources(file, ["whatever"]), file);
});

test("a broken JSON file is reported by name", () => {
  const file = writeTempConfig("{ not json", "broken.json");
  // A bare SyntaxError only says "Unexpected token"; these paths are assembled
  // per platform/architecture, so the file name is what matters most.
  assert.throws(
    () => configWithoutResources(file, ["x"]),
    (error) => error.message.includes("broken.json"),
  );
});
