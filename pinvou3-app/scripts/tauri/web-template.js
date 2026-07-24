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
const LOCKFILE_PATH = path.join(WEB_TEMPLATE_ROOT, "package-lock.json");
const MARKER_PATH = path.join(
  WEB_TEMPLATE_ROOT,
  "node_modules",
  ".pinvou-prepared.json",
);
const PREPARE_FORMAT_VERSION = 1;

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

function isPrepared(expected = expectedMarker()) {
  try {
    const actual = JSON.parse(fs.readFileSync(MARKER_PATH, "utf8"));
    const requiredPackages = ["vite", "esbuild", "react", "react-dom"];
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
  spawn = spawnSync,
} = {}) {
  const expected = expectedMarker({ platform, architecture });
  if (isPrepared(expected)) {
    console.log(
      `[web-template] 复用 ${platform}/${architecture} 的 lockfile 依赖`,
    );
    return false;
  }

  const npm = platform === "win32" ? "npm.cmd" : "npm";
  console.log(
    `[web-template] 为 ${platform}/${architecture} 从 package-lock.json 准备离线模板依赖`,
  );
  const result = spawn(
    npm,
    ["ci", "--prefer-offline", "--no-audit", "--no-fund"],
    {
      cwd: WEB_TEMPLATE_ROOT,
      env: process.env,
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
  WEB_TEMPLATE_ROOT,
  expectedMarker,
  isPrepared,
  main,
  prepareWebTemplate,
};
