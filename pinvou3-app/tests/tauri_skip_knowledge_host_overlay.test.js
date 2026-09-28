// Skipping the shared knowledge host must also remove its resource
// declaration from the platform overlay.
//
// Counterpart of `tauri_skip_asr_overlay.test.js`: that file pins the ASR
// declaration in the architecture layer, this one the `knowledge-host/`
// declaration in the platform layer. Pure-function tests are not enough for
// either: with correct functions but wrong wiring (a misspelled switch, a lost
// Linux guard, the replacement not swapped in) nothing fails:
//   - clean tree: the directory is missing, zero files are enumerated and the
//     package silently lacks the server;
//   - dirty tree: a server from an earlier host-architecture build is
//     packaged for the other architecture.
// Both surface only when a user reaches the shared knowledge base on a device,
// so these tests go through the real `prepareTauriArgs` and assert on the
// `--config` list it returns.
//
// Why the overlay is replaced instead of layering `{key: null}` is argued once,
// in the `configWithoutResources` doc comment in build.js.
const assert = require("node:assert/strict");
const fs = require("node:fs");
const test = require("node:test");

const { configSpecs, prepareTauriArgs } = require("../scripts/tauri/build.js");
const { platformConfigPath } = require("../scripts/tauri/platform-config.js");

const BASE = ["build", "--target", "aarch64-unknown-linux-gnu"];
const LINUX = { platform: "linux", architecture: "x64", stageRuntime: () => null };
const SWITCH = "PINVOU3_SKIP_KNOWLEDGE_HOST";

/** An overlay is either a file path or an inline JSON string starting with `{`. */
const isInline = (spec) => spec.trim().startsWith("{");

/** Resource sources declared by an inline overlay; file paths are not expanded. */
function inlineResourceSources(spec) {
  return isInline(spec) ? Object.keys(JSON.parse(spec)?.bundle?.resources || {}) : [];
}

/**
 * Run assertions under the given switch value and restore the environment.
 *
 * The switch must be saved and restored: it acts on the same `--config` list,
 * and one leftover value makes later tests read the previous test's state.
 * @param {string|undefined} value Switch value; undefined unsets it.
 * @param {() => void} body Assertions.
 */
function withSwitch(value, body) {
  const saved = process.env[SWITCH];
  try {
    if (value === undefined) delete process.env[SWITCH];
    else process.env[SWITCH] = value;
    body();
  } finally {
    if (saved === undefined) delete process.env[SWITCH];
    else process.env[SWITCH] = saved;
  }
}

test("by default the platform overlay stays a file path and declares knowledge-host", () => {
  withSwitch(undefined, () => {
    const specs = configSpecs(prepareTauriArgs(BASE, LINUX));
    assert.equal(
      specs[0],
      platformConfigPath("linux"),
      "the platform overlay comes first and, unskipped, is the file path itself",
    );
    // No inline overlay may declare knowledge-host (that would mean the
    // replacement kicked in when it should not).
    assert.deepEqual(
      specs.flatMap(inlineResourceSources).filter((source) => source.includes("knowledge-host")),
      [],
    );
  });
});

test("skipKnowledgeHost=true replaces the platform overlay minus only knowledge-host", () => {
  withSwitch(undefined, () => {
    // Explicit parameter: this is how main() wires it (read once, passed down).
    const specs = configSpecs(prepareTauriArgs(BASE, { ...LINUX, skipKnowledgeHost: true }));
    const platformOverlay = specs[0];
    assert.ok(isInline(platformOverlay), "skipping must replace the platform overlay with inline JSON");

    // Replacement, not addition: the file path must not stay next to the
    // inline version, or merging would bring the declaration back.
    assert.ok(
      !specs.includes(platformConfigPath("linux")),
      "the platform overlay file path must be replaced, not kept next to the inline version",
    );

    // The main risk of replacing a whole config is dropping keys, so compare
    // layer by layer: only the knowledge-host resource may be missing.
    const original = JSON.parse(fs.readFileSync(platformConfigPath("linux"), "utf8"));
    const replaced = JSON.parse(platformOverlay);
    assert.deepEqual(Object.keys(replaced), Object.keys(original), "top-level keys must survive");
    assert.deepEqual(
      Object.keys(replaced.bundle),
      Object.keys(original.bundle),
      "bundle keys must survive",
    );
    assert.deepEqual(
      Object.keys(replaced.bundle.resources),
      Object.keys(original.bundle.resources).filter((source) => !source.includes("knowledge-host")),
      "every other resource declaration must remain, in the original order",
    );
    assert.ok(
      Object.keys(original.bundle.resources).some((source) => source.includes("knowledge-host")),
      "the platform overlay must declare knowledge-host, or this test asserts nothing",
    );
  });
});

test("non-Linux platforms: the switch leaves the platform overlay untouched", () => {
  withSwitch("1", () => {
    // knowledge-host is only on the Linux packaging path. Off Linux the
    // removal is currently a no-op (no overlay declares it), so this cannot
    // distinguish a guarded from an unguarded switch today; what it pins is
    // that setting the switch never disturbs the non-Linux overlay chain,
    // which the `platform === "linux"` guard keeps true even if a future
    // non-Linux overlay ever gains a matching declaration.
    const specs = configSpecs(prepareTauriArgs(["build"], { ...LINUX, platform: "win32" }));
    assert.equal(specs[0], platformConfigPath("win32"));
  });
});

test("the knowledge-host marker removes the knowledge-host declaration and nothing wider", () => {
  // The marker is a substring match. Widening it (say `knowledge-host` to
  // `knowledge`) would silently drop unrelated declarations from the platform
  // overlay, and the layer-by-layer test above cannot see that: the tracked
  // overlay happens to contain no near-miss key. This synthetic overlay
  // supplies the near-miss witness. The test goes through the production
  // wrapper so the marker string pinned here is the one the build actually
  // uses.
  const { platformConfigWithoutKnowledgeHost } = require("../scripts/tauri/build.js");
  const os = require("node:os");
  const path = require("node:path");
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "pinvou3-kb-marker-"));
  try {
    const overlay = path.join(dir, "tauri.conf.json");
    const resources = {
      "resources/platforms/linux/asr/": "runtime/asr",
      "resources/platforms/linux/knowledge-host/": "runtime/knowledge-host",
      "resources/platforms/linux/knowledge-assets/": "runtime/knowledge-assets",
    };
    fs.writeFileSync(overlay, JSON.stringify({ bundle: { resources } }));
    const result = JSON.parse(platformConfigWithoutKnowledgeHost(overlay));
    assert.deepEqual(
      Object.keys(result.bundle.resources),
      ["resources/platforms/linux/asr/", "resources/platforms/linux/knowledge-assets/"],
      "only the knowledge-host declaration may be removed",
    );
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

test("callers that bypass main(): the environment default still applies", () => {
  withSwitch("1", () => {
    const specs = configSpecs(prepareTauriArgs(BASE, LINUX));
    assert.ok(isInline(specs[0]), "without the parameter PINVOU3_SKIP_KNOWLEDGE_HOST applies");
    assert.deepEqual(
      inlineResourceSources(specs[0]).filter((source) => source.includes("knowledge-host")),
      [],
    );
  });
  withSwitch("0", () => {
    // Only "1" counts; any other value is off, so `=0` never skips.
    assert.equal(configSpecs(prepareTauriArgs(BASE, LINUX))[0], platformConfigPath("linux"));
  });
});
