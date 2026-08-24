#!/usr/bin/env node
// 文件图标主题映射单元测试：无外部依赖，纯函数行为校验。
// 运行: node tests/file_icon_theme.test.js
const assert = require('assert');
const fs = require('fs');
const path = require('path');
const vm = require('vm');

// 源文件用 ES module export（运行时经 Vite 加载，测试 vm 加载时剥 export）。
const logicPath = path.join(__dirname, '..', 'src', 'components', 'files', 'file-icon-theme.js');
const code = fs.readFileSync(logicPath, 'utf8')
  .replace(/\bexport\s+\{[^}]+\};?/g, '')
  .replace(/\bexport\s+/g, '');
const ctx = { console };
vm.createContext(ctx);
vm.runInContext(
  `${code}\nthis.resolveFileIcon = resolveFileIcon;\nthis.FILE_EXTENSION_ICONS = FILE_EXTENSION_ICONS;\n`
    + 'this.SPECIAL_FILE_ICONS = SPECIAL_FILE_ICONS;\nthis.SPECIAL_FILE_RULES = SPECIAL_FILE_RULES;\n'
    + 'this.DEFAULT_FILE_ICON = DEFAULT_FILE_ICON;\nthis.FOLDER_ICON = FOLDER_ICON;\nthis.FOLDER_OPEN_ICON = FOLDER_OPEN_ICON;',
  ctx,
  { filename: logicPath },
);
const {
  resolveFileIcon,
  FILE_EXTENSION_ICONS,
  SPECIAL_FILE_ICONS,
  SPECIAL_FILE_RULES,
  DEFAULT_FILE_ICON,
  FOLDER_ICON,
  FOLDER_OPEN_ICON,
} = ctx;

const themeDir = path.join(__dirname, '..', 'src', 'file-icons', 'theme');

let pass = 0, fail = 0;
function test(name, fn) {
  try { fn(); pass++; console.log(`  ✓ ${name}`); }
  catch (e) { fail++; console.error(`  ✗ ${name}\n    ${e.message}`); }
}

// 1. 常见扩展名映射
test('extension mapping covers common types', () => {
  const cases = {
    'app.js': 'javascript.svg',
    'app.mjs': 'javascript.svg',
    'main.ts': 'typescript.svg',
    'view.tsx': 'react_ts.svg',
    'view.jsx': 'react.svg',
    'lib.rs': 'rust.svg',
    'script.py': 'python.svg',
    'data.json': 'json.svg',
    'note.md': 'markdown.svg',
    'config.yml': 'yaml.svg',
    'config.yaml': 'yaml.svg',
    'app.toml': 'toml.svg',
    'index.html': 'html.svg',
    'style.css': 'css.svg',
    'style.scss': 'sass.svg',
    'style.less': 'less.svg',
    'App.vue': 'vue.svg',
    'photo.png': 'image.svg',
    'photo.jpg': 'image.svg',
    'icon.svg': 'svg.svg',
    'doc.pdf': 'pdf.svg',
    'bundle.zip': 'zip.svg',
    'bundle.tar.gz': 'zip.svg',
    'notes.txt': 'document.svg',
    'layout.xml': 'xml.svg',
    'data.csv': 'csv.svg',
    'run.sh': 'console.svg',
    'run.ps1': 'console.svg',
    'schema.sql': 'database.svg',
    'Main.java': 'java.svg',
    'main.c': 'c.svg',
    'main.cpp': 'cpp.svg',
    'Program.cs': 'csharp.svg',
    'main.go': 'go.svg',
    'song.mp3': 'audio.svg',
    'clip.mp4': 'video.svg',
  };
  for (const [name, expected] of Object.entries(cases)) {
    assert.strictEqual(resolveFileIcon(name), expected, `resolveFileIcon(${JSON.stringify(name)})`);
  }
});

