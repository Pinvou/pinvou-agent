const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");

const appRoot = path.resolve(__dirname, "..");
const tauriRoot = path.join(appRoot, "src-tauri");

function readJson(relativePath) {
  return JSON.parse(fs.readFileSync(path.join(tauriRoot, relativePath), "utf8"));
}

function resourceSources(config) {
  return Object.keys(config.bundle?.resources || {});
}

function assertResourceSourcesExist(config, label) {
  for (const source of resourceSources(config)) {
    assert.ok(
      fs.existsSync(path.join(tauriRoot, source)),
      `${label} resource source must exist: ${source}`,
    );
  }
}

const common = readJson("tauri.conf.json");
const linux = readJson("config/platforms/linux/tauri.conf.json");
const macos = readJson("config/platforms/macos/tauri.conf.json");
const windows = readJson("config/platforms/windows/tauri.conf.json");

assert.ok(resourceSources(common).length > 0, "common Tauri config must declare shared resources");
assert.ok(
  resourceSources(common).every((source) => source.startsWith("resources/common/")),
  "common Tauri config may only package resources/common",
);
assert.ok(
  resourceSources(linux).every((source) => source.startsWith("resources/platforms/linux/")),
  "Linux overlay may only package resources/platforms/linux",
);
assert.ok(
  resourceSources(macos).every((source) => source.startsWith("resources/platforms/macos/")),
  "macOS overlay may only package resources/platforms/macos",
);
assert.ok(
  resourceSources(windows).every((source) => source.startsWith("resources/platforms/windows/")),
  "Windows overlay may only package resources/platforms/windows",
);

assertResourceSourcesExist(common, "common");
assertResourceSourcesExist(linux, "Linux");
assertResourceSourcesExist(macos, "macOS");
assertResourceSourcesExist(windows, "Windows");

for (const legacyPath of ["resources/bundle", "resources/skill-marketplace", "resources/web-template", "resources/asr"]) {
  assert.equal(fs.existsSync(path.join(tauriRoot, legacyPath)), false, `legacy resource root must be removed: ${legacyPath}`);
}

console.log("tauri platform layout contract: ok");
