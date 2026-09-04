import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import {
  assistantItemCopyText,
  assistantMarkdownCopyText,
  assistantResponseAvailable,
  assistantResponseText,
  copyClipboardText,
  normalizeAssistantMessageText,
} from '../src/features/conversation/message-clipboard.js';
import {
  assistantExportFilename,
  buildAssistantResponseExport,
} from '../src/features/conversation/assistant-response-export.js';
import { dict } from '../src/shared/i18n-all.js'; // 三语全量断言:浏览器入口用 i18n.js 惰性装载,测试用聚合 shim
import { renderMarkdownMarkup } from '../src/shared/markdown-renderer.js';

const source = relative => readFileSync(new URL(`../src/${relative}`, import.meta.url), 'utf8');

const markdown = '## 结论\n\n- 第一项\n- 第二项\n\n```js\nconst value = 1;\n```';
assert.equal(
  assistantMarkdownCopyText(`  ${markdown}\r\n`),
  markdown,
  'canonical copy text should retain Markdown structure and normalize line endings',
);
const fixedExportDate = new Date(2026, 7, 14, 9, 8, 7);
assert.equal(
  assistantExportFilename('md', fixedExportDate),
  'pinvou-response-20260814-090807.md',
  'Markdown must be the stable default export naming format',
);
assert.equal(
  buildAssistantResponseExport('## Result\r\n\r\nDone', 'md').content,
  '## Result\n\nDone\n',
  'Markdown export must retain canonical source formatting with normalized line endings',
);
const htmlExport = buildAssistantResponseExport('## Result\n\n<script>alert(1)</script>', 'html', {
  title: 'Response <unsafe>',
  language: 'en',
});
assert.match(htmlExport.content, /^<!doctype html>/);
assert.match(htmlExport.content, /<html lang="en">/);
assert.match(htmlExport.content, /<title>Response &lt;unsafe&gt;<\/title>/);
assert.doesNotMatch(htmlExport.content, /<script>/i, 'HTML export must not preserve executable response markup');
assert.match(
  htmlExport.content,
  /Content-Security-Policy" content="default-src 'none'; img-src data:; style-src 'unsafe-inline'"/,
  'HTML export CSP must block remote requests (tracking pixels) — the document must stay self-contained',
);
assert.throws(
  () => buildAssistantResponseExport('answer', 'pdf'),
  /Unsupported assistant export format/,
  'only the requested Markdown and HTML formats may be exported',
);
const indentedCode = '    const first = 1;\n    const second = 2;';
assert.equal(
  normalizeAssistantMessageText(`\n${indentedCode}\n`),
  indentedCode,
  'normalization must preserve indentation that defines a Markdown code block',
);
const nonBreakingMarkdown = 'Text\u00a0value\n\n```text\ncode\u00a0value\n```';
assert.equal(
  assistantMarkdownCopyText(nonBreakingMarkdown),
  nonBreakingMarkdown,
  'canonical source Markdown must preserve non-breaking spaces',
);
assert.equal(
  await assistantItemCopyText({ text: markdown, html: '<h2>不应使用 HTML</h2>' }),
  markdown,
  'new ordinary messages should copy their source Markdown instead of rendered DOM text',
);
assert.equal(
  await assistantItemCopyText({ html: '<h2>历史标题</h2><p>历史内容</p>' }),
  '## 历史标题\n\n历史内容',
  'legacy HTML-only messages should remain copyable through a Markdown compatibility conversion',
);
const legacyPersonaHtml = renderMarkdownMarkup(
  '```persona-card\n{"name":"历史审查员","description":"可见卡","body":"隐藏提示词"}\n```',
);
assert.equal(
  await assistantItemCopyText({ html: legacyPersonaHtml }),
  '历史审查员\n\n可见卡',
  'legacy HTML-only persona cards should keep the same semantic serializer',
);
assert.equal(
  await assistantItemCopyText({ html: renderMarkdownMarkup(
    '```card-question\n{"question":"历史问题？","options":["甲","乙"]}\n```',
  ) }),
  '历史问题？\n\n1. 甲\n2. 乙',
  'legacy HTML-only question cards should use the same semantic serializer',
);
assert.equal(
  await assistantItemCopyText(
    { html: renderMarkdownMarkup(
      '```scheduled-task-draft\n{"name":"历史简报","prompt":"汇总进展","rrule":"FREQ=DAILY"}\n```',
    ) },
    { allowScheduledTaskDraft: true },
  ),
  '历史简报\n\n汇总进展\n\nFREQ=DAILY',
  'legacy HTML-only scheduled-task cards should serialize in task-creation context',
);
assert.equal(
  await assistantItemCopyText({ html: renderMarkdownMarkup(
    '```card-question\n{"question":"历史问题？","options":["甲","乙"]}\n```',
  ) }, { allowScheduledTaskDraft: false }),
  '历史问题？\n\n1. 甲\n2. 乙',
  'legacy question cards must not depend on the scheduled-task context',
);

