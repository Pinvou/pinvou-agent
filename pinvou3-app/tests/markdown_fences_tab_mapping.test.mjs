import test from 'node:test';
import assert from 'node:assert/strict';

import { scanMarkdownFences } from '../src/shared/markdown-fences.js';

// marked 14+ 在 list/blockquote token 的 raw 中保留字面 tab,而 fence 扫描的
// lexerSourceMap 会把前导 tab 展开为空格——matchTokenText 的偏移映射必须
// 把展开侧坐标无损折叠回原始 source。以下用例锁定该回归(修复前代码在
// marked 16 下会漏检前导 tab 嵌套的 fence)。

function assertFenceAtSource(src, fence) {
  // start/end 是 fence 标记所在行的行首/行尾(见 mappedFence 的 lineStart/lineEnd)
  assert.ok(fence.start === 0 || src.charAt(fence.start - 1) === '\n', 'start must be a line boundary');
  assert.ok(
    fence.end === src.length || src.charAt(fence.end) === '\n' || src.charAt(fence.end) === '\r',
    'end must point at the line-ending newline (exclusive, CRLF-aware)',
  );
  const openingLine = src.slice(fence.start, src.indexOf('\n', fence.start));
  assert.ok(
    openingLine.includes(fence.marker.repeat(fence.markerLength)),
    `opening line should contain the fence marker, got ${JSON.stringify(openingLine)}`,
  );
  if (fence.closed) {
    const region = src.slice(fence.start, fence.end);
    const lines = region.split('\n').filter(Boolean);
    assert.ok(
      lines.at(-1).includes(fence.marker.repeat(fence.markerLength)),
      `closed fence should end on the closing marker line, got ${JSON.stringify(lines.at(-1))}`,
    );
  }
}

test('blockquote 内 fence(无 tab)坐标映射', () => {
  const src = '> ```js\n> code()\n> ```\n';
  const fences = scanMarkdownFences(src);
  assert.equal(fences.length, 1);
  assert.equal(fences[0].info, 'js');
  assert.equal(fences[0].content, 'code()');
  assert.equal(fences[0].nested, true);
  assertFenceAtSource(src, fences[0]);
});

test('前导 tab blockquote 内 fence 坐标映射', () => {
  const src = 'intro\n>\t```py\n>\tprint(1)\n>\t```\n';
  const fences = scanMarkdownFences(src);
  assert.equal(fences.length, 1);
  assert.equal(fences[0].info, 'py');
  assert.equal(fences[0].content, 'print(1)');
  assertFenceAtSource(src, fences[0]);
});

test('嵌套 blockquote(>> + tab)内 fence 坐标映射', () => {
  const src = '> outer\n> >\t```ts\n> >\tlet x = 1\n> >\t```\n';
  const fences = scanMarkdownFences(src);
  assert.equal(fences.length, 1);
  assert.equal(fences[0].info, 'ts');
  assert.equal(fences[0].content, 'let x = 1');
  assertFenceAtSource(src, fences[0]);
});

test('前导 tab 嵌套 list 内 fence 坐标映射', () => {
  const src = '- item\n\t- ```js\n\t  code()\n\t  ```\n';
  const fences = scanMarkdownFences(src);
  assert.equal(fences.length, 1);
  assert.equal(fences[0].info, 'js');
  assert.equal(fences[0].content, 'code()');
  assert.equal(fences[0].nested, true);
  assertFenceAtSource(src, fences[0]);
});

test('两级 tab 嵌套 list 内 fence 坐标映射', () => {
  const src = '- a\n\t- b\n\t\t- ```py\n\t\t  x=1\n\t\t  ```\n';
  const fences = scanMarkdownFences(src);
  assert.equal(fences.length, 1);
  assert.equal(fences[0].info, 'py');
  assert.equal(fences[0].content, 'x=1');
  assertFenceAtSource(src, fences[0]);
});

test('CRLF + tab 混合缩进 list 内 fence 坐标映射', () => {
  const src = '- item\r\n\t- ```sh\r\n\t  echo hi\r\n\t  ```\r\n';
  const fences = scanMarkdownFences(src);
  assert.equal(fences.length, 1);
  assert.equal(fences[0].info, 'sh');
  assert.equal(fences[0].content, 'echo hi');
  assertFenceAtSource(src, fences[0]);
});

test('重复段落下 cursor 推进不错位', () => {
  const src = '> ```js\n> a()\n> ```\n\n> ```js\n> a()\n> ```\n';
  const fences = scanMarkdownFences(src);
  assert.equal(fences.length, 2);
  assert.ok(fences[1].start > fences[0].end, 'second fence must map after the first');
  assertFenceAtSource(src, fences[0]);
  assertFenceAtSource(src, fences[1]);
});
