#!/usr/bin/env node
/**
 * browser-wrapper.mjs —— 品悟浏览器 MCP server 的 stdio 协调包装（懒启动代理）。
 *
 * 职责：
 *  1. 懒启动：引擎在会话首个 turn 即 connect 全部 MCP server（CodeWhale
 *     `McpPool::connect_all`），若本包装在进程启动时就拉起 Chrome，则每个工作
 *     模式会话的首条消息都会常驻一个屏外 Chrome（数百 MB）。因此本包装先以
 *     shim 身份直接应答 MCP 握手（`initialize` / `ping` / `tools/list`，目录
 *     来自构建期捕获的 catalog-shim.json），**直到首个 `tools/call`（或其他真实
 *     请求）到达**才启动 Chrome 与官方 chrome-devtools-mcp 子进程，随后转为
 *     透明 stdio 代理。
 *  2. 与品悟桌面端（Rust BrowserManager）协调"专用有头 Chrome"实例的生命周期，
 *     双方通过 `~/.pinvou3/browser/cdp-port.json` + 独占锁文件幂等协调：
 *     - 端口文件有效（Chrome 还活着）→ 直接复用；
 *     - 否则自己启动 Chrome（隐藏窗口、独立 profile、随机 CDP 端口）并写回端口文件。
 *  3. 以 `--browser-url` 把官方 chrome-devtools-mcp 指向该 Chrome。
 *  4. 强制离线：关闭遥测/更新检查/CrUX 上报。
 *
 * 协议约束：MCP 走 stdin/stdout（JSON-RPC over stdio，NDJSON 行分帧），本包装
 * 往 stdout 只写协议消息；日志一律走 stderr。
 *
 * 用法：
 *   node browser-wrapper.mjs <chrome-devtools-mcp-bin> <cdp-port-json> <profile-dir> [extra-args...]
 *
 * 退出：wrapper 是 MCP server 的父进程，启动后以子进程方式托管
 * chrome-devtools-mcp，自身生命周期即 MCP server 生命周期；若本包装启动过
 * Chrome，退出前会清理它。
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

// chrome-devtools-mcp 的运行时要求（上游 package.json engines）：
// ^20.19.0 || ^22.12.0 || >=23。系统 node 过旧时 shim 仍能应答握手/工具目录
// （构建期捕获的 catalog 文件），但首个真实请求会失败并给出可读原因。
function nodeTooOld() {
  const [major, minor] = process.versions.node.split('.').map(Number);
  return !(major >= 23 || (major === 22 && minor >= 12) || (major === 20 && minor >= 19));
}

// ---------------------------------------------------------------------------
// Chrome 可执行文件探测（macOS / Linux / Windows）
// ---------------------------------------------------------------------------
function findChrome() {
  // 显式覆盖（测试/特殊安装路径）：设置后只用该路径、不存在即判不可用——
  // 不得回落系统候选，否则测试会拉起真实浏览器。
  if (process.env.PINVOU_BROWSER_CHROME_PATH) {
    return existsSync(process.env.PINVOU_BROWSER_CHROME_PATH)
      ? process.env.PINVOU_BROWSER_CHROME_PATH
      : null;
  }
  const candidates = [];
  switch (process.platform) {
    case 'darwin': {
      // 与 Rust 侧 platform::os::macos::chrome_candidates 保持一致（漂移时两侧
      // 同步）：系统级 /Applications 优先，~/Applications 用户级安装兜底。
      const homeApps = (process.env.HOME || '') + '/Applications';
      candidates.push(
        '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',
        '/Applications/Chromium.app/Contents/MacOS/Chromium',
        '/Applications/Brave Browser.app/Contents/MacOS/Brave Browser',
        '/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge'
      );
      if (process.env.HOME) {
        candidates.push(
          `${homeApps}/Google Chrome.app/Contents/MacOS/Google Chrome`,
          `${homeApps}/Chromium.app/Contents/MacOS/Chromium`
        );
      }
      break;
    }
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
    // 收紧端口文件权限：CDP 无鉴权，同机其他本地用户不应能读到端口坐标。
    // mode 随创建生效（writeFileSync 仅新建时应用 mode），消除先写后 chmod 的
    // 宽松权限窗口；Windows 无 chmod 语义，由平台忽略。
    writeFileSync(tmp, JSON.stringify({ port, pid: process.pid, owner, started_at: Date.now() }), {
      mode: 0o600,
    });
    // tmp 复用上次的残留文件时 mode 不生效（writeFileSync 仅新建应用 mode），补一刀。
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
  // 区间耗尽：返回 0 由调用方按失败处理（与 Rust 侧 pick_free_port 报错同口径；
  // 回落到已知被占的 base 只会让 Chrome bind 失败、白等 15s 探测超时）。
  return 0;
}

let chromeChild = null;
let startedByUs = false;

async function startChrome(port) {
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
      // 探测仍判"可用"，必须把原因落盘，否则 last-error 注入机制在该场景失效。
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
      // 仅在代理建立后退出进程：启动期（probeCdp 同步阻塞期间死亡等）退出权归
      // ensureBrowserRunning 的失败出口——它还要给触发启动的请求应答可读错误，
      // 此处抢先 exit 会让引擎只看到连接断开、拿不到原因。
      if (state !== 'proxy') return;
      void cleanup().finally(() => process.exit(1));
    });
    // spawn 失败（ENOENT/EACCES）走异步 error 事件：让事件循环转一拍，使上面的
    // error 回调有机会在 CDP 探测（execFileSync 同步阻塞、事件循环不流转）之前
    // 落盘具体原因；否则调用方只能记录泛化的「CDP 未就绪」。
    await new Promise((resolve) => setImmediate(resolve));
    if (!chromeChild) return false;
    return true;
  } catch (e) {
    log('启动 Chrome 失败:', e.message);
    writeLastError(`Chrome 启动失败: ${e.message}`);
    return false;
  }
}

// ---------------------------------------------------------------------------
// Chrome 协调：复用端口文件指向的存活实例，否则持独占锁自启。
// 成功返回端口号；失败抛错（原因已写入 last-error.json），调用方留在 shim 态
// 继续服务握手/目录，不退出进程——懒启动语义下 connect 阶段绝不能产生失败噪音。
// ---------------------------------------------------------------------------
async function ensureBrowserRunning() {
  const portFile = readPortFile();
  let port = portFile?.port ?? 0;
  if (port > 0 && probeCdp(port, 2000)) {
    return port;
  }

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
  let chromeReady = false;
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
    throw new Error('等待浏览器启动锁超时');
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
        if (port === 0) {
          writeLastError('CDP 端口区间耗尽（9222-12221 扫描 200 个端口均被占用）');
          throw new Error('无可用 CDP 端口');
        }
        if (await startChrome(port)) {
          chromeReady = probeCdp(port, 15000);
          if (chromeReady && !writePortFile(port, 'mcp')) {
            // Chrome 已就绪但端口文件没落盘：Rust 侧永远发现不了该实例
            // （前端 Tab 不出现），用户触发 ensure_started 还会对同一
            // profile 双启 Chrome 撞单实例锁。按致命处理：记录原因后走统一
            // 失败出口（finally 释放启动锁，下方回收自启 Chrome）。
            writeLastError(`CDP 端口文件写入失败: ${CDP_PORT_JSON}`);
            chromeReady = false;
          }
          if (!chromeReady && chromeChild) {
            // Chrome 拉起来了但 CDP 没就绪：记录具体原因（spawn 失败的具体
            // 原因已由 startChrome 的 error 回调先行落盘，此处不覆盖）。
            writeLastError('Chrome 已启动但 CDP 未就绪');
          }
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

  if (!chromeReady) {
    // 失败出口：回收自启 Chrome，避免孤儿进程占住 profile 单实例锁导致后续
    // 所有启动尝试失败；进程保持 shim 态存活（懒启动下不在 connect 期产生
    // 引擎侧失败噪音），首个真实请求拿到可读错误。
    await killChromeChild();
    throw new Error('浏览器不可用：未找到 Chrome 或 CDP 未就绪（重试可恢复）');
  }

  // 本次成功，清掉历史失败记录（若 Chrome 后崩，下次启动失败会重新写）。
  clearLastError();
  log('使用 Chrome CDP 端口:', port);
  return port;
}

// ---------------------------------------------------------------------------
// MCP 目录（initialize / tools/list 应答来源）
//
// 构建期 vendor 脚本捕获 `catalog-shim.json`（与 MCP bin 同级的包根目录）：
// 官方 server 的工具目录是静态注册、无需浏览器连接，因此可以离线捕获并在 shim
// 阶段原样应答。文件缺失（开发环境直接指向自编译 bin 等）时运行时探测一次
// （不启动 Chrome：上游仅在 tools/call 时才经 getContext() 连接浏览器）。
// ---------------------------------------------------------------------------
const CATALOG_JSON = join(dirname(MCP_BIN), '..', '..', '..', 'catalog-shim.json');
let catalog = null;

function validCatalog(value) {
  return (
    value &&
    typeof value === 'object' &&
    value.initializeResult &&
    typeof value.initializeResult === 'object' &&
    value.toolsListResult &&
    Array.isArray(value.toolsListResult.tools)
  );
}

function loadCatalogFile() {
  try {
    const data = JSON.parse(readFileSync(CATALOG_JSON, 'utf8'));
    if (validCatalog(data)) return data;
    log('catalog-shim.json 形状不符，回退运行时探测');
  } catch {
    /* 无文件/坏 json → 运行时探测 */
  }
  return null;
}

