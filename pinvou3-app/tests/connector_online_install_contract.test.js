const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");

const root = path.resolve(__dirname, "..");
const read = (...parts) => fs.readFileSync(path.join(root, ...parts), "utf8");
const installer = read("src-tauri", "src", "features", "connectors", "native_installer.rs");
const paths = read("src-tauri", "src", "platform", "paths.rs");
const build = read("scripts", "tauri", "build.js");
const tmeet = read("src-tauri", "src", "features", "connectors", "tmeet.rs");
const toolCommon = read("src", "features", "tools", "tool-common.jsx");
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

// 版本联动：工具卡展示版本必须与 lock 钉扎（及 tmeet.rs npm 钉扎）一致，
// 防止再次出现卡片版本与实际安装版本脱节（如历史上的 v1.0.56 vs lock 1.0.65）。
// 5 份平台 lock 全量校验（NOTICE 承诺「5 份任一即可，版本字段一致」由此背书）：
// 只改其中一份的版本而漏改其余（或漏改卡片）都会在此失败。
const lockVersionsByPlatform = {};
for (const [osDir, archDir] of [
  ["macos", "aarch64"],
  ["macos", "x86_64"],
  ["linux", "aarch64"],
  ["linux", "x86_64"],
  ["windows", "x86_64"],
]) {
  const lock = JSON.parse(
    read("src-tauri", "resources", "platforms", osDir, archDir, "bundle", "connectors", "connectors.lock.json"),
  );
  lockVersionsByPlatform[`${osDir}/${archDir}`] = Object.fromEntries(lock.artifacts.map((a) => [a.name, a.version]));
}
const lockVersions = lockVersionsByPlatform["macos/aarch64"];
for (const [platform, versions] of Object.entries(lockVersionsByPlatform)) {
  assert.deepEqual(versions, lockVersions, `${platform} connectors.lock.json 版本与其他平台不一致`);
}
const cardVersion = (marker) => {
  const line = toolCommon.split("\n").find((l) => l.includes(marker));
  const match = line && line.match(/version:\s*['"]v([\d.]+)['"]/);
  assert.ok(match, `tool-common.jsx 未在 ${marker} 行找到 version: 'v…'（格式变更须同步本断言）`);
  return match[1];
};
assert.equal(cardVersion("backendId: 'feishu', feishuCli: true"), lockVersions["lark-cli"]);
assert.equal(cardVersion("backendId: 'dingtalk', dingtalkCli: true"), lockVersions["dws"]);
assert.equal(cardVersion("backendId: 'wecom', wecomCli: true"), lockVersions["wecom-cli"]);
const tmeetPin = tmeet.match(/@tencentcloud\/tmeet@([\d.]+)/)[1];
assert.equal(cardVersion("backendId: 'tmeet', tmeetCli: true"), tmeetPin);

console.log("✓ connector first-use online install contract passed");
