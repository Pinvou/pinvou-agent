#!/usr/bin/env node
/**
 * browser-wrapper.mjs —— 品悟浏览器 MCP server 的 stdio 协调包装。
 *
 * 职责：
 *  1. 与品悟桌面端（Rust BrowserManager）协调"专用有头 Chrome"实例的生命周期，
 *     双方通过 `~/.pinvou3/browser/cdp-port.json` + 独占锁文件幂等协调：
 *     - 端口文件有效（Chrome 还活着）→ 直接复用；
 *     - 否则自己启动 Chrome（隐藏窗口、独立 profile、随机 CDP 端口）并写回端口文件。
 *  2. 以 `--browser-url` 把官方 chrome-devtools-mcp 指向该 Chrome。
 *  3. 强制离线：关闭遥测/更新检查/CrUX 上报。
 *
 * 协议约束：MCP 走 stdin/stdout（JSON-RPC over stdio），本包装不能往 stdout 写任何
 * 非协议内容；日志一律走 stderr。
 *
 * 用法：
 *   node browser-wrapper.mjs <chrome-devtools-mcp-bin> <cdp-port-json> <profile-dir> [extra-args...]
 *
 * 退出：wrapper 是 MCP server 的父进程，随后以子进程方式托管 chrome-devtools-mcp
 * （stdio 继承），自身生命周期即 MCP server 生命周期；若本包装启动过 Chrome，
 * 退出前会清理它。
 */

import { execFileSync, spawn } from 'node:child_process';
import {
  chmodSync,
  closeSync,
  existsSync,
  mkdirSync,
  openSync,
  readFileSync,
  renameSync,
  statSync,
  unlinkSync,
  writeFileSync,
  writeSync,
} from 'node:fs';
import { dirname, join } from 'node:path';
import { setTimeout as sleep } from 'node:timers/promises';

const log = (...args) => console.error('[browser-wrapper]', ...args);

// ---------------------------------------------------------------------------
// 参数：node browser-wrapper.mjs <mcp-bin> <cdp-port-json> <profile-dir> [extra...]
// ---------------------------------------------------------------------------
const [, , MCP_BIN, CDP_PORT_JSON, PROFILE_DIR, ...EXTRA_ARGS] = process.argv;
if (!MCP_BIN || !CDP_PORT_JSON || !PROFILE_DIR) {
  console.error(
    '[browser-wrapper] usage: node browser-wrapper.mjs <mcp-bin> <cdp-port-json> <profile-dir> [extra-args...]'
  );
  process.exit(2);
}

// ---------------------------------------------------------------------------
// Chrome 可执行文件探测（macOS / Linux / Windows）
// ---------------------------------------------------------------------------
function findChrome() {
  const candidates = [];
  switch (process.platform) {
    case 'darwin':
      candidates.push(
        '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',
        '/Applications/Chromium.app/Contents/MacOS/Chromium',
        '/Applications/Brave Browser.app/Contents/MacOS/Brave Browser',
        '/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge'
      );
      break;
    case 'linux':
      candidates.push(
        'google-chrome',
        'google-chrome-stable',
        'chromium',
        'chromium-browser',
        'brave-browser',
        'microsoft-edge'
      );
      break;
    case 'win32':
      candidates.push(
        process.env.PROGRAMFILES + '\\Google\\Chrome\\Application\\chrome.exe',
        process.env['PROGRAMFILES(X86)'] + '\\Google\\Chrome\\Application\\chrome.exe',
        process.env.LOCALAPPDATA + '\\Google\\Chrome\\Application\\chrome.exe',
        'chrome'
      );
      break;
  }
  for (const c of candidates) {
    if (!c) continue;
    try {
      if (c.includes('/') || c.includes('\\')) {
        if (existsSync(c)) return c;
      } else {
        execFileSync(process.platform === 'win32' ? 'where' : 'which', [c], {
          stdio: 'pipe',
        });
        return c;
      }
    } catch {
      /* 继续找下一个候选 */
    }
  }
  return null;
}

// ---------------------------------------------------------------------------
// CDP 存活探测（GET /json/version，同步等待）
// ---------------------------------------------------------------------------
function probeCdp(port, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    try {
      execFileSync(
        process.execPath,
        [
          '-e',
          [
            `const http=require('http');`,
            `http.get({host:'127.0.0.1',port:${port},path:'/json/version',timeout:1000},r=>{`,
            `  r.resume();`,
            `  process.exit(r.statusCode===200?0:1)`,
            `}).on('error',()=>process.exit(1));`,
          ].join('\n'),
        ],
        { stdio: 'ignore', timeout: 2500 }
      );
      return true;
    } catch {
      /* 未就绪，重试 */
    }
  }
  return false;
}

// ---------------------------------------------------------------------------
// 端口文件（cdp-port.json）：{ port, pid, owner: "app"|"mcp", started_at }
// ---------------------------------------------------------------------------
function readPortFile() {
  try {
    const data = JSON.parse(readFileSync(CDP_PORT_JSON, 'utf8'));
    if (typeof data.port === 'number' && data.port > 0 && data.port < 65536) return data;
  } catch {
    /* 无文件/坏 json */
  }
  return null;
}