const cardQuestion = [
  '请选择部署方式：',
  '',
  '```card-question',
  '{"question":"部署到哪里？","options":["本机","测试环境"]}',
  '```',
].join('\n');
assert.equal(
  assistantMarkdownCopyText(cardQuestion),
  '请选择部署方式：\n\n部署到哪里？\n\n1. 本机\n2. 测试环境',
  'question cards should copy their visible question and numbered options instead of hidden JSON',
);

const personaOnly = [
  '```persona-card',
  '{"name":"代码审查员","emoji":"🔎","description":"检查设计与副作用","body":"完整内部提示词"}',
  '```',
].join('\n');
assert.equal(
  assistantMarkdownCopyText(personaOnly),
  '🔎 代码审查员\n\n检查设计与副作用',
  'card-only replies should still produce meaningful copy text',
);

const scheduledTaskOnly = [
  '```scheduled-task-draft',
  '{"name":"每日简报","prompt":"汇总今日进展","rrule":"FREQ=DAILY;BYHOUR=9"}',
  '```',
].join('\n');
assert.equal(
  assistantMarkdownCopyText(scheduledTaskOnly, { allowScheduledTaskDraft: true }),
  '每日简报\n\n汇总今日进展\n\nFREQ=DAILY;BYHOUR=9',
  'scheduled-task cards should copy readable task content instead of hidden JSON',
);
assert.equal(
  assistantMarkdownCopyText(scheduledTaskOnly),
  scheduledTaskOnly,
  'scheduled-task payloads must remain source Markdown outside the task-card context',
);

const ordinaryScheduledJson = scheduledTaskOnly.replace('scheduled-task-draft', 'json');
assert.equal(
  await assistantItemCopyText({ text: ordinaryScheduledJson }),
  ordinaryScheduledJson,
  'ordinary JSON examples must not be inferred as scheduled-task cards',
);
assert.equal(
  await assistantItemCopyText({ text: ordinaryScheduledJson }, { allowScheduledTaskDraft: true }),
  '每日简报\n\n汇总今日进展\n\nFREQ=DAILY;BYHOUR=9',
  'task creation conversations should serialize the same generic payload that their UI renders as a card',
);
assert.equal(
  await assistantResponseText({ items: [{ type: 'agent_message', text: ordinaryScheduledJson }] }),
  ordinaryScheduledJson,
  'Codex must retain ordinary JSON that is not rendered through the DeepSeek task-card protocol',
);

const secondPersonaFence = [
  '```json',
  '{"name":"第二位审查员","body":"第二段可见提示词","description":"第二段完整 JSON 必须保留"}',
  '```',
].join('\n');
assert.equal(
  assistantMarkdownCopyText(`${personaOnly}\n\n${secondPersonaFence}`),
  `🔎 代码审查员\n\n检查设计与副作用\n\n${secondPersonaFence}`,
  'only the persona fence selected by the UI should be serialized',
);

const genericPersonaFence = personaOnly.replace('persona-card', 'json');
const unlabeledPersonaFence = personaOnly.replace('persona-card', '');
assert.equal(
  assistantMarkdownCopyText(unlabeledPersonaFence),
  assistantMarkdownCopyText(personaOnly),
  'unlabelled fences classified as persona cards by the UI must use the same copy serializer',
);
assert.equal(
  assistantMarkdownCopyText(`${genericPersonaFence}\n\n${personaOnly}`),
  `${genericPersonaFence}\n\n🔎 代码审查员\n\n检查设计与副作用`,
  'an explicitly tagged persona must win over an earlier generic candidate like the UI parser',
);

const secondScheduledFence = [
  '```json',
  '{"name":"每周简报","prompt":"汇总本周进展","rrule":"FREQ=WEEKLY"}',
  '```',
].join('\n');
assert.equal(
  assistantMarkdownCopyText(`${ordinaryScheduledJson}\n\n${secondScheduledFence}`, { allowScheduledTaskDraft: true }),
  `每日简报\n\n汇总今日进展\n\nFREQ=DAILY;BYHOUR=9\n\n${secondScheduledFence}`,
  'only the scheduled-task fence selected by the UI should be serialized',
);

