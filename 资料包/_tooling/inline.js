// 把 pinvou3-使用指南.html 里 shots-web/*.webp 的引用替换成 base64 内联，产出单文件版。
const fs = require('fs');
const path = require('path');
const BASE = '/home/hexin/opencode_projects/pinvou3-model-download/资料包';
const SRC = path.join(BASE, 'pinvou3-使用指南.html');
const OUT = path.join(BASE, 'pinvou3-使用指南-单文件.html');

let html = fs.readFileSync(SRC, 'utf8');
html = html.replace(/src="shots-web\/([^"]+\.webp)"/g, (m, file) => {
  const b64 = fs.readFileSync(path.join(BASE, 'shots-web', file)).toString('base64');
  return `src="data:image/webp;base64,${b64}"`;
});
fs.writeFileSync(OUT, html);
console.log('单文件版:', OUT, (fs.statSync(OUT).size / 1024 / 1024).toFixed(2), 'MB');
