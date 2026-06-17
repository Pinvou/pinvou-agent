// 把 shots/*.png 压成网页友好的 webp（宽 1500, q80）；附件图裁到底部输入区。
const sharp = require('/home/hexin/opencode_projects/pinvou3-model-download/pinvou3-app/node_modules/sharp');
const path = require('path');
const fs = require('fs');
const SRC = '/home/hexin/opencode_projects/pinvou3-model-download/资料包/shots';
const OUT = '/home/hexin/opencode_projects/pinvou3-model-download/资料包/shots-web';
fs.mkdirSync(OUT, { recursive: true });

// key → 可选裁剪 {top,height}（基于 2880x1840 原图）
const CROP = { attachments: { left: 0, top: 1140, width: 2880, height: 700 } };

(async () => {
  const files = fs.readdirSync(SRC).filter(f => f.endsWith('.png'));
  for (const f of files) {
    const key = f.replace('.png', '');
    let img = sharp(path.join(SRC, f));
    if (CROP[key]) img = img.extract(CROP[key]);
    const outPath = path.join(OUT, key + '.webp');
    await img.resize({ width: 1500, withoutEnlargement: true }).webp({ quality: 80 }).toFile(outPath);
    const kb = (fs.statSync(outPath).size / 1024).toFixed(0);
    console.log(`${key}.webp  ${kb} KB`);
  }
  const total = fs.readdirSync(OUT).reduce((s, f) => s + fs.statSync(path.join(OUT, f)).size, 0);
  console.log('TOTAL', (total / 1024 / 1024).toFixed(2), 'MB');
})();
