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
//   5. Both package versions (pinvou3-tauri and pinvou-knowledge) in
//      pinvou3-app/src-tauri/Cargo.lock (that lock contains both this crate and
//      the path dependency pinvou-knowledge; missing one breaks
//      cargo metadata/build --locked, and every build then silently rewrites the lock)
//   6. The "version" field of pinvou3-app/package.json
//   7. The root "version" and packages[""].version of pinvou3-app/package-lock.json (skipped when the file does not exist)

import { existsSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const SCRIPT_PATH = fileURLToPath(import.meta.url);
const REPO_ROOT = resolve(dirname(SCRIPT_PATH), '..');
const CHECK_ONLY = process.argv.includes('--check');

// SemVer 2.0.0：禁止数字段前导零，预发布数字标识同样禁止前导零；
// 允许合法的预发布与构建元数据，例如 1.2.3-rc.1+build.7。
const SEMVER_RE = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-((?:0|[1-9]\d*|\d*[A-Za-z-][0-9A-Za-z-]*)(?:\.(?:0|[1-9]\d*|\d*[A-Za-z-][0-9A-Za-z-]*))*))?(?:\+([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?$/;

export function isValidVersion(version) {
  return SEMVER_RE.test(version);
}

// Derive all sync target paths from a repository root; tests sandbox this with a temp root and never touch the real repository.
function repoPaths(repoRoot) {
  return {
    versionFile: resolve(repoRoot, 'VERSION'),
    tauriConf: resolve(repoRoot, 'pinvou3-app/src-tauri/tauri.conf.json'),
    cargoToml: resolve(repoRoot, 'pinvou3-app/src-tauri/Cargo.toml'),
    knowledgeCargoToml: resolve(repoRoot, 'pinvou-knowledge/Cargo.toml'),
    knowledgeCargoLock: resolve(repoRoot, 'pinvou-knowledge/Cargo.lock'),
    srcTauriCargoLock: resolve(repoRoot, 'pinvou3-app/src-tauri/Cargo.lock'),
    packageJson: resolve(repoRoot, 'pinvou3-app/package.json'),
    packageLock: resolve(repoRoot, 'pinvou3-app/package-lock.json'),
  };
}

// 读取单一事实来源：根目录 VERSION 文件（内容只有一行版本号）
function readTargetVersion(versionFile) {
  const version = readFileSync(versionFile, 'utf8').trim();
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

// Validate/sync all targets. Returns the exit code: 0 when consistent (or once
// synced), 1 when --check finds inconsistencies; fatal errors (invalid VERSION,
// missing [package] version line) still process.exit(2) directly.
// repoRoot and checkOnly are injectable so unit tests can run in a temp-dir sandbox.
export function main(repoRoot = REPO_ROOT, { checkOnly = CHECK_ONLY } = {}) {
  const paths = repoPaths(repoRoot);
  const target = readTargetVersion(paths.versionFile);
  const targets = [
    { name: 'pinvou3-app/src-tauri/tauri.conf.json', read: () => readJsonVersion(paths.tauriConf), write: () => writeJsonVersion(paths.tauriConf, target) },
    { name: 'pinvou3-app/src-tauri/Cargo.toml', read: () => readCargoVersion(paths.cargoToml), write: () => writeCargoVersion(paths.cargoToml, target) },
    { name: 'pinvou-knowledge/Cargo.toml', read: () => readCargoVersion(paths.knowledgeCargoToml), write: () => writeCargoVersion(paths.knowledgeCargoToml, target) },
    { name: 'pinvou-knowledge/Cargo.lock', read: () => readCargoLockVersion(paths.knowledgeCargoLock, 'pinvou-knowledge'), write: () => writeCargoLockVersion(paths.knowledgeCargoLock, 'pinvou-knowledge', target) },
    // The src-tauri Cargo.lock contains both pinvou3-tauri and the path dependency
    // pinvou-knowledge, exactly one [[package]] section each in that lock; registering
    // them separately lets --check pinpoint which package lags behind.
    { name: 'pinvou3-app/src-tauri/Cargo.lock(pinvou3-tauri)', read: () => readCargoLockVersion(paths.srcTauriCargoLock, 'pinvou3-tauri'), write: () => writeCargoLockVersion(paths.srcTauriCargoLock, 'pinvou3-tauri', target) },
    { name: 'pinvou3-app/src-tauri/Cargo.lock(pinvou-knowledge)', read: () => readCargoLockVersion(paths.srcTauriCargoLock, 'pinvou-knowledge'), write: () => writeCargoLockVersion(paths.srcTauriCargoLock, 'pinvou-knowledge', target) },
    { name: 'pinvou3-app/package.json', read: () => readJsonVersion(paths.packageJson), write: () => writeJsonVersion(paths.packageJson, target) },
  ];
  // package-lock.json 可能不存在（未提交 lock 等场景），存在才纳入同步/校验
  if (existsSync(paths.packageLock)) {
    targets.push({ name: 'pinvou3-app/package-lock.json', read: () => readPackageLockVersion(paths.packageLock), write: () => writePackageLockVersion(paths.packageLock, target) });
  }

  let inconsistent = 0;
  for (const item of targets) {
    // Missing target files (ENOENT) and missing/duplicate Cargo.lock package
    // entries surface here as thrown errors; keep the script's convention of a
    // clean message + exit 2 for fatal errors instead of an uncaught stack trace.
    let current;
    try {
      current = item.read();
    } catch (error) {
      console.error(`${item.name}: ${error instanceof Error ? error.message : String(error)}`);
      process.exit(2);
    }
    if (current === target) {
      console.log(`[一致] ${item.name}: ${current}`);
      continue;
    }
    if (checkOnly) {
      console.error(`[不一致] ${item.name}: 当前 ${current}，应为 ${target}`);
      inconsistent += 1;
    } else {
      item.write();
      console.log(`[已更新] ${item.name}: ${current} -> ${target}`);
    }
  }

  if (checkOnly && inconsistent > 0) {
    console.error(`共 ${inconsistent} 处版本号与 VERSION(${target}) 不一致，请运行 node scripts/sync-version.mjs 同步`);
    return 1;
  }
  if (!checkOnly) {
    console.log(`版本号已同步为 ${target}`);
  }
  return 0;
}

// Node realpaths the entry module before setting import.meta.url, so on
// symlinked paths (macOS /var/folders -> /private/var/folders, or /tmp on
// macOS) import.meta.url differs from resolve(process.argv[1]) even when both
// point at this file. Compare realpaths too, otherwise the CLI entry is
// silently skipped and the script becomes a no-op library import.
function isCliEntry() {
  if (!process.argv[1]) return false;
  const entry = resolve(process.argv[1]);
  if (entry === SCRIPT_PATH) return true;
  try {
    return realpathSync(entry) === realpathSync(SCRIPT_PATH);
  } catch {
    return false;
  }
}

if (isCliEntry()) {
  process.exit(main());
}
