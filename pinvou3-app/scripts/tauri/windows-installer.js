const crypto = require("node:crypto");
const fs = require("node:fs");
const path = require("node:path");

const { APP_ROOT } = require("./platform-config.js");

const WINDOWS_NSIS_STAGING_ROOT = path.resolve(
  APP_ROOT,
  "src-tauri",
  "target",
  "windows-runtime",
  "nsis",
);

function fileSha256(filePath) {
  return crypto.createHash("sha256").update(fs.readFileSync(filePath)).digest("hex");
}

function verifiedFile(filePath, expected) {
  const item = fs.statSync(filePath, { throwIfNoEntry: false });
  return (
    item?.isFile() &&
    item.size === expected.bytes &&
    fileSha256(filePath) === expected.sha256
  );
}

function stageWindowsInstaller({
  platform = process.platform,
  bundleTargets = [],
  runtime,
  destinationRoot = WINDOWS_NSIS_STAGING_ROOT,
} = {}) {
  if (platform !== "win32" || !bundleTargets.includes("nsis")) return null;
  if (!runtime?.vcRedist) {
    throw new Error("NSIS 构建缺少 Windows runtime descriptor 中的 VC++ Runtime");
  }

  const sourcePath = runtime.vcRedist.sourcePath;
  const sourceItem = fs.statSync(sourcePath, { throwIfNoEntry: false });
  if (!sourceItem?.isFile()) {
    throw new Error(`Windows runtime descriptor 指向的 VC++ Runtime 不存在：${sourcePath}`);
  }
  const expected = {
    sourcePath,
    bytes: runtime.vcRedist.bytes,
    sha256: runtime.vcRedist.sha256,
  };
  if (!verifiedFile(sourcePath, expected)) {
    throw new Error(`Windows runtime descriptor 指向的 VC++ Runtime 指纹不匹配：${sourcePath}`);
  }

  const destinationPath = path.join(destinationRoot, "vc_redist", "VC_redist.x64.exe");
  if (verifiedFile(destinationPath, expected)) {
    return { vcRedistPath: destinationPath };
  }

  const parent = path.dirname(destinationRoot);
  const temporaryRoot = path.join(
    parent,
    `.tmp-nsis-${process.pid}-${Date.now()}`,
  );
  const temporaryPath = path.join(temporaryRoot, "vc_redist", "VC_redist.x64.exe");
  fs.rmSync(temporaryRoot, { recursive: true, force: true });
  fs.mkdirSync(path.dirname(temporaryPath), { recursive: true });
  try {
    fs.copyFileSync(sourcePath, temporaryPath);
    if (!verifiedFile(temporaryPath, expected)) {
      throw new Error(`NSIS VC++ Runtime 暂存后校验失败：${temporaryPath}`);
    }
    fs.rmSync(destinationRoot, { recursive: true, force: true });
    fs.renameSync(temporaryRoot, destinationRoot);
  } finally {
    fs.rmSync(temporaryRoot, { recursive: true, force: true });
  }

  return { vcRedistPath: destinationPath };
}

module.exports = {
  WINDOWS_NSIS_STAGING_ROOT,
  fileSha256,
  stageWindowsInstaller,
  verifiedFile,
};
