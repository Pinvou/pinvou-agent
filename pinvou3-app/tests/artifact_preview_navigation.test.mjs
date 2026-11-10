import assert from 'node:assert/strict';

import {
  ARTIFACT_PREVIEW_OPEN_EXTERNAL,
  artifactPreviewExternalUrlFromMessage,
  buildArtifactPreviewDocument,
  normalizeUserExternalUrl,
} from '../src/features/artifacts/artifact-preview-navigation.js';

assert.equal(
  normalizeUserExternalUrl('https://example.com/docs?q=1#intro'),
  'https://example.com/docs?q=1#intro',
);
assert.equal(normalizeUserExternalUrl('http://127.0.0.1:8080/preview'), 'http://127.0.0.1:8080/preview');
for (const rejected of [
  'javascript:alert(1)',
  'file:///etc/passwd',
  'data:text/html,hello',
  'https://user@example.com/',
  'https:///missing-host',
  'https://\\example.com',
  'README.md',
  '',
]) {
  assert.equal(normalizeUserExternalUrl(rejected), '', `must reject ${rejected}`);
}

assert.equal(
  artifactPreviewExternalUrlFromMessage({
    type: ARTIFACT_PREVIEW_OPEN_EXTERNAL,
    url: 'https://example.com/',
  }),
  'https://example.com/',
);
assert.equal(
  artifactPreviewExternalUrlFromMessage({
    type: 'untrusted-message',
    url: 'https://example.com/',
  }),
  '',
);

