import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

// visual-design/SKILL.md 与海报场景（visual-poster-scene.js）在同轮对同一
// 请求生效：二者对「真实图片怎么进产物」的口径必须一致。2026-10-07 审计
// （PR #658）前二者互相矛盾——skill 禁止真实图片、场景要求下载外链图片；
// 统一成「下载后相对路径引用」后，PR #658 评审又指出产物卡预览用 iframe
// srcDoc 装载 HTML，buildArtifactPreviewDocument 既不注入 <base> 也没有资
// 产解析器，相对路径图片按应用地址解析必裂且向宿主发请求。终版口径：检
// 索下载后 base64 内联为 data URL（与用户传图同一形态）。此处钉住一致口径
// 与两版旧措辞不回潮。
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

test('visual-design skill and poster scene agree: inline retrieved images as data URLs, no external URLs', () => {
  for (const [name, text] of [['SKILL.md', skill], ['visual-poster-scene.js', scene]]) {
    assert.ok(
      text.includes('禁止外链图片 URL') || text.includes('不要使用外链图片 URL'),
      `${name} lost the external-image-URL prohibition`,
    );
    assert.ok(
      text.includes('base64 内联') && text.includes('data URL'),
      `${name} lost the download-then-data-URL mandate`,
    );
    // 旧措辞不得回潮：外链图片白名单、「下载后相对路径引用」唯一形态。
    assert.ok(!text.includes('稳定可访问的图片'), `${name} resurrected the external-URL allowance`);
    assert.ok(!text.includes('相对路径引用'), `${name} resurrected the download-then-relative-path mandate`);
    assert.ok(!text.includes('应用内嵌入预览不解析相对路径图片'), `${name} resurrected the broken-preview caveat`);
  }
});

test('visual-design self-containment is scoped to zero network requests with the data-URL image carve-out', () => {
  // 「严格自包含」若不定义边界，会与上一条强制的内联图片互斥（字面模型
  // 二选一）。钉住定义、唯一例外与预览可用承诺：相对路径版曾被迫声明
  // 「应用内预览不解析相对路径图片、以浏览器为准」，data URL 版必须真正
  // 兑现应用内预览，旧免责声明回潮即红。
  const selfContainment = skill.split('\n').find((line) => line.includes('严格自包含'));
  assert.ok(selfContainment, 'SKILL.md lost the 严格自包含 bullet');
  assert.ok(selfContainment.includes('零网络请求'), '自包含 bullet must define zero-network-request scope');
  assert.ok(
    selfContainment.includes('唯一许可的图片形态'),
    '自包含 bullet must carve out the inline data-URL image form',
  );
  assert.ok(
    selfContainment.includes('data URL'),
    '自包含 bullet must name the data-URL form',
  );
  assert.ok(
    selfContainment.includes('应用内产物预览'),
    '自包含 bullet must promise the in-app artifact preview renders the image',
  );
});

test('poster scene keeps the in-app preview promise without a relative-path caveat', () => {
  // 场景承诺「产物预览里继续点选/编辑」；图片内联 data URL 后该承诺无需
  // 限定词，旧「预览不解析相对路径图片、以浏览器为准」免责声明不得回潮。
  assert.ok(
    scene.includes('产物预览'),
    'poster scene lost the in-app preview promise',
  );
  assert.ok(
    scene.includes('零网络请求'),
    'poster scene lost the zero-network-request framing',
  );
});