function writePortFile(port, owner) {
  try {
    mkdirSync(dirname(CDP_PORT_JSON), { recursive: true });
    const tmp = CDP_PORT_JSON + '.tmp';
    writeFileSync(tmp, JSON.stringify({ port, pid: process.pid, owner, started_at: Date.now() }));
    // 收紧端口文件权限：CDP 无鉴权，同机其他本地用户不应能读到端口坐标。
    try {
      chmodSync(tmp, 0o600);
    } catch {
      /* Windows 无 chmod 语义，忽略 */
    }
    renameSync(tmp, CDP_PORT_JSON); // 原子替换
  } catch (e) {
    log('写端口文件失败:', e.message);
  }
}

/// 启动锁 stale 判定：mtime 超过 60s 视为持有者崩溃/被杀后的残留（与 Rust 侧
/// `lock_file_stale` 同语义，双方都可在等锁时抢占删除，避免永久死锁）。
function lockFileStale(lockPath) {
  try {
    const st = statSync(lockPath);
    return Date.now() - st.mtimeMs > 60_000;
  } catch {
    return false;
  }
}

function clearPortFile() {
  try {
    unlinkSync(CDP_PORT_JSON);
  } catch {
    /* 不存在就算了 */
  }
}

// 最近一次启动失败记录（{ reason, at }）：Rust 侧（browser_unavailability_reason）
// 在下次会话把原因注入模型可见的 instructions，让模型能精确引导用户修复。
// 成功启动（CDP 就绪）时清除。
const LAST_ERROR_JSON = join(dirname(CDP_PORT_JSON), 'last-error.json');
function writeLastError(reason) {
  try {
    mkdirSync(dirname(LAST_ERROR_JSON), { recursive: true });
    writeFileSync(LAST_ERROR_JSON, JSON.stringify({ reason, at: Date.now() }));
  } catch {
    /* 写失败不影响主流程 */
  }
}
function clearLastError() {
  try {
    unlinkSync(LAST_ERROR_JSON);
  } catch {
    /* 不存在就算了 */
  }
}

// ---------------------------------------------------------------------------
// Chrome 启动（有头渲染、窗口置于屏外、独立 profile、随机端口）
// ---------------------------------------------------------------------------
const BROWSER_FLAGS = [
  '--no-first-run',
  '--no-default-browser-check',
  '--disable-extensions',
  '--disable-component-update',
  '--disable-background-networking',
  '--disable-sync',
  '--metrics-recording-only',
  '--noerrdialogs',
  '--mute-audio',
  '--disable-features=Translate,MediaRouter',
  '--window-position=-32000,-32000', // 有头渲染但窗口在屏外（品悟 Tab 是唯一视图）
  '--window-size=1280,800',
];

function pickFreePort() {
  const base = 9222 + Math.floor(Math.random() * 3000); // 9222-12221 随机起点
  for (let p = base; p < base + 200; p++) {
    try {
      execFileSync(
        process.execPath,
        [
          '-e',
          `const net=require('net');const s=net.connect(${p},'127.0.0.1');s.on('connect',()=>process.exit(1));s.on('error',()=>process.exit(0));`,
        ],
        { stdio: 'ignore', timeout: 1500 }
      );
      return p;
    } catch {
      /* 被占用，试下一个 */
    }
  }
  return base;
}

let chromeChild = null;
let startedByUs = false;

function startChrome(port) {
  const chrome = findChrome();
  if (!chrome) {
    log('未找到 Chrome/Chromium，无法启动浏览器');
    writeLastError('未找到 Chrome/Chromium/Edge 浏览器');
    return false;
  }
  try {
    mkdirSync(PROFILE_DIR, { recursive: true });
    const args = [
      `--remote-debugging-port=${port}`,
      `--user-data-dir=${PROFILE_DIR}`,
      'about:blank',
      ...BROWSER_FLAGS,
    ];
    log('启动 Chrome:', chrome, args.join(' '));
    chromeChild = spawn(chrome, args, { stdio: 'ignore' });
    startedByUs = true;
    chromeChild.on('exit', (code) => {
      log('Chrome 退出, code=', code);
      if (startedByUs) clearPortFile();
      chromeChild = null;
    });
    return true;
  } catch (e) {
    log('启动 Chrome 失败:', e.message);
    writeLastError(`Chrome 启动失败: ${e.message}`);
    return false;
  }
}

