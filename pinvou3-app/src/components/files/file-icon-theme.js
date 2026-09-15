// 文件图标主题映射：特殊文件名 > 扩展名 > 默认图标，目录单独处理。
// 图标资产来自 Material Icon Theme（MIT 许可），子集位于 src/file-icons/theme/：
// https://github.com/material-extensions/vscode-material-icon-theme
// 其中 file.svg / folder.svg / folder-open.svg 是上游构建期生成、未提交进仓库的默认图标，
// 按上游生成器源码（src/core/generator，默认色 #90a4ae）等价复刻；csv.svg 取自上游 table.svg。
// 纯函数、无 React 依赖，node 测试可直接加载本模块。

const FOLDER_ICON = 'folder.svg';
const FOLDER_OPEN_ICON = 'folder-open.svg';
const DEFAULT_FILE_ICON = 'file.svg';

// 特殊完整文件名（比较前统一转小写）→ 图标文件名。
const SPECIAL_FILE_ICONS = {
  'package.json': 'nodejs.svg',
  'package-lock.json': 'lock.svg',
  'npm-shrinkwrap.json': 'lock.svg',
  'yarn.lock': 'lock.svg',
  'pnpm-lock.yaml': 'lock.svg',
  'cargo.toml': 'rust.svg',
  'cargo.lock': 'lock.svg',
  'dockerfile': 'docker.svg',
  '.dockerignore': 'docker.svg',
  'docker-compose.yml': 'docker.svg',
  'docker-compose.yaml': 'docker.svg',
  '.gitignore': 'git.svg',
  '.gitattributes': 'git.svg',
  '.gitmodules': 'git.svg',
  'makefile': 'makefile.svg',
  'gnumakefile': 'makefile.svg',
  'tsconfig.json': 'typescript.svg',
};

// 前缀/规则型特殊文件名（在精确匹配之后按顺序判断）。
const SPECIAL_FILE_RULES = [
  (name) => name.startsWith('dockerfile.') || name.endsWith('.dockerfile') ? 'docker.svg' : null,
  (name) => name.startsWith('tsconfig.') && name.endsWith('.json') ? 'typescript.svg' : null,
  (name) => name.startsWith('vite.config.') ? 'vite.svg' : null,
  (name) => name === 'license' || name.startsWith('license.') || name === 'licence' || name.startsWith('licence.') ? 'license.svg' : null,
  (name) => name === 'readme' || name.startsWith('readme.') ? 'readme.svg' : null,
  (name) => name === '.env' || name.startsWith('.env.') ? 'settings.svg' : null,
];

// 扩展名（小写、不含点）→ 图标文件名。
const FILE_EXTENSION_ICONS = {
  js: 'javascript.svg', mjs: 'javascript.svg', cjs: 'javascript.svg',
  ts: 'typescript.svg', mts: 'typescript.svg', cts: 'typescript.svg',
  jsx: 'react.svg',
  tsx: 'react_ts.svg',
  rs: 'rust.svg',
  py: 'python.svg', pyw: 'python.svg',
  json: 'json.svg', jsonc: 'json.svg', json5: 'json.svg',
  md: 'markdown.svg', markdown: 'markdown.svg', mdx: 'markdown.svg',
  yml: 'yaml.svg', yaml: 'yaml.svg',
  toml: 'toml.svg',
  html: 'html.svg', htm: 'html.svg', xhtml: 'html.svg',
  css: 'css.svg',
  scss: 'sass.svg', sass: 'sass.svg',
  less: 'less.svg',
  vue: 'vue.svg',
  png: 'image.svg', jpg: 'image.svg', jpeg: 'image.svg', gif: 'image.svg',
  webp: 'image.svg', bmp: 'image.svg', ico: 'image.svg', avif: 'image.svg',
  tif: 'image.svg', tiff: 'image.svg', heic: 'image.svg',
  svg: 'svg.svg',
  pdf: 'pdf.svg',
  zip: 'zip.svg', rar: 'zip.svg', '7z': 'zip.svg', tar: 'zip.svg',
  gz: 'zip.svg', tgz: 'zip.svg', bz2: 'zip.svg', xz: 'zip.svg',
  txt: 'document.svg', text: 'document.svg', log: 'document.svg',
  xml: 'xml.svg', xsd: 'xml.svg', xsl: 'xml.svg', plist: 'xml.svg',
  csv: 'csv.svg', tsv: 'csv.svg',
  sh: 'console.svg', bash: 'console.svg', zsh: 'console.svg', fish: 'console.svg',
  ps1: 'console.svg', psm1: 'console.svg', bat: 'console.svg', cmd: 'console.svg',
  sql: 'database.svg', db: 'database.svg', sqlite: 'database.svg', sqlite3: 'database.svg',
  mp3: 'audio.svg', wav: 'audio.svg', ogg: 'audio.svg', flac: 'audio.svg',
  m4a: 'audio.svg', aac: 'audio.svg',
  mp4: 'video.svg', webm: 'video.svg', mkv: 'video.svg', mov: 'video.svg', avi: 'video.svg',
  java: 'java.svg', class: 'java.svg', jar: 'java.svg',
  c: 'c.svg', h: 'c.svg',
  cpp: 'cpp.svg', cxx: 'cpp.svg', cc: 'cpp.svg', hpp: 'cpp.svg', hxx: 'cpp.svg', hh: 'cpp.svg',
  cs: 'csharp.svg', csx: 'csharp.svg',
  go: 'go.svg',
  mk: 'makefile.svg',
  lock: 'lock.svg',
  ini: 'settings.svg', conf: 'settings.svg', cfg: 'settings.svg', properties: 'settings.svg',
};

function baseName(name) {
  return String(name || '').split(/[\\/]/u).pop().toLowerCase();
}

function extensionOf(base) {
  const dot = base.lastIndexOf('.');
  // 点号文件（.gitignore）没有扩展名。
  return dot > 0 ? base.slice(dot + 1) : '';
}

// 解析文件/目录名对应的主题图标文件名（不含目录前缀）。
export function resolveFileIcon(name, { isDir = false, isOpen = false } = {}) {
  if (isDir) return isOpen ? FOLDER_OPEN_ICON : FOLDER_ICON;
  const base = baseName(name);
  if (!base) return DEFAULT_FILE_ICON;
  const special = SPECIAL_FILE_ICONS[base];
  if (special) return special;
  for (const rule of SPECIAL_FILE_RULES) {
    const matched = rule(base);
    if (matched) return matched;
  }
  return FILE_EXTENSION_ICONS[extensionOf(base)] || DEFAULT_FILE_ICON;
}

export {
  DEFAULT_FILE_ICON,
  FILE_EXTENSION_ICONS,
  FOLDER_ICON,
  FOLDER_OPEN_ICON,
  SPECIAL_FILE_ICONS,
  SPECIAL_FILE_RULES,
};
