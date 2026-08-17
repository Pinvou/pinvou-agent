#!/usr/bin/env node

// setup.js - 跨平台环境检查与 slidep 安装脚本
// 用法: node setup.js
//
// 环境变量:
//   WORKBUDDY_ROOT  - workbuddy binaries 根目录，默认自动检测
//   NODE_PATH       - 强制指定 node 路径
//   PYTHON_PATH     - 强制指定 python 路径

const fs = require('fs');
const path = require('path');
const https = require('https');
const { spawnSync } = require('child_process');

// 配置
const SLIDEP_REGISTRY = 'https://mirrors.tencent.com/npm/';
const SLIDEP_SCOPE = 'tencent';
const SLIDEP_NAME = 'slidep';
const SLIDEP_PKG_BASE = 'https://docs.gtimg.com/tgz/tencent-slidep-';
const SLIDEP_VERSION = '3.3.0';


// 颜色控制
const isColorSupported = process.stdout.isTTY && process.platform !== 'win32';
const RED = isColorSupported ? '\x1b[0;31m' : '';
const NC = isColorSupported ? '\x1b[0m' : '';

// ========== 工具函数 ==========

function exec(cmd, args, options = {}) {
  const result = spawnSync(cmd, args, {
    encoding: 'utf8',
    shell: process.platform === 'win32',
    ...options,
  });
  return {
    stdout: (result.stdout || '').trim(),
    stderr: (result.stderr || '').trim(),
    status: result.status,
  };
}

function which(command) {
  const cmd = process.platform === 'win32' ? 'where' : 'which';
  const result = exec(cmd, [command], { shell: true });
  if (result.status === 0 && result.stdout) {
    // where 可能返回多行，取第一个
    return result.stdout.split(/\r?\n/)[0].trim();
  }
  return null;
}

function existsAndExecutable(filePath) {
  try {
    const stats = fs.statSync(filePath);
    if (!stats.isFile()) return false;
    // Windows 上没有可执行权限位，只要存在 .exe / .cmd / .bat 就算可执行
    if (process.platform === 'win32') {
      const ext = path.extname(filePath).toLowerCase();
      return ext === '.exe' || ext === '.cmd' || ext === '.bat' || ext === '.ps1' || ext === '';
    }
    // Unix: 检查 owner/group/other 的可执行位 (mode & 0o111)
    return (stats.mode & 0o111) !== 0;
  } catch {
    return false;
  }
}

function getHomeDir() {
  return process.env.HOME || process.env.USERPROFILE || '';
}

// 获取 WorkBuddy 根目录
function getWorkbuddyRoot() {
  // 1. 环境变量优先级最高
  if (process.env.WORKBUDDY_ROOT) {
    const dir = process.env.WORKBUDDY_ROOT;
    try {
      if (fs.statSync(path.join(dir, 'binaries')).isDirectory()) {
        return dir;
      }
    } catch { /* ignore */ }
  }

  // 2. 尝试自动检测常见位置
  const candidates = [
    path.join(getHomeDir(), '.workbuddy'),
    path.join(getHomeDir(), 'WorkBuddy', '.workbuddy'),
    process.platform === 'win32' ? 'C:\\workbuddy' : '/opt/workbuddy',
  ];

  for (const dir of candidates) {
    try {
      if (fs.statSync(path.join(dir, 'binaries')).isDirectory()) {
        return dir;
      }
    } catch { /* ignore */ }
  }

  return null;
}

// 语义化版本排序（支持预发布标识）
function semverCompare(a, b) {
  const parse = (v) => {
    const [main, pre] = v.split('-');
    const parts = main.split('.').map(Number);
    return { parts, pre: pre || null };
  };
  const av = parse(a);
  const bv = parse(b);
  for (let i = 0; i < Math.max(av.parts.length, bv.parts.length); i++) {
    const ap = av.parts[i] || 0;
    const bp = bv.parts[i] || 0;
    if (ap !== bp) return ap - bp;
  }
  // 有预发布标识的版本 < 无预发布标识
  if (av.pre && !bv.pre) return -1;
  if (!av.pre && bv.pre) return 1;
  if (av.pre && bv.pre) return av.pre.localeCompare(bv.pre);
  return 0;
}

// 在目录中找最新版本文件夹
function findLatestVersionDir(baseDir) {
  try {
    const entries = fs.readdirSync(baseDir);
    const versions = entries
      .filter((e) => /^\d+\.\d+\.\d+/.test(e))
      .sort(semverCompare);
    return versions.length > 0 ? versions[versions.length - 1] : null;
  } catch {
    return null;
  }
}

// 查找 managed node 路径
function findManagedNode() {
  const wbRoot = getWorkbuddyRoot();
  if (!wbRoot) return null;
  const baseDir = path.join(wbRoot, 'binaries', 'node', 'versions');
  const latest = findLatestVersionDir(baseDir);
  if (latest) {
    const nodePath = path.join(baseDir, latest, 'bin', process.platform === 'win32' ? 'node.exe' : 'node');
    if (existsAndExecutable(nodePath)) return nodePath;
  }
  return null;
}

