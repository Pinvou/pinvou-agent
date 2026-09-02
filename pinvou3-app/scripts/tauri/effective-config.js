const crypto = require("node:crypto");
const fs = require("node:fs");
const path = require("node:path");

const { APP_ROOT } = require("./platform-config.js");

const TAURI_ROOT = path.join(APP_ROOT, "src-tauri");
const BASE_CONFIG_PATH = path.join(TAURI_ROOT, "tauri.conf.json");

function clone(value) {
  return value === undefined ? undefined : JSON.parse(JSON.stringify(value));
}

function isObject(value) {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

// Tauri applies --config overlays as JSON Merge Patch: objects merge,
// arrays/scalars replace, and null removes the inherited key.
function mergeConfig(target, patch) {
  if (!isObject(patch)) return clone(patch);
  const result = isObject(target) ? clone(target) : {};
  for (const [key, value] of Object.entries(patch)) {
    if (value === null) {
      delete result[key];
    } else {
      result[key] = mergeConfig(result[key], value);
    }
  }
  return result;
}

function loadConfigSpec(spec) {
  const value = String(spec).trim();
  if (value.startsWith("{")) {
    return { label: "<inline-config>", config: JSON.parse(value) };
  }
  const absolutePath = path.isAbsolute(value) ? value : path.resolve(APP_ROOT, value);
  if (!fs.existsSync(absolutePath)) {
    throw new Error(`Tauri overlay 不存在: ${absolutePath}`);
  }
  return {
    label: path.relative(APP_ROOT, absolutePath).replaceAll("\\", "/"),
    config: JSON.parse(fs.readFileSync(absolutePath, "utf8")),
  };
}

function composeEffectiveConfig(configSpecs = []) {
  const configs = [
    {
      label: path.relative(APP_ROOT, BASE_CONFIG_PATH).replaceAll("\\", "/"),
      config: JSON.parse(fs.readFileSync(BASE_CONFIG_PATH, "utf8")),
    },
    ...configSpecs.map(loadConfigSpec),
  ];
  const effectiveConfig = configs.reduce(
    (merged, entry) => mergeConfig(merged, entry.config),
    {},
  );
  return { effectiveConfig, configLabels: configs.map((entry) => entry.label) };
}

function normalizeDestination(destination) {
  const normalized = String(destination).replaceAll("\\", "/").replace(/^\.\//, "");
  if (path.posix.isAbsolute(normalized) || normalized.split("/").includes("..")) {
    throw new Error(`安装包资源目标路径越界: ${destination}`);
  }
  return normalized.replace(/\/$/, "");
}

function walkFiles(root) {
  const item = fs.lstatSync(root);
  if (item.isSymbolicLink()) {
    return [{
      absolutePath: root,
      relativePath: "",
      bytes: Buffer.byteLength(fs.readlinkSync(root)),
      kind: "symlink",
    }];
  }
  if (item.isFile()) {
    return [{ absolutePath: root, relativePath: "", bytes: item.size, kind: "file" }];
  }
  if (!item.isDirectory()) throw new Error(`不支持的安装包资源类型: ${root}`);

  const files = [];
  const visit = (directory, prefix) => {
    const entries = fs.readdirSync(directory, { withFileTypes: true });
    for (const entry of entries) {
      const absolutePath = path.join(directory, entry.name);
      const relativePath = prefix ? `${prefix}/${entry.name}` : entry.name;
      if (entry.isDirectory()) {
        visit(absolutePath, relativePath);
      } else if (entry.isFile()) {
        files.push({
          absolutePath,
          relativePath,
          bytes: fs.statSync(absolutePath).size,
          kind: "file",
        });
      } else if (entry.isSymbolicLink()) {
        files.push({
          absolutePath,
          relativePath,
          bytes: Buffer.byteLength(fs.readlinkSync(absolutePath)),
          kind: "symlink",
        });
      } else {
        throw new Error(`安装包资源不允许特殊文件: ${absolutePath}`);
      }
    }
  };
  visit(root, "");
  return files;
}

function buildResourceManifest(effectiveConfig, { platform = process.platform } = {}) {
  const resources = effectiveConfig.bundle?.resources || {};
  if (!isObject(resources)) {
    throw new Error("bundle.resources 必须使用 source -> destination 对象映射");
  }

  const mappings = [];
  const files = [];
  const destinations = new Map();
  for (const [source, destination] of Object.entries(resources)) {
    if (/[*?[]/.test(source)) {
      throw new Error(`安装包资源清单不支持 glob: ${source}`);
    }
    const absoluteSource = path.resolve(TAURI_ROOT, source);
    const relativeToTauri = path.relative(TAURI_ROOT, absoluteSource);
    if (relativeToTauri.startsWith("..") || path.isAbsolute(relativeToTauri)) {
      throw new Error(`安装包资源源路径越界: ${source}`);
    }
    if (!fs.existsSync(absoluteSource)) {
      throw new Error(`安装包资源不存在: ${source} (${absoluteSource})`);
    }

    const normalizedDestination = normalizeDestination(destination);
    const sourceFiles = walkFiles(absoluteSource);
    mappings.push({ source, destination: normalizedDestination, files: sourceFiles.length });
    for (const sourceFile of sourceFiles) {
      const target = [normalizedDestination, sourceFile.relativePath]
        .filter(Boolean)
        .join("/");
      const previous = destinations.get(target.toLowerCase());
      if (previous) {
        throw new Error(`安装包资源目标冲突: ${target} (${previous} / ${source})`);
      }
      destinations.set(target.toLowerCase(), source);
      files.push({
        source: path.relative(TAURI_ROOT, sourceFile.absolutePath).replaceAll("\\", "/"),
        destination: target,
        bytes: sourceFile.bytes,
        kind: sourceFile.kind,
      });
    }
  }

  files.sort((left, right) => left.destination.localeCompare(right.destination));
  return {
    schemaVersion: 1,
    platform,
    resourceFileCount: files.length,
    resourceBytes: files.reduce((sum, file) => sum + file.bytes, 0),
    mappings,
    files,
  };
}

function writeEffectiveArtifacts(configSpecs, { platform = process.platform } = {}) {
  const { effectiveConfig, configLabels } = composeEffectiveConfig(configSpecs);
  const resourceManifest = buildResourceManifest(effectiveConfig, { platform });
  const outputDirectory = path.join(TAURI_ROOT, "target", "tauri-config", platform);
  fs.mkdirSync(outputDirectory, { recursive: true });

  const effectiveConfigPath = path.join(outputDirectory, "effective-config.json");
  const resourceManifestPath = path.join(outputDirectory, "installer-resources.manifest.json");
  const serializedConfig = `${JSON.stringify(effectiveConfig, null, 2)}\n`;
  fs.writeFileSync(effectiveConfigPath, serializedConfig, "utf8");
  fs.writeFileSync(
    resourceManifestPath,
    `${JSON.stringify(
      {
        ...resourceManifest,
        configs: configLabels,
        effectiveConfigSha256: crypto.createHash("sha256").update(serializedConfig).digest("hex"),
      },
      null,
      2,
    )}\n`,
    "utf8",
  );
  return { effectiveConfig, effectiveConfigPath, resourceManifest, resourceManifestPath };
}

module.exports = {
  BASE_CONFIG_PATH,
  TAURI_ROOT,
  buildResourceManifest,
  composeEffectiveConfig,
  mergeConfig,
  writeEffectiveArtifacts,
};
