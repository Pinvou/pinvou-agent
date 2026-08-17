#!/usr/bin/env node
// 版本号同步脚本：根目录 VERSION 文件是版本号的单一事实来源。
//
// 用法：
//   node scripts/sync-version.mjs          # 把 VERSION 写入各处版本文件
//   node scripts/sync-version.mjs --check  # 只校验各处与 VERSION 一致，不一致时非零退出（供 CI 使用）
//
// 同步目标：
//   1. pinvou3-app/src-tauri/tauri.conf.json 的 "version" 字段
//   2. pinvou3-app/src-tauri/Cargo.toml 的 [package] 下第一处 version = "..." 行（不动依赖版本）
//   3. pinvou-knowledge/Cargo.toml 的 [package] 版本
//   4. pinvou-knowledge/Cargo.lock 中 pinvou-knowledge 包版本
//   5. pinvou3-app/package.json 的 "version" 字段
//   6. pinvou3-app/package-lock.json 的根 "version" 与 packages[""].version（文件不存在时跳过）

import { existsSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const SCRIPT_PATH = fileURLToPath(import.meta.url);
const REPO_ROOT = resolve(dirname(SCRIPT_PATH), '..');
const CHECK_ONLY = process.argv.includes('--check');

const VERSION_FILE = resolve(REPO_ROOT, 'VERSION');
const TAURI_CONF = resolve(REPO_ROOT, 'pinvou3-app/src-tauri/tauri.conf.json');
const CARGO_TOML = resolve(REPO_ROOT, 'pinvou3-app/src-tauri/Cargo.toml');
const KNOWLEDGE_CARGO_TOML = resolve(REPO_ROOT, 'pinvou-knowledge/Cargo.toml');
const KNOWLEDGE_CARGO_LOCK = resolve(REPO_ROOT, 'pinvou-knowledge/Cargo.lock');
const PACKAGE_JSON = resolve(REPO_ROOT, 'pinvou3-app/package.json');
const PACKAGE_LOCK = resolve(REPO_ROOT, 'pinvou3-app/package-lock.json');

// SemVer 2.0.0：禁止数字段前导零，预发布数字标识同样禁止前导零；
// 允许合法的预发布与构建元数据，例如 1.2.3-rc.1+build.7。
const SEMVER_RE = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-((?:0|[1-9]\d*|\d*[A-Za-z-][0-9A-Za-z-]*)(?:\.(?:0|[1-9]\d*|\d*[A-Za-z-][0-9A-Za-z-]*))*))?(?:\+([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?$/;

export function isValidVersion(version) {
  return SEMVER_RE.test(version);
}

// 读取单一事实来源：根目录 VERSION 文件（内容只有一行版本号）
function readTargetVersion() {
  const version = readFileSync(VERSION_FILE, 'utf8').trim();
  if (!isValidVersion(version)) {
    console.error(`VERSION 文件内容不是合法版本号: "${version}"`);
    process.exit(2);
  }
  return version;
}

// JSON 文件：解析后改写 version 字段，保持 2 空格缩进和末尾换行
function readJsonVersion(path) {
  return JSON.parse(readFileSync(path, 'utf8')).version;
}

function writeJsonVersion(path, version) {
  const data = JSON.parse(readFileSync(path, 'utf8'));
  data.version = version;
  writeFileSync(path, JSON.stringify(data, null, 2) + '\n');
}

// package-lock.json：根 version 与 packages[""].version 两处都要同步。
// 两处一致时返回该版本；不一致时返回组合描述串（必然不等于目标版本），
// 使 --check 报错、同步模式写回修正。
function readPackageLockVersion(path) {
  const data = JSON.parse(readFileSync(path, 'utf8'));
  const root = data.version;
  const pkg = data.packages?.['']?.version;
  return root === pkg ? root : `${root}（packages[""] 为 ${pkg}）`;
}

function writePackageLockVersion(path, version) {
  const data = JSON.parse(readFileSync(path, 'utf8'));
  data.version = version;
  if (data.packages && data.packages['']) {
    data.packages[''].version = version;
  }
  writeFileSync(path, JSON.stringify(data, null, 2) + '\n');
}

// Cargo.toml：只在 [package] 段内定位第一处 version = "..."，避免误伤
// [workspace.package] 等出现在 [package] 之前、同样含 version 的段落。
const CARGO_VERSION_RE = /^version\s*=\s*"([^"]*)"/;

// 返回 [package] 段内第一处 version 行的下标；找不到返回 -1
function findCargoPackageVersionLine(lines) {
  let inPackage = false;
  for (let i = 0; i < lines.length; i++) {
    const trimmed = lines[i].trim();
    if (trimmed.startsWith('[')) {
      // 进入新段落；只有 [package] 段内的 version 才是包版本
      inPackage = trimmed === '[package]';
      continue;
    }
    if (inPackage && CARGO_VERSION_RE.test(lines[i])) {
      return i;
    }
  }
  return -1;
}

function readCargoVersion(path) {
  const lines = readFileSync(path, 'utf8').split('\n');
  const index = findCargoPackageVersionLine(lines);
  return index === -1 ? null : CARGO_VERSION_RE.exec(lines[index])[1];
}

function writeCargoVersion(path, version) {
  const lines = readFileSync(path, 'utf8').split('\n');
  const index = findCargoPackageVersionLine(lines);
  if (index === -1) {
    console.error(`未能在 ${path} 的 [package] 段中找到 version 行`);
    process.exit(2);
  }
  lines[index] = lines[index].replace(CARGO_VERSION_RE, `version = "${version}"`);
  writeFileSync(path, lines.join('\n'));
}

export function updateCargoLockPackageVersion(content, packageName, targetVersion = null) {
  const lines = content.split('\n');
  const matches = [];
  for (let start = 0; start < lines.length; start++) {
    if (lines[start].trim() !== '[[package]]') continue;
    let name = null;
    let version = null;
    let versionIndex = -1;
    for (let index = start + 1; index < lines.length && lines[index].trim() !== '[[package]]'; index++) {
      const nameMatch = /^name\s*=\s*"([^"]+)"/u.exec(lines[index]);
      const versionMatch = /^version\s*=\s*"([^"]+)"/u.exec(lines[index]);
      if (nameMatch) name = nameMatch[1];
      if (versionMatch && versionIndex === -1) {
        version = versionMatch[1];
        versionIndex = index;
      }
    }
    if (name === packageName && versionIndex !== -1) {
      matches.push({ version, versionIndex });
    }
  }
  if (matches.length !== 1) {
    throw new Error(`Cargo.lock 中应恰好存在一个 ${packageName} 包，实际找到 ${matches.length} 个`);
  }
  if (targetVersion !== null) {
    const { versionIndex } = matches[0];
    lines[versionIndex] = lines[versionIndex].replace(
      CARGO_VERSION_RE,
      `version = "${targetVersion}"`,
    );
  }
  return { version: matches[0].version, content: lines.join('\n') };
}

