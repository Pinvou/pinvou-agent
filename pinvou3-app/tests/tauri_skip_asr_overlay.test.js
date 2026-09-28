// Resources in the architecture overlay that cannot always be prepared must
// be removed correctly when their skip switch is set.
//
// The ASR runtime is declared in the tracked aarch64 overlay, but SenseVoice
// can only be built natively on the target architecture
// (build-sensevoice-runtime.sh asserts requested_arch == host_arch), so cross
// builds must skip it. The file then does not exist while the overlay still
// declares it, and `writeEffectiveArtifacts` stops with a missing-resource
// error. Switch: PINVOU3_SKIP_LINUX_ASR=1, off by default.
//
// Why the overlay is replaced instead of layering `{key: null}` is argued once,
// in the `configWithoutResources` doc comment in build.js.
//
// These tests pin the observable behavior of the current approach, not "only
// replacement is right". The property that must not loosen is: with the switch
// set, the unpreparable file must not appear in any effective resources
// declaration. If the implementation moves to null deletion, rewrite the two
// assertions about the inline overlay and the absent file path for the new
// shape, but keep that property.
const assert = require("node:assert");
const fs = require("node:fs");
const test = require("node:test");

const { prepareTauriArgs } = require("../scripts/tauri/build.js");
const { platformArchitectureConfigPath } = require("../scripts/tauri/platform-config.js");

/** Every --config entry (inline JSON or path) produced by prepareTauriArgs. */
function overlayEntries(args) {
  const specs = [];
  for (let i = 0; i < args.length; i += 1) {
    if (args[i] === "--config" && args[i + 1]) specs.push(args[i + 1]);
  }
  return specs;
}

/** Resource sources declared by all inline overlays. */
function inlineResourceSources(args) {
  const sources = [];
  for (const spec of overlayEntries(args)) {
    if (!spec.trim().startsWith("{")) continue;
    sources.push(...Object.keys(JSON.parse(spec)?.bundle?.resources || {}));
  }
  return sources;
}

const BASE = ["build", "--target", "aarch64-unknown-linux-gnu"];
const OPTS = { platform: "linux", architecture: "x64", stageRuntime: () => null };
const SWITCHES = ["PINVOU3_SKIP_LINUX_ASR"];

/**
 * Run prepareTauriArgs under the given switches and restore the environment.
 *
 * All switches are saved and restored together: they act on the same
 * --config list, and one leftover value makes later tests read the previous
 * test's state ("passes alone, fails together").
 * @param {Record<string, string|undefined>} env Switches to set (missing = unset).
 * @param {(run: () => string[]) => void} body Assertions; call run() for the args.
 */
function withSwitches(env, body) {
  const saved = Object.fromEntries(SWITCHES.map((name) => [name, process.env[name]]));
  try {
    for (const name of SWITCHES) {
      if (env[name] === undefined) delete process.env[name];
      else process.env[name] = env[name];
    }
    body(() => prepareTauriArgs(BASE, OPTS));
  } finally {
    for (const name of SWITCHES) {
      if (saved[name] === undefined) delete process.env[name];
      else process.env[name] = saved[name];
    }
  }
}

test("non-Linux platforms: the switch leaves the overlay chain untouched", () => {
  // Mirrors the case in tauri_skip_knowledge_host_overlay.test.js. Off Linux
  // no overlay matches the marker, so the removal is a no-op there and this
  // cannot distinguish a guarded from an unguarded switch today; what it
  // pins is that setting the switch never disturbs the non-Linux overlay
  // chain, which the `platform === "linux"` guard keeps true even if a
  // future non-Linux overlay ever gains a matching declaration. win32 has no
  // architecture overlay, so its platform overlay must stay a file path.
  const saved = process.env.PINVOU3_SKIP_LINUX_ASR;
  try {
    process.env.PINVOU3_SKIP_LINUX_ASR = "1";
    const specs = overlayEntries(
      prepareTauriArgs(["build"], { platform: "win32", stageRuntime: () => null }),
    );
    assert.ok(specs.length > 0, "a win32 build must still inject the platform overlay");
    for (const spec of specs) {
      assert.ok(
        !spec.trim().startsWith("{"),
        `no inline replacement may appear off Linux: ${spec.slice(0, 48)}`,
      );
    }
  } finally {
    if (saved === undefined) delete process.env.PINVOU3_SKIP_LINUX_ASR;
    else process.env.PINVOU3_SKIP_LINUX_ASR = saved;
  }
});

