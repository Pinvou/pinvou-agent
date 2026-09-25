const assert = require("node:assert/strict");
const { EventEmitter } = require("node:events");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const test = require("node:test");

const {
  configureIsolatedWindowsRustToolchain,
  ensureWindowsRustToolchain,
  formatElapsed,
  pinnedRustToolchainChannel,
  prepareWindowsRustcStackWrapper,
  windowsRustHostTriple,
} = require("../scripts/tauri/build.js");
const { APP_ROOT } = require("../scripts/tauri/platform-config.js");

const read = (...parts) => fs.readFileSync(path.join(APP_ROOT, ...parts), "utf8");
const toolchainConfig = read("src-tauri", "rust-toolchain.toml");
const channel = toolchainConfig.match(/^\s*channel\s*=\s*"([^"]+)"/mu)[1];
const rustToolchainGuard = read("scripts", "ci", "ensure-rust-toolchain.ps1");
const rustupRepairSmoke = read("tests", "windows_rustup_repair_smoke.ps1");
const buildScript = read("scripts", "tauri", "build.js");
const packageJson = JSON.parse(read("package.json"));

const normalized = (value) => value.replaceAll("\\", "/");

function scriptedRustToolchainSpawn(exitCodes, invocations) {
  return (command, args, options) => {
    invocations.push({ command, args, options });
    const child = new EventEmitter();
    const exitCode = exitCodes.shift();
    queueMicrotask(() => child.emit("exit", exitCode, null));
    return child;
  };
}

test("the pinned channel comes from rust-toolchain.toml", () => {
  assert.equal(pinnedRustToolchainChannel(), channel);
  assert.equal(windowsRustHostTriple("x64"), "x86_64-pc-windows-msvc");
  assert.equal(windowsRustHostTriple("arm64"), "aarch64-pc-windows-msvc");
  assert.equal(windowsRustHostTriple("ia32"), "i686-pc-windows-msvc");
  assert.throws(() => windowsRustHostTriple("riscv64"), /Unsupported Windows Rust architecture/u);
});

test("the isolated RUSTUP_HOME is workspace scoped and marked as managed", () => {
  const env = {};
  const isolated = configureIsolatedWindowsRustToolchain({ env, architecture: "x64" });
  assert.equal(isolated.channel, channel);
  assert.equal(isolated.hostTriple, "x86_64-pc-windows-msvc");
  assert.equal(env.RUSTUP_HOME, isolated.rustupHome);
  assert.equal(env.PINVOU3_MANAGED_RUSTUP, "1");
  assert.equal(env.RUSTUP_TOOLCHAIN, channel);
  assert.ok(
    normalized(isolated.rustupHome).endsWith(
      `/.cache/rustup/${channel}-x86_64-pc-windows-msvc`,
    ),
    `unexpected isolated RUSTUP_HOME: ${isolated.rustupHome}`,
  );

  const customEnv = { PINVOU3_RUSTUP_HOME: "custom-rustup" };
  const custom = configureIsolatedWindowsRustToolchain({
    env: customEnv,
    architecture: "arm64",
  });
  assert.equal(custom.hostTriple, "aarch64-pc-windows-msvc");
  assert.equal(custom.rustupHome, path.join(APP_ROOT, "custom-rustup"));
});

test("a complete account toolchain is reused read-only", async () => {
  const invocations = [];
  const env = {};
  const result = await ensureWindowsRustToolchain({
    env,
    architecture: "x64",
    log: () => {},
    spawnChild: scriptedRustToolchainSpawn([0], invocations),
  });
  assert.equal(result.source, "account");
  assert.equal(invocations.length, 1);
  assert.equal(invocations[0].command, "powershell.exe");
  assert.equal(invocations[0].args.at(-1), "-CheckOnly");
  assert.equal(invocations[0].options.env.PINVOU3_MANAGED_RUSTUP, undefined);
  assert.equal(env.RUSTUP_HOME, undefined, "the account RUSTUP_HOME must not be replaced");
  assert.equal(env.RUSTUP_TOOLCHAIN, channel);
});