function readCargoLockVersion(path, packageName) {
  return updateCargoLockPackageVersion(readFileSync(path, 'utf8'), packageName).version;
}

function writeCargoLockVersion(path, packageName, version) {
  const result = updateCargoLockPackageVersion(readFileSync(path, 'utf8'), packageName, version);
  writeFileSync(path, result.content);
}

export function main() {
  const target = readTargetVersion();
  const targets = [
    { name: 'pinvou3-app/src-tauri/tauri.conf.json', read: () => readJsonVersion(TAURI_CONF), write: () => writeJsonVersion(TAURI_CONF, target) },
    { name: 'pinvou3-app/src-tauri/Cargo.toml', read: () => readCargoVersion(CARGO_TOML), write: () => writeCargoVersion(CARGO_TOML, target) },
    { name: 'pinvou-knowledge/Cargo.toml', read: () => readCargoVersion(KNOWLEDGE_CARGO_TOML), write: () => writeCargoVersion(KNOWLEDGE_CARGO_TOML, target) },
    { name: 'pinvou-knowledge/Cargo.lock', read: () => readCargoLockVersion(KNOWLEDGE_CARGO_LOCK, 'pinvou-knowledge'), write: () => writeCargoLockVersion(KNOWLEDGE_CARGO_LOCK, 'pinvou-knowledge', target) },
    { name: 'pinvou3-app/package.json', read: () => readJsonVersion(PACKAGE_JSON), write: () => writeJsonVersion(PACKAGE_JSON, target) },
  ];
  // package-lock.json 可能不存在（未提交 lock 等场景），存在才纳入同步/校验
  if (existsSync(PACKAGE_LOCK)) {
    targets.push({ name: 'pinvou3-app/package-lock.json', read: () => readPackageLockVersion(PACKAGE_LOCK), write: () => writePackageLockVersion(PACKAGE_LOCK, target) });
  }

  let inconsistent = 0;
  for (const item of targets) {
    const current = item.read();
    if (current === target) {
      console.log(`[一致] ${item.name}: ${current}`);
      continue;
    }
    if (CHECK_ONLY) {
      console.error(`[不一致] ${item.name}: 当前 ${current}，应为 ${target}`);
      inconsistent += 1;
    } else {
      item.write();
      console.log(`[已更新] ${item.name}: ${current} -> ${target}`);
    }
  }

  if (CHECK_ONLY && inconsistent > 0) {
    console.error(`共 ${inconsistent} 处版本号与 VERSION(${target}) 不一致，请运行 node scripts/sync-version.mjs 同步`);
    process.exit(1);
  }
  if (!CHECK_ONLY) {
    console.log(`版本号已同步为 ${target}`);
  }
}

if (process.argv[1] && resolve(process.argv[1]) === SCRIPT_PATH) {
  main();
}
