/**
 * 工具商店技能包拖放契约:capture 阶段接管隔离附件通道、拖放走字节通道、
 * 不装 Tauri 原生 onDragDropEvent、dragDropEnabled 保持 false。
 * 仿 attachment_drop_logic.test.mjs 的正则契约风格。
 */
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';

const dropControllerSource = await readFile(
  new URL('../src/features/attachments/attachment-drop-controller.js', import.meta.url),
  'utf8',
);
const toolStoreSource = await readFile(
  new URL('../src/features/tools/ToolStoreView.jsx', import.meta.url),
  'utf8',
);
const importLogicSource = await readFile(
  new URL('../src/features/tools/skill-import-logic.js', import.meta.url),
  'utf8',
);
const tauriConfigSource = await readFile(
  new URL('../src-tauri/tauri.conf.json', import.meta.url),
  'utf8',
);

// 1. 拖放控制器:capture 选项存在,四个监听带 capture,已受理路径 stopPropagation
assert.match(
  dropControllerSource,
  /var capture = options\.capture === true/,
  'controller must support the capture option',
);
assert.match(
  dropControllerSource,
  /event\.stopPropagation\(\)/,
  'accepted drag paths must stopPropagation to isolate the global attachment channel',
);
for (const type of ['dragenter', 'dragover', 'dragleave', 'drop']) {
  assert.match(
    dropControllerSource,
    new RegExp(`addEventListener\\("${type}", on\\w+, capture\\)`),
    `addEventListener(${type}) must be registered with the capture flag`,
  );
  assert.match(
    dropControllerSource,
    new RegExp(`removeEventListener\\("${type}", on\\w+, capture\\)`),
    `removeEventListener(${type}) must mirror the capture flag`,
  );
}

// 2. 工具商店:capture: true 挂载,onFiles 走字节导入命令
assert.match(
  toolStoreSource,
  /window\.PinvouAttachmentDropController/,
  'tool store must reference the drop controller',
);
assert.match(toolStoreSource, /ctrl\.install\(/, 'tool store must install the drop controller');
assert.match(toolStoreSource, /capture: true/, 'tool store must take over in capture phase');
assert.match(
  toolStoreSource,
  /import_skill_package_bytes/,
  'dropped zips must go through the base64 byte channel command',
);
assert.match(
  toolStoreSource,
  /data-testid="tool-store-upload-btn"/,
  'header upload button must exist',
);
assert.match(
  toolStoreSource,
  /data-testid="tool-store-drop-overlay"/,
  'drop overlay must exist',
);
// 上传按钮与拖放都走统一导入逻辑(成功/失败/取消处理一处)
assert.match(toolStoreSource, /doImportSkillZip/);

// 3. 桌面端不得重装 Tauri 原生拖放处理器(与附件契约同语义)
assert.doesNotMatch(toolStoreSource, /onDragDropEvent/);
assert.equal(
  JSON.parse(tauriConfigSource).app.windows[0].dragDropEnabled,
  false,
  'dragDropEnabled must stay false (WebView2 drag feedback + no path access)',
);

// 4. 纯逻辑:大小软限对齐后端 5MiB
assert.match(importLogicSource, /5 \* 1024 \* 1024/);

console.log('tool store skill drop contract tests passed');
