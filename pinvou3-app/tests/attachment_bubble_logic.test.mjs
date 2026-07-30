import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import {
  formatAttachmentDisplayText,
  sessionTitlePlainText,
  sessionTitlePresentation,
  splitAttachmentLine,
} from '../src/features/attachments/attachment-message.js';
import { _ARTIFACT_FMT } from '../src/shared/artifact-utils.js';

assert.equal(
  Object.keys(_ARTIFACT_FMT).length,
  13,
  'the shared attachment icon set must keep all 13 reviewed file categories',
);
assert.equal(
  _ARTIFACT_FMT.data.viewBox,
  '0 -960 960 960',
  'the data icon uses negative Material coordinates and must remain inside its SVG viewBox',
);

// ── splitAttachmentLine: JSON 协议无损保存文件名，旧分隔格式继续可读 ──
assert.equal(
  formatAttachmentDisplayText('看一下这个', ['预算 · 最终.xlsx', ' leading.txt']),
  '看一下这个\n\n📎 ["预算 · 最终.xlsx"," leading.txt"]',
);
assert.equal(
  formatAttachmentDisplayText('第一条\n\n第二条', ['一.pdf', '二.xlsx']),
  '第一条\n\n第二条\n\n📎 ["一.pdf","二.xlsx"]',
  'a merged queue must emit one attachment marker containing every attachment',
);
assert.deepEqual(
  splitAttachmentLine('看一下这个\n\n📎 ["预算 · 最终.xlsx"," leading.txt"]'),
  { text: '看一下这个', attachments: ['预算 · 最终.xlsx', ' leading.txt'] },
  'JSON attachment markers must preserve legal filename characters exactly',
);
assert.deepEqual(
  splitAttachmentLine('📎 报告.pdf · 数据.xlsx'),
  { text: '', attachments: ['报告.pdf', '数据.xlsx'] },
  'legacy attachment-only messages must remain readable',
);
assert.deepEqual(
  splitAttachmentLine('📎 [草稿].md'),
  { text: '', attachments: ['[草稿].md'] },
  'a legacy filename beginning with [ must not be mistaken for malformed JSON',
);
assert.deepEqual(
  splitAttachmentLine('多行正文\n第二行\n\n📎 截图.png'),
  { text: '多行正文\n第二行', attachments: ['截图.png'] },
  'only the trailing attachment line may be split away',
);
assert.deepEqual(
  splitAttachmentLine('正文提到 📎 这个符号但没有附件'),
  { text: '正文提到 📎 这个符号但没有附件', attachments: [] },
  'inline paperclip text must stay in the body',
);
assert.deepEqual(
  splitAttachmentLine('正文\n📎 不是附件行'),
  { text: '正文\n📎 不是附件行', attachments: [] },
  'without the blank-line separator the line belongs to the body',
);
assert.deepEqual(
  splitAttachmentLine('正文\n\n📎 名字.md\n还有别的'),
  { text: '正文\n\n📎 名字.md\n还有别的', attachments: [] },
  'an attachment line must be the final line of the message',
);
assert.deepEqual(splitAttachmentLine(''), { text: '', attachments: [] });
assert.deepEqual(splitAttachmentLine(null), { text: '', attachments: [] });

// ── 侧边栏标题:隐藏持久化协议标记,以完整附件名恢复类型图标 ──
const historicalTitle = sessionTitlePresentation(
  '看看这个\n\n📎 PINV',
  ['PINVOU-M0-开源决策基线.md'],
);
assert.deepEqual(historicalTitle, {
  text: '看看这个',
  attachments: ['PINVOU-M0-开源决策基线.md'],
});
assert.equal(
  sessionTitlePlainText(historicalTitle),
  '看看这个 PINVOU-M0-开源决策基线.md',
);
assert.deepEqual(
  sessionTitlePresentation('普通会话标题', ['不应显示.pdf']),
  { text: '普通会话标题', attachments: [] },
  'backend enrichment must not change manually named sessions without the reserved marker',
);

// ── UserBubble 结构契约:附件独立气泡 + 类型图标,正文为空时不渲染空气泡 ──
const chatViewSource = await readFile(
  new URL('../src/features/chat/ChatView.jsx', import.meta.url),
  'utf8',
);
assert.match(
  chatViewSource,
  /splitAttachmentLine\(item\.text\)/,
  'UserBubble must derive body text and attachments from the shared parser',
);

