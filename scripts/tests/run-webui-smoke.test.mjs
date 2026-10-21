import test from "node:test";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import {
  mkdtempSync,
  mkdirSync,
  writeFileSync,
  chmodSync,
  readFileSync,
  rmSync,
} from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = path.resolve(HERE, "..", "..");
const WRAPPER = path.join(REPO_ROOT, "pinvou3-app", "scripts", "run-webui-smoke.mjs");

// PR #745 follow-up: the wrapper is load-bearing for CI's dist reuse
// (PINVOU_WEBUI_SKIP_BUILD=1) and for the user-journey SKIP contract (the
// relay smoke's exit-2 must pass through untouched). Nothing executed it —
// these cover build gating, ordering, and exit-code passthrough against a
// stub smoke and a fake `npm` on PATH. The fixtures are POSIX shell scripts,
// so the suite skips on Windows like the other PATH-stubbing tests.

const POSIX_ONLY = process.platform === "win32" ? "posix-only fixtures" : false;

function makeSandbox(t) {
  const dir = mkdtempSync(path.join(tmpdir(), "webui-wrapper-"));
  const log = path.join(dir, "calls.log");

  const smoke = path.join(dir, "stub-smoke.mjs");
  writeFileSync(
    smoke,
    [
      "import { appendFileSync } from 'node:fs';",
      "appendFileSync(process.env.STUB_LOG, 'smoke-ran\\n');",
      "process.exit(Number(process.env.STUB_EXIT ?? '0'));",
      "",
    ].join("\n"),
  );

  const fakeBin = path.join(dir, "fake-bin");
  mkdirSync(fakeBin);
  const npm = path.join(fakeBin, "npm");
  writeFileSync(
    npm,
    [
      "#!/bin/sh",
      "printf '%s\\n' \"$*\" >> \"$STUB_LOG\"",
      "exit \"${FAKE_NPM_EXIT:-0}\"",
      "",
    ].join("\n"),
  );
  chmodSync(npm, 0o755);

  t.after(() => rmSync(dir, { recursive: true, force: true }));
  return { smoke, fakeBin, log };
}

function calls(log) {
  return readFileSync(log, "utf8").split("\n").filter(Boolean);
}

function runWrapper({ smoke, fakeBin, log, skipBuild, stubExit, fakeNpmExit }) {
  const env = { ...process.env, STUB_LOG: log };
  delete env.PINVOU_WEBUI_SKIP_BUILD;
  if (skipBuild !== undefined) env.PINVOU_WEBUI_SKIP_BUILD = skipBuild;
  if (stubExit !== undefined) env.STUB_EXIT = String(stubExit);
  if (fakeNpmExit !== undefined) env.FAKE_NPM_EXIT = String(fakeNpmExit);
  if (fakeBin) env.PATH = `${fakeBin}${path.delimiter}${env.PATH ?? ""}`;
  return spawnSync(process.execPath, [WRAPPER, smoke], { env });
}

test(
  "SKIP=1 reuses the dist: no build call, smoke exit code passes through",
  { skip: POSIX_ONLY },
  (t) => {
    const { smoke, fakeBin, log } = makeSandbox(t);
    for (const stubExit of [0, 2, 5]) {
      writeFileSync(log, "");
      const run = runWrapper({ smoke, fakeBin, log, skipBuild: "1", stubExit });
      assert.equal(run.status, stubExit, `stub exit ${stubExit} must pass through`);
      assert.deepEqual(calls(log), ["smoke-ran"], "the build must not run under SKIP=1");
    }
  },
);

test(
  "without SKIP it runs build:web before the smoke",
  { skip: POSIX_ONLY },
  (t) => {
    const { smoke, fakeBin, log } = makeSandbox(t);
    const run = runWrapper({ smoke, fakeBin, log });
    assert.equal(run.status, 0);
    assert.deepEqual(calls(log), ["run build:web", "smoke-ran"]);
  },
);

test(
  "a failed build exits with its status and never runs the smoke",
  { skip: POSIX_ONLY },
  (t) => {
    const { smoke, fakeBin, log } = makeSandbox(t);
    const run = runWrapper({ smoke, fakeBin, log, fakeNpmExit: 3 });
    assert.equal(run.status, 3);
    assert.deepEqual(calls(log), ["run build:web"]);
  },
);
