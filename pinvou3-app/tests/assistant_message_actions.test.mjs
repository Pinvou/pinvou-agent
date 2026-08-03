import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import {
  assistantMessageText,
  assistantResponseText,
} from '../src/features/conversation/message-clipboard.js';
import { dict } from '../src/shared/i18n.js';

const source = relative => readFileSync(new URL(`../src/${relative}`, import.meta.url), 'utf8');

assert.equal(
  assistantMessageText({ innerText: '  第一段\n\n\n第二段  \n' }),
  '第一段\n\n第二段',
  'copy text should preserve paragraph structure while trimming rendered whitespace',
);
assert.equal(
  assistantMessageText({ textContent: '回答\u00a0内容' }),
  '回答 内容',
  'copy text should fall back to textContent and normalize non-breaking spaces',
);
assert.equal(assistantMessageText(null), '', 'missing rendered content should not copy placeholder text');
assert.equal(
  assistantResponseText({
    items: [
      { type: 'agent_message', phase: 'commentary', text: '处理中' },
      { type: 'tool', text: 'tool output' },
      { type: 'agent_message', phase: 'message', text: '最终回答第一段' },
      { type: 'agent_message', text: '最终回答第二段' },
    ],
  }),
  '最终回答第一段\n\n最终回答第二段',
  'turn copy should include final assistant messages without commentary or tool output',
);
assert.equal(
  assistantResponseText({ items: [], assistantText: '  兼容旧会话回复  ' }),
  '兼容旧会话回复',
  'legacy turns should fall back to their accumulated assistant text',
);

for (const language of ['zh', 'en', 'ja']) {
  assert.ok(dict[language].uiConversation.copyReply, `${language}.uiConversation.copyReply must exist`);
  assert.ok(dict[language].uiConversation.copyReplySuccess, `${language}.uiConversation.copyReplySuccess must exist`);
  assert.ok(dict[language].uiConversation.copyReplyFailed, `${language}.uiConversation.copyReplyFailed must exist`);
}

const chatView = source('features/chat/ChatView.jsx');
const timeline = source('features/conversation/ConversationTimeline.jsx');
const codexView = source('features/codex/CodexAcpView.jsx');
const actions = source('features/conversation/AssistantMessageActions.jsx');
assert.match(chatView, /!item\.streaming && <AssistantMessageActions[^>]+targetRef=\{assistantSelectionTargetRef\}/);
assert.match(actions, /data-testid="assistant-message-actions"/);
assert.match(actions, /copyClipboardText\(value\)/);
assert.match(actions, /aria-live="polite"/);
assert.match(timeline, /!running && assistantText && <AssistantMessageActions text=\{assistantText\} copy=\{c\}/);
assert.match(codexView, /!running && assistantText && <AssistantMessageActions text=\{assistantText\} copy=\{copy\}/);

console.log('assistant message actions tests passed');