for (const relativePath of [
  '../src/platform/tauri/bridge/chat.js',
  '../src/platform/web/bridge.js',
]) {
  const bridgeSource = await readFile(new URL(relativePath, import.meta.url), 'utf8');
  assert.match(
    bridgeSource,
    /var displayText = formatAttachmentDisplayText\(text, attachments\)/,
    `${relativePath} must rebuild merged queue display text from the combined attachment list`,
  );
  assert.doesNotMatch(
    bridgeSource,
    /items\.map\(function \(i\) \{ return i\.displayText; \}\)/,
    `${relativePath} must not concatenate already-decorated queued messages`,
  );
}
assert.match(
  chatViewSource,
  /\{bodyText && <div className=\{`min-w-0 max-w-full break-words/,
  'attachment-only messages must not render an empty text bubble',
);
assert.match(
  chatViewSource,
  /<ConversationAttachmentBubble/,
  'user attachments must use the interactive conversation attachment component',
);
assert.match(
  chatViewSource,
  /displayText=\{item\.text\}/,
  'attachment references must include the persisted display text to reject stale index collisions',
);

const bubbleSource = await readFile(
  new URL('../src/features/attachments/ConversationAttachmentBubble.jsx', import.meta.url),
  'utf8',
);
assert.match(
  bubbleSource,
  /FileTypeIcon name=\{name\}/,
  'attachment bubbles must reuse the shared file-type glyphs instead of a new icon dependency',
);
assert.doesNotMatch(
  bubbleSource,
  /\.\.\/tools\/tool-common/,
  'attachments must not depend on the tools feature for shared file icons',
);
assert.match(
  bubbleSource,
  /onClick=\{openAttachment\}/,
  'left click must open or download the conversation attachment',
);
assert.match(
  bubbleSource,
  /onContextMenu=\{openContextMenu\}/,
  'right click must expose attachment actions',
);
assert.doesNotMatch(
  bubbleSource,
  /title=\{`\$\{name\}/,
  'attachment bubbles must not show a native hover tooltip',
);
assert.match(
  bubbleSource,
  /data-conversation-attachment-menu/,
  'the global outside-click handler must distinguish interactions inside the portal menu',
);
assert.match(
  bubbleSource,
  /resolveConversationAttachment/,
  'desktop copy-address must resolve the persisted attachment reference',
);
assert.match(
  bubbleSource,
  /revealConversationAttachment/,
  'desktop context menu must support revealing the file in its manager',
);

const webBridgeSource = await readFile(
  new URL('../src/platform/web/bridge.js', import.meta.url),
  'utf8',
);
assert.match(
  webBridgeSource,
  /web_access_read_conversation_attachment_chunk/,
  'WebUI must download referenced attachments without exposing or requiring their host path',
);

// ── 待发 chip 契约:输入框附件 chip 与消息气泡使用同一套类型图标 ──
const chipsSource = await readFile(
  new URL('../src/features/attachments/AttachmentChips.jsx', import.meta.url),
  'utf8',
);
assert.match(
  chipsSource,
  /FileTypeIcon name=\{attachment\.basename\}/,
  'composer chips must render the shared file-type glyph tile',
);
assert.doesNotMatch(
  chipsSource,
  /<span>📎<\/span>/,
  'composer chips must not fall back to the emoji paperclip',
);

const mainSource = await readFile(
  new URL('../src/app/main.jsx', import.meta.url),
  'utf8',
);
assert.match(mainSource, /sessionTitlePresentation\(s\.title, s\.title_attachment_names\)/);
assert.match(mainSource, /<SessionAttachmentTitle presentation=\{titlePresentation\}/);
assert.match(
  mainSource,
  /sessionTitlePresentation\(s\.title \|\| t\.newChat, s\.title_attachment_names\)/,
  'archived session titles must use the same attachment presentation adapter',
);

const navigationSource = await readFile(
  new URL('../src/components/layout/NavigationComponents.jsx', import.meta.url),
  'utf8',
);
assert.match(
  navigationSource,
  /\{chat\.titleContent \|\| chat\.title\}/,
  'the generic navigation component must accept an app-composed rich title',
);

const timelineSource = await readFile(
  new URL('../src/features/conversation/ConversationTimeline.jsx', import.meta.url),
  'utf8',
);
assert.match(timelineSource, /FileTypeIcon name=\{attachment\.name\}/);
assert.doesNotMatch(timelineSource, /<span>📎<\/span>/);

const codexSource = await readFile(
  new URL('../src/features/codex/CodexAcpView.jsx', import.meta.url),
  'utf8',
);
assert.match(codexSource, /FileTypeIcon name=\{attachment\.name\}/);
assert.doesNotMatch(codexSource, /<span>📎<\/span>/);

const sessionsSource = await readFile(
  new URL('../src-tauri/src/app/commands/sessions.rs', import.meta.url),
  'utf8',
);
assert.match(sessionsSource, /pub title_attachment_names: Vec<String>/);
assert.match(sessionsSource, /session_title_attachment_names\(&store, &metadata\)/);

console.log('attachment bubble logic tests passed');