async function probeCatalog() {
  return new Promise((resolve) => {
    let child;
    try {
      child = spawn(
        process.execPath,
        [MCP_BIN, '--no-usage-statistics', '--no-performance-crux'],
        {
          stdio: ['pipe', 'pipe', 'ignore'],
          env: {
            ...process.env,
            CHROME_DEVTOOLS_MCP_NO_UPDATE_CHECKS: '1',
            CI: '1',
          },
        }
      );
    } catch {
      resolve(null);
      return;
    }
    let buf = '';
    let initializeResult = null;
    const tools = [];
    let done = false;
    let listId = 100;
    const finish = (value) => {
      if (done) return;
      done = true;
      clearTimeout(timer);
      try {
        child.kill('SIGKILL');
      } catch {
        /* ignore */
      }
      resolve(value);
    };
    const timer = setTimeout(() => finish(null), 20000);
    child.on('error', () => finish(null));
    child.on('exit', () =>
      finish(
        initializeResult && tools.length > 0
          ? { initializeResult, toolsListResult: { tools } }
          : null
      )
    );
    child.stdout.on('data', (chunk) => {
      buf += chunk;
      let idx;
      while ((idx = buf.indexOf('\n')) >= 0) {
        const line = buf.slice(0, idx);
        buf = buf.slice(idx + 1);
        let msg;
        try {
          msg = JSON.parse(line);
        } catch {
          continue;
        }
        if (msg.id === 1 && msg.result) {
          initializeResult = msg.result;
          child.stdin.write(JSON.stringify({ jsonrpc: '2.0', method: 'notifications/initialized' }) + '\n');
          child.stdin.write(JSON.stringify({ jsonrpc: '2.0', id: listId, method: 'tools/list', params: {} }) + '\n');
        } else if (msg.id === listId && msg.result) {
          tools.push(...(msg.result.tools ?? []));
          if (msg.result.nextCursor) {
            listId += 1;
            child.stdin.write(
              JSON.stringify({ jsonrpc: '2.0', id: listId, method: 'tools/list', params: { cursor: msg.result.nextCursor } }) + '\n'
            );
          } else {
            finish({ initializeResult, toolsListResult: { tools } });
          }
        }
      }
    });
    child.stdin.write(
      JSON.stringify({
        jsonrpc: '2.0',
        id: 1,
        method: 'initialize',
        params: {
          protocolVersion: '2024-11-05',
          capabilities: {},
          clientInfo: { name: 'pinvou-browser-wrapper', version: '0' },
        },
      }) + '\n'
    );
  });
}