const secondQuestionFence = cardQuestion.replace('请选择部署方式：\n\n', '')
  .replace('部署到哪里？', '是否继续？')
  .replace('["本机","测试环境"]', '["继续","取消"]');
assert.equal(
  assistantMarkdownCopyText(`${cardQuestion}\n\n${secondQuestionFence}`),
  `请选择部署方式：\n\n部署到哪里？\n\n1. 本机\n2. 测试环境\n\n${secondQuestionFence}`,
  'only the first valid card-question fence should be serialized',
);

const prefixedJsonFence = '```persona-card\n说明：{"name":"不可见卡片","body":"不应被解析"}\n```';
assert.equal(
  assistantMarkdownCopyText(prefixedJsonFence),
  prefixedJsonFence,
  'copy classification must reject fenced content that the UI parser does not treat as JSON',
);

const personaPayload = '{"name":"Reviewer","body":"hidden prompt","description":"Visible card"}';
for (const [label, variant] of [
  ['longer closing fence', `\`\`\`persona-card\n${personaPayload}\n\`\`\`\``],
  ['indented fence with trailing spaces', `  \`\`\`persona-card\n  ${personaPayload}\n  \`\`\`   `],
  ['unclosed fence', `\`\`\`persona-card\n${personaPayload}`],
  ['tilde fence', `~~~persona-card\n${personaPayload}\n~~~~`],
  ['payload with inline tab', `\`\`\`persona-card\n{"name":\t"Reviewer","body":"hidden prompt","description":"Visible card"}\n\`\`\``],
]) {
  assert.equal(
    assistantMarkdownCopyText(variant),
    'Reviewer\n\nVisible card',
    `${label} must use the same fenced-code semantics as the Markdown renderer`,
  );
}

for (const [label, variant] of [
  ['blockquote fence', `> \`\`\`persona-card\n> ${personaPayload}\n> \`\`\``],
  ['list fence', `- \`\`\`persona-card\n  ${personaPayload}\n  \`\`\``],
  ['nested blockquote list fence', `> - \`\`\`persona-card\n>   ${personaPayload}\n>   \`\`\``],
]) {
  assert.equal(
    assistantMarkdownCopyText(variant),
    'Reviewer\n\nVisible card',
    `${label} must not expose a structured payload hidden by the rendered UI`,
  );
}

const tabbedQuote = `> before\ttext\n>\n> \`\`\`persona-card\n> ${personaPayload}\n> \`\`\``;
assert.equal(
  assistantMarkdownCopyText(tabbedQuote),
  '> before\ttext\n>\nReviewer\n\nVisible card',
  'inline tabs in container text must not break structured fence mapping',
);

const quotedPersonaWithContext = [
  '> 引用前文',
  '> ```persona-card',
  `> ${personaPayload}`,
  '> ```',
  '> 引用后文',
].join('\n');
assert.equal(
  assistantMarkdownCopyText(quotedPersonaWithContext),
  '> 引用前文\nReviewer\n\nVisible card\n> 引用后文',
  'nested fence replacement must preserve source content outside the selected block',
);

const secondQuotedPersona = quotedPersonaWithContext.replaceAll('Reviewer', 'Second')
  .replaceAll('Visible card', 'Second card')
  .replace('> 引用前文\n', '')
  .replace('\n> 引用后文', '');
assert.equal(
  assistantMarkdownCopyText(`${quotedPersonaWithContext}\n\n${secondQuotedPersona}`),
  `> 引用前文\nReviewer\n\nVisible card\n> 引用后文\n\n${secondQuotedPersona}`,
  'only the nested structured fence selected by the UI should be serialized',
);

for (const [label, variant] of [
  ['ordered-list continuation', `10. item\n    \`\`\`persona-card\n    ${personaPayload}\n    \`\`\``],
  ['unordered-list continuation', `- item\n    \`\`\`persona-card\n    ${personaPayload}\n    \`\`\``],
  ['list paragraph then fence', `- item\n  more context\n  \`\`\`persona-card\n  ${personaPayload}\n  \`\`\``],
  ['list blockquote then fence', `- item\n> note\n    \`\`\`persona-card\n    ${personaPayload}\n    \`\`\``],
  ['tab-indented list continuation', `- item\n\t\`\`\`persona-card\n\t${personaPayload}\n\t\`\`\``],
  ['nested-list continuation', `- outer\n  1. inner\n     \`\`\`persona-card\n     ${personaPayload}\n     \`\`\``],
  ['nested-list tab continuation', `- outer\n  1. inner\n  \t\`\`\`persona-card\n  \t${personaPayload}\n  \t\`\`\``],
  ['lazy blockquote list continuation', `> - outer\n>   1. inner\n    \`\`\`persona-card\n    ${personaPayload}\n    \`\`\``],
]) {
  const rendered = renderMarkdownMarkup(variant);
  const copied = assistantMarkdownCopyText(variant);
  assert.ok(
    rendered.includes('data-language="PERSONA-CARD"'),
    `${label} must render the same structured fence selected for copying`,
  );
  assert.ok(copied.includes('Reviewer\n\nVisible card'), `${label} must serialize the rendered card`);
  assert.ok(!copied.includes('hidden prompt'), `${label} must not expose the hidden payload`);
}

