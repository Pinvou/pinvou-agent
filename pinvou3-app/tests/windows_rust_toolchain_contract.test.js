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

  const absoluteEnv = {
    PINVOU3_RUSTUP_HOME: path.join(os.tmpdir(), "pinvou-absolute-rustup"),
  };
  const absolute = configureIsolatedWindowsRustToolchain({
    env: absoluteEnv,
    architecture: "x64",
  });
  assert.equal(
    absolute.rustupHome,
    path.join(os.tmpdir(), "pinvou-absolute-rustup"),
    "an absolute override must not be nested under the app root",
  );
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

test("a toolchain check that fails to spawn stops the build", async () => {
  const failingSpawn = () => {
    const child = new EventEmitter();
    queueMicrotask(() =>
      child.emit("error", new Error("spawn powershell.exe ENOENT")),
    );
    return child;
  };
  await assert.rejects(
    ensureWindowsRustToolchain({
      env: {},
      architecture: "x64",
      log: () => {},
      spawnChild: failingSpawn,
    }),
    /spawn powershell.exe ENOENT/u,
  );
});

test("a signalled toolchain check stops the build", async () => {
  const signallingSpawn = () => {
    const child = new EventEmitter();
    queueMicrotask(() => child.emit("exit", null, "SIGTERM"));
    return child;
  };
  await assert.rejects(
    ensureWindowsRustToolchain({
      env: {},
      architecture: "x64",
      log: () => {},
      spawnChild: signallingSpawn,
    }),
    /stopped by signal: SIGTERM/u,
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
    /if \(process\.platform === "win32" && \(hasTauriBuildCommand \|\| isDev\)\) \{\s*await ensureWindowsRustToolchain\(\);/u,
    "dev compiles with cargo too, so the toolchain check must cover dev, not only build/bundle",
  );
  assert.ok(
    buildScript.indexOf("await ensureWindowsRustToolchain();") <
      buildScript.indexOf("? stageWindowsRuntime()"),
    "the toolchain must be ready before the runtime is staged and Tauri starts",
  );
});

test("the repair script only mutates an isolated, marked RUSTUP_HOME", () => {
  // Workspace scoping itself is pinned functionally against build.js in the
  // "isolated RUSTUP_HOME is workspace scoped" test above; here the guard
  // script's own decisions are pinned.
  assert.match(rustToolchainGuard, /if \(\$CheckOnly\)/u);
  assert.match(rustToolchainGuard, /exit 2/u);
  assert.match(rustToolchainGuard, /PINVOU3_MANAGED_RUSTUP -ne "1"/u);
  assert.match(rustToolchainGuard, /\.pinvou3-managed-rustup/u);
  // The marker value must be verified, not just the filename: a torn write
  // must be rejected instead of adopted for destructive repair.
  assert.match(rustToolchainGuard, /pinvou3-managed-rustup-v1/u);
  assert.match(rustToolchainGuard, /Invalid managed RUSTUP_HOME marker/u);
  assert.match(rustToolchainGuard, /non-empty unmarked RUSTUP_HOME/u);
  // A repair with RUSTUP_HOME unset must refuse before any path resolution:
  // [IO.Path]::GetFullPath("") would otherwise resolve to the process CWD.
  assert.match(
    rustToolchainGuard,
    /Refusing automatic repair without an isolated RUSTUP_HOME/u,
  );
  assert.match(rustToolchainGuard, /\.pinvou3-toolchain\.lock/u);
  assert.match(rustToolchainGuard, /shared RUSTUP_HOME/u);
  assert.match(rustToolchainGuard, /filesystem root as RUSTUP_HOME/u);
  assert.match(rustToolchainGuard, /rust-toolchain\.toml/u);
  assert.doesNotMatch(rustToolchainGuard, /Get-Process|Stop-Process/u);
  // The adopt-or-refuse decision must run under the managed-home lock, so
  // two concurrent first-time builds cannot both adopt the same home.
  assert.ok(
    rustToolchainGuard.indexOf("[IO.FileShare]::None") <
      rustToolchainGuard.indexOf('".pinvou3-managed-rustup"'),
    "the marker adopt-or-refuse decision must be serialized by the lock",
  );
});

test("the toolchain probe covers rust-std, not just the binaries", () => {
  assert.match(rustToolchainGuard, /function Test-RustStdTargetLib/u);
  assert.match(rustToolchainGuard, /--print target-libdir/u);
  // Every completeness gate must consult the std check: the read-only
  // account probe, the initial isolated probe, the post-install success
  // gate and the final verification.
  assert.equal(
    (rustToolchainGuard.match(/Test-RustStdTargetLib -RustupPath/gu) || []).length,
    4,
    "rust-std must be probed at every completeness gate",
  );
  assert.match(rustToolchainGuard, /invalidAccountCommands \+= "rust-std"/u);
  assert.match(rustToolchainGuard, /invalidEntries \+= "rust-std"/u);
  // cargo clippy / cargo fmt shell out to the cargo-clippy/cargo-fmt shims,
  // so both the account probe and the isolated repair gate must cover them.
  const probeList = '"cargo", "rustc", "clippy-driver", "rustfmt", "cargo-clippy", "cargo-fmt"';
  assert.match(
    rustToolchainGuard,
    new RegExp(`\\$requiredCommands = @\\(\\s*${probeList}`, "u"),
  );
  assert.match(
    rustToolchainGuard,
    new RegExp(`foreach \\(\\$command in @\\(\\s*${probeList}`, "u"),
  );
});

test("the repair is bounded overall and survives a stalled kill", () => {
  assert.match(rustToolchainGuard, /RepairTimeoutSeconds = 1500/u);
  assert.match(rustToolchainGuard, /exceeded its \{0\}s budget/u);
  assert.match(rustToolchainGuard, /\[ValidateRange\(1, 7200\)\]/u);
  // The bounded ranges must stay attached to their parameters: without them
  // a 0/negative timeout makes WaitForExit(0) degenerate into an instant
  // timeout per attempt.
  assert.match(
    rustToolchainGuard,
    /\[ValidateRange\(1, 3600\)\]\s*\r?\n\s*\[int\]\$InstallAttemptTimeoutSeconds/u,
  );
  assert.match(
    rustToolchainGuard,
    /\[ValidateRange\(1, 5\)\]\s*\r?\n\s*\[int\]\$RepairAttemptsPerSource/u,
  );
  // The derived lock default is an expression; the range must stay attached
  // so a caller-passed 0 cannot degenerate into an instant, misleading lock
  // timeout while an unrelated repair holds the lock.
  assert.match(
    rustToolchainGuard,
    /\[ValidateRange\(1, 7200\)\]\s*\r?\n\s*\[int\]\$LockTimeoutSeconds/u,
  );
  // The lock wait must out-wait the holder's legitimate worst case (the
  // overall repair budget plus one more bounded install attempt and slack),
  // so a second build queues behind a live repair instead of failing
  // spuriously mid-repair.
  assert.match(
    rustToolchainGuard,
    /LockTimeoutSeconds = \(\s*\$RepairTimeoutSeconds \+ \$InstallAttemptTimeoutSeconds \+ 300\s*\)/u,
  );
  // A rustup kill race (InvalidOperationException, Win32Exception) must count
  // as a failed attempt and must never hang on an unbounded wait or stream
  // read afterwards.
  assert.match(rustToolchainGuard, /Win32Exception/u);
  assert.match(rustToolchainGuard, /WaitForExit\(5000\)/u);
  assert.match(rustToolchainGuard, /\$stdoutTask\.Wait\(10000\)/u);
});

test("fallback sources stay mirror-first with pinned endpoints", () => {
  // Mirror-first per the #619 runtime-download convention: the repair engine
  // exists for machines where the official source stalls. The endpoints are
  // pinned exactly — rsproxy and USTC serve dist manifests and the rustup
  // update root for rolling and version-pinned channels alike, while TUNA
  // 404s version-pinned manifests, and a moved or mistyped mirror URL must
  // not silently strand the fallback chain.
  assert.match(rustToolchainGuard, /DistServer = "https:\/\/rsproxy\.cn"/u);
  assert.match(rustToolchainGuard, /UpdateRoot = "https:\/\/rsproxy\.cn\/rustup"/u);
  assert.match(rustToolchainGuard, /DistServer = "https:\/\/mirrors\.ustc\.edu\.cn\/rust-static"/u);
  assert.match(rustToolchainGuard, /UpdateRoot = "https:\/\/mirrors\.ustc\.edu\.cn\/rust-static\/rustup"/u);
  assert.match(rustToolchainGuard, /DistServer = "https:\/\/static\.rust-lang\.org"/u);
  assert.match(rustToolchainGuard, /UpdateRoot = "https:\/\/static\.rust-lang\.org\/rustup"/u);
  const fallbackOrder = [
    rustToolchainGuard.indexOf('DistServer = "https://rsproxy.cn"'),
    rustToolchainGuard.indexOf('DistServer = "https://mirrors.ustc.edu.cn/rust-static"'),
    rustToolchainGuard.indexOf('DistServer = "https://static.rust-lang.org"'),
  ];
  assert.ok(
    fallbackOrder.every((index) => index >= 0) &&
      fallbackOrder[0] < fallbackOrder[1] && fallbackOrder[1] < fallbackOrder[2],
    "fallback order must be mirror-first: rsproxy, USTC, then the official source",
  );
  assert.ok(
    rustToolchainGuard.indexOf('Name = "configured source"') < fallbackOrder[0],
    "a configured RUSTUP_DIST_SERVER must keep priority over the built-in fallbacks",
  );
});

test("the repair installs every component pinned in rust-toolchain.toml", () => {
  const componentsMatch = toolchainConfig.match(/^\s*components\s*=\s*\[([^\]]*)\]/mu);
  const pinned = componentsMatch
    ? [...componentsMatch[1].matchAll(/"([^"]+)"/gu)].map((entry) => entry[1])
    : [];
  assert.ok(
    pinned.includes("clippy") && pinned.includes("rustfmt"),
    "fixture expectation: rust-toolchain.toml pins clippy and rustfmt",
  );
  for (const component of pinned) {
    assert.match(
      rustToolchainGuard,
      new RegExp(`"--component", "${component}"`, "u"),
      `the repair must install the pinned ${component} component`,
    );
  }
});

