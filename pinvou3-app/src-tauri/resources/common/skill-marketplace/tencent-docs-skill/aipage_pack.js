#!/usr/bin/env node
/* eslint-disable no-console */
/**
 * aipage_pack.js — 把一个本地 HTML 目录（或单文件）打包成符合 aicanvas
 * McpImport 规范的 .aipage 压缩包，供 tencent-docs MCP 的导入流程使用。
 *
 * 设计原则：
 *   - tencent-docs skill 自持「打包 + 导入」全链路，
 *     上游 skill（如 smart-page）只负责输出 HTML，不再关心 manifest/zip。
 *   - 跨平台：纯 Node.js（>= 14），零 npm 依赖。
 *     手写 ZIP（store 模式，method=0），同时兼容 macOS / Linux / Windows
 *     原生 cmd / PowerShell（无需 bash / Git Bash / WSL）。
 *
 * Usage:
 *   node aipage_pack.js --html <html_path> [--title <title>] [--output <out_path>]
 *   node aipage_pack.js --dir  <html_dir>  [--title <title>] [--output <out_path>]
 *
 * Behaviors:
 *   1. 创建临时打包目录
 *   2. 将入口 HTML 复制为 index.html（aipage 硬要求）
 *   3. 复制同级 assets/ 目录（如存在），目录模式下复制整个目录的全部文件
 *   4. 生成 manifest.json（标题安全转义；未传 --title 时自动从 <title> 提取，
 *      再 fallback 用文件名/目录名）
 *   5. 生成 janus.manifest.json（固定内容）
 *   6. 扁平化 zip 打包（zip 内无顶层目录），后缀强制为 .aipage
 *   7. 输出结构化结果（供 SKILL 内 agent 直接解析）：
 *        AIPAGE_PATH=...
 *        AIPAGE_SIZE=...
 *        AIPAGE_MD5=...
 *        AIPAGE_TITLE=...
 *
 * Exit codes:
 *   0  成功
 *   1  参数错误
 *   2  源 HTML / 目录不存在或不合法
 *   3  打包失败 / 内部错误
 */

'use strict';

const fs = require('fs');
const path = require('path');
const os = require('os');
const crypto = require('crypto');
const zlib = require('zlib');

// ─────────────────────────────────────────────────────────────────────────────
// 1. 参数解析
// ─────────────────────────────────────────────────────────────────────────────

function usage(exitCode) {
  const msg = [
    'Usage:',
    '  node aipage_pack.js --html <html_path> [--title <title>] [--output <out_path>]',
    '  node aipage_pack.js --dir  <html_dir>  [--title <title>] [--output <out_path>]',
    '',
    '  --html    单个 HTML 文件路径（推荐：smart-page 等上游产物）',
    '  --dir     已组织好的 HTML 目录路径，目录中必须有且仅有一个 .html / .htm 入口',
    '  --title   可选，文档标题；缺省时自动读 <title> 标签，再 fallback 用文件名/目录名',
    '  --output  可选，输出 .aipage 路径；缺省为 <tmpdir>/<stem>.aipage',
    '',
    '示例:',
    '  node aipage_pack.js --html "output/立项方案.html"',
    '  node aipage_pack.js --html "output/邀请函.html" --title "邀请函" --output /tmp/x.aipage',
    '  node aipage_pack.js --dir  "output/site" --title "站点演示"',
    '',
  ].join('\n');
  process.stderr.write(msg);
  process.exit(typeof exitCode === 'number' ? exitCode : 1);
}

function parseArgs(argv) {
  const opts = { html: '', dir: '', title: '', output: '' };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    switch (a) {
      case '--html':
        opts.html = argv[++i] || '';
        break;
      case '--dir':
        opts.dir = argv[++i] || '';
        break;
      case '--title':
        opts.title = argv[++i] || '';
        break;
      case '--output':
        opts.output = argv[++i] || '';
        break;
      case '-h':
      case '--help':
        usage(0);
        break;
      default:
        process.stderr.write(`aipage_pack.js: unknown argument: ${a}\n`);
        usage(1);
    }
  }
  return opts;
}