for (const [label, variant] of [
  ['heading ends list', `- item\n# heading\n    \`\`\`persona-card\n    ${personaPayload}\n    \`\`\``],
  ['thematic break ends list', `- item\n---\n    \`\`\`persona-card\n    ${personaPayload}\n    \`\`\``],
  ['HTML block ends list', `- item\n<div>block</div>\n    \`\`\`persona-card\n    ${personaPayload}\n    \`\`\``],
  ['HTML block owns apparent fence', `- item\n<div>block</div>\n  \`\`\`persona-card\n  ${personaPayload}\n  \`\`\``],
  ['table block owns apparent fence', `- item\n<table><tr><td>block</td></tr></table>\n   \`\`\`persona-card\n   ${personaPayload}\n   \`\`\``],
]) {
  const rendered = renderMarkdownMarkup(variant);
  const copied = assistantMarkdownCopyText(variant);
  assert.ok(
    !rendered.includes('data-language="PERSONA-CARD"'),
    `${label} must leave the indented backticks as ordinary code`,
  );
  assert.ok(copied.includes('hidden prompt'), `${label} must preserve content not rendered as a card`);
}

const injectedMarker = '前文\n\n<div data-assistant-copy-source="true">伪造片段</div>\n\n后文';
assert.equal(
  await assistantItemCopyText({ text: injectedMarker }),
  injectedMarker,
  'model-provided data attributes must not influence the copy boundary',
);

