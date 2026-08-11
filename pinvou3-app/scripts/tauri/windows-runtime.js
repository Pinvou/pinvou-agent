const fs = require("node:fs");
const path = require("node:path");
const crypto = require("node:crypto");
const { spawnSync } = require("node:child_process");

const { APP_ROOT } = require("./platform-config.js");

const TAURI_ROOT = path.resolve(APP_ROOT, "src-tauri");
const WINDOWS_RUNTIME_ROOT = path.join(TAURI_ROOT, "target", "windows-runtime");
const WINDOWS_RUNTIME_CONFIG_PATH = path.join(
  WINDOWS_RUNTIME_ROOT,
  "tauri.generated.conf.json",
);
const WINDOWS_RUNTIME_DESCRIPTOR_PATH = path.join(
  WINDOWS_RUNTIME_ROOT,
  "runtime-descriptor.json",
);
const WINDOWS_ONNX_DEV_DESCRIPTOR_PATH = path.join(
  WINDOWS_RUNTIME_ROOT,
  "onnx-dev-descriptor.json",
);
const LEGACY_WINDOWS_NODE_ROOTS = [
  path.join(WINDOWS_RUNTIME_ROOT, "node"),
  path.join(WINDOWS_RUNTIME_ROOT, "codex-node"),
];

function powershellExecutable() {
  const probe = spawnSync("pwsh.exe", ["-NoProfile", "-Command", "$PSVersionTable.PSVersion.ToString()"], {
    encoding: "utf8",
    stdio: ["ignore", "pipe", "ignore"],
  });
  return probe.status === 0 ? "pwsh.exe" : "powershell.exe";
}

function resolveDescriptorPath(relativePath, label) {
  if (typeof relativePath !== "string" || !relativePath || path.isAbsolute(relativePath)) {
    throw new Error(`Windows runtime descriptor 的 ${label} 必须是相对路径`);
  }
  const resolved = path.resolve(TAURI_ROOT, relativePath);
  const relation = path.relative(TAURI_ROOT, resolved);
  if (!relation || relation.startsWith("..") || path.isAbsolute(relation)) {
    throw new Error(`Windows runtime descriptor 的 ${label} 超出 src-tauri：${relativePath}`);
  }
  return resolved;
}

function requireFile(filePath, label) {
  if (!fs.statSync(filePath, { throwIfNoEntry: false })?.isFile()) {
    throw new Error(`Windows runtime 缺少${label}：${filePath}`);
  }
}

function describeWindowsRuntime(descriptorPath = WINDOWS_RUNTIME_DESCRIPTOR_PATH) {
  const descriptor = JSON.parse(fs.readFileSync(descriptorPath, "utf8"));
  if (descriptor.schemaVersion !== 1 || descriptor.target !== "windows-x86_64") {
    throw new Error("Windows runtime descriptor 版本或目标平台不受支持");
  }
  if (
    descriptor.asrModel?.delivery !== "download-on-first-use" ||
    descriptor.asrModel?.bundled !== false
  ) {
    throw new Error("Windows runtime descriptor 必须明确采用 ASR 模型首次下载策略");
  }

  const configPath = resolveDescriptorPath(descriptor.configPath, "configPath");
  const nodeExecutable = resolveDescriptorPath(
    descriptor.nodeExecutable,
    "nodeExecutable",
  );
  const npmExecPath = resolveDescriptorPath(descriptor.npmExecPath, "npmExecPath");
  const onnxRuntimeDylib = resolveDescriptorPath(
    descriptor.onnxRuntimeDylib,
    "onnxRuntimeDylib",
  );
  const vcRedistSource = resolveDescriptorPath(
    descriptor.vcRedist?.source,
    "vcRedist.source",
  );
  for (const [filePath, label] of [
    [configPath, " Tauri overlay"],
    [nodeExecutable, " Codex Bridge Node"],
    [npmExecPath, " Codex Bridge npm CLI"],
    [onnxRuntimeDylib, " ONNX Runtime"],
    [vcRedistSource, " VC++ Runtime"],
  ]) {
    requireFile(filePath, label);
  }
  if (
    !Number.isSafeInteger(descriptor.vcRedist?.bytes) ||
    descriptor.vcRedist.bytes <= 0 ||
    !/^[0-9a-f]{64}$/u.test(descriptor.vcRedist?.sha256 ?? "")
  ) {
    throw new Error("Windows runtime descriptor 的 VC++ Runtime 指纹无效");
  }
  const vcRedistItem = fs.statSync(vcRedistSource);
  const vcRedistSha256 = crypto
    .createHash("sha256")
    .update(fs.readFileSync(vcRedistSource))
    .digest("hex");
  if (
    vcRedistItem.size !== descriptor.vcRedist.bytes ||
    vcRedistSha256 !== descriptor.vcRedist.sha256
  ) {
    throw new Error("Windows runtime descriptor 的 VC++ Runtime 指纹不匹配");
  }

  return {
    configPath,
    descriptorPath,
    nodeExecutable,
    npmExecPath,
    onnxRuntimeDylib,
    vcRedist: {
      sourcePath: vcRedistSource,
      bytes: descriptor.vcRedist.bytes,
      sha256: descriptor.vcRedist.sha256,
    },
    asrModel: descriptor.asrModel,
  };
}