// 查找 managed python 路径
function findManagedPython() {
  const wbRoot = getWorkbuddyRoot();
  if (!wbRoot) return null;
  const baseDir = path.join(wbRoot, 'binaries', 'python', 'versions');
  const latest = findLatestVersionDir(baseDir);
  if (latest) {
    const names = process.platform === 'win32'
      ? ['python.exe', 'python3.exe']
      : ['python3', 'python'];
    for (const name of names) {
      const pyPath = path.join(baseDir, latest, 'bin', name);
      if (existsAndExecutable(pyPath)) return pyPath;
    }
  }
  return null;
}

// 检查 node
function checkNode() {
  let nodePath = null;

  // 优先级: 环境变量 > managed > PATH
  if (process.env.NODE_PATH && existsAndExecutable(process.env.NODE_PATH)) {
    nodePath = process.env.NODE_PATH;
  } else {
    nodePath = findManagedNode();
    if (!nodePath) {
      nodePath = which('node');
    }
  }

  if (!nodePath) {
    console.error(`${RED}✗ Node.js 未找到${NC}`);
    console.error('  可通过以下方式指定:');
    if (process.platform === 'win32') {
      console.error('    $env:NODE_PATH = "C:\\path\\to\\node.exe"');
    } else {
      console.error('    export NODE_PATH=/path/to/node');
    }
    console.error('    或安装到 PATH 中');
    return null;
  }

  const result = exec(nodePath, ['--version']);
  const version = result.stdout.replace(/^v/, '');
  console.log(`node: ${nodePath} (v${version})`);
  return nodePath;
}

// 检查 python
function checkPython() {
  let pyPath = null;

  // 优先级: 环境变量 > managed > PATH
  if (process.env.PYTHON_PATH && existsAndExecutable(process.env.PYTHON_PATH)) {
    pyPath = process.env.PYTHON_PATH;
  } else {
    pyPath = findManagedPython();
    if (!pyPath) {
      pyPath = which('python3') || which('python');
    }
  }

  if (!pyPath) {
    console.error(`${RED}✗ Python 未找到${NC}`);
    console.error('  可通过以下方式指定:');
    if (process.platform === 'win32') {
      console.error('    $env:PYTHON_PATH = "C:\\path\\to\\python.exe"');
    } else {
      console.error('    export PYTHON_PATH=/path/to/python3');
    }
    console.error('    或安装到 PATH 中');
    return null;
  }

  const result = exec(pyPath, ['--version']);
  // Python 输出格式: "Python 3.x.y"
  const match = result.stdout.match(/Python\s+(\S+)/);
  const version = match ? match[1] : 'unknown';
  console.log(`python: ${pyPath} (v${version})`);
  return pyPath;
}

// 解析 node 对应的 npm 全局 prefix（绑定到 managed node 自己的目录）
function getNodePrefix(nodePath) {
  if (!nodePath) return null;
  const dir = path.dirname(nodePath);       // bin 目录或版本目录
  const parent = path.dirname(dir);         // 版本目录或 versions 目录
  // macOS/Linux: node 在 versions/X.X.X/bin/ 下，parent 才是 prefix
  // Windows: node.exe 直接在 versions/X.X.X/ 下，dir 就是 prefix
  return path.basename(dir) === 'bin' ? parent : dir;
}

// 获取 slidep 二进制路径
function getSlidepBin() {
  let nodePath = null;
  if (process.env.NODE_PATH && existsAndExecutable(process.env.NODE_PATH)) {
    nodePath = process.env.NODE_PATH;
  } else {
    nodePath = findManagedNode();
  }

  if (nodePath) {
    const binDir = path.dirname(nodePath);
    const slidepName = process.platform === 'win32' ? 'slidep.cmd' : 'slidep';
    const slidepPath = path.join(binDir, slidepName);
    if (fs.existsSync(slidepPath)) return slidepPath;
    // Windows 下也可能是 slidep.exe（如果它不是 cmd 包装器）
    if (process.platform === 'win32') {
      const slidepExe = path.join(binDir, 'slidep.exe');
      if (fs.existsSync(slidepExe)) return slidepExe;
    }
  }

  // 从 PATH 找
  const fromPath = which('slidep');
  if (fromPath) return fromPath;

  return null;
}

// 获取已安装 slidep 版本
function getInstalledSlidepVersion() {
  const slidepBin = getSlidepBin();
  if (!slidepBin) return null;
  const result = exec(slidepBin, ['--version']);
  if (result.status !== 0) return null;
  const match = result.stdout.match(/(\d+\.\d+\.\d+(?:-[0-9A-Za-z.]+)?)/);
  return match ? match[1] : null;
}

