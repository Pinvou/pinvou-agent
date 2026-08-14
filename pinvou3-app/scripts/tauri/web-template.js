const crypto = require("node:crypto");
const fs = require("node:fs");
const path = require("node:path");
const { spawnSync } = require("node:child_process");

const { APP_ROOT } = require("./platform-config.js");

const WEB_TEMPLATE_ROOT = path.join(
  APP_ROOT,
  "src-tauri",
  "resources",
  "common",
  "web-template",
);
const PACKAGE_JSON_PATH = path.join(WEB_TEMPLATE_ROOT, "package.json");
const LOCKFILE_PATH = path.join(WEB_TEMPLATE_ROOT, "package-lock.json");
const MARKER_PATH = path.join(
  WEB_TEMPLATE_ROOT,
  "node_modules",
  ".pinvou-prepared.json",
);
const PREPARE_FORMAT_VERSION = 1;
const NPM_CI_ARGS = ["ci", "--prefer-offline", "--no-audit", "--no-fund"];

function npmInstallInvocation({
  platform = process.platform,
  environment = process.env,
  nodeExecutable = process.execPath,
  npmArgs = NPM_CI_ARGS,
} = {}) {
  const args = [...npmArgs];
  if (args.some((argument) => !/^[A-Za-z0-9@._=:/-]+$/u.test(argument))) {
    throw new Error("npm 参数包含不受支持的字符");
  }
  if (platform !== "win32") {
    return { command: "npm", args };
  }

  const npmExecPath = String(environment.npm_execpath || "").trim();
  if (npmExecPath && !/\.(?:cmd|bat)$/iu.test(npmExecPath)) {
    return {
      command: nodeExecutable,
      args: [npmExecPath, ...args],
    };
  }

  // Newer Node releases reject spawning .cmd files directly with EINVAL.
  // Keep the fallback command static and run it through the Windows command
  // interpreter instead of enabling `shell` for arbitrary arguments.
  const commandInterpreter = String(
    environment.ComSpec || environment.COMSPEC || "cmd.exe",
  ).trim();
  return {
    command: commandInterpreter || "cmd.exe",
    args: ["/d", "/s", "/c", `npm.cmd ${args.join(" ")}`],
  };
}

// 保留旧的内部 helper 形状，避免已有脚本或测试在迁移期间失效。
function npmInvocation({ platform = process.platform, env = process.env } = {}) {
  return npmInstallInvocation({ platform, environment: env });
}

function expectedMarker({
  platform = process.platform,
  architecture = process.arch,
} = {}) {
  const lockfile = fs.readFileSync(LOCKFILE_PATH);
  return {
    format: PREPARE_FORMAT_VERSION,
    platform,
    architecture,
    lockfileSha256: crypto.createHash("sha256").update(lockfile).digest("hex"),
  };
}

function declaredPackageNames(packageJsonPath = PACKAGE_JSON_PATH) {
  const manifest = JSON.parse(fs.readFileSync(packageJsonPath, "utf8"));
  return [
    ...new Set([
      ...Object.keys(manifest.dependencies || {}),
      ...Object.keys(manifest.devDependencies || {}),
    ]),
  ].sort();
}

function isPrepared(expected = expectedMarker()) {
  try {
    const actual = JSON.parse(fs.readFileSync(MARKER_PATH, "utf8"));
    const requiredPackages = declaredPackageNames();
    return (
      JSON.stringify(actual) === JSON.stringify(expected) &&
      requiredPackages.every((packageName) =>
        fs.existsSync(path.join(WEB_TEMPLATE_ROOT, "node_modules", packageName)),
      )
    );
  } catch {
    return false;
  }
}

function prepareWebTemplate({
  platform = process.platform,
  architecture = process.arch,
  environment = process.env,
  nodeExecutable = process.execPath,
  spawn = spawnSync,
} = {}) {
  const expected = expectedMarker({ platform, architecture });
  if (isPrepared(expected)) {
    console.log(
      `[web-template] 复用 ${platform}/${architecture} 的 lockfile 依赖`,
    );
    return false;
  }

  const invocation = npmInstallInvocation({
    platform,
    environment,
    nodeExecutable,
  });
  console.log(
    `[web-template] 为 ${platform}/${architecture} 从 package-lock.json 准备离线模板依赖`,
  );
  const result = spawn(
    invocation.command,
    invocation.args,
    {
      cwd: WEB_TEMPLATE_ROOT,
      env: environment,
      stdio: "inherit",
    },
  );
  if (result.error) throw result.error;
  if (result.status !== 0) {
    throw new Error(`网页模板依赖安装失败，npm ci 退出码：${result.status}`);
  }

  fs.writeFileSync(MARKER_PATH, `${JSON.stringify(expected, null, 2)}\n`);
  if (!isPrepared(expected)) {
    throw new Error("网页模板依赖准备后校验失败");
  }
  return true;
}

function main() {
  prepareWebTemplate();
}

if (require.main === module) {
  try {
    main();
  } catch (error) {
    console.error(`[web-template] ${error.message}`);
    process.exitCode = 1;
  }
}

module.exports = {
  LOCKFILE_PATH,
  MARKER_PATH,
  PACKAGE_JSON_PATH,
  WEB_TEMPLATE_ROOT,
  declaredPackageNames,
  expectedMarker,
  isPrepared,
  main,
  npmInstallInvocation,
  npmInvocation,
  prepareWebTemplate,
};