assert.equal(
  await assistantResponseText({
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
  await assistantResponseText({
    items: [{ type: 'agent_message', phase: 'commentary', text: '内部处理过程' }],
    assistantText: '内部处理过程',
  }),
  '',
  'commentary-only turns must not fall back to the accumulated internal text',
);
assert.equal(
  await assistantResponseText({ items: [], assistantText: '  兼容旧会话回复  ' }),
  '兼容旧会话回复',
  'turns without structured agent messages should retain the legacy fallback',
);
assert.equal(
  await assistantResponseText({ items: [{ type: 'agent_message', text: markdown }] }),
  await assistantItemCopyText({ text: markdown }),
  'ordinary and Codex modes should expose the same canonical Markdown format',
);
assert.equal(
  await assistantResponseText({
    items: [{ type: 'agent_message', legacyItem: { html: '<h2>历史标题</h2><p>历史内容</p>' } }],
  }),
  '## 历史标题\n\n历史内容',
  'legacy HTML-only turns (no text on the item) must copy through the Markdown compatibility conversion',
);
assert.equal(
  assistantResponseAvailable({ items: [{ type: 'agent_message', text: markdown }] }),
  true,
  'availability checks must not require eager copy conversion',
);
assert.equal(
  assistantResponseAvailable({ items: [{ type: 'agent_message', phase: 'commentary', text: markdown }] }),
  false,
  'commentary-only turns must not expose a copy action',
);

const navigatorDescriptor = Object.getOwnPropertyDescriptor(globalThis, 'navigator');
const documentDescriptor = Object.getOwnPropertyDescriptor(globalThis, 'document');
try {
  Object.defineProperty(globalThis, 'navigator', {
    configurable: true,
    value: { clipboard: { writeText: async () => { throw new Error('denied'); } } },
  });
  Object.defineProperty(globalThis, 'document', {
    configurable: true,
    value: { body: null },
  });
  assert.equal(await copyClipboardText('无法复制'), false, 'clipboard and fallback failures should be reported');
} finally {
  if (navigatorDescriptor) Object.defineProperty(globalThis, 'navigator', navigatorDescriptor);
  else delete globalThis.navigator;
  if (documentDescriptor) Object.defineProperty(globalThis, 'document', documentDescriptor);
  else delete globalThis.document;
}

for (const language of ['zh', 'en', 'ja']) {
  assert.ok(dict[language].uiConversation.copyReply, `${language}.uiConversation.copyReply must exist`);
  assert.ok(dict[language].uiConversation.copyReplySuccess, `${language}.uiConversation.copyReplySuccess must exist`);
  assert.ok(dict[language].uiConversation.copyReplyFailed, `${language}.uiConversation.copyReplyFailed must exist`);
  for (const key of [
    'exportReply', 'exportReplyTitle', 'exportMarkdown', 'exportMarkdownHint', 'exportHtml', 'exportHtmlHint', 'exportSuccess', 'exportFailed',
    'shareReply', 'shareReplyTitle', 'shareSystem', 'shareSystemHint', 'shareApps', 'shareSuccess', 'shareFailed', 'shareUnavailable',
  ]) {
    assert.ok(dict[language].uiConversation[key], `${language}.uiConversation.${key} must exist`);
  }
  for (const fn of ['shareCopiedOpen', 'shareCopiedWeb']) {
    assert.equal(typeof dict[language].uiConversation[fn], 'function', `${language}.uiConversation.${fn} must be a function`);
  }
  for (const target of ['wechat', 'wecom', 'feishu', 'dingtalk', 'qq']) {
    assert.ok(dict[language].uiConversation.shareTargets[target], `${language} share target ${target} must exist`);
  }
}

const chatView = source('features/chat/ChatView.jsx');
const timeline = source('features/conversation/ConversationTimeline.jsx');
const actions = source('features/conversation/AssistantMessageActions.jsx');
const clipboard = source('features/conversation/message-clipboard.js');
const exportLogic = source('features/conversation/assistant-response-export.js');
const saveActions = source('features/conversation/assistant-response-save.js');
const nativeActions = readFileSync(new URL('../src-tauri/src/app/commands/assistant_response.rs', import.meta.url), 'utf8');
const nativeHandler = readFileSync(new URL('../src-tauri/src/lib.rs', import.meta.url), 'utf8');
assert.match(chatView, /showAssistantActions && assistantCopyAvailable && <AssistantMessageFooter>[\s\S]*?<AssistantMessageActions[\s\S]*?resolveText=\{\(\) => assistantItemCopyText/);
assert.match(chatView, /allowScheduledTaskDraft=\{isScheduledTaskCreationChat\} showAssistantActions=\{false\}/);
assert.match(chatView, /allowScheduledTaskDraft: isScheduledTaskCreationChat/);
assert.doesNotMatch(chatView, /data-assistant-copy-source/);
assert.match(actions, /data-testid="assistant-message-footer"/);
assert.match(actions, /className="!mt-0 flex min-h-8 flex-wrap items-center gap-x-2 gap-y-1 pt-2"/);
assert.match(actions, /data-testid="assistant-message-actions"/);
assert.match(actions, /typeof resolveText === 'function' \? await resolveText\(\) : text/);
assert.match(actions, /copyClipboardText\(value\)/);
assert.match(actions, /aria-live="polite"/);
assert.match(actions, /data-testid="assistant-message-export"/);
assert.match(actions, /data-testid="assistant-message-share"/);
assert.match(actions, /data-testid="assistant-export-md"/);
assert.match(actions, /data-testid="assistant-export-html"/);
// eslint-disable-next-line no-unused-vars -- stub keeps the full call signature
for (const target of ['wechat', 'wecom', 'feishu', 'dingtalk', 'qq']) {
  assert.match(actions, new RegExp(`assistant-share-\\$\\{target\\}`));
}
assert.match(exportLogic, /renderMarkdown\(normalized\)/);
assert.match(saveActions, /invokeTauri\('export_assistant_response'/);
assert.match(saveActions, /navigator\?\.share/);
assert.match(saveActions, /invokeTauri\('open_assistant_share_target'/);
assert.match(nativeActions, /MAX_EXPORT_BYTES/);
assert.match(nativeActions, /share_target_uri\(&target\)/);
assert.match(nativeHandler, /commands::assistant_response::export_assistant_response/);
assert.match(nativeHandler, /commands::assistant_response::open_assistant_share_target/);
assert.doesNotMatch(actions, /targetRef|querySelector/);
assert.doesNotMatch(clipboard, /querySelectorAll|data-assistant-copy-source/);
assert.match(timeline, /<AssistantMessageFooter>[\s\S]*?<AssistantMessageActions resolveText=\{\(\) => assistantResponseText\(turn\)\} copy=\{c\}/);
// The legacy codex Turn footer was removed along with the old timeline; the completed-response actions contract is carried solely by the unified timeline.
const conversationTimeline = source('features/conversation/ConversationTimeline.jsx');
assert.match(conversationTimeline, /<AssistantMessageActions resolveText=\{\(\) => assistantResponseText\(turn\)\} copy=\{c\}/);

console.log('assistant message actions tests passed');