// 发起 HTTPS GET 请求（Promise 包装）
function httpsGet(url) {
  return new Promise((resolve, reject) => {
    https
      .get(url, { timeout: 10000 }, (res) => {
        if (res.statusCode >= 300 && res.statusCode < 400 && res.headers.location) {
          // 跟随重定向
          httpsGet(res.headers.location).then(resolve).catch(reject);
          return;
        }
        let data = '';
        res.on('data', (chunk) => { data += chunk; });
        res.on('end', () => resolve(data));
      })
      .on('error', reject)
      .on('timeout', () => reject(new Error('Request timeout')));
  });
}

// 获取远程最新版本
async function getRemoteVersion() {
  // 方法1: 通过 npm registry API 查询 scoped 包最新版本
  try {
    const encodedScope = `@${SLIDEP_SCOPE}%2f${SLIDEP_NAME}`;
    const data = await httpsGet(`${SLIDEP_REGISTRY}${encodedScope}/latest`);
    const match = data.match(/"version"\s*:\s*"(\d+\.\d+\.\d+(?:-[0-9A-Za-z.]+)?)"/);
    if (match) return match[1];
  } catch { /* ignore */ }

  // 方法2: 回退到 tgz 目录页解析
  try {
    const data = await httpsGet('https://docs.gtimg.com/tgz/');
    const matches = data.match(/tencent-slidep-\d+\.\d+\.\d+\.tgz/g);
    if (matches && matches.length > 0) {
      // 提取版本号并排序
      const versions = matches
        .map((m) => m.replace(/tencent-slidep-([\d.]+)\.tgz/, '$1'))
        .sort(semverCompare);
      return versions[versions.length - 1];
    }
  } catch { /* ignore */ }

  // 方法3: 兜底使用 npm view 命令
  try {
    const npmBin = process.platform === 'win32' ? 'npm.cmd' : 'npm';
    const result = exec(npmBin, [
      'view',
      `@${SLIDEP_SCOPE}/${SLIDEP_NAME}`,
      'version',
      '--registry',
      SLIDEP_REGISTRY,
    ]);
    if (result.status === 0 && result.stdout) {
      const v = result.stdout.trim();
      if (/^\d+\.\d+\.\d+/.test(v)) return v;
    }
  } catch { /* ignore */ }

  // 最终兜底
  return SLIDEP_VERSION;
}

// 安装/更新 slidep
async function installSlidep() {
  let nodePath = null;
  if (process.env.NODE_PATH && existsAndExecutable(process.env.NODE_PATH)) {
    nodePath = process.env.NODE_PATH;
  } else {
    nodePath = findManagedNode() || which('node');
  }

  if (!nodePath) {
    console.error(`${RED}✗ 无法找到 node${NC}`);
    process.exit(1);
  }

  const npmBin = process.platform === 'win32'
    ? path.join(path.dirname(nodePath), 'npm.cmd')
    : path.join(path.dirname(nodePath), 'npm');
  const npmPrefix = getNodePrefix(nodePath);
  const remote = await getRemoteVersion();
  const installed = getInstalledSlidepVersion();

  if (installed && installed === remote) {
    const slidepPath = getSlidepBin();
    console.log(`slidep: 已安装最新版 v${installed} (${slidepPath})`);
    return;
  }

  const url = `${SLIDEP_PKG_BASE}${remote}.tgz`;

  if (installed) {
    console.log(`slidep: 升级 v${installed} → v${remote}`);
  } else {
    console.log(`slidep: 安装 v${remote}`);
  }

  const args = ['install', '--registry', SLIDEP_REGISTRY, '-g', url];
  if (npmPrefix) {
    args.push('--prefix', npmPrefix);
  }

  const result = spawnSync(npmBin, args, {
    stdio: 'inherit',
    shell: process.platform === 'win32',
  });

  if (result.status !== 0) {
    console.error(`${RED}✗ npm install 失败 (exit code ${result.status})${NC}`);
    process.exit(1);
  }

  const newVersion = getInstalledSlidepVersion();
  const slidepPath = getSlidepBin();
  if (newVersion === remote) {
    console.log(`slidep: 安装成功 v${newVersion} (${slidepPath})`);
  } else {
    console.error(`${RED}✗ slidep 安装失败 (期望 v${remote}, 实际 v${newVersion || '未找到'})${NC}`);
    if (slidepPath) console.error(`  当前 slidep: ${slidepPath}`);
    process.exit(1);
  }
}

// ========== 主流程 ==========

(async () => {
  console.log('---');

  const nodePath = checkNode();
  const pyPath = checkPython();

  if (!nodePath || !pyPath) {
    console.log('');
    console.log('提示: 可通过环境变量指定路径');
    if (process.platform === 'win32') {
      console.log('  $env:NODE_PATH = "C:\\path\\to\\node.exe"');
      console.log('  $env:PYTHON_PATH = "C:\\path\\to\\python.exe"');
    } else {
      console.log('  export NODE_PATH=/path/to/node');
      console.log('  export PYTHON_PATH=/path/to/python3');
    }
    process.exit(1);
  }

  await installSlidep();

  console.log('---');
  console.log('✓ setup 完成');
})();
