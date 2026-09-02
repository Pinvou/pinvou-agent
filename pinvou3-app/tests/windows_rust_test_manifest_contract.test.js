#!/usr/bin/env node
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");

const buildScript = fs.readFileSync(
  path.join(__dirname, "..", "src-tauri", "build.rs"),
  "utf8",
);
const rustLib = fs.readFileSync(
  path.join(__dirname, "..", "src-tauri", "src", "lib.rs"),
  "utf8",
);

assert.match(
  buildScript,
  /pinvou3_lib_test_resource\.lib/,
  "the Windows library-test resource must be archived separately",
);
assert.match(buildScript, /\.join\("resource\.lib"\)/);
assert.match(buildScript, /cc::windows_registry::find_tool\(&target, "lib\.exe"\)/);
assert.match(buildScript, /cargo:rustc-link-search=native=\{\}/);
assert.ok(
  buildScript.indexOf("tauri_build::build();") <
    buildScript.indexOf('join("resource.lib")'),
  "tauri-build must generate resource.lib before the test archive is created",
);
assert.match(rustLib, /cfg\(all\(test, target_os = "windows", target_env = "msvc"\)\)/);
assert.match(rustLib, /name = "pinvou3_lib_test_resource"/);
assert.match(rustLib, /modifiers = "\+whole-archive"/);

console.log("windows Rust test manifest contract tests passed");