test("an incomplete account toolchain falls back to the isolated repair", async () => {
  const invocations = [];
  const env = { PINVOU3_MANAGED_RUSTUP: "1" };
  const result = await ensureWindowsRustToolchain({
    env,
    architecture: "x64",
    log: () => {},
    spawnChild: scriptedRustToolchainSpawn([2, 0], invocations),
  });
  assert.equal(result.source, "isolated");
  assert.equal(invocations.length, 2);
  assert.equal(invocations[0].args.at(-1), "-CheckOnly");
  assert.equal(
    invocations[0].options.env.PINVOU3_MANAGED_RUSTUP,
    undefined,
    "the read-only account probe must never run in managed (destructive) mode",
  );
  assert.ok(!invocations[1].args.includes("-CheckOnly"));
  assert.equal(invocations[1].options.env.PINVOU3_MANAGED_RUSTUP, "1");
  assert.ok(
    normalized(invocations[1].options.env.RUSTUP_HOME).endsWith(
      `/.cache/rustup/${channel}-x86_64-pc-windows-msvc`,
    ),
  );
});

test("unexpected probe or repair failures stop the build", async () => {
  await assert.rejects(
    ensureWindowsRustToolchain({
      env: {},
      architecture: "x64",
      log: () => {},
      spawnChild: scriptedRustToolchainSpawn([1], []),
    }),
    /Account Rust toolchain check failed with exit code 1/u,
  );
  await assert.rejects(
    ensureWindowsRustToolchain({
      env: {},
      architecture: "x64",
      log: () => {},
      spawnChild: scriptedRustToolchainSpawn([2, 1], []),
    }),
    /Isolated Rust toolchain repair failed with exit code 1/u,
  );
});

test("elapsed time is formatted for build heartbeats", () => {
  assert.equal(formatElapsed(0), "0s");
  assert.equal(formatElapsed(61_000), "1m 1s");
  assert.equal(formatElapsed(3_600_000), "1h 0m 0s");
  assert.equal(formatElapsed(-5), "0s");
});

