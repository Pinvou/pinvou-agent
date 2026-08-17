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
      // 与 Rust 侧 platform::os::windows::chrome_candidates 保持一致：
      // env 变量（非硬编码 C 盘）+ Edge 候选（提示文案宣称 Chrome/Chromium/Edge），
      // 绝对路径优先于 PATH 命令名。
      candidates.push(
        process.env.PROGRAMFILES + '\\Google\\Chrome\\Application\\chrome.exe',
        process.env['PROGRAMFILES(X86)'] + '\\Google\\Chrome\\Application\\chrome.exe',
        process.env.LOCALAPPDATA + '\\Google\\Chrome\\Application\\chrome.exe',
        process.env.PROGRAMFILES + '\\Microsoft\\Edge\\Application\\msedge.exe',
        process.env['PROGRAMFILES(X86)'] + '\\Microsoft\\Edge\\Application\\msedge.exe',
        process.env.LOCALAPPDATA + '\\Microsoft\\Edge\\Application\\msedge.exe',
        'chrome',
        'msedge'
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
    return true;
  } catch (e) {
    log('写端口文件失败:', e.message);
    return false;
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
    // at 用 **秒**（与 Rust 侧 `browser_unavailability_reason` 的
    // `duration_since(UNIX_EPOCH).as_secs()` 同单位）：若写毫秒（Date.now()），
    // Rust 侧 `now.saturating_sub(at)` 恒为 0，「24h 内新鲜才注入」门禁成死代码，
    // 过期失败原因会无限期注入。
    writeFileSync(LAST_ERROR_JSON, JSON.stringify({ reason, at: Math.floor(Date.now() / 1000) }));
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
    // profile 内含登录会话/Cookie/缓存：收紧为仅当前用户可访问（与 Rust 侧
    // make_private_dir 一致；Windows 无 POSIX 语义，靠用户目录 ACL）。
    if (process.platform !== 'win32') {
      try {
        chmodSync(PROFILE_DIR, 0o700);
      } catch {
        /* ignore */
      }
    }
    const args = [
      `--remote-debugging-port=${port}`,
      // CDP 无鉴权：显式绑定回环，不依赖各浏览器默认绑定地址。
      '--remote-debugging-address=127.0.0.1',
      `--user-data-dir=${PROFILE_DIR}`,
      'about:blank',
      ...BROWSER_FLAGS,
    ];
    log('启动 Chrome:', chrome, args.join(' '));
    chromeChild = spawn(chrome, args, { stdio: 'ignore' });
    startedByUs = true;
    // spawn 的 ENOENT/EACCES 等失败走异步 error 事件而非同步 throw，必须监听，
    // 否则会变成未捕获异常使 wrapper 崩溃、MCP 子进程孤儿化。
    chromeChild.on('error', (err) => {
      log('Chrome 进程错误:', err.message);
      // spawn 的 ENOENT/EACCES 走这里：Chrome 二进制存在但不可执行时 Rust 静态
      // 探测仍判"可用"，必须把原因落盘，否则 last-error 注入机制在该场景失效
      // （随后 CDP 探测空转超时，退出路径的 writeLastError 守卫因 chromeChild
      // 已被置 null 而跳过）。
      writeLastError(`Chrome 进程启动失败: ${err.message}`);
      chromeChild = null;
    });
    chromeChild.on('exit', (code) => {
      log('Chrome 退出, code=', code);
      if (startedByUs) clearPortFile();
      chromeChild = null;
      // 自启 Chrome 中途死亡（崩溃/被杀）：本会话的 --browser-url 已永久失效
      //（品悟兜底重启会用新随机端口），继续存活只会让所有 mcp_browser_* 工具
      // 持续报错。退出让引擎下次拉起 wrapper 时重新协调 Chrome，自愈恢复。
      // process.exit() 不会终止 MCP 子进程，必须经 cleanup() 先 kill，否则孤儿
      // MCP 持有引擎 stdio 管道，引擎复用管道时会新旧双读竞争。
      cleanup();
      process.exit(1);
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
  // 自启 Chrome 后端口文件写入失败标记：走统一失败出口时保留具体原因，
  // 不被「CDP 未就绪」的泛化记录覆盖。
  let portFileWriteFailed = false;

  if (!chromeReady) {
    // 需要（重新）启动：先拿独占锁，避免与品悟 BrowserManager 双启同一 profile
    const lockPath = join(dirname(CDP_PORT_JSON), 'start.lock');
    mkdirSync(dirname(CDP_PORT_JSON), { recursive: true });
    if (process.platform !== 'win32') {
      try {
        chmodSync(dirname(CDP_PORT_JSON), 0o700);
      } catch {
        /* ignore */
      }
    }
    let lockFd = null;
    try {
      lockFd = openSync(lockPath, 'wx');
    } catch {
      log('浏览器启动锁被占用，等待另一个启动者…');
      const deadline = Date.now() + 20000;
      while (Date.now() < deadline && !chromeReady) {
        try {
          lockFd = openSync(lockPath, 'wx');
          break; // 拿到锁：结束等待（后续不再重新尝试 openSync）
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
    // 等锁 20s 超时（对端 wedged）：锁从未拿到且 Chrome 未就绪——写入原因供
    // Rust 侧注入模型可见的「不可用原因」，否则模型只能给用户泛化引导。
    if (lockFd == null && !chromeReady) {
      writeLastError('等待浏览器启动锁超时（另一个启动者可能已卡死）');
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
            if (chromeReady && !writePortFile(port, 'mcp')) {
              // Chrome 已就绪但端口文件没落盘：Rust 侧永远发现不了该实例
              // （前端 Tab 不出现），用户触发 ensure_started 还会对同一
              // profile 双启 Chrome 撞单实例锁。与 Rust 侧同等场景一致按
              // 致命处理：记录原因后置 chromeReady=false，走下方统一失败
              // 出口（先由 finally 释放启动锁，再 cleanup 回收自启 Chrome
              // 并退出）。
              writeLastError(`CDP 端口文件写入失败: ${CDP_PORT_JSON}`);
              portFileWriteFailed = true;
              chromeReady = false;
            }
            if (!chromeReady && !portFileWriteFailed) log('Chrome 已启动但 CDP 未就绪');
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
    if (startedByUs && chromeChild && !portFileWriteFailed) {
      // Chrome 拉起来了但 CDP 没就绪：记录具体原因，供 Rust 侧注入模型可见提示。
      // （端口文件写入失败的原因已在写入处记录，不覆盖。）
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

  mcpChild = spawn(process.execPath, mcpArgs, {
    stdio: 'inherit',
    env: {
      ...process.env,
      CHROME_DEVTOOLS_MCP_NO_UPDATE_CHECKS: '1', // 离线：禁用更新检查
      CI: '1', // 离线：禁用 usage statistics
    },
  });
  mcpChild.on('exit', (code, signal) => {
    log('chrome-devtools-mcp 退出', { code, signal });
    mcpChild = null; // cleanup() 不再重复 kill（双向退出都不会留下孤儿）
    cleanup();
    process.exit(code ?? (signal ? 1 : 0));
  });
  mcpChild.on('error', (err) => {
    log('chrome-devtools-mcp 启动失败:', err.message);
    mcpChild = null;
    cleanup();
    process.exit(1);
  });

  // 复用他人（品悟 BrowserManager / 其他会话 wrapper）启动的 Chrome 时
  // chromeChild 为 null，没有 exit 事件可监听；而该 Chrome 可能被 UI 停止、或被
  // 启动方会话退出时回收。本会话的 --browser-url 端口随之永久失效（兜底重启
  // 会用新随机端口），继续存活只会让所有 mcp_browser_* 工具持续报错——周期
  // 探测，连续失败即退出，让引擎下次拉起 wrapper 时重新协调端口，自愈恢复。
  // 自启路径由 chromeChild 的 exit 事件覆盖，不走这里。
  if (!startedByUs) {
    let misses = 0;
    const timer = setInterval(() => {
      if (probeCdp(port, 1000)) {
        misses = 0;
        return;
      }
      misses += 1;
      if (misses >= 2) {
        clearInterval(timer);
        log('复用的 Chrome 已失联（可能被停止或启动方会话退出），退出以待重新协调');
        cleanup();
        process.exit(1);
      }
    }, 10000);
    timer.unref();
  }
}

// 托管的 chrome-devtools-mcp 子进程：wrapper 是其父进程，任何退出路径都必须
// 显式 kill——stdio 继承意味着孤儿 MCP 仍持有引擎管道的读端，引擎若复用管道
// 会新旧双读竞争（process.exit() 不会终止子进程）。
let mcpChild = null;

function cleanup() {
  if (mcpChild && !mcpChild.killed) {
    try {
      mcpChild.kill('SIGTERM');
    } catch {
      /* ignore */
    }
  }
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
  cleanup();
  process.exit(1);
});
