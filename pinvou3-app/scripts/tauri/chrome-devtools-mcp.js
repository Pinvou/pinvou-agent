// 构建期 vendor 官方 chrome-devtools-mcp（Apache-2.0，自包含构建）到
// `src-tauri/resources/platforms/<os>/chrome-devtools-mcp/`，随安装包 resource overlay
// 打进 `runtime/chrome-devtools-mcp`。构建机需要网络（与品悟使用期离线无关）。
//
// 幂等：目标目录存在且 `.vendor-version.json` 版本一致即跳过；版本/完整性变更自动重做。
// 完整性：npm registry tarball 的 sha512 integrity 硬编码校验（防止供应链篡改）。
//
// 用法：node scripts/tauri/chrome-devtools-mcp.js
// 由 build.js 在 `hasTauriBuildCommand` 分支自动调用；也可手动 `node scripts/tauri/chrome-devtools-mcp.js` 预热。

const crypto = require("node:crypto");
const fs = require("node:fs");
const path = require("node:path");
const { spawnSync } = require("node:child_process");
const { APP_ROOT } = require("./platform-config.js");

const VERSION = "1.7.0";
// npm registry 对 chrome-devtools-mcp@1.7.0 的 integrity（sha512 base64，去掉 `sha512-` 前缀）
const INTEGRITY_SHA512 =
  "6xFW7oiUxTxZuHcfyYBkKQtmttjCbfifKZMSEk5CV8H2FucvKweYiJr8CblddYHtYjA4C14K9VAs1r49906RBA==";
const TARBALL_URL = `https://registry.npmjs.org/chrome-devtools-mcp/-/chrome-devtools-mcp-${VERSION}.tgz`;
const MARKER_NAME = ".vendor-version.json";

const PLATFORM_DIR = { darwin: "macos", linux: "linux", win32: "windows" };

// 被 git 跟踪的占位 .gitkeep：vendor 重建（rmSync 整个目录）后重写，内容必须与
// resources/platforms/*/chrome-devtools-mcp/.gitkeep 的提交版本逐字节一致，否则
// vendor 后工作区必现 .gitkeep 改动、来回提交互相覆盖。CI 的 Tauri 资源布局测试
// （tauri_platform_layout / tauri_effective_config）靠资源存在性断言把关。
const GITKEEP = `本目录由 pinvou3-app/scripts/tauri/chrome-devtools-mcp.js 构建期 vendor：
npm registry 官方 chrome-devtools-mcp（Apache-2.0，sha512 硬编码校验），
产物 build/（rollup 自包含，约 13MB）+ catalog-shim.json（懒启动 shim 应答目录）
+ .vendor-version.json，随安装包经 resource overlay 分发到 runtime/chrome-devtools-mcp。

本 .gitkeep 被 git 跟踪，保证 CI 的 Tauri 资源布局测试（tauri_platform_layout /
tauri_effective_config）通过资源存在性断言；构建产物被 .gitignore 忽略。
开发环境未跑 vendor 时，浏览器 MCP 条目自动跳过注册。
`;

function outputRoot(platform = process.platform) {
  const dir = PLATFORM_DIR[platform];
  if (!dir) throw new Error(`不支持的平台: ${platform}`);
  return path.join(APP_ROOT, "src-tauri", "resources", "platforms", dir, "chrome-devtools-mcp");
}

function expectedMarker() {
  return { name: "chrome-devtools-mcp", version: VERSION };
}

function isPrepared(platform = process.platform) {
  const root = outputRoot(platform);
  try {
    const actual = JSON.parse(fs.readFileSync(path.join(root, MARKER_NAME), "utf8"));
    if (JSON.stringify(actual) !== JSON.stringify(expectedMarker())) return false;
    // 自包含构建的关键入口 + 懒启动 shim 目录存在即视为完整（发布物无 node_modules）
    return (
      fs.existsSync(path.join(root, "build", "src", "bin", "chrome-devtools-mcp.js")) &&
      fs.existsSync(path.join(root, "catalog-shim.json"))
    );
  } catch {
    return false;
  }
}

function run(cmd, args, { cwd, inherit = false, input, env } = {}) {
  const result = spawnSync(cmd, args, {
    cwd,
    input,
    env,
    stdio: inherit ? "inherit" : ["pipe", "pipe", "pipe"],
    maxBuffer: 32 * 1024 * 1024,
    timeout: 60000,
    encoding: input != null ? "utf8" : undefined,
  });
  if (result.error) throw result.error;
  if (result.status !== 0) {
    const err = String(result.stderr || result.stdout || "").trim();
    throw new Error(`${cmd} ${args.join(" ")} 失败 status=${result.status}${err ? `: ${err.slice(0, 500)}` : ""}`);
  }
  return result;
}