// ─────────────────────────────────────────────────────────────────────────────
// 2. 工具函数
// ─────────────────────────────────────────────────────────────────────────────

function fail(code, msg) {
  process.stderr.write(`aipage_pack.js: ${msg}\n`);
  process.exit(code);
}

function md5OfFile(filePath) {
  const h = crypto.createHash('md5');
  h.update(fs.readFileSync(filePath));
  return h.digest('hex');
}

function sizeOfFile(filePath) {
  return fs.statSync(filePath).size;
}

function extractHtmlTitle(htmlPath) {
  let html = '';
  try {
    html = fs.readFileSync(htmlPath, 'utf8');
  } catch (_) {
    return '';
  }
  const m = html.match(/<title>([\s\S]*?)<\/title>/i);
  return m ? m[1].trim() : '';
}

function rmrf(p) {
  if (!fs.existsSync(p)) return;
  // Node 14.14+ 支持 rmSync({recursive:true}); 兼容更早版本回退到 rmdirSync
  try {
    fs.rmSync(p, { recursive: true, force: true });
  } catch (_) {
    fs.rmdirSync(p, { recursive: true });
  }
}

function mkTempDir(prefix) {
  return fs.mkdtempSync(path.join(os.tmpdir(), prefix));
}

function copyFile(src, dst) {
  fs.mkdirSync(path.dirname(dst), { recursive: true });
  fs.copyFileSync(src, dst);
}

// 递归复制目录内容到 dstDir（不包含 dstDir 自身的创建）
function copyDirContents(srcDir, dstDir) {
  fs.mkdirSync(dstDir, { recursive: true });
  const entries = fs.readdirSync(srcDir, { withFileTypes: true });
  for (const ent of entries) {
    const sp = path.join(srcDir, ent.name);
    const dp = path.join(dstDir, ent.name);
    if (ent.isDirectory()) {
      copyDirContents(sp, dp);
    } else if (ent.isFile()) {
      fs.copyFileSync(sp, dp);
    }
    // 软链/特殊文件直接忽略，避免打入压缩包污染
  }
}

// 递归收集打包目录下所有相对路径（POSIX 风格，给 zip 用）
function listFilesRel(rootDir) {
  const result = [];
  function walk(absDir, relDir) {
    const entries = fs.readdirSync(absDir, { withFileTypes: true });
    for (const ent of entries) {
      const abs = path.join(absDir, ent.name);
      const rel = relDir ? `${relDir}/${ent.name}` : ent.name;
      if (shouldExcludeName(ent.name)) continue;
      if (ent.isDirectory()) {
        walk(abs, rel);
      } else if (ent.isFile()) {
        result.push({ abs, rel });
      }
    }
  }
  walk(rootDir, '');
  return result;
}

// 排除 macOS / Windows 副产物（与原 .sh 保持一致）
function shouldExcludeName(name) {
  if (name === '__MACOSX') return true;
  if (name === '.DS_Store') return true;
  if (name === 'Thumbs.db') return true;
  if (name.startsWith('._')) return true;
  return false;
}

// ─────────────────────────────────────────────────────────────────────────────
// 3. 手写 ZIP（store 模式 + DEFLATE 模式自动选择，扁平、无目录条目、无外部依赖）
//    采用 ZIP 标准（PKZIP appnote 6.3.x），仅使用 method=0/8、CRC32、本地头/中央目录头/EOCD。
//    不支持 Zip64（aipage 单文件不会大到需要 Zip64）。
// ─────────────────────────────────────────────────────────────────────────────

// CRC32（标准多项式 0xEDB88320），构建查表，处理 Buffer
const CRC_TABLE = (() => {
  const t = new Uint32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) {
      c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    }
    t[n] = c >>> 0;
  }
  return t;
})();

