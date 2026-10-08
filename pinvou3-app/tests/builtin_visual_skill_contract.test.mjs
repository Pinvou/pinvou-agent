import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

// visual-design/SKILL.md 与海报场景（visual-poster-scene.js）在同轮对同一
// 请求生效：二者对「真实图片怎么进产物」的口径必须一致。2026-10-07 审计
// （PR #658）前二者互相矛盾——skill 禁止真实图片、场景要求下载外链图片，
// 模型同轮收到两套相反指令。此处钉住一致口径与旧矛盾措辞不回潮。
const bundleRoot = join(
  dirname(fileURLToPath(import.meta.url)),
  '..',
  'src-tauri',
  'resources',
  'common',
  'bundle',
);
const skill = readFileSync(
  join(bundleRoot, 'builtin-skills', 'visual-design', 'SKILL.md'),
  'utf8',
);
const scene = readFileSync(
  join(dirname(fileURLToPath(import.meta.url)), '..', 'src', 'features', 'chat', 'visual-poster-scene.js'),
  'utf8',
);

test('visual-design skill and poster scene agree: download locally, no external image URLs', () => {
  for (const [name, text] of [['SKILL.md', skill], ['visual-poster-scene.js', scene]]) {
    assert.ok(
      text.includes('禁止外链图片 URL') || text.includes('不要使用外链图片 URL'),
      `${name} lost the external-image-URL prohibition`,
    );
    assert.ok(
      /本地相对路径引用|相对路径引用/.test(text),
      `${name} lost the download-then-relative-path mandate`,
    );
    // 旧矛盾措辞（允许稳定外链图片 URL）不得回潮。
    assert.ok(!text.includes('稳定可访问的图片'), `${name} resurrected the external-URL allowance`);
  }
});

test('visual-design self-containment is scoped to zero network requests with the local-image carve-out', () => {
  // 「严格自包含」若不定义边界，会与上一条强制的相对路径图片互斥（字面模型
  // 二选一）。钉住定义与唯一例外，二者回退其一即红。
  const selfContainment = skill.split('\n').find((line) => line.includes('严格自包含'));
  assert.ok(selfContainment, 'SKILL.md lost the 严格自包含 bullet');
  assert.ok(selfContainment.includes('零网络请求'), '自包含 bullet must define zero-network-request scope');
  assert.ok(
    selfContainment.includes('唯一许可的图片形态'),
    '自包含 bullet must carve out the download-then-relative-path image form',
  );
  assert.ok(
    selfContainment.includes('应用内嵌入预览不解析相对路径图片'),
    '自包含 bullet must keep the in-app preview limitation',
  );
});

test('poster scene keeps the in-app preview caveat next to its preview promise', () => {
  // 场景承诺「产物预览里继续点选/编辑」，又强制相对路径图片——预览不解析
  // 相对路径图片，承诺必须自带限定，否则对用户不真实。
  assert.ok(
    scene.includes('应用内嵌入预览不解析相对路径图片'),
    'poster scene lost the in-app preview caveat',
  );
});