// ---------------------------------------------------------------------------
// 主流程：确保 Chrome 就绪 → 托管 chrome-devtools-mcp
// ---------------------------------------------------------------------------
async function main() {
  const portFile = readPortFile();
  let port = portFile?.port ?? 0;
  let chromeReady = port > 0 && probeCdp(port, 2000);

  if (!chromeReady) {
    // 需要（重新）启动：先拿独占锁，避免与品悟 BrowserManager 双启同一 profile
    const lockPath = join(dirname(CDP_PORT_JSON), 'start.lock');
    mkdirSync(dirname(CDP_PORT_JSON), { recursive: true });
    let lockFd = null;
    try {
      lockFd = openSync(lockPath, 'wx');
    } catch {
      log('浏览器启动锁被占用，等待另一个启动者…');
      const deadline = Date.now() + 20000;
      while (Date.now() < deadline && !chromeReady) {
        try {
          lockFd = openSync(lockPath, 'wx');
        } catch {
          // stale 锁（持有者崩溃/被杀后残留 >60s）：抢占删除后重试。
          if (lockFileStale(lockPath)) {
            log('启动锁 stale，抢占删除');
            try {
              unlinkSync(lockPath);
            } catch {
              /* ignore */
            }
            continue;
          }
          const pf = readPortFile();
          if (pf?.port && probeCdp(pf.port, 1000)) {
            port = pf.port;
            chromeReady = true;
          } else {
            await sleep(300);
          }
        }
      }
    }
    // 记录持有者 pid（诊断 + 与 Rust 侧 stale 判定一致）。
    if (lockFd != null) {
      try {
        writeSync(lockFd, String(process.pid));
      } catch {
        /* ignore */
      }
    }
    if (!chromeReady && lockFd != null) {
      try {
        // 持锁后二次确认（品悟可能刚启动完）
        const pf = readPortFile();
        if (pf?.port && probeCdp(pf.port, 1000)) {
          port = pf.port;
          chromeReady = true;
        } else {
          port = pickFreePort();
          if (startChrome(port)) {
            chromeReady = probeCdp(port, 15000);
            if (chromeReady) writePortFile(port, 'mcp');
            else log('Chrome 已启动但 CDP 未就绪');
          }
        }
      } finally {
        closeSync(lockFd);
        try {
          unlinkSync(lockPath);
        } catch {
          /* ignore */
        }
      }
    }
  }

  if (chromeReady) {
    // 本次成功，清掉历史失败记录（若 Chrome 后崩，下次启动失败会重新写）。
    clearLastError();
    log('使用 Chrome CDP 端口:', port);
  } else {
    // Chrome 不可用（未找到 / 启动失败 / CDP 未就绪）：直接退出。
    // chrome-devtools-mcp 启动时会同步连接 `--browser-url`，连不上会抛错退出，
    // 工具根本不会注册——与其以端口 0 误导 spawn，不如干净退出并给出可读日志；
    // 引擎对非 required server 的启动失败是非致命的，品悟 BrowserManager 之后
    // 兜底拉起 Chrome，下次会话重试即恢复。
    if (startedByUs && chromeChild) {
      // Chrome 拉起来了但 CDP 没就绪：记录具体原因，供 Rust 侧注入模型可见提示。
      writeLastError('Chrome 已启动但 CDP 未就绪');
    }
    // 未找到 Chrome / Chrome 启动失败的原因已由 startChrome 写入 last-error.json。
    log('浏览器不可用：未找到 Chrome 或 CDP 未就绪，退出（品悟会兜底启动 Chrome，重试后恢复）');
    // 本包装可能已启动 Chrome 但 CDP 未就绪：退出前清理自启实例，避免孤儿进程
    // 占住 profile 单实例锁导致后续所有启动尝试失败。
    cleanup();
    process.exit(1);
  }

  // 托管官方 chrome-devtools-mcp：stdio 继承（MCP 协议），stderr 日志透传
  const mcpArgs = [
    MCP_BIN,
    '--browser-url',
    `http://127.0.0.1:${port}`,
    '--no-usage-statistics',
    '--no-performance-crux',
    ...EXTRA_ARGS,
  ];
  log('启动 chrome-devtools-mcp:', process.execPath, mcpArgs.join(' '));

  const child = spawn(process.execPath, mcpArgs, {
    stdio: 'inherit',
    env: {
      ...process.env,
      CHROME_DEVTOOLS_MCP_NO_UPDATE_CHECKS: '1', // 离线：禁用更新检查
      CI: '1', // 离线：禁用 usage statistics
    },
  });
  child.on('exit', (code, signal) => {
    log('chrome-devtools-mcp 退出', { code, signal });
    cleanup();
    process.exit(code ?? (signal ? 1 : 0));
  });
  child.on('error', (err) => {
    log('chrome-devtools-mcp 启动失败:', err.message);
    cleanup();
    process.exit(1);
  });
}

function cleanup() {
  // 只有我们启动的 Chrome 才清理；品悟 BrowserManager 启动的由品悟负责。
  if (startedByUs && chromeChild) {
    log('清理本包装启动的 Chrome (pid=', chromeChild.pid, ')');
    try {
      chromeChild.kill('SIGTERM');
    } catch {
      /* ignore */
    }
    clearPortFile();
  }
}

process.on('SIGINT', () => {
  cleanup();
  process.exit(130);
});
process.on('SIGTERM', () => {
  cleanup();
  process.exit(143);
});

main().catch((e) => {
  console.error('[browser-wrapper] 致命错误:', e);
  process.exit(1);
});
