const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");

const root = path.resolve(__dirname, "..");
const read = (...parts) => fs.readFileSync(path.join(root, ...parts), "utf8");
const installer = read("src-tauri", "src", "features", "connectors", "native_installer.rs");
const paths = read("src-tauri", "src", "platform", "paths.rs");
const build = read("scripts", "tauri", "build.js");
const tmeet = read("src-tauri", "src", "features", "connectors", "tmeet.rs");
const linux = read("src-tauri", "src", "platform", "os", "linux", "linux_path.rs");
const macos = read("src-tauri", "src", "platform", "os", "macos", "macos_path.rs");

assert.doesNotMatch(build, /prepareConnectorClis|fetch-connectors/);
assert.match(paths, /pinvou3_home\(\)\.join\("connectors"\)/);
assert.doesNotMatch(paths, /bundle_root\(\)\.join\("connectors"\)/);

for (const connector of ["lark-cli", "wecom-cli", "dws"]) {
  const feature = read(
    "src-tauri",
    "src",
    "features",
    "connectors",
    connector === "lark-cli" ? "feishu.rs" : connector === "wecom-cli" ? "wecom.rs" : "dingtalk.rs",
  );
  assert.match(feature, new RegExp(`ensure_native_cli\\("${connector}"\\)`));
}

assert.match(installer, /archive_sha256/);
assert.match(installer, /binary_sha256/);
assert.match(installer, /url\.scheme\(\) != "https"/);
assert.match(installer, /MAX_ARCHIVE_BYTES/);
assert.match(installer, /normalized_path_eq/);
assert.match(installer, /\.installing-/);

assert.match(tmeet, /@tencentcloud\/tmeet@1\.0\.15/);
for (const platformSource of [linux, macos]) {
  assert.match(platformSource, /bundled_connector_npm_cli/);
  assert.match(platformSource, /cli_bin == "tmeet"/);
  assert.match(platformSource, /bundled_connector_node/);
}

console.log("✓ connector first-use online install contract passed");
