import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { splitAttachmentLine } from '../src/features/attachments/attachment-message.js';

// ── splitAttachmentLine: 与 bridge 侧 displayText 拼装约定一一对应 ──
assert.deepEqual(
  splitAttachmentLine('看一下这个\n\n📎 PINVOU-M0-开源决策基线.md'),
  { text: '看一下这个', attachments: ['PINVOU-M0-开源决策基线.md'] },
  'text plus attachment line must split into body and names',
);
assert.deepEqual(
  splitAttachmentLine('📎 报告.pdf · 数据.xlsx'),
  { text: '', attachments: ['报告.pdf', '数据.xlsx'] },
  'attachment-only messages must keep an empty body',
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
  /_artifactKind\(name\)/,
  'attachment bubbles must reuse the shared file-type mapping',
);
assert.match(
  bubbleSource,
  /AcFmtIcon kind=\{kind\}/,
  'attachment bubbles must reuse the shared file-type glyphs instead of a new icon dependency',
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
  /_artifactKind\(attachment\.basename\)/,
  'composer chips must map the file type from the shared table',
);
assert.match(
  chipsSource,
  /AcFmtIcon kind=\{kind\}/,
  'composer chips must render the shared file-type glyph tile',
);
assert.doesNotMatch(
  chipsSource,
  /<span>📎<\/span>/,
  'composer chips must not fall back to the emoji paperclip',
);

console.log('attachment bubble logic tests passed');
