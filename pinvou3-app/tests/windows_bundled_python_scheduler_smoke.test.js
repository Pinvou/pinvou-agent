const assert = require("assert");
const fs = require("fs");
const os = require("os");
const path = require("path");
const { spawnSync } = require("child_process");

if (process.platform !== "win32") {
  console.log("SKIP: Windows bundled Python scheduler smoke only runs on Windows");
  process.exit(0);
}

const appRoot = path.resolve(__dirname, "..");
const scheduler = path.join(
  appRoot,
  "src-tauri",
  "resources",
  "bundle",
  "workflow",
  "sansheng-liubu",
  "scripts",
  "scheduler.py",
);

const explicitPython = process.env.PINVOU3_BUNDLED_PYTHON;
const programFiles = [
  process.env.ProgramW6432,
  process.env.ProgramFiles,
  process.env["ProgramFiles(x86)"],
].filter(Boolean);
const candidates = [
  explicitPython,
  ...programFiles.map((root) =>
    path.join(root, "pinvou3", "runtime", "python", "pythonw.exe"),
  ),
].filter(Boolean);
const python = candidates.find((candidate) => fs.existsSync(candidate));

assert.ok(
  python,
  `未找到安装包内置 Python；请先安装品悟，或设置 PINVOU3_BUNDLED_PYTHON。已检查：${candidates.join(", ")}`,
);
assert.ok(fs.existsSync(scheduler), `scheduler.py 不存在：${scheduler}`);

const project = fs.mkdtempSync(path.join(os.tmpdir(), "pinvou3-scheduler-smoke-"));
const stateDir = path.join(project, "_state");
fs.mkdirSync(stateDir, { recursive: true });
fs.writeFileSync(
  path.join(stateDir, "workflow_progress.json"),
  JSON.stringify(
    {
      scenario: "sansheng_liubu",
      created_at: new Date().toISOString(),
      version: 1,
    },
    null,
    2,
  ),
  "utf8",
);
fs.writeFileSync(
  path.join(stateDir, "brief.json"),
  JSON.stringify({ scenario: "sansheng_liubu", user_request_raw: "冒烟测试" }),
  "utf8",
);

function runScheduler(action) {
  const env = {
    ...process.env,
    PYTHONIOENCODING: "utf-8",
    PYTHONDONTWRITEBYTECODE: "1",
  };
  // CPython embeddable runtime 的 python*._pth 会忽略 PYTHONPATH；清空它可以确保
  // 本测试真正验证 scheduler.py 自己建立同目录模块搜索路径，而非借助开发机环境。
  delete env.PYTHONPATH;
  const result = spawnSync(
    python,
    [scheduler, project, "--scenario", "sansheng_liubu", action],
    {
      cwd: path.dirname(scheduler),
      env,
      encoding: "utf8",
      windowsHide: true,
      timeout: 30_000,
    },
  );
  assert.ifError(result.error);
  assert.strictEqual(
    result.status,
    0,
    `${action} 失败\nstdout:\n${result.stdout || ""}\nstderr:\n${result.stderr || ""}`,
  );
  assert.ok(result.stdout.trim(), `${action} 未输出 JSON`);
  return JSON.parse(result.stdout);
}

try {
  const status = runScheduler("--status");
  assert.ok(status.roles && Object.keys(status.roles).length > 0, "--status 未初始化角色状态");

  const next = runScheduler("--next");
  assert.ok(
    ["dispatch", "dispatch_batch"].includes(next.action),
    `--next 未返回可派发动作：${JSON.stringify(next)}`,
  );
  assert.ok(next.role_id, "--next 未返回 role_id");

  console.log(`PASS: bundled Python scheduler --status/--next (${python})`);
} finally {
  fs.rmSync(project, { recursive: true, force: true });
}