test("the repair script retries across download sources with bounded attempts", () => {
  // The channel must be read from rust-toolchain.toml at both entry points
  // (read-only account probe and destructive repair), never hardcoded: the
  // channel in the toml is the single source of truth and currently floats
  // with stable.
  const channelProbe = '\'(?m)^\\s*channel\\s*=\\s*"([^"]+)"\'';
  assert.ok(
    (rustToolchainGuard.split(channelProbe).length - 1) >= 2,
    "both -CheckOnly and the repair path must extract the channel from rust-toolchain.toml",
  );
  assert.ok(
    (rustupRepairSmoke.split(channelProbe).length - 1) >= 1,
    "the smoke must resolve the channel from rust-toolchain.toml too",
  );
  assert.match(rustToolchainGuard, /"toolchain", "install"/u);
  assert.match(rustToolchainGuard, /"--profile", "minimal"/u);
  assert.match(rustToolchainGuard, /"toolchain", "uninstall"/u);
  // The uninstall reset must be bounded like the install: it is the one
  // rustup call over ~1 GB of files, and an unbounded reset outlasts the
  // repair budget and the waiters' derived lock deadline.
  assert.match(
    rustToolchainGuard,
    /"toolchain", "uninstall", \$toolchain\s*\)\s*-TimeoutSeconds \$InstallAttemptTimeoutSeconds/u,
  );
  assert.match(rustToolchainGuard, /RUSTUP_DOWNLOAD_TIMEOUT/u);
  // The bounded download timeout must have an actual value, not just a name.
  assert.match(rustToolchainGuard, /RUSTUP_DOWNLOAD_TIMEOUT = "600"/u);
  assert.match(rustToolchainGuard, /function Invoke-Rustup/u);
  assert.match(rustToolchainGuard, /ErrorActionPreference = "Continue"/u);
  assert.match(rustToolchainGuard, /RUSTUP_DIST_SERVER/u);
  assert.match(rustToolchainGuard, /Name = "configured source"/u);
  assert.match(rustToolchainGuard, /https:\/\/static\.rust-lang\.org/u);
  assert.match(rustToolchainGuard, /https:\/\/rsproxy\.cn/u);
  assert.match(rustToolchainGuard, /mirrors\.ustc\.edu\.cn\/rust-static/u);
  assert.match(rustToolchainGuard, /InstallAttemptTimeoutSeconds = 600/u);
  assert.match(rustToolchainGuard, /RepairAttemptsPerSource = 2/u);
  assert.match(rustToolchainGuard, /Repair attempt \$attempt\/\$RepairAttemptsPerSource/u);
  assert.match(rustToolchainGuard, /WaitForExit\(\$TimeoutSeconds \* 1000\)/u);
  assert.match(rustToolchainGuard, /return 124/u);
  // The Kill() race catch must stay: a rustup exiting inside the kill window
  // must count as a failed attempt, not abort the whole repair.
  assert.match(
    rustToolchainGuard,
    /catch \[System\.InvalidOperationException\], \[System\.ComponentModel\.Win32Exception\]/u,
  );
  // A wedged reset or exhausted repair must tell the operator the safe way
  // out instead of repeating an opaque failure.
  assert.match(rustToolchainGuard, /safe to delete the isolated RUSTUP_HOME/u);
  assert.match(rustToolchainGuard, /postInstallInvalid/u);
  for (const command of [
    "cargo", "rustc", "clippy-driver", "rustfmt", "cargo-clippy", "cargo-fmt",
  ]) {
    assert.match(rustToolchainGuard, new RegExp(`"${command}"`, "u"));
  }
});