const preview = buildArtifactPreviewDocument(
  '<!doctype html><html><body><a href="https://example.com/">Docs</a></body></html>',
);
assert.match(preview, /window\.parent\.postMessage/);
assert.match(preview, new RegExp(ARTIFACT_PREVIEW_OPEN_EXTERNAL));
assert.match(preview, /document\.addEventListener\("submit"/);
assert.match(preview, /<!doctype html>/i);

// Image contract (PR #658 review): artifact HTML reaches the preview card as
// iframe srcDoc built by buildArtifactPreviewDocument, which supplies neither
// the artifact directory nor an asset resolver, so a relative image src
// resolves against the application URL and renders broken while also hitting
// the host. The visual-design/poster-scene model contract therefore mandates
// inline data URLs. Pin both sides of that handoff here: the builder must
// carry a data-URL image through byte-for-byte (renderable under srcDoc with
// the app's csp:null), and must not fabricate a <base> that would pretend a
// relative src is resolvable — it passes through untouched, i.e. stays broken
// by contract, which is exactly why the skill layer must inline.
const PIXEL_DATA_URL =
  'data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==';
const inlinedDoc = buildArtifactPreviewDocument(
  `<!doctype html><html><body><img alt="hero" src="${PIXEL_DATA_URL}"></body></html>`,
);
assert.equal(
  inlinedDoc.split(`src="${PIXEL_DATA_URL}"`).length - 1,
  1,
  'builder must preserve an inline data-URL image exactly once, byte-for-byte',
);
assert.match(inlinedDoc, /<img alt="hero" src="data:image\/png;base64,/);

const relativeDoc = buildArtifactPreviewDocument(
  '<!doctype html><html><body><img src="photo.png"></body></html>',
);
assert.ok(
  relativeDoc.includes('src="photo.png"'),
  'builder passes a relative image src through untouched (no rewrite contract)',
);
assert.ok(
  !/<base\s/i.test(relativeDoc),
  'builder must not inject <base>: relative images stay unresolvable in the srcDoc preview',
);
assert.ok(
  !relativeDoc.includes(PIXEL_DATA_URL),
  'builder must not fabricate image data the artifact never inlined',
);

// Canvas contract (2026-10-18 visual sweep): the builder must keep the UA
// body margin at 0 (the zoom measurer derives the natural size from
// scrollWidth/Height, which the 8px UA margin would inflate) and must NOT
// force a canvas color into the document — light/dark is delegated to the
// themed iframe surface (bg-white dark:bg-[#15171a] + color-scheme classes),
// so a forced #15171a here would make light-authored documents unreadable in
// the light theme again.
assert.ok(
  relativeDoc.includes('<style>html,body{margin:0;}</style>'),
  'builder must inject exactly html,body{margin:0;} (zoom measurer assumption)',
);
assert.ok(
  !/<style>[^<]*background[^<]*<\/style>/.test(relativeDoc),
  'builder must not force a canvas background into the document (theme-following contract)',
);

console.log('artifact preview navigation tests passed');

// ── sandbox 收紧后的 postMessage 通道（评审 r2 M2）──
import {
  ARTIFACT_PREVIEW_CONTENT_SIZE,
  ARTIFACT_PREVIEW_FRAME_WHEEL,
  ARTIFACT_PREVIEW_INSTALL_RUNTIME,
  ARTIFACT_PREVIEW_REQUEST_SIZE,
  artifactPreviewContentSizeFromMessage,
  artifactPreviewFrameWheelFromMessage,
  artifactPreviewInstallRuntimeMessage,
} from '../src/features/artifacts/artifact-preview-navigation.js';
import { readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

// 纯函数：消息解析与来源数据净化
assert.deepEqual(
  artifactPreviewContentSizeFromMessage({ type: ARTIFACT_PREVIEW_CONTENT_SIZE, w: 800, h: 600 }),
  { w: 800, h: 600 },
);
assert.equal(artifactPreviewContentSizeFromMessage({ type: ARTIFACT_PREVIEW_CONTENT_SIZE, w: 0, h: 0 }), null,
  '空测量不得进入父窗口缓存');
assert.equal(artifactPreviewContentSizeFromMessage({ type: ARTIFACT_PREVIEW_CONTENT_SIZE, w: '9;alert(1)', h: 1 }), null,
  '非数值尺寸必须拒绝');
assert.equal(artifactPreviewContentSizeFromMessage({ type: 'untrusted-message' }), null);
assert.equal(artifactPreviewFrameWheelFromMessage({ type: ARTIFACT_PREVIEW_FRAME_WHEEL, deltaY: -12 }), -12);
assert.equal(artifactPreviewFrameWheelFromMessage({ type: ARTIFACT_PREVIEW_FRAME_WHEEL, deltaY: 'x' }), null);
assert.deepEqual(
  artifactPreviewInstallRuntimeMessage('const x=1;'),
  { type: ARTIFACT_PREVIEW_INSTALL_RUNTIME, script: 'const x=1;' },
);

// bootstrap 载荷：尺寸自报 / ctrl+滚轮转发 / 安装中继必须齐备
const doc = buildArtifactPreviewDocument('<p>hi</p>');
for (const fragment of [
  'reportSize',
  JSON.stringify(ARTIFACT_PREVIEW_CONTENT_SIZE).slice(1, -1),
  JSON.stringify(ARTIFACT_PREVIEW_REQUEST_SIZE).slice(1, -1),
  JSON.stringify(ARTIFACT_PREVIEW_FRAME_WHEEL).slice(1, -1),
  JSON.stringify(ARTIFACT_PREVIEW_INSTALL_RUNTIME).slice(1, -1),
  'new ResizeObserver(reportSize)',
  'lastW=-1,lastH=-1',
  '(0,eval)(d.script)',
]) {
  assert.ok(doc.includes(fragment), `bootstrap 必须包含 ${fragment}`);
}
// 请求尺寸与安装命令只在父→帧方向；帧内不得把 request-size 误当上报
assert.ok(doc.includes('if(d.type===' + JSON.stringify(ARTIFACT_PREVIEW_REQUEST_SIZE) + '){reportSize();return;}'),
  'request-size 必须触发帧内重新测量');

// 组件契约：iframe 不再持有 allow-same-origin（导入产物是第三方可控输入，
// 同源 srcdoc 脚本可直达 window.parent.__TAURI__）；测量/缩放/安装走消息通道。
const appRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const composerShared = readFileSync(path.join(appRoot, 'src', 'features', 'settings', 'composer-shared.jsx'), 'utf8');
assert.ok(composerShared.includes('sandbox="allow-scripts"'), 'ScaledHtmlPreview 必须 drop allow-same-origin');
assert.ok(!composerShared.includes('sandbox="allow-same-origin'), '不得残留任何 allow-same-origin 沙箱');
assert.ok(!composerShared.includes('contentWindow.document'), '父窗口不得再读跨源帧 document');
assert.ok(composerShared.includes('artifactPreviewContentSizeFromMessage'), '测量必须来自帧内上报');
assert.ok(composerShared.includes('artifactPreviewFrameWheelFromMessage'), 'ctrl+滚轮必须来自帧内转发');
const panel = readFileSync(path.join(appRoot, 'src', 'features', 'artifacts', 'ArtifactsPanel.jsx'), 'utf8');
assert.ok(panel.includes('ARTIFACT_PREVIEW_INSTALL_RUNTIME'), 'design runtime 必须经消息中继安装');
assert.ok(!panel.includes('.eval('), '父窗口不得再向跨源帧 eval 注入');

console.log('artifact_preview_navigation: sandbox-hardening assertions passed');
