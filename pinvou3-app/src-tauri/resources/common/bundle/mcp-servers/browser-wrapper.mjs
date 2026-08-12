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
  closeSync,
  existsSync,
  mkdirSync,
  openSync,
  readFileSync,
  renameSync,
  unlinkSync,
  writeFileSync,
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
    renameSync(tmp, CDP_PORT_JSON); // 原子替换
  } catch (e) {
    log('写端口文件失败:', e.message);
  }
}

function clearPortFile() {
  try {
    unlinkSync(CDP_PORT_JSON);
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
    log('使用 Chrome CDP 端口:', port);
  } else {
    log('浏览器不可用，MCP 工具将报错（品悟打开浏览器 Tab 或重试后恢复）');
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