// ---------------------------------------------------------------------------
// stdio shim / 透明代理状态机
// ---------------------------------------------------------------------------
// shim     ：wrapper 直接应答 initialize/ping/tools/list（不启动 Chrome）；
//            其余一切请求触发启动。
// starting ：Chrome + MCP 子进程启动中，到达的请求行缓冲、取消通知登记；
//            启动失败 → 缓冲请求统一报错，回到 shim（可重试）。
// proxy    ：双向透传（stdin 行 → 子进程 stdin；子进程 stdout → stdout）。
let state = 'shim';
let startPromise = null;
let mcpChild = null;
let clientInitializeParams = null;
let bufferedLines = [];
const cancelledIds = new Set();

function writeOut(msg) {
  process.stdout.write(JSON.stringify(msg) + '\n');
}

function respondError(id, message) {
  writeOut({ jsonrpc: '2.0', id, error: { code: -32000, message } });
}

function handleShimRequest(msg, raw) {
  // 目录不可得（catalog 文件缺失且运行时探测失败）：握手/目录如实报错，
  // 进程保持 shim 态存活，引擎下轮重连可恢复。
  if (!catalog && (msg.method === 'initialize' || msg.method === 'tools/list')) {
    respondError(msg.id, 'browser MCP 工具目录不可用（catalog-shim.json 缺失且探测失败）');
    return;
  }
  switch (msg.method) {
    case 'initialize': {
      clientInitializeParams = msg.params ?? null;
      // protocolVersion 回显客户端请求值（上游 SDK 同款协商行为；实测
      // chrome-devtools-mcp 对 2024-11-05 请求应答 2024-11-05）。
      const result = { ...catalog.initializeResult };
      if (typeof msg.params?.protocolVersion === 'string') {
        result.protocolVersion = msg.params.protocolVersion;
      }
      writeOut({ jsonrpc: '2.0', id: msg.id, result });
      return;
    }
    case 'ping':
      writeOut({ jsonrpc: '2.0', id: msg.id, result: {} });
      return;
    case 'tools/list':
      writeOut({ jsonrpc: '2.0', id: msg.id, result: catalog.toolsListResult });
      return;
    default:
      triggerStart(raw);
  }
}

