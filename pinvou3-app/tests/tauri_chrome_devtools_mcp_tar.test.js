// Regression contract for the Windows NSIS release build: PATH order on Windows CI
// resolves `tar` to Git for Windows' GNU tar, which reads the drive letter of
// `D:\...` as an rsh remote host and fails with "Cannot connect to D: resolve
// failed". The chrome-devtools-mcp vendor step must therefore extract through the
// System32 bsdtar instead of a bare PATH-resolved `tar`.
const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

const {
  systemTarCommand,
  tarExtractionArguments,
} = require("../scripts/tauri/chrome-devtools-mcp.js");

assert.equal(systemTarCommand({ platform: "darwin" }), "tar");
assert.equal(systemTarCommand({ platform: "linux" }), "tar");

const previousSystemRoot = process.env.SystemRoot;
const windowsRoot = fs.mkdtempSync(path.join(os.tmpdir(), "pinvou-tar-probe-"));
try {
  const system32 = path.join(windowsRoot, "System32");
  fs.mkdirSync(system32);
  const bsdtar = path.join(system32, "tar.exe");
  fs.writeFileSync(bsdtar, "");
  assert.equal(
    systemTarCommand({ platform: "win32", systemRoot: windowsRoot }),
    bsdtar,
    "Windows must extract with the System32 bsdtar, not a PATH-resolved GNU tar",
  );

  process.env.SystemRoot = windowsRoot;
  assert.equal(
    systemTarCommand({ platform: "win32" }),
    bsdtar,
    "without an explicit systemRoot the command must come from the SystemRoot environment",
  );

  fs.rmSync(bsdtar);
  assert.equal(
    systemTarCommand({ platform: "win32" }),
    "tar",
    "without System32 tar.exe the command falls back to PATH resolution",
  );

  assert.equal(
    systemTarCommand({ platform: "win32", systemRoot: path.join(windowsRoot, "Missing") }),
    "tar",
    "a non-existent SystemRoot must fall back to PATH resolution",
  );
} finally {
  if (previousSystemRoot === undefined) delete process.env.SystemRoot;
  else process.env.SystemRoot = previousSystemRoot;
  fs.rmSync(windowsRoot, { recursive: true, force: true });
}

assert.deepEqual(
  tarExtractionArguments(
    String.raw`D:\jenkins\workspace\target\chrome-devtools-mcp-1.7.0.tgz`,
    String.raw`D:\jenkins\workspace\target`,
    { pathApi: path.win32 },
  ),
  ["-xzf", "chrome-devtools-mcp-1.7.0.tgz", "-C", "."],
  "the archive argument must not expose a Windows drive letter to GNU tar",
);
assert.throws(
  () => {
    return tarExtractionArguments(
      String.raw`D:\jenkins\workspace\outside.tgz`,
      String.raw`D:\jenkins\workspace\target`,
      { pathApi: path.win32 },
    );
  },
  /must stay inside its staging directory/,
  "the relative-path workaround must not allow archives outside staging",
);

// The extraction call site must go through systemTarCommand(); a bare "tar" would
// reintroduce the drive-letter failure whenever PATH resolves to GNU tar first.
const source = fs.readFileSync(
  path.join(__dirname, "..", "scripts", "tauri", "chrome-devtools-mcp.js"),
  "utf8",
);
assert.match(
  source,
  /run\(systemTarCommand\(\), tarExtractionArguments\(tarball, stagingRoot\)/,
  "the tarball extraction must resolve tar through System32 and use staging-relative arguments",
);
assert.doesNotMatch(
  source,
  /run\("tar",\s*\[/,
  "tar must not be spawned as a bare PATH-resolved command on any platform",
);