test("the native repair smoke corrupts only its own temporary toolchain", () => {
  // The smoke must invoke the real repair script, not a vendored copy that
  // can silently drift from scripts/ci/ensure-rust-toolchain.ps1.
  assert.match(rustupRepairSmoke, /\.\.\\scripts\\ci\\ensure-rust-toolchain\.ps1/u);
  assert.match(rustupRepairSmoke, /pinvou-rustup-repair-/u);
  assert.match(rustupRepairSmoke, /-CheckOnly/u);
  assert.match(rustupRepairSmoke, /127\.0\.0\.1:1/u);
  assert.match(rustupRepairSmoke, /-RepairAttemptsPerSource 1/u);
  assert.match(rustupRepairSmoke, /rust-toolchain\.toml/u);
  assert.match(rustupRepairSmoke, /\.pinvou3-managed-rustup/u);
  assert.match(rustupRepairSmoke, /Refusing to corrupt a path outside the isolated test root/u);
  assert.match(rustupRepairSmoke, /Refusing to remove an unexpected test path/u);
  assert.match(rustupRepairSmoke, /Isolated inconsistent-toolchain repair: PASS/u);
  // The smoke must exercise the refusal guards for real, not just the repair.
  assert.match(
    rustupRepairSmoke,
    /Refusing to modify the build account's shared RUSTUP_HOME/u,
  );
  assert.match(rustupRepairSmoke, /Refusing to use a filesystem root as RUSTUP_HOME/u);
  assert.match(rustupRepairSmoke, /Refusing to adopt a non-empty unmarked RUSTUP_HOME/u);
  assert.match(rustupRepairSmoke, /Refusing automatic repair without an isolated RUSTUP_HOME/u);
  // The fifth guard (destructive mode without the managed flag) must be
  // exercised live too, not just text-pinned against the condition.
  assert.match(rustupRepairSmoke, /Managed = \$null/u);
  assert.match(rustupRepairSmoke, /PINVOU3_MANAGED_RUSTUP -ErrorAction SilentlyContinue/u);
  // The guard verdict must actually compare against the expected message: a
  // neutered comparison would pass vacuously while every pinned string stays.
  assert.match(
    rustupRepairSmoke,
    /-not \$rejectionMessage\.Contains\(\$guard\.Message\)/u,
  );
  assert.match(rustupRepairSmoke, /Guard rejected/u);
  // The incomplete -CheckOnly branch must execute for real twice: once over
  // the corrupted binaries, once over the removed rust-std target libdir.
  assert.equal(
    (rustupRepairSmoke.match(/-ne 2/gu) || []).length,
    2,
    "the smoke must require exit 2 from both real -CheckOnly corruption probes",
  );
  assert.match(rustupRepairSmoke, /A missing rust-std was not detected/u);
  assert.match(rustupRepairSmoke, /InstallAttemptTimeoutSeconds 300/u);
  // Every environment variable the smoke mutates must be restored on both
  // the pass and fail paths; pinning only one entry let the others regress.
  for (const name of [
    "RUSTUP_HOME",
    "CARGO_HOME",
    "PINVOU3_MANAGED_RUSTUP",
    "RUSTUP_DIST_SERVER",
    "RUSTUP_UPDATE_ROOT",
    "RUSTUP_DOWNLOAD_TIMEOUT",
  ]) {
    assert.match(
      rustupRepairSmoke,
      new RegExp(`@\\("${name}", \\$previous`, "u"),
      `the smoke must restore ${name} even when it fails`,
    );
  }
  assert.match(
    rustupRepairSmoke,
    /RUSTUP_DOWNLOAD_TIMEOUT", \$previousDownloadTimeout/u,
    "mutated environment must be restored even when the smoke fails",
  );
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
      fs.writeFileSync(args[3], "fixture executable");
      return { status: 0 };
    },
  });
  assert.deepEqual(prepared, { path: wrapperPath, source: "compiled" });
  assert.equal(environment.RUSTC_WRAPPER, wrapperPath);
  assert.equal(compileInvocations.length, 1);
  assert.equal(compileInvocations[0].command, "rustc");
  assert.equal(compileInvocations[0].args[0], "-O");
  assert.equal(compileInvocations[0].args[1], sourcePath);
  assert.equal(compileInvocations[0].args[2], "-o");
  assert.match(
    compileInvocations[0].args[3],
    /rustc-stack-wrapper\.exe\.\d+\.tmp$/u,
    "the wrapper must be compiled to a unique temporary name",
  );
  assert.equal(compileInvocations[0].options.env.RUSTUP_TOOLCHAIN, "fixture-toolchain");
  assert.equal(compileInvocations[0].options.windowsHide, true);
  assert.ok(fs.existsSync(wrapperPath), "the compiled wrapper must be renamed into place");
  assert.ok(
    !fs.existsSync(compileInvocations[0].args[3]),
    "the temporary wrapper must not survive a successful compile",
  );

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
    spawnCompiler: (command, args) => {
      rebuilt = true;
      fs.writeFileSync(args[3], "rebuilt fixture executable");
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

  // A failed rebuild must leave an existing wrapper untouched: the compile
  // goes to a temporary name and only a successful build is renamed into
  // place, so an interrupted rebuild cannot poison the mtime cache.
  fs.writeFileSync(path.join(scriptsPath, "rustc-stack-wrapper.exe"), "previous wrapper");
  const newer = new Date(Date.now() + 120_000);
  fs.utimesSync(path.join(scriptsPath, "rustc-stack-wrapper.rs"), newer, newer);
  assert.throws(
    () => prepareWindowsRustcStackWrapper({
      environment: {},
      platform: "win32",
      scriptsPath,
      log: () => {},
      spawnCompiler: (command, args) => {
        fs.writeFileSync(args[3], "partial executable");
        return { status: 1 };
      },
    }),
    /Failed to compile Windows rustc stack wrapper \(exit 1\)/u,
  );
  assert.equal(
    fs.readFileSync(path.join(scriptsPath, "rustc-stack-wrapper.exe"), "utf8"),
    "previous wrapper",
    "a failed rebuild must not clobber the existing wrapper",
  );
  assert.equal(
    fs.readdirSync(scriptsPath).filter((entry) => entry.endsWith(".tmp")).length,
    0,
    "a failed rebuild must clean up its temporary output",
  );
});

test("the npm/Tauri entry injects the wrapper into the Tauri child environment", () => {
  assert.match(
    buildScript,
    /prepareWindowsRustcStackWrapper\(\{ environment: tauriEnvironment \}\);\s*process\.exitCode = await runTauri\(preparedArgs, \{ environment: tauriEnvironment \}\);/u,
    "dev and build must both go through the Windows compiler stack wrapper",
  );
});