function handleLine(line) {
  if (state === 'proxy') {
    writeChild(line);
    return;
  }
  let msg = null;
  try {
    msg = JSON.parse(line);
  } catch {
    /* 坏行丢弃（协议对端是引擎，正常不会发生） */
  }
  if (state === 'starting') {
    // 启动期：取消通知登记（flush 时跳过该请求），其余请求缓冲，通知丢弃。
    if (msg && msg.method === 'notifications/cancelled' && msg.params?.requestId != null) {
      cancelledIds.add(msg.params.requestId);
    } else if (msg && msg.id != null) {
      bufferedLines.push(line);
    }
    return;
  }
  // shim 态
  if (!msg) return;
  if (msg.id == null) return; // initialized 等通知：无需处理
  handleShimRequest(msg, line);
}

function triggerStart(raw) {
  bufferedLines.push(raw);
  if (startPromise) return;
  state = 'starting';
  startPromise = startProxy();
}

async function startProxy() {
  let port = 0;
  try {
    if (nodeTooOld()) {
      const reason = `Node.js 版本过低（当前 ${process.versions.node}，chrome-devtools-mcp 要求 ^20.19 || ^22.12 || >=23）`;
      writeLastError(reason);
      throw new Error(reason);
    }
    port = await ensureBrowserRunning();
    await spawnMcpChild(port);
  } catch (e) {
    const reason = e?.message || String(e);
    log('浏览器启动失败:', reason);
    const failed = bufferedLines;
    bufferedLines = [];
    cancelledIds.clear();
    state = 'shim';
    startPromise = null;
    for (const raw of failed) {
      try {
        const m = JSON.parse(raw);
        if (m.id != null) respondError(m.id, `浏览器不可用: ${reason}`);
      } catch {
        /* ignore */
      }
    }
    return;
  }
  state = 'proxy';
  startReusedChromeWatchdog(port);
  const pending = bufferedLines;
  bufferedLines = [];
  startPromise = null;
  for (const raw of pending) {
    try {
      const m = JSON.parse(raw);
      if (m.id != null && cancelledIds.has(m.id)) continue; // 启动期已被取消
    } catch {
      /* 坏行照样转发由上游报错 */
    }
    writeChild(raw);
  }
  cancelledIds.clear();
}