function crc32(buf) {
  let c = 0xffffffff;
  for (let i = 0; i < buf.length; i++) {
    c = CRC_TABLE[(c ^ buf[i]) & 0xff] ^ (c >>> 8);
  }
  return (c ^ 0xffffffff) >>> 0;
}

// 把 JS Date 转为 DOS 时间/日期
function toDosDateTime(date) {
  const year = Math.max(1980, date.getFullYear());
  const dosTime =
    ((date.getHours() & 0x1f) << 11) |
    ((date.getMinutes() & 0x3f) << 5) |
    ((Math.floor(date.getSeconds() / 2)) & 0x1f);
  const dosDate =
    (((year - 1980) & 0x7f) << 9) |
    (((date.getMonth() + 1) & 0x0f) << 5) |
    (date.getDate() & 0x1f);
  return { dosTime, dosDate };
}

/**
 * 创建符合 aipage 要求的扁平 zip。
 * @param {string} outPath  输出 .aipage 文件路径
 * @param {{abs:string, rel:string}[]} files  待打包文件列表，rel 必须是 POSIX 风格相对路径
 */
function buildZip(outPath, files) {
  const localChunks = [];
  const centralChunks = [];
  let offset = 0;
  const now = new Date();
  const { dosTime, dosDate } = toDosDateTime(now);

  for (const f of files) {
    const data = fs.readFileSync(f.abs);
    const nameBuf = Buffer.from(f.rel, 'utf8');
    const crc = crc32(data);
    const uncompressedSize = data.length;

    // 选择压缩算法：默认 DEFLATE（method=8），但若压缩反而变大则回退到 STORE（method=0）
    let method = 8;
    let compressed = zlib.deflateRawSync(data, { level: 9 });
    if (compressed.length >= uncompressedSize) {
      method = 0;
      compressed = data;
    }
    const compressedSize = compressed.length;

    // ── Local file header (30 bytes + name + extra(0))
    const lfh = Buffer.alloc(30);
    lfh.writeUInt32LE(0x04034b50, 0);            // signature
    lfh.writeUInt16LE(20, 4);                     // version needed
    // bit 11: UTF-8 file name; 其它位为 0（无加密、无 data descriptor）
    lfh.writeUInt16LE(0x0800, 6);                 // general purpose bit flag
    lfh.writeUInt16LE(method, 8);                 // compression method
    lfh.writeUInt16LE(dosTime, 10);
    lfh.writeUInt16LE(dosDate, 12);
    lfh.writeUInt32LE(crc, 14);
    lfh.writeUInt32LE(compressedSize, 18);
    lfh.writeUInt32LE(uncompressedSize, 22);
    lfh.writeUInt16LE(nameBuf.length, 26);
    lfh.writeUInt16LE(0, 28);                     // extra length

    localChunks.push(lfh, nameBuf, compressed);

    // ── Central directory header (46 bytes + name + extra(0) + comment(0))
    const cdh = Buffer.alloc(46);
    cdh.writeUInt32LE(0x02014b50, 0);             // signature
    cdh.writeUInt16LE(20, 4);                      // version made by
    cdh.writeUInt16LE(20, 6);                      // version needed
    cdh.writeUInt16LE(0x0800, 8);                  // general purpose bit flag
    cdh.writeUInt16LE(method, 10);                 // compression method
    cdh.writeUInt16LE(dosTime, 12);
    cdh.writeUInt16LE(dosDate, 14);
    cdh.writeUInt32LE(crc, 16);
    cdh.writeUInt32LE(compressedSize, 20);
    cdh.writeUInt32LE(uncompressedSize, 24);
    cdh.writeUInt16LE(nameBuf.length, 28);
    cdh.writeUInt16LE(0, 30);                      // extra length
    cdh.writeUInt16LE(0, 32);                      // comment length
    cdh.writeUInt16LE(0, 34);                      // disk number start
    cdh.writeUInt16LE(0, 36);                      // internal file attrs
    cdh.writeUInt32LE(0, 38);                      // external file attrs
    cdh.writeUInt32LE(offset, 42);                 // relative offset of local header

    centralChunks.push(cdh, nameBuf);

    offset += lfh.length + nameBuf.length + compressed.length;
  }

  const centralStart = offset;
  const centralBuf = Buffer.concat(centralChunks);
  const centralSize = centralBuf.length;

  // ── End of central directory record
  const eocd = Buffer.alloc(22);
  eocd.writeUInt32LE(0x06054b50, 0);              // signature
  eocd.writeUInt16LE(0, 4);                        // disk number
  eocd.writeUInt16LE(0, 6);                        // disk with central dir
  eocd.writeUInt16LE(files.length, 8);             // entries on this disk
  eocd.writeUInt16LE(files.length, 10);            // total entries
  eocd.writeUInt32LE(centralSize, 12);             // central dir size
  eocd.writeUInt32LE(centralStart, 16);            // central dir offset
  eocd.writeUInt16LE(0, 20);                       // comment length

  const out = Buffer.concat([Buffer.concat(localChunks), centralBuf, eocd]);
  fs.mkdirSync(path.dirname(outPath), { recursive: true });
  fs.writeFileSync(outPath, out);
}

