import assert from "node:assert/strict";
import { chmod, mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { spawnSync } from "node:child_process";
import { dirname, join } from "node:path";
import { tmpdir } from "node:os";
import { fileURLToPath } from "node:url";
import { test } from "node:test";

const repoRoot = dirname(dirname(dirname(fileURLToPath(import.meta.url))));
const deployScript = join(repoRoot, "scripts", "deploy-remote-relay.sh");

test("legacy rollback targets the Relay's served web/dist entry", async () => {
  const source = await readFile(deployScript, "utf8");
  assert.match(source, /remote_dir\/web\/dist\/index\.html/);
  assert.doesNotMatch(source, /remote_dir\/web\/index\.html/);
});

async function executable(path, content) {
  await writeFile(path, content);
  await chmod(path, 0o755);
}

test("deploy script rolls back when post-deploy public verification fails", {
  skip: process.platform === "win32" ? "deployment script targets a POSIX host" : false,
}, async () => {
  const root = await mkdtemp(join(tmpdir(), "pinvou-relay-deploy-test-"));
  const bin = join(root, "bin");
  const log = join(root, "calls.log");
  await mkdir(bin);

  await executable(join(bin, "node"), "#!/usr/bin/env bash\nexit 0\n");
  await executable(join(bin, "npm"), "#!/usr/bin/env bash\nexit 0\n");
  await executable(join(bin, "sha256sum"), "#!/usr/bin/env bash\nexit 0\n");
  await executable(join(bin, "scp"), `#!/usr/bin/env bash\necho scp >> "${log}"\n`);
  await executable(join(bin, "ssh"), `#!/usr/bin/env bash
cat >/dev/null
if (( $# >= 10 )); then
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
      PINVOU_ALLOW_LEGACY_REMOTE_DEPLOY: "1",
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

  await rm(root, { recursive: true, force: true });
});