test("by default the architecture overlay stays a file path and declares ASR", () => {
  withSwitches({}, (run) => {
    const args = run();
    const specs = overlayEntries(args);
    const archOverlay = specs.find((spec) => spec.includes("aarch64"));
    assert.ok(archOverlay, "the aarch64 architecture overlay must be injected");
    assert.ok(!archOverlay.trim().startsWith("{"), "without the switch the file path is used as is");
    // No inline overlay may carry ASR resources (that would mean the
    // replacement kicked in when it should not).
    assert.deepEqual(
      inlineResourceSources(args).filter((source) => source.includes("linux-asr-runtime")),
      [],
    );
  });
});

test("PINVOU3_SKIP_LINUX_ASR=1 replaces the architecture overlay without the ASR resource", () => {
  withSwitches({ PINVOU3_SKIP_LINUX_ASR: "1" }, (run) => {
    const args = run();
    const specs = overlayEntries(args);
    const inlineArch = specs.find((spec) => spec.trim().startsWith("{") && spec.includes("bundle"));
    assert.ok(inlineArch, "skipping ASR must replace the architecture overlay with inline JSON");
    assert.ok(
      !Object.keys(JSON.parse(inlineArch).bundle.resources).some((source) =>
        source.includes("linux-asr-runtime"),
      ),
      "the ASR resource must be removed",
    );
    // Replacement, not addition: the file path must be gone, or deep merging
    // would bring the ASR declaration back.
    assert.ok(
      !specs.some((spec) => !spec.trim().startsWith("{") && spec.includes("aarch64")),
      "the architecture overlay file path must be replaced, not kept next to the inline version",
    );
    // Fixture self-check (mirrors the knowledge-host test): if the aarch64
    // overlay ever stops declaring ASR, "removed" above would pass vacuously.
    const original = JSON.parse(
      fs.readFileSync(platformArchitectureConfigPath("linux", "arm64"), "utf8"),
    );
    assert.ok(
      Object.keys(original?.bundle?.resources || {}).some((source) =>
        source.includes("linux-asr-runtime"),
      ),
      "the aarch64 overlay must declare ASR, or this test asserts nothing",
    );
  });
});

test("the ASR marker removes the ASR declaration and nothing wider", () => {
  // The marker is a substring match. Widening it (say `linux-asr-runtime` to
  // `linux-asr`) would silently drop unrelated declarations from every
  // architecture overlay, and the prepareTauriArgs tests above cannot see
  // that: the tracked overlay happens to contain no near-miss key. This
  // synthetic overlay supplies the near-miss witness. The test goes through
  // the production wrapper so the marker string pinned here is the one the
  // build actually uses.
  const { architectureConfigWithoutAsr } = require("../scripts/tauri/build.js");
  const os = require("node:os");
  const path = require("node:path");
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "pinvou3-asr-marker-"));
  try {
    const overlay = path.join(dir, "tauri.conf.json");
    const resources = {
      "target/linux-asr-runtime/aarch64/sense-voice-main": "runtime/asr/sense-voice-main",
      "target/linux-asr-models/config.json": "runtime/asr/config.json",
      "resources/platforms/linux/codex-bridge/": "runtime/codex-bridge",
    };
    fs.writeFileSync(overlay, JSON.stringify({ bundle: { resources } }));
    const result = JSON.parse(architectureConfigWithoutAsr(overlay));
    assert.deepEqual(
      Object.keys(result.bundle.resources),
      ["target/linux-asr-models/config.json", "resources/platforms/linux/codex-bridge/"],
      "only the linux-asr-runtime declaration may be removed",
    );
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});