// ─────────────────────────────────────────────────────────────────────────────
// 4. 主流程
// ─────────────────────────────────────────────────────────────────────────────

function main() {
  const opts = parseArgs(process.argv.slice(2));

  if (opts.html && opts.dir) {
    process.stderr.write('aipage_pack.js: --html 与 --dir 二选一，不可同时使用\n');
    usage(1);
  }
  if (!opts.html && !opts.dir) {
    process.stderr.write('aipage_pack.js: 必须指定 --html 或 --dir\n');
    usage(1);
  }

  // ── 创建临时打包目录 ────────────────────────────────────────────────
  const packDir = mkTempDir('aipage_pack_');
  let cleaned = false;
  const cleanup = () => {
    if (cleaned) return;
    cleaned = true;
    rmrf(packDir);
  };
  process.on('exit', cleanup);
  process.on('SIGINT', () => { cleanup(); process.exit(130); });
  process.on('SIGTERM', () => { cleanup(); process.exit(143); });

  let stem = '';
  let entryHtmlInPack = '';

  try {
    if (opts.html) {
      if (!fs.existsSync(opts.html) || !fs.statSync(opts.html).isFile()) {
        fail(2, `HTML 不存在: ${opts.html}`);
      }
      const htmlAbs = path.resolve(opts.html);
      const htmlAbsDir = path.dirname(htmlAbs);
      const base = path.basename(htmlAbs);
      const ext = path.extname(base).toLowerCase();
      stem = (ext === '.html' || ext === '.htm') ? base.slice(0, -ext.length) : base;

      copyFile(htmlAbs, path.join(packDir, 'index.html'));
      const assetsDir = path.join(htmlAbsDir, 'assets');
      if (fs.existsSync(assetsDir) && fs.statSync(assetsDir).isDirectory()) {
        copyDirContents(assetsDir, path.join(packDir, 'assets'));
      }
      entryHtmlInPack = path.join(packDir, 'index.html');
    } else {
      if (!fs.existsSync(opts.dir) || !fs.statSync(opts.dir).isDirectory()) {
        fail(2, `目录不存在: ${opts.dir}`);
      }
      const htmlDirAbs = path.resolve(opts.dir);

      // 找入口 HTML：优先 index.html / index.htm，其次唯一 *.html / *.htm
      let entry = '';
      if (fs.existsSync(path.join(htmlDirAbs, 'index.html'))) {
        entry = path.join(htmlDirAbs, 'index.html');
      } else if (fs.existsSync(path.join(htmlDirAbs, 'index.htm'))) {
        entry = path.join(htmlDirAbs, 'index.htm');
      } else {
        const candidates = fs.readdirSync(htmlDirAbs)
          .filter((n) => /\.html?$/i.test(n))
          .map((n) => path.join(htmlDirAbs, n))
          .filter((p) => fs.statSync(p).isFile());
        if (candidates.length === 1) {
          entry = candidates[0];
        } else if (candidates.length === 0) {
          fail(2, `目录下找不到 HTML 入口: ${htmlDirAbs}`);
        } else {
          fail(2, `目录下存在多个 HTML，请显式 --html 指定: ${candidates.join(' ')}`);
        }
      }
      stem = path.basename(htmlDirAbs);
      copyDirContents(htmlDirAbs, packDir);
      const entryName = path.basename(entry);
      if (entryName !== 'index.html') {
        const src = path.join(packDir, entryName);
        const dst = path.join(packDir, 'index.html');
        // 入口归一化为 index.html；如果同名 index.html 与入口同名（理论不会到这里）则跳过
        if (src !== dst) {
          fs.renameSync(src, dst);
        }
      }
      entryHtmlInPack = path.join(packDir, 'index.html');
    }

    // ── 推导 TITLE ──────────────────────────────────────────────────
    let title = opts.title;
    if (!title) {
      title = extractHtmlTitle(entryHtmlInPack) || stem;
    }

    // ── 默认 OUT ───────────────────────────────────────────────────
    let zipOut = opts.output;
    if (!zipOut) {
      zipOut = path.join(os.tmpdir(), `${stem}.aipage`);
    }
    // 后缀强制 .aipage
    const lower = zipOut.toLowerCase();
    if (lower.endsWith('.aipage')) {
      // 保持原样
    } else if (lower.endsWith('.page')) {
      zipOut = zipOut.slice(0, -'.page'.length) + '.aipage';
    } else if (lower.endsWith('.zip')) {
      zipOut = zipOut.slice(0, -'.zip'.length) + '.aipage';
    } else {
      zipOut = zipOut + '.aipage';
    }
    fs.mkdirSync(path.dirname(zipOut), { recursive: true });
    if (fs.existsSync(zipOut)) {
      fs.unlinkSync(zipOut);
    }

    // ── 生成 manifest.json ────────────────────────────────────────
    const manifest = { entry: 'index.html', title, version: '1.0' };
    fs.writeFileSync(
      path.join(packDir, 'manifest.json'),
      JSON.stringify(manifest, null, 2),
      'utf8',
    );

    // ── 生成 janus.manifest.json（固定内容）─────────────────────
    fs.writeFileSync(
      path.join(packDir, 'janus.manifest.json'),
      '{"version":"1.0.0","render_engine":"native","scene":""}',
      'utf8',
    );

    // ── 校验 ─────────────────────────────────────────────────────
    if (!fs.existsSync(path.join(packDir, 'index.html'))) {
      fail(3, 'missing index.html');
    }
    if (!fs.existsSync(path.join(packDir, 'manifest.json'))) {
      fail(3, 'missing manifest.json');
    }
    try {
      JSON.parse(fs.readFileSync(path.join(packDir, 'manifest.json'), 'utf8'));
    } catch (_) {
      fail(3, 'manifest.json 不是合法 JSON');
    }

    // ── 打包（扁平化）────────────────────────────────────────────
    const files = listFilesRel(packDir);
    if (files.length === 0) {
      fail(3, '打包目录为空');
    }
    buildZip(zipOut, files);

    if (!fs.existsSync(zipOut)) {
      fail(3, `zip 没产出: ${zipOut}`);
    }

    const size = sizeOfFile(zipOut);
    const md5 = md5OfFile(zipOut);

    // ── 结构化输出（与 .sh 完全一致）────────────────────────────
    process.stdout.write(`AIPAGE_PATH=${zipOut}\n`);
    process.stdout.write(`AIPAGE_SIZE=${size}\n`);
    process.stdout.write(`AIPAGE_MD5=${md5}\n`);
    process.stdout.write(`AIPAGE_TITLE=${title}\n`);
  } finally {
    cleanup();
  }
}

main();
