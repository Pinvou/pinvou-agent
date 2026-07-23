import assert from "node:assert/strict";
import { access, chmod, mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { spawnSync } from "node:child_process";
import { dirname, join } from "node:path";
import { tmpdir } from "node:os";
import { fileURLToPath } from "node:url";
import { test } from "node:test";

const repoRoot = dirname(dirname(dirname(fileURLToPath(import.meta.url))));
const deployScript = join(repoRoot, "scripts", "deploy-remote-relay.sh");

test("production deploy builds and replaces the complete shared WebUI dist", async () => {
  const source = await readFile(deployScript, "utf8");
  assert.match(source, /PINVOU_REMOTE_PUBLIC_BASE_PATH="\$BASE_PATH" npm run build:web/);
  assert.match(source, /tar -czf - -C "\$RELAY_DIR\/web\/dist" \./);
  assert.match(source, /deploy_output=.*<<'REMOTE'\r?\nset -Eeuo pipefail/);
  assert.match(source, /tar --no-same-owner --no-same-permissions -xzf "\$web_tmp" -C "\$web_stage"/);
  assert.match(source, /chown -R root:root "\$web_stage"/);
  assert.match(source, /find "\$web_stage" -type d -exec chmod 755 \{\} \+/);
  assert.match(source, /find "\$web_stage" -type f -exec chmod 644 \{\} \+/);
  assert.match(source, /cp -a "\$remote_dir\/web\/dist" "\$backup\/web-dist"/);
  assert.match(source, /mv "\$web_stage" "\$remote_dir\/web\/dist"/);
  assert.match(source, /PINVOU_CONFIRM_PRODUCTION_DEPLOY/);
  assert.doesNotMatch(source, /PINVOU_ALLOW_LEGACY_REMOTE_DEPLOY/);
  assert.doesNotMatch(source, /nginx -t|systemctl reload nginx/);
});

async function executable(path, content) {
  await writeFile(path, content);
  await chmod(path, 0o755);
}

async function pathExists(path) {
  try {
    await access(path);
    return true;
  } catch {
    return false;
  }
}

async function ensureMinimalWebDist(t) {
  const dist = join(repoRoot, "remote-control-relay", "web", "dist");
  const index = join(dist, "index.html");
  const bridge = join(dist, "tauri-bridge.js");
  const distExisted = await pathExists(dist);
  const createdFiles = [];

  await mkdir(dist, { recursive: true });
  if (!(await pathExists(index))) {
    await writeFile(index, [
      "<!doctype html>",
      '<base href="/pinvou3/remote/">',
      '<script src="/pinvou3/remote/tauri-bridge.js"></script>',
    ].join("\n"));
    createdFiles.push(index);
  }
  if (!(await pathExists(bridge))) {
    await writeFile(bridge, "");
    createdFiles.push(bridge);
  }

  t.after(async () => {
    if (!distExisted) {
      await rm(dist, { recursive: true, force: true });
      return;
    }
    await Promise.all(createdFiles.map((path) => rm(path, { force: true })));
  });
}

test("deploy script rolls back when post-deploy public verification fails", {
  skip: process.platform === "win32" ? "deployment script targets a POSIX host" : false,
}, async (t) => {
  await ensureMinimalWebDist(t);
  const root = await mkdtemp(join(tmpdir(), "pinvou-relay-deploy-test-"));
  t.after(() => rm(root, { recursive: true, force: true }));
  const bin = join(root, "bin");
  const log = join(root, "calls.log");
  await mkdir(bin);

  await executable(join(bin, "node"), "#!/usr/bin/env bash\nexit 0\n");
  await executable(join(bin, "npm"), "#!/usr/bin/env bash\nexit 0\n");
  await executable(join(bin, "sha256sum"), "#!/usr/bin/env bash\nexit 0\n");
  await executable(join(bin, "scp"), `#!/usr/bin/env bash\necho scp >> "${log}"\n`);
  await executable(join(bin, "ssh"), `#!/usr/bin/env bash
cat >/dev/null
if [[ "$*" == *"cat > '/tmp/pinvou-remote-web-"* ]]; then
  echo upload >> "${log}"
elif (( $# >= 10 )); then
  echo deploy >> "${log}"
  echo 'backup=/opt/pinvou-remote-relay/backups/fake'
else
  echo rollback >> "${log}"
  echo '已恢复备份'
fi
`);
  await executable(join(bin, "curl"), `#!/usr/bin/env bash
output=""
previous=""
for arg in "$@"; do
  if [[ "$previous" == "-o" ]]; then output="$arg"; fi
  previous="$arg"
  url="$arg"
done
if [[ -n "$output" ]]; then
  : > "$output"
  exit 0
fi
case "$url" in
  http://direct.invalid/*) exit 22 ;;
  */healthz) printf '%s' '{"ok":true,"room_count":0}' ;;
  */r/deploy-check) printf '%s' '<!doctype html><title>PINVOU Remote</title>' ;;
  *) exit 22 ;;
esac
`);

  const result = spawnSync("bash", [deployScript], {
    cwd: repoRoot,
    env: {
      ...process.env,
      PATH: `${bin}:${process.env.PATH}`,
      XDG_CACHE_HOME: join(root, "cache"),
      PINVOU_REMOTE_PUBLIC_URL: "https://public.invalid/pinvou3/remote",
      PINVOU_REMOTE_DIRECT_URL: "http://direct.invalid/pinvou3/remote",
      PINVOU_CONFIRM_PRODUCTION_DEPLOY: "1",
      SKIP_WEB_BUILD: "1",
      SKIP_LOCAL_TESTS: "1",
    },
    encoding: "utf8",
  });
  const output = `${result.stdout}\n${result.stderr}`;
  const calls = await readFile(log, "utf8");

  assert.equal(result.status, 1);
  assert.match(output, /部署后检查失败，开始回滚/);
  assert.match(output, /部署失败，已恢复并验证上一线上版本/);
  assert.match(calls, /deploy/);
  assert.match(calls, /rollback/);
  assert.deepEqual(
    calls.split(/\r?\n/).filter((entry) => ["upload", "deploy", "rollback"].includes(entry)),
    ["upload", "deploy", "rollback"],
  );
});