// 捕获 MCP 握手/工具目录（catalog-shim.json）：browser-wrapper 懒启动 shim 阶段
// 直接用它应答引擎的 initialize/tools/list（chrome-devtools-mcp 的工具是静态注册，
// 无浏览器连接即可枚举；stdin EOF 后 server 自行退出，spawnSync 一次性喂入即可）。
function captureCatalog(entry, root) {
  const lines = [
    JSON.stringify({
      jsonrpc: "2.0",
      id: 1,
      method: "initialize",
      params: {
        protocolVersion: "2024-11-05",
        capabilities: {},
        clientInfo: { name: "pinvou-vendor", version: "0" },
      },
    }),
    JSON.stringify({ jsonrpc: "2.0", method: "notifications/initialized" }),
    JSON.stringify({ jsonrpc: "2.0", id: 2, method: "tools/list", params: {} }),
    "",
  ].join("\n");
  const result = run(
    process.execPath,
    [entry, "--no-usage-statistics", "--no-performance-crux"],
    {
      cwd: root,
      input: lines,
      env: { ...process.env, CHROME_DEVTOOLS_MCP_NO_UPDATE_CHECKS: "1", CI: "1" },
    },
  );
  let initializeResult = null;
  let toolsListResult = null;
  for (const line of String(result.stdout || "").split("\n")) {
    if (!line.trim()) continue;
    let msg;
    try {
      msg = JSON.parse(line);
    } catch {
      continue; // 非协议输出（不应出现，stdout 仅供协议）
    }
    if (msg.id === 1 && msg.result) initializeResult = msg.result;
    if (msg.id === 2 && msg.result) toolsListResult = msg.result;
  }
  if (!initializeResult || !toolsListResult || !Array.isArray(toolsListResult.tools)) {
    throw new Error("捕获 MCP 工具目录失败（initialize/tools/list 应答缺失）");
  }
  if (toolsListResult.nextCursor) {
    throw new Error("捕获 MCP 工具目录失败：tools/list 出现分页（需扩展捕获逻辑）");
  }
  const catalog = { initializeResult, toolsListResult };
  fs.writeFileSync(path.join(root, "catalog-shim.json"), JSON.stringify(catalog));
  console.log(`[chrome-devtools-mcp] catalog: ${toolsListResult.tools.length} tools`);
}

function prepareChromeDevtoolsMcp({ platform = process.platform } = {}) {
  if (isPrepared(platform)) return false;

  const root = outputRoot(platform);
  const stagingRoot = path.join(APP_ROOT, "target", "chrome-devtools-mcp-staging", `${platform}-${process.pid}`);
  const tarball = path.join(stagingRoot, `chrome-devtools-mcp-${VERSION}.tgz`);

  fs.mkdirSync(stagingRoot, { recursive: true });
  console.log(`[chrome-devtools-mcp] vendor ${VERSION} → ${root}`);
  try {
    // 1) 同步下载（curl 三平台自带；弱网构建机加重试，release 构建不脆断）
    run("curl", ["-fsSL", "--retry", "3", "--retry-delay", "2", "--retry-all-errors", "-o", tarball, TARBALL_URL], { cwd: stagingRoot });
    // 2) sha512 完整性校验
    const hash = crypto.createHash("sha512").update(fs.readFileSync(tarball)).digest("base64");
    if (hash !== INTEGRITY_SHA512) {
      throw new Error(
        `chrome-devtools-mcp@${VERSION} sha512 校验失败（期望 ${INTEGRITY_SHA512.slice(0, 16)}…，实际 ${hash.slice(0, 16)}…）`,
      );
    }
    // 3) 解压（tar 三平台自带：macOS/Linux bsdtar、Windows 10+ tar.exe）
    run("tar", ["-xzf", tarball, "-C", stagingRoot], { cwd: stagingRoot });
    const unpacked = path.join(stagingRoot, "package");
    if (!fs.existsSync(unpacked)) {
      throw new Error(`解压后缺少 package/ 目录（tarball 结构变化）`);
    }
    // 4) 原子落位
    fs.rmSync(root, { recursive: true, force: true });
    fs.mkdirSync(path.dirname(root), { recursive: true });
    fs.renameSync(unpacked, root);
    // 4.5) 重写被 git 跟踪的占位 .gitkeep（rmSync 已删掉旧文件；CI 资源布局
    //      测试依赖目录在 checkout 上存在，占位文件不可缺失）。
    fs.writeFileSync(path.join(root, ".gitkeep"), GITKEEP);
    // 5) 自包含冒烟：当前 node 直接跑 --help（不装任何依赖，验证零依赖可离线跑）
    const entry = path.join(root, "build", "src", "bin", "chrome-devtools-mcp.js");
    if (!fs.existsSync(entry)) throw new Error("解压后缺少 build/src/bin/chrome-devtools-mcp.js");
    run(process.execPath, [entry, "--help"], { cwd: root });
    // 5.5) 捕获 initialize/tools/list 目录（browser-wrapper 懒启动 shim 应答来源）
    captureCatalog(entry, root);
    // 6) marker
    fs.writeFileSync(path.join(root, MARKER_NAME), JSON.stringify(expectedMarker(), null, 2));
    console.log(`[chrome-devtools-mcp] ready: ${root}`);
    return true;
  } finally {
    fs.rmSync(stagingRoot, { recursive: true, force: true });
  }
}

module.exports = { prepareChromeDevtoolsMcp, isPrepared, outputRoot };

if (require.main === module) {
  try {
    prepareChromeDevtoolsMcp();
  } catch (error) {
    console.error(`[chrome-devtools-mcp] ${error.message}`);
    process.exitCode = 1;
  }
}