test("the Windows build entry checks the toolchain before building", () => {
  assert.match(
    buildScript,
    /if \(hasTauriBuildCommand && process\.platform === "win32"\) \{\s*await ensureWindowsRustToolchain\(\);/u,
  );
  assert.ok(
    buildScript.indexOf("await ensureWindowsRustToolchain();") <
      buildScript.indexOf("? stageWindowsRuntime()"),
    "the toolchain must be ready before the runtime is staged and Tauri starts",
  );
});

test("the repair script only mutates an isolated, marked RUSTUP_HOME", () => {
  assert.match(rustToolchainGuard, /workspace-scoped RUSTUP_HOME/u);
  assert.match(rustToolchainGuard, /if \(\$CheckOnly\)/u);
  assert.match(rustToolchainGuard, /exit 2/u);
  assert.match(rustToolchainGuard, /PINVOU3_MANAGED_RUSTUP -ne "1"/u);
  assert.match(rustToolchainGuard, /\.pinvou3-managed-rustup/u);
  assert.match(rustToolchainGuard, /non-empty unmarked RUSTUP_HOME/u);
  assert.match(rustToolchainGuard, /\.pinvou3-toolchain\.lock/u);
  assert.match(rustToolchainGuard, /shared RUSTUP_HOME/u);
  assert.match(rustToolchainGuard, /filesystem root as RUSTUP_HOME/u);
  assert.match(rustToolchainGuard, /rust-toolchain\.toml/u);
  assert.doesNotMatch(rustToolchainGuard, /Get-Process|Stop-Process/u);
});

test("the repair script retries across download sources with bounded attempts", () => {
  assert.match(rustToolchainGuard, /"toolchain", "install"/u);
  assert.match(rustToolchainGuard, /"--profile", "minimal"/u);
  assert.match(rustToolchainGuard, /"toolchain", "uninstall"/u);
  assert.match(rustToolchainGuard, /RUSTUP_DOWNLOAD_TIMEOUT/u);
  assert.match(rustToolchainGuard, /function Invoke-Rustup/u);
  assert.match(rustToolchainGuard, /ErrorActionPreference = "Continue"/u);
  assert.match(rustToolchainGuard, /RUSTUP_DIST_SERVER/u);
  assert.match(rustToolchainGuard, /Name = "configured source"/u);
  assert.match(rustToolchainGuard, /https:\/\/static\.rust-lang\.org/u);
  assert.match(rustToolchainGuard, /https:\/\/rsproxy\.cn/u);
  assert.match(rustToolchainGuard, /mirrors\.tuna\.tsinghua\.edu\.cn\/rustup/u);
  assert.match(rustToolchainGuard, /InstallAttemptTimeoutSeconds = 600/u);
  assert.match(rustToolchainGuard, /RepairAttemptsPerSource = 2/u);
  assert.match(rustToolchainGuard, /Repair attempt \$attempt\/\$RepairAttemptsPerSource/u);
  assert.match(rustToolchainGuard, /WaitForExit\(\$TimeoutSeconds \* 1000\)/u);
  assert.match(rustToolchainGuard, /return 124/u);
  assert.match(rustToolchainGuard, /postInstallInvalid/u);
  for (const command of ["cargo", "rustc", "clippy-driver", "rustfmt"]) {
    assert.match(rustToolchainGuard, new RegExp(`"${command}"`, "u"));
  }
});

test("the native repair smoke corrupts only its own temporary toolchain", () => {
  assert.match(rustupRepairSmoke, /pinvou-rustup-repair-/u);
  assert.match(rustupRepairSmoke, /-CheckOnly/u);
  assert.match(rustupRepairSmoke, /127\.0\.0\.1:1/u);
  assert.match(rustupRepairSmoke, /-RepairAttemptsPerSource 1/u);
  assert.match(rustupRepairSmoke, /rust-toolchain\.toml/u);
  assert.match(rustupRepairSmoke, /\.pinvou3-managed-rustup/u);
  assert.match(rustupRepairSmoke, /Refusing to corrupt a path outside the isolated test root/u);
  assert.match(rustupRepairSmoke, /Refusing to remove an unexpected test path/u);
  assert.match(rustupRepairSmoke, /Isolated inconsistent-toolchain repair: PASS/u);
  assert.match(
    packageJson.scripts["test:windows-rustup-repair"],
    /tests\/windows_rustup_repair_smoke\.ps1/u,
  );
});

test("the Windows rustc stack wrapper is compiled once and then reused", (t) => {
  const scriptsPath = fs.mkdtempSync(path.join(os.tmpdir(), "pinvou-rustc-stack-wrapper-"));
  t.after(() => fs.rmSync(scriptsPath, { recursive: true, force: true }));
  const sourcePath = path.join(scriptsPath, "rustc-stack-wrapper.rs");
  const wrapperPath = path.join(scriptsPath, "rustc-stack-wrapper.exe");
  fs.writeFileSync(sourcePath, "fn main() {}\n");
  // Keep the source strictly older than the fake executable written below.
  const past = new Date(Date.now() - 60_000);
  fs.utimesSync(sourcePath, past, past);

  const compileInvocations = [];
  const environment = { RUSTUP_TOOLCHAIN: "fixture-toolchain" };
  const prepared = prepareWindowsRustcStackWrapper({
    environment,
    platform: "win32",
    scriptsPath,
    log: () => {},
    spawnCompiler: (command, args, options) => {
      compileInvocations.push({ command, args, options });
      fs.writeFileSync(wrapperPath, "fixture executable");
      return { status: 0 };
    },
  });
  assert.deepEqual(prepared, { path: wrapperPath, source: "compiled" });
  assert.equal(environment.RUSTC_WRAPPER, wrapperPath);
  assert.equal(compileInvocations.length, 1);
  assert.equal(compileInvocations[0].command, "rustc");
  assert.deepEqual(compileInvocations[0].args, ["-O", sourcePath, "-o", wrapperPath]);
  assert.equal(compileInvocations[0].options.env.RUSTUP_TOOLCHAIN, "fixture-toolchain");
  assert.equal(compileInvocations[0].options.windowsHide, true);

  const cachedEnvironment = {};
  const cached = prepareWindowsRustcStackWrapper({
    environment: cachedEnvironment,
    platform: "win32",
    scriptsPath,
    log: () => {},
    spawnCompiler: () => {
      throw new Error("a fresh wrapper must not be rebuilt");
    },
  });
  assert.deepEqual(cached, { path: wrapperPath, source: "cached" });
  assert.equal(cachedEnvironment.RUSTC_WRAPPER, wrapperPath);

  const future = new Date(Date.now() + 60_000);
  fs.utimesSync(sourcePath, future, future);
  let rebuilt = false;
  const stale = prepareWindowsRustcStackWrapper({
    environment: {},
    platform: "win32",
    scriptsPath,
    log: () => {},
    spawnCompiler: () => {
      rebuilt = true;
      fs.writeFileSync(wrapperPath, "rebuilt fixture executable");
      return { status: 0 };
    },
  });
  assert.equal(rebuilt, true, "a wrapper older than its source must be rebuilt");
  assert.equal(stale.source, "compiled");
});

test("an explicit RUSTC_WRAPPER wins and non-Windows hosts are untouched", () => {
  const configuredEnvironment = { RUSTC_WRAPPER: "  C:\\custom\\rustc-wrapper.exe  " };
  const configured = prepareWindowsRustcStackWrapper({
    environment: configuredEnvironment,
    platform: "win32",
    scriptsPath: path.join(os.tmpdir(), "pinvou-missing-wrapper-scripts"),
    log: () => {},
    spawnCompiler: () => {
      throw new Error("an explicit wrapper must not be rebuilt");
    },
  });
  assert.deepEqual(configured, {
    path: "C:\\custom\\rustc-wrapper.exe",
    source: "environment",
  });
  assert.equal(configuredEnvironment.RUSTC_WRAPPER, "C:\\custom\\rustc-wrapper.exe");

  for (const platform of ["linux", "darwin"]) {
    const environment = {};
    assert.equal(
      prepareWindowsRustcStackWrapper({ environment, platform, log: () => {} }),
      null,
    );
    assert.equal(environment.RUSTC_WRAPPER, undefined);
  }
});

test("a missing or failed wrapper build stops before Cargo can overflow", (t) => {
  const scriptsPath = fs.mkdtempSync(path.join(os.tmpdir(), "pinvou-rustc-stack-wrapper-"));
  t.after(() => fs.rmSync(scriptsPath, { recursive: true, force: true }));
  assert.throws(
    () => prepareWindowsRustcStackWrapper({
      environment: {},
      platform: "win32",
      scriptsPath,
      log: () => {},
    }),
    /Windows rustc stack wrapper source is missing/u,
  );

  fs.writeFileSync(path.join(scriptsPath, "rustc-stack-wrapper.rs"), "fn main() {}\n");
  const environment = {};
  assert.throws(
    () => prepareWindowsRustcStackWrapper({
      environment,
      platform: "win32",
      scriptsPath,
      log: () => {},
      spawnCompiler: () => ({ status: 1 }),
    }),
    /Failed to compile Windows rustc stack wrapper \(exit 1\)/u,
  );
  assert.equal(environment.RUSTC_WRAPPER, undefined);
  assert.throws(
    () => prepareWindowsRustcStackWrapper({
      environment: {},
      platform: "win32",
      scriptsPath,
      log: () => {},
      spawnCompiler: () => ({ error: new Error("rustc not found") }),
    }),
    /Failed to compile Windows rustc stack wrapper: rustc not found/u,
  );
});

test("the npm/Tauri entry injects the wrapper into the Tauri child environment", () => {
  assert.match(
    buildScript,
    /prepareWindowsRustcStackWrapper\(\{ environment: tauriEnvironment \}\);\s*process\.exitCode = await runTauri\(preparedArgs, \{ environment: tauriEnvironment \}\);/u,
    "dev and build must both go through the Windows compiler stack wrapper",
  );
});