function writeChild(line) {
  try {
    mcpChild?.stdin.write(line + '\n');
  } catch {
    /* 子进程已死由 exit 处理器兜底 */
  }
}

// 与 MCP 子进程的握手 id：字符串形式，与引擎的数字 id 不会冲突。
const HANDSHAKE_ID = 'pinvou-wrapper-handshake';

function spawnMcpChild(port) {
  const mcpArgs = [
    MCP_BIN,
    '--browser-url',
    `http://127.0.0.1:${port}`,
    '--no-usage-statistics',
    '--no-performance-crux',
    ...EXTRA_ARGS,
  ];
  log('启动 chrome-devtools-mcp:', process.execPath, mcpArgs.join(' '));
  return new Promise((resolve, reject) => {
    let child;
    try {
      child = spawn(process.execPath, mcpArgs, {
        stdio: ['pipe', 'pipe', 'inherit'], // stderr 日志透传
        env: {
          ...process.env,
          CHROME_DEVTOOLS_MCP_NO_UPDATE_CHECKS: '1', // 离线：禁用更新检查
          CI: '1', // 离线：禁用 usage statistics
        },
      });
    } catch (e) {
      reject(e);
      return;
    }
    mcpChild = child;
    let hsBuf = '';
    let settled = false;
    const timer = setTimeout(() => {
      if (!settled) {
        settled = true;
        reject(new Error('chrome-devtools-mcp 握手超时'));
      }
    }, 20000);
    const onData = (chunk) => {
      hsBuf += chunk;
      let idx;
      while ((idx = hsBuf.indexOf('\n')) >= 0) {
        const line = hsBuf.slice(0, idx);
        hsBuf = hsBuf.slice(idx + 1);
        let msg;
        try {
          msg = JSON.parse(line);
        } catch {
          continue;
        }
        if (msg.id !== HANDSHAKE_ID) continue;
        if (msg.error) {
          settled = true;
          clearTimeout(timer);
          child.stdout.off('data', onData);
          reject(new Error(`chrome-devtools-mcp 握手失败: ${msg.error.message}`));
          return;
        }
        // 握手完成：initialized 通知 + 转入透传（残留缓冲先发，避免丢消息）。
        settled = true;
        clearTimeout(timer);
        child.stdout.off('data', onData);
        writeChild(JSON.stringify({ jsonrpc: '2.0', method: 'notifications/initialized' }));
        if (hsBuf.trim()) process.stdout.write(hsBuf);
        hsBuf = '';
        child.stdout.on('data', (c) => process.stdout.write(c));
        resolve();
        return;
      }
    };
    child.stdout.on('data', onData);
    child.on('error', (err) => {
      log('chrome-devtools-mcp 启动失败:', err.message);
      mcpChild = null;
      if (!settled) {
        settled = true;
        clearTimeout(timer);
        reject(err);
      }
    });
    child.on('exit', (code, signal) => {
      log('chrome-devtools-mcp 退出', { code, signal });
      mcpChild = null; // cleanup() 不再重复 kill（双向退出都不会留下孤儿）
      if (!settled) {
        settled = true;
        clearTimeout(timer);
        reject(new Error(`chrome-devtools-mcp 握手前退出 code=${code}`));
        return;
      }
      void cleanup().finally(() => process.exit(code ?? (signal ? 1 : 0)));
    });
    // 与引擎一致的 initialize 参数（含 protocolVersion 协商与 clientInfo）。
    const params = clientInitializeParams ?? {
      protocolVersion: '2024-11-05',
      capabilities: {},
      clientInfo: { name: 'pinvou', version: '0' },
    };
    try {
      child.stdin.write(
        JSON.stringify({ jsonrpc: '2.0', id: HANDSHAKE_ID, method: 'initialize', params }) + '\n'
      );
    } catch (e) {
      settled = true;
      clearTimeout(timer);
      reject(e);
    }
  });
}

