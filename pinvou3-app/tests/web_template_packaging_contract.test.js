const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const { execFileSync } = require("node:child_process");

const APP_ROOT = path.resolve(__dirname, "..");
const REPO_ROOT = path.resolve(APP_ROOT, "..");
const TEMPLATE_ROOT = path.join(
  APP_ROOT,
  "src-tauri",
  "resources",
  "common",
  "web-template",
);

for (const required of ["package.json", "package-lock.json"]) {
  assert.ok(
    fs.existsSync(path.join(TEMPLATE_ROOT, required)),
    `网页模板必须保留 ${required}`,
  );
}

const trackedNodeModules = execFileSync(
  "git",
  ["ls-files", "--", ":(glob)**/web-template/node_modules/**"],
  { cwd: REPO_ROOT, encoding: "utf8" },
).trim();
assert.equal(
  trackedNodeModules,
  "",
  "源码仓库不得跟踪网页模板 node_modules",
);

const buildSource = fs.readFileSync(
  path.join(APP_ROOT, "scripts", "tauri", "build.js"),
  "utf8",
);
const prepareIndex = buildSource.indexOf("prepareWebTemplate();");
const manifestIndex = buildSource.indexOf("writeEffectiveArtifacts(");
assert.ok(prepareIndex >= 0, "统一 Tauri wrapper 必须准备网页模板依赖");
assert.ok(
  manifestIndex > prepareIndex,
  "必须先准备平台依赖，再生成安装包资源清单",
);

const prepareSource = fs.readFileSync(
  path.join(APP_ROOT, "scripts", "tauri", "web-template.js"),
  "utf8",
);
assert.match(prepareSource, /package-lock\.json/);
assert.match(prepareSource, /"ci"/);
assert.match(prepareSource, /platform/);
assert.match(prepareSource, /architecture/);

console.log("web template packaging contract ok");