// 2. 特殊完整文件名（大小写不敏感）
test('special file names win over extension', () => {
  const cases = {
    'package.json': 'nodejs.svg',
    'package-lock.json': 'lock.svg',
    'yarn.lock': 'lock.svg',
    'Cargo.toml': 'rust.svg',
    'Cargo.lock': 'lock.svg',
    'Dockerfile': 'docker.svg',
    'dockerfile.dev': 'docker.svg',
    'backend.Dockerfile': 'docker.svg',
    '.gitignore': 'git.svg',
    '.gitmodules': 'git.svg',
    'README.md': 'readme.svg',
    'readme': 'readme.svg',
    'LICENSE': 'license.svg',
    'license.md': 'license.svg',
    'Makefile': 'makefile.svg',
    'tsconfig.json': 'typescript.svg',
    'tsconfig.app.json': 'typescript.svg',
    'vite.config.ts': 'vite.svg',
    '.env': 'settings.svg',
    '.env.local': 'settings.svg',
  };
  for (const [name, expected] of Object.entries(cases)) {
    assert.strictEqual(resolveFileIcon(name), expected, `resolveFileIcon(${JSON.stringify(name)})`);
  }
});

// 3. 目录图标与展开状态
test('directories resolve to folder icons', () => {
  assert.strictEqual(resolveFileIcon('src', { isDir: true }), 'folder.svg');
  assert.strictEqual(resolveFileIcon('src', { isDir: true, isOpen: false }), 'folder.svg');
  assert.strictEqual(resolveFileIcon('src', { isDir: true, isOpen: true }), 'folder-open.svg');
  // 目录不受文件名影响。
  assert.strictEqual(resolveFileIcon('package.json', { isDir: true }), 'folder.svg');
});

// 4. 未知扩展名 / 无扩展名回落默认图标
test('unknown names fall back to default file icon', () => {
  assert.strictEqual(resolveFileIcon('data.xyz123'), 'file.svg');
  assert.strictEqual(resolveFileIcon('no-extension'), 'file.svg');
  assert.strictEqual(resolveFileIcon(''), 'file.svg');
  assert.strictEqual(resolveFileIcon(), 'file.svg');
});

// 5. 路径形式入参只取末段文件名
test('only the basename is considered', () => {
  assert.strictEqual(resolveFileIcon('src/features/app.tsx'), 'react_ts.svg');
  assert.strictEqual(resolveFileIcon('a/b/package.json'), 'nodejs.svg');
  assert.strictEqual(resolveFileIcon('a\\b\\main.rs'), 'rust.svg');
});

// 6. 映射表引用的每个 SVG 都必须真实存在于 theme 目录
test('every mapped icon file exists in src/file-icons/theme', () => {
  const referenced = new Set([
    DEFAULT_FILE_ICON,
    FOLDER_ICON,
    FOLDER_OPEN_ICON,
    ...Object.values(SPECIAL_FILE_ICONS),
    ...Object.values(FILE_EXTENSION_ICONS),
  ]);
  // 规则型特殊文件名的输出也要覆盖。
  const probeNames = [
    'dockerfile.dev', 'x.dockerfile', 'tsconfig.app.json', 'vite.config.ts',
    'license', 'license.md', 'licence', 'readme', 'readme.md', '.env', '.env.local',
  ];
  for (const probe of probeNames) {
    for (const rule of SPECIAL_FILE_RULES) {
      const matched = rule(probe);
      if (matched) referenced.add(matched);
    }
  }
  assert.ok(referenced.size > 0);
  for (const iconFile of referenced) {
    const target = path.join(themeDir, iconFile);
    assert.ok(fs.existsSync(target), `missing icon asset: ${iconFile}`);
    const content = fs.readFileSync(target, 'utf8');
    assert.ok(content.startsWith('<svg'), `icon asset is not an SVG: ${iconFile}`);
  }
});

console.log(`\n${pass} passed, ${fail} failed`);
if (fail > 0) process.exit(1);