// ---------------------------------------------------------------------------
// 子进程回收（SIGTERM → 3s 宽限 → SIGKILL 升级；Chrome 挂死忽略 SIGTERM 时
// 不放任它占住 profile 单实例锁导致后续启动进入失败循环）
// ---------------------------------------------------------------------------
function waitExit(child, timeoutMs) {
  return new Promise((resolve) => {
    if (child.exitCode != null || child.signalCode != null) {
      resolve();
      return;
    }
    const timer = setTimeout(resolve, timeoutMs);
    child.once('exit', () => {
      clearTimeout(timer);
      resolve();
    });
  });
}

async function killChromeChild() {
  if (!(startedByUs && chromeChild)) return;
  log('清理本包装启动的 Chrome (pid=', chromeChild.pid, ')');
  const victim = chromeChild;
  try {
    victim.kill('SIGTERM');
  } catch {
    /* ignore */
  }
  await waitExit(victim, 3000);
  if (victim.exitCode == null && victim.signalCode == null) {
    try {
      victim.kill('SIGKILL');
    } catch {
      /* ignore */
    }
    await waitExit(victim, 2000);
  }
  clearPortFile();
}

async function cleanup() {
  if (mcpChild && !mcpChild.killed) {
    const victim = mcpChild;
    try {
      victim.kill('SIGTERM');
    } catch {
      /* ignore */
    }
    await waitExit(victim, 3000);
    if (victim.exitCode == null && victim.signalCode == null) {
      try {
        victim.kill('SIGKILL');
      } catch {
        /* ignore */
      }
    }
  }
  // 只有我们启动的 Chrome 才清理；品悟 BrowserManager 启动的由品悟负责。
  await killChromeChild();
}

// ---------------------------------------------------------------------------
// 复用他人（品悟 BrowserManager / 其他会话 wrapper）启动的 Chrome 时
// chromeChild 为 null，没有 exit 事件可监听；而该 Chrome 可能被 UI 停止、或被
// 启动方会话退出时回收。本会话的 --browser-url 端口随之永久失效（兜底重启
// 会用新随机端口），继续存活只会让所有 mcp_browser_* 工具持续报错——周期
// 探测，连续失败即退出，让引擎下次拉起 wrapper 时重新协调端口，自愈恢复。
// ---------------------------------------------------------------------------
function startReusedChromeWatchdog(port) {
  if (startedByUs) return; // 自启路径由 chromeChild 的 exit 事件覆盖
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
      void cleanup().finally(() => process.exit(1));
    }
  }, 10000);
  timer.unref();
}

// ---------------------------------------------------------------------------
// 主流程：加载目录 → shim 待命（懒启动：首个真实请求才拉起 Chrome + MCP）
// ---------------------------------------------------------------------------
let stdinBuf = '';

async function main() {
  catalog = loadCatalogFile();
  if (!catalog) {
    log('catalog-shim.json 缺失，运行时探测工具目录（不启动浏览器）…');
    catalog = await probeCatalog();
  }
  if (!catalog) {
    // 目录不可得：tools/list 直接报错（引擎记录失败状态），进程保持存活。
    // 不退出——懒启动语义下 connect 阶段的退出会让引擎每次重连都刷失败噪音。
    log('工具目录不可用（catalog 文件缺失且运行时探测失败）');
    catalog = null;
  }

  process.stdin.on('data', (chunk) => {
    stdinBuf += chunk;
    let idx;
    while ((idx = stdinBuf.indexOf('\n')) >= 0) {
      const line = stdinBuf.slice(0, idx);
      stdinBuf = stdinBuf.slice(idx + 1);
      if (line.trim()) handleLine(line);
    }
  });
  // 引擎关闭 stdin（断开/会话结束）：无论处于哪一态都退出并回收子进程。
  process.stdin.on('end', () => {
    void cleanup().finally(() => process.exit(0));
  });
  process.stdin.resume();
}

process.on('SIGINT', () => {
  void cleanup().finally(() => process.exit(130));
});
process.on('SIGTERM', () => {
  void cleanup().finally(() => process.exit(143));
});

main().catch((e) => {
  console.error('[browser-wrapper] 致命错误:', e);
  void cleanup().finally(() => process.exit(1));
});