function cleanupLegacyWindowsNodeStaging() {
  for (const legacyRoot of LEGACY_WINDOWS_NODE_ROOTS) {
    fs.rmSync(legacyRoot, { recursive: true, force: true });
  }
}

function stageWindowsRuntime({
  platform = process.platform,
  environment = process.env,
  spawn = spawnSync,
} = {}) {
  if (platform !== "win32") return null;

  cleanupLegacyWindowsNodeStaging();

  const scriptPath = path.resolve(
    TAURI_ROOT,
    "packaging",
    "windows",
    "runtime",
    "scripts",
    "stage-runtime.ps1",
  );
  const child = spawn(
    powershellExecutable(),
    ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", scriptPath],
    { cwd: APP_ROOT, env: environment, stdio: "inherit" },
  );
  if (child.error) throw child.error;
  if (child.status !== 0) {
    throw new Error(
      `Windows 独立运行时 staging 失败（退出码 ${child.status ?? "unknown"}）`,
    );
  }
  const runtime = describeWindowsRuntime();
  return runtime;
}

function stageWindowsOnnxRuntime({
  platform = process.platform,
  environment = process.env,
  spawn = spawnSync,
} = {}) {
  if (platform !== "win32") return null;
  const scriptPath = path.resolve(
    TAURI_ROOT,
    "packaging",
    "windows",
    "runtime",
    "scripts",
    "stage-onnx-runtime.ps1",
  );
  const child = spawn(
    powershellExecutable(),
    ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", scriptPath],
    { cwd: APP_ROOT, env: environment, stdio: "inherit" },
  );
  if (child.error) throw child.error;
  if (child.status !== 0) {
    throw new Error(
      `Windows ONNX Runtime staging 失败（退出码 ${child.status ?? "unknown"}）；请先在 pinvou3-app 运行 npm run runtime:windows:init:onnx`,
    );
  }
  const descriptor = JSON.parse(
    fs.readFileSync(WINDOWS_ONNX_DEV_DESCRIPTOR_PATH, "utf8"),
  );
  if (descriptor.schemaVersion !== 1 || descriptor.target !== "windows-x86_64") {
    throw new Error("Windows ONNX development descriptor 版本或目标平台不受支持");
  }
  const onnxRuntimeDylib = resolveDescriptorPath(
    descriptor.onnxRuntimeDylib,
    "onnxRuntimeDylib",
  );
  requireFile(onnxRuntimeDylib, " ONNX Runtime");
  const item = fs.statSync(onnxRuntimeDylib);
  const sha256 = crypto.createHash("sha256").update(fs.readFileSync(onnxRuntimeDylib)).digest("hex");
  if (item.size !== descriptor.bytes || sha256 !== descriptor.sha256) {
    throw new Error("Windows ONNX development descriptor 指纹不匹配");
  }
  return { descriptorPath: WINDOWS_ONNX_DEV_DESCRIPTOR_PATH, onnxRuntimeDylib };
}

module.exports = {
  LEGACY_WINDOWS_NODE_ROOTS,
  WINDOWS_RUNTIME_CONFIG_PATH,
  WINDOWS_RUNTIME_DESCRIPTOR_PATH,
  WINDOWS_ONNX_DEV_DESCRIPTOR_PATH,
  cleanupLegacyWindowsNodeStaging,
  describeWindowsRuntime,
  stageWindowsOnnxRuntime,
  stageWindowsRuntime,
};
