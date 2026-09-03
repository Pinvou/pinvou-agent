#!/usr/bin/env node
import assert from 'node:assert/strict';
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.join(here, '..');
const temp = mkdtempSync(path.join(tmpdir(), 'pinvou3-deepseek-conversation-'));
const conversationDir = path.join(temp, 'features', 'conversation');
mkdirSync(conversationDir, { recursive: true });
writeFileSync(path.join(temp, 'package.json'), '{"type":"module"}\n');
for (const file of ['conversation-model.js', 'deepseek-conversation.js']) {
  copyFileSync(
    path.join(root, 'src', 'features', 'conversation', file),
    path.join(conversationDir, file),
  );
}

try {
  const { pairDeepSeekTimeline, projectDeepSeekConversation } = await import(
    `${pathToFileURL(path.join(conversationDir, 'deepseek-conversation.js')).href}?t=${Date.now()}`
  );
  const {
    countsAsFailedOperation,
    externalMarkdownUrl,
    fetchToolDetails,
    isAgentWaitCall,
    isFetchTool,
    isNearConversationBottom,
    isSearchTool,
    searchToolDetails,
  } = await import(
    `${pathToFileURL(path.join(conversationDir, 'conversation-model.js')).href}?t=${Date.now()}`
  );
  assert.equal(externalMarkdownUrl('http://localhost:8080/'), 'http://localhost:8080/');
  assert.equal(externalMarkdownUrl('https://example.com/demo'), 'https://example.com/demo');
  assert.equal(externalMarkdownUrl('javascript:alert(1)'), '');
  assert.equal(externalMarkdownUrl('README.md'), '');
  assert.equal(isAgentWaitCall('agent', { action: 'wait' }), true);
  assert.equal(isAgentWaitCall('agents/wait', {}), true);
  assert.equal(isAgentWaitCall('agent', { action: 'status' }), false);
  assert.equal(isAgentWaitCall('agents/list', {}), false);
  assert.equal(countsAsFailedOperation({
    type: 'tool',
    status: 'failed',
    tool: { name: 'agent', rawInput: { action: 'wait' } },
  }), false, 'legacy wait failures must not become child-task failures');
  assert.equal(countsAsFailedOperation({
    type: 'tool',
    status: 'failed',
    tool: { name: 'agents/wait', rawInput: {} },
  }), false, 'canonical wait failures must have the same presentation semantics');
  assert.equal(countsAsFailedOperation({
    type: 'tool',
    status: 'failed',
    tool: { name: 'read_file', rawInput: {} },
  }), true, 'ordinary tool failures must remain visible');
  assert.equal(isNearConversationBottom({ scrollHeight: 1000, scrollTop: 820, clientHeight: 100 }), true);
  assert.equal(isNearConversationBottom({ scrollHeight: 1000, scrollTop: 700, clientHeight: 100 }), false);
  const chatItems = [
    { id: 1, type: 'system', text: '会话已恢复' },
    { id: 2, type: 'user', text: '检查仓库' },
    {
      id: 10,
      type: 'reasoning',
      text: '先确认仓库状态。',
      streaming: false,
      startedAt: 1100,
      completedAt: 1400,
    },
    { id: 3, type: 'assistant', html: '<p>先看状态。</p>', streaming: false },
    {
      id: 4,
      type: 'tool',
      toolId: 'shell-1',
      name: 'exec_shell',
      args: { command: 'git status', cwd: '/workspace/pinvou3' },
      output: 'clean',
      success: true,
      state: 'done',
    },
    {
      id: 5,
      type: 'tool',
      toolId: 'read-1',
      name: 'read_file',
      args: { path: 'README.md' },
      output: '读取失败',
      success: false,
      state: 'failed',
    },
    { id: 6, type: 'artifact_card', path: '/tmp/report.md', title: '报告' },
    { id: 7, type: 'user', text: '继续' },
    { id: 8, type: 'assistant', html: '', streaming: true },
    { id: 9, type: 'user_input', resolved: false, questions: [] },
  ];
  const before = structuredClone(chatItems);
  const projected = projectDeepSeekConversation({
    chatItems,
    busy: true,
    thinking: { active: true, phase: 'thinking', startedAt: 123456 },
    tokens: { input: 320, max: 4096 },
    sessionId: 'session-1',
    timelineEvents: [
      { turn_id: 'turn-old', event: 'user_start', timestamp: 1000, ts: '1970-01-01T00:00:01Z' },
      {
        turn_id: 'turn-old',
        event: 'assistant_done',
        timestamp: 4000,
        ts: '1970-01-01T00:00:04Z',
        status: 'Completed',
        usage: { input_tokens: 120, output_tokens: 30 },
      },
      { turn_id: 'turn-live', event: 'user_start', timestamp: 123456, ts: '1970-01-01T00:02:03Z' },
    ],
  });

  assert.deepEqual(chatItems, before, 'projection must never rewrite the DeepSeek chatItems fact source');
  assert.equal(projected.thread.id, 'session-1');
  assert.equal(projected.turns.length, 3, 'preamble and each user message must become stable turns');
  assert.equal(projected.turns[1].userText, '检查仓库');
  assert.deepEqual(
    projected.turns[1].items.map(item => item.type),
    ['reasoning', 'agent_message', 'command_execution', 'tool', 'artifact'],
  );
  assert.deepEqual(
    projected.turns[1].presentation.map(item => item.type),
    ['reasoning', 'agent_message', 'tool_group', 'artifact'],
    'consecutive operations must only be grouped in the presentation projection',
  );
  assert.equal(projected.turns[1].items[0].text, '先确认仓库状态。');
  assert.equal(projected.turns[1].items[0].status, 'completed');
  assert.equal(projected.turns[1].presentation[2].items.length, 2);
  assert.equal(projected.turns[1].operationCount, 2);
  assert.equal(projected.turns[1].failedOperationCount, 1);
  assert.equal(projected.turns[1].status, 'Completed');
  assert.equal(projected.turns[1].completedAt, 4000);
  assert.deepEqual(projected.turns[1].usage, {
    inputTokens: 120,
    outputTokens: 30,
    cacheHitTokens: 0,
    cacheMissTokens: 0,
    cacheWriteTokens: 0,
    reasoningTokens: 0,
  });
  assert.equal(projected.turns[1].items[2].legacyItem, chatItems[4], 'tool cards must retain the original item for provider rendering');
  assert.equal(projected.turns[1].items[3].tool.name, 'read_file', 'shared presentation must retain the provider tool name');
  assert.equal(projected.turns[2].status, 'running');
  assert.equal(projected.turns[2].startedAt, 123456);
  assert.equal(projected.turns[2].waitingPermission, false);
  assert.equal(projected.turns[2].waitingInput, true);
  assert.deepEqual(projected.turns[2].usage, { used: 320, size: 4096 });

  const waitCompatibilityProjection = projectDeepSeekConversation({
    sessionId: 'wait-compatibility',
    chatItems: [
      { id: 1, type: 'user', text: 'wait for child' },
      {
        id: 2,
        type: 'tool',
        name: 'agent',
        args: { action: 'wait', agent_id: 'agent_legacy' },
        output: 'not available yet',
        success: false,
        state: 'failed',
      },
      {
        id: 3,
        type: 'tool',
        name: 'agents/wait',
        args: { agent_id: 'agent_canonical' },
        output: 'not available yet',
        success: false,
        state: 'failed',
      },
      {
        id: 4,
        type: 'tool',
        name: 'read_file',
        args: { path: 'missing.txt' },
        output: 'missing',
        success: false,
        state: 'failed',
      },
    ],
  });
  assert.equal(waitCompatibilityProjection.turns[0].operationCount, 3);
  assert.equal(
    waitCompatibilityProjection.turns[0].failedOperationCount,
    1,
    'only the real execution failure should be counted',
  );

  const rawMarkdownProjection = projectDeepSeekConversation({
    sessionId: 'copy-contract',
    chatItems: [
      { id: 1, type: 'user', text: 'copy' },
      { id: 2, type: 'assistant', text: '## Result\n\n- item', html: '<h2>Result</h2><ul><li>item</li></ul>' },
    ],
  });
  assert.equal(rawMarkdownProjection.turns[0].items[0].text, '## Result\n\n- item');
  assert.equal(rawMarkdownProjection.turns[0].items[0].copyText, undefined);
  assert.deepEqual(
    rawMarkdownProjection.turns[0].items[0].copyOptions,
    { allowScheduledTaskDraft: false },
    'DeepSeek projection must defer canonical copy conversion until the user clicks copy',
  );

  const scheduledMarkdown = '```json\n{"name":"Daily","prompt":"Summarize","rrule":"FREQ=DAILY"}\n```';
  const ordinaryScheduledProjection = projectDeepSeekConversation({
    sessionId: 'ordinary-copy-contract',
    chatItems: [
      { id: 1, type: 'user', text: 'show schema' },
      { id: 2, type: 'assistant', text: scheduledMarkdown, html: '<pre><code>schema</code></pre>' },
    ],
  });
  assert.equal(ordinaryScheduledProjection.turns[0].items[0].text, scheduledMarkdown);
  assert.deepEqual(
    ordinaryScheduledProjection.turns[0].items[0].copyOptions,
    { allowScheduledTaskDraft: false },
  );
  const taskCreationProjection = projectDeepSeekConversation({
    sessionId: 'scheduled-copy-contract',
    allowScheduledTaskDraft: true,
    chatItems: [
      { id: 1, type: 'user', text: 'create task' },
      { id: 2, type: 'assistant', text: scheduledMarkdown, html: '<pre><code>schema</code></pre>' },
    ],
  });
  assert.equal(taskCreationProjection.turns[0].items[0].text, scheduledMarkdown);
  assert.deepEqual(
    taskCreationProjection.turns[0].items[0].copyOptions,
    { allowScheduledTaskDraft: true },
    'scheduled-task classification must remain available to the lazy copy resolver',
  );

  const history = projectDeepSeekConversation({
    chatItems,
    busy: false,
    sessionId: 'session-1',
    timelineEvents: [
      { turn_id: 'turn-live', event: 'user_start', timestamp: 123456, ts: '1970-01-01T00:02:03Z' },
      { turn_id: 'turn-live', event: 'assistant_done', timestamp: 125456, ts: '1970-01-01T00:02:05Z', status: 'Failed', error: '模型失败' },
    ],
  });
  assert.equal(history.turns[2].status, 'Failed');
  assert.equal(history.turns[2].startedAt, 123456);
  assert.equal(history.turns[2].completedAt, 125456);
  assert.equal(history.turns[2].error, '模型失败');
  assert.equal(history.turns[2].lifecycleKnown, true);

  const paired = pairDeepSeekTimeline([
    { turn_id: 'not-admitted', event: 'user_start', timestamp: 1, ts: '1970-01-01T00:00:00Z' },
    { turn_id: 'not-admitted', event: 'assistant_done', timestamp: 2, ts: '1970-01-01T00:00:00Z', status: 'send_error' },
    { turn_id: 'interrupted', event: 'user_start', timestamp: 3, ts: '1970-01-01T00:00:00Z' },
  ]);
  assert.equal(paired.length, 1, 'send_error timing records must not shift visible user turns');
  assert.equal(paired[0].status, 'incomplete');

  const emptyHistory = projectDeepSeekConversation({
    chatItems: [],
    sessionId: 'empty-session',
    timelineEvents: [
      { turn_id: 'orphan-turn', event: 'user_start', timestamp: 10, ts: '1970-01-01T00:00:00Z' },
    ],
  });
  assert.equal(
    emptyHistory.turns.length,
    0,
    'orphan timeline records must not be assigned when the restored session has no visible turns',
  );

  const search = searchToolDetails({
    name: 'web_search',
    rawInput: { query: '2026 年 AI 新闻' },
    rawOutput: '[web_search output compacted to protect context]\\n'
      + 'Snippet: {"query":"2026 年 AI 新闻","source":"bing","count":10,"results":['
      + '{"title":"第一条新闻","url":"https://example.com/one","snippet":"摘要"},'
      + '\\n[... output truncated for context ...]\\n'
      + '{"title":"第二条新闻","url":"https://example.org/two","snippet":"摘要"}]}',
  });
  assert.equal(isSearchTool({ name: 'web_search' }), true);
  assert.equal(isSearchTool({ name: 'file_search' }), false, 'file search must keep its existing file-tool renderer');
  assert.equal(search.query, '2026 年 AI 新闻');
  assert.equal(search.source, 'Bing');
  assert.equal(search.count, 10);
  assert.equal(search.compacted, true);
  assert.deepEqual(search.results, [
    { title: '第一条新闻', url: 'https://example.com/one' },
    { title: '第二条新闻', url: 'https://example.org/two' },
  ]);

  const iwencaiSearch = searchToolDetails({
    name: 'mcp_iwencai_news_search',
    rawInput: { query: 'AI 行业新闻' },
    rawOutput: '[mcp_iwencai_news_search output compacted to protect context]\\n'
      + 'Snippet: {"content":[{"type":"text","text":"{\\n'
      + '  \\"data\\": [{\\"url\\": \\"https://news.example.com/a\\", '
      + '\\"id\\": \\"1\\", \\"title\\": \\"行业新闻标题\\"}]\\n}"}]}',
  });
  assert.equal(iwencaiSearch.source, '同花顺新闻');
  assert.deepEqual(iwencaiSearch.results, [
    { title: '行业新闻标题', url: 'https://news.example.com/a' },
  ]);

  const fetched = fetchToolDetails({
    name: 'fetch_url',
    rawInput: { url: 'https://www.36kr.com/newsflashes', format: 'text' },
    rawOutput: JSON.stringify({
      url: 'https://www.36kr.com/newsflashes',
      status: 200,
      headers: { 'set-cookie': 'internal-cookie-must-not-be-primary-ui' },
      content_type: 'text/html; charset=utf-8',
      content: '快讯 融资 互联网 资本 科技 最新快讯',
      truncated: true,
    }),
  });
  assert.equal(isFetchTool({ name: 'fetch_url' }), true);
  assert.equal(isFetchTool({ name: 'read_file' }), false);
  assert.equal(fetched.target, '36kr.com/newsflashes');
  assert.equal(fetched.status, 200);
  assert.equal(fetched.contentTypeLabel, 'HTML');
  assert.equal(fetched.preview, '快讯 融资 互联网 资本 科技 最新快讯');
  assert.equal(fetched.truncated, true);

  const chatView = readFileSync(path.join(root, 'src', 'features', 'chat', 'ChatView.jsx'), 'utf8');
  const conversationView = readFileSync(path.join(root, 'src', 'features', 'conversation', 'ConversationTimeline.jsx'), 'utf8');
  const questionChoiceCard = readFileSync(path.join(root, 'src', 'features', 'conversation', 'QuestionChoiceCard.jsx'), 'utf8');
  const toolRenderers = readFileSync(path.join(root, 'src', 'features', 'tools', 'tool-renderers.jsx'), 'utf8');
  assert.ok(chatView.includes('<ConversationTimeline'), 'DeepSeek must render through the shared timeline by default');
  assert.ok(chatView.includes('data-testid="chat-artifacts-entry"')
    && chatView.includes('{activeSessionId && (')
    && chatView.includes('const artifactsVisible = Boolean(activeSessionId && artifactsOpen)')
    && chatView.includes('if (!activeSessionId) setArtifactsOpen(false)'),
  'the empty Work home must hide and close the artifacts entry until a session exists');
  assert.ok(chatView.includes('<ConversationActivityIndicator')
    && chatView.includes('turn={activeConversationTurn}')
    && conversationView.includes("if (!turn || turn.status !== 'running') return null"),
  'the composer activity timer must be shared and visible only while a turn is active');
  assert.ok(chatView.includes('!isSearchTool(item.tool)') && chatView.includes('!isFetchTool(item.tool)'),
    'web search and fetch tools must use shared structured renderers while other DeepSeek tools retain provider cards');
  assert.ok(chatView.includes('variant="timeline"'),
    'legacy DeepSeek tool details must use the shared timeline visual shell');
  assert.ok(toolRenderers.includes('data-tool-card-variant="timeline"')
    && toolRenderers.includes('const displayExpanded = hasLiveShellOutput || expanded')
    && conversationView.includes('const expanded = running || open;'),
  'timeline tool cards must stay compact except while a live shell operation needs visible output and controls');
  assert.ok(toolRenderers.includes('<QuestionChoiceCard'),
    'DeepSeek request_user_input must use the shared Codex-style choice card');
  assert.ok(toolRenderers.includes('isFreeTextPlaceholderOption')
    && toolRenderers.includes('!allowOther || !isFreeTextPlaceholderOption(option)'),
  'free-text questions must not render duplicate Other placeholder choices');
  assert.ok(toolRenderers.includes('otherPlaceholder={t.uiToolRender.other}'),
    'DeepSeek and Codex question cards must use the same free-text placeholder');
  assert.ok(questionChoiceCard.includes("type={question.multiSelect ? 'checkbox' : 'radio'}")
    && questionChoiceCard.includes('onClick={submit}'),
  'the shared card must expose explicit radio/checkbox choices and an explicit submit action');
  // 「查看原始数据」文案回退已收敛到 dict.zh.uiConversation.viewRaw。
  const conversationZhDict = readFileSync(path.join(root, 'src', 'shared', 'i18n', 'zh.js'), 'utf8');
  assert.ok(conversationView.includes('c.viewRaw') && conversationZhDict.includes("viewRaw:'查看原始数据'"),
    'model-facing compacted payloads must be secondary diagnostic details');
  // Steered mid-turn messages are not turn admissions: they must never
  // consume a timing record or inherit a phantom lifecycle. Live repros
  // (#308): a naturally queued steer showed an "interrupted" badge, and the
  // parked-steer resume continuation can log an orphan user_start whose
  // assistant_done attaches to the original turn id.
  const steerBadgeCases = [
    {
      label: 'one completed pair (real disk shape, no ui_turn_index)',
      timelineEvents: [
        { turn_id: 't0', event: 'user_start', timestamp: 100 },
        { turn_id: 't0', event: 'assistant_done', timestamp: 200, status: 'Completed' },
      ],
      expectedSteered: 'Completed',
    },
    {
      label: 'orphan resume user_start without its own terminal',
      timelineEvents: [
        { turn_id: 't0', event: 'user_start', timestamp: 100 },
        { turn_id: 't0', event: 'assistant_done', timestamp: 200, status: 'Completed' },
        { turn_id: 't1', event: 'user_start', timestamp: 300 },
      ],
      expectedSteered: 'Completed',
    },
    {
      label: 'genuine stop during the continuation inherits the honest interrupted terminal',
      timelineEvents: [
        { turn_id: 't0', event: 'user_start', timestamp: 100 },
        { turn_id: 't1', event: 'user_start', timestamp: 300 },
        { turn_id: 't1', event: 'assistant_done', timestamp: 400, status: 'Interrupted' },
      ],
      expectedSteered: 'Interrupted',
    },
  ];
  for (const steerCase of steerBadgeCases) {
    const steeredProjection = projectDeepSeekConversation({
      chatItems: [
        { id: 20, type: 'user', text: '总结战国七雄的兴衰历史' },
        { id: 21, type: 'assistant', text: 'seg1', html: '<p>seg1</p>' },
        { id: 22, type: 'user', text: '以秦国为主', steeredMidTurn: true },
        { id: 23, type: 'assistant', text: 'seg2', html: '<p>seg2</p>' },
      ],
      busy: false,
      thinking: null,
      sessionId: 'session-steer',
      timelineEvents: steerCase.timelineEvents,
    });
    assert.equal(steeredProjection.turns.length, 2, `steered message still renders as its own visual turn (${steerCase.label})`);
    assert.ok(
      !steeredProjection.turns[0].lifecycleKnown && !steeredProjection.turns[0].completedAt,
      `the run head carries no badge once the terminal moved to the tail (${steerCase.label})`,
    );
    assert.equal(steeredProjection.turns[1].status, steerCase.expectedSteered, `the run tail shows the engine turn's terminal (${steerCase.label})`);
  }

  // While the turn is still running, the admitted turn's user_start has no
  // assistant_done yet — and its own response segment has already finished,
  // so the queue/continuation has nothing to do with it: NO badge at all
  // (live repro: it badged "interrupted" during a steered continuation and
  // only flipped to completed at the end; "processing" was equally wrong).
  const inFlightProjection = projectDeepSeekConversation({
    chatItems: [
      { id: 30, type: 'user', text: '总结战国七雄的兴衰历史' },
      { id: 31, type: 'assistant', text: 'seg1', html: '<p>seg1</p>' },
      { id: 32, type: 'user', text: '以秦国为主', steeredMidTurn: true },
      { id: 33, type: 'assistant', text: 'seg2', html: '<p>seg2</p>', streaming: true },
    ],
    busy: true,
    thinking: { active: true, phase: 'thinking', startedAt: 123456 },
    sessionId: 'session-steer-live',
    timelineEvents: [
      // Live records carry ui_turn_index (the frontend stamps it when the
      // turn starts; authoritative refreshes backfill it within the open
      // window) — the primary matching path is what runs while busy.
      { turn_id: 't0', event: 'user_start', timestamp: 100, ui_turn_index: 0 },
    ],
  });
  assert.ok(
    !inFlightProjection.turns[0].lifecycleKnown && !inFlightProjection.turns[0].completedAt && !inFlightProjection.turns[0].error,
    'an in-flight turn renders no status footer at all while the session is busy',
  );
  assert.equal(inFlightProjection.turns[1].status, 'running', 'the trailing steered turn stays the active one');

  // Older terminals must not stick to a queue in flight: an earlier turn's
  // Completed/Interrupted belongs to that turn only — while a new engine turn
  // runs with a steered queue, its turns show nothing until their own
  // terminal (live feedback: every queue's previous turn inherited the first
  // terminal ever seen).
  const stickyProjection = projectDeepSeekConversation({
    chatItems: [
      { id: 40, type: 'user', text: '更早的问题' },
      { id: 41, type: 'assistant', text: 'done seg', html: '<p>done</p>' },
      { id: 42, type: 'user', text: '本轮问题' },
      { id: 43, type: 'assistant', text: 'seg1', html: '<p>seg1</p>' },
      { id: 44, type: 'user', text: '本轮排队', steeredMidTurn: true },
      { id: 45, type: 'assistant', text: 'seg2', html: '<p>seg2</p>', streaming: true },
    ],
    busy: true,
    thinking: { active: true, phase: 'thinking', startedAt: 123456 },
    sessionId: 'session-steer-sticky',
    timelineEvents: [
      { turn_id: 'old', event: 'user_start', timestamp: 10, ui_turn_index: 0 },
      { turn_id: 'old', event: 'assistant_done', timestamp: 20, status: 'Completed', ui_turn_index: 0 },
      { turn_id: 'now', event: 'user_start', timestamp: 100, ui_turn_index: 1 },
    ],
  });
  assert.equal(stickyProjection.turns[0].status, 'Completed', 'the earlier turn keeps its own terminal');
  assert.ok(
    !stickyProjection.turns[1].lifecycleKnown && !stickyProjection.turns[1].completedAt,
    'the running turn inherits no old terminal while busy',
  );
  assert.equal(
    stickyProjection.turns[2].status, 'running',
    'the trailing steered queue runs (no inherited old terminal badge)',
  );

  assert.ok(
    conversationView.includes('const assistantRowVisible = running || presentation.length > 0 || assistantFooterVisible;')
      && conversationView.includes('{assistantRowVisible && ('),
    'a turn with no assistant content must not render an avatar-only row (steered message sandwiched between consecutive injections)',
  );

  assert.ok(conversationView.includes("closest('a[href]')")
    && conversationView.includes('event.preventDefault()')
    && conversationView.includes('onOpenExternal(external)'),
  'conversation markdown links must not navigate the application webview');
  assert.ok(conversationView.includes("String(tool.name || '').trim() || 'web_search'")
    && conversationView.includes("String(tool.name || '').trim() || 'fetch_url'")
    && conversationView.includes('title={toolName}'),
  'search and fetch cards must preserve their actual tool names instead of translated titles');
  assert.ok(!conversationView.includes('`搜索“${query.length') && !conversationView.includes('`抓取 ${details.target}`'),
    'search and fetch tool names must not be replaced by Chinese action phrases');
  assert.ok(conversationView.includes('const [open, setOpen] = useState(false)')
    && !conversationView.includes('setOpen(autoOpen)')
    && !chatView.includes('shouldAutoOpenToolGroup='),
  'tool groups must keep a user-owned expansion state instead of opening and closing with execution status');
  assert.ok(chatView.includes('const transition = transitionConversationScrollState({')
    && chatView.includes('autoScrollRef.current = transition.following'),
    'DeepSeek streaming must pause auto-follow while the user reads history');
  assert.ok(chatView.includes('previousScrollHeight: lastScrollHeightRef.current')
    && chatView.includes('lastScrollHeightRef.current = transition.scrollHeight'),
    'a shrink-induced scrollTop clamp must not be mistaken for the user browsing history');
  assert.ok(chatView.includes('startConversationBottomFollower({')
    && chatView.includes('isFollowing: () => autoScrollRef.current')
    && chatView.includes('onMeasured: () => {'),
    'bottom-following conversations must recover after delayed layout and window visibility changes');
  assert.ok(chatView.includes('<ThinkingBubble'), 'the original rendering path must remain available as a fallback');
  assert.ok(chatView.includes("pinvou_conversation_ui_v2"), 'the local rollback switch must be explicit');

  console.log('deepseek_conversation_timeline: ok');
} finally {
  rmSync(temp, { recursive: true, force: true });
}
