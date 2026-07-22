import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { test } from "node:test";

const repoRoot = dirname(dirname(dirname(fileURLToPath(import.meta.url))));
const deployScript = join(repoRoot, "scripts", "deploy-remote-relay-test.sh");
const posixOnly = {
  skip: process.platform === "win32" ? "deployment script targets a POSIX host" : false,
};

function validate(overrides = {}) {
  return spawnSync("bash", [deployScript, "--validate-only"], {
    cwd: repoRoot,
    env: { ...process.env, ...overrides },
    encoding: "utf8",
  });
}

test("remote-test deploy validation accepts the registered safe layout", posixOnly, () => {
  const syntax = spawnSync("bash", ["-n", deployScript], { encoding: "utf8" });
  assert.equal(syntax.status, 0, syntax.stderr);
  const result = validate();
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /配置校验通过/);
});

test("remote-test deploy builds and uploads the shared WebUI dist", () => {
  const source = readFileSync(deployScript, "utf8");
  assert.match(source, /PINVOU_REMOTE_PUBLIC_BASE_PATH="\$BASE_PATH" npm run build:web/);
  assert.match(source, /grep -Fq "\$BASE_PATH\/" "\$RELAY_DIR\/web\/dist\/index\.html"/);
  assert.match(source, /test -f "\$RELAY_DIR\/web\/dist\/tauri-bridge\.js"/);
  assert.match(source, /package-lock\.json web\/dist web\/stats\.html/);
});

test("remote-test tunnel account is restricted to the registered remote forward", () => {
  const source = readFileSync(deployScript, "utf8");
  assert.match(source, /Match User %s/);
  assert.match(source, /AllowTcpForwarding remote/);
  assert.match(source, /PermitListen 127\.0\.0\.1:%s/);
  assert.match(source, /BEGIN PINVOU3 REMOTE TEST TUNNEL/);
  assert.match(source, /sshd -t/);
});

test("remote-test deploy validation rejects TEST_DIR traversal and aliases", posixOnly, () => {
  for (const testDir of [
    "/opt/pinvou-remote-relay-test/..",
    "/opt/pinvou-remote-relay-test/sub/../..",
    "/opt/pinvou-remote-relay-test-child",
    "/opt/pinvou-remote-relay-test/subdir",
  ]) {
    const result = validate({ TEST_DIR: testDir });
    assert.notEqual(result.status, 0, `must reject TEST_DIR=${testDir}`);
    assert.match(result.stderr, /TEST_DIR=.*必须精确等于/);
  }
});

test("remote-test deploy validation rejects BASE_PATH dot segments", posixOnly, () => {
  for (const basePath of [
    "/pinvou3/remote-test/../remote",
    "/pinvou3/remote-test/./child",
  ]) {
    const result = validate({
      BASE_PATH: basePath,
      PUBLIC_URL: `https://pinvou.com${basePath}`,
    });
    assert.notEqual(result.status, 0, `must reject BASE_PATH=${basePath}`);
    assert.match(result.stderr, /不允许包含 \. 或 \.\./);
  }
});
