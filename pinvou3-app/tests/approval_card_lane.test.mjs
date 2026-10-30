#!/usr/bin/env node
// tri-mode 审批面（approval card）双车道回归：
// 1. 原生车道纯逻辑（code-native-lane.js，临时 type:module 目录导入，风格对齐
//    code_native_lane.test.mjs）：chat:approval_required 幂等去重、snake_case /
//    camelCase 双通道负载、guardrail 硬拒通知、mode_changed 无可视变化、
//    get_pending_approvals 重放幂等（含决策后重放不得复活已收口的卡）。
// 2. 源码契约（multiagent_plan_normalize 风格的 regex pin）：bridge chat-events
//    监听器去重 + mode_changed 走统一收敛点、interaction 决策的显式 sessionId
//    invoke 形状、sessions 切会话重放、CodexAcpView 事件表与还原路径、
//    ChatView 的 auto 分支不经 Yolo 确认门（门只对 target==='yolo'）。
import assert from 'node:assert/strict';
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.join(here, '..');
const read = (...segments) => readFileSync(path.join(root, ...segments), 'utf8');

// ── 1. 原生车道运行时 ─────────────────────────────────────────────
const temp = mkdtempSync(path.join(tmpdir(), 'pinvou3-approval-lane-'));
writeFileSync(path.join(temp, 'package.json'), '{"type":"module"}\n');
mkdirSync(path.join(temp, 'features', 'conversation'), { recursive: true });
mkdirSync(path.join(temp, 'features', 'codex'), { recursive: true });
mkdirSync(path.join(temp, 'features', 'multiagent'), { recursive: true });
mkdirSync(path.join(temp, 'shared'), { recursive: true });
for (const file of ['conversation-model.js', 'deepseek-conversation.js']) {
  copyFileSync(path.join(root, 'src', 'features', 'conversation', file), path.join(temp, 'features', 'conversation', file));
}
copyFileSync(path.join(root, 'src', 'features', 'codex', 'code-native-lane.js'), path.join(temp, 'features', 'codex', 'code-native-lane.js'));
copyFileSync(path.join(root, 'src', 'features', 'codex', 'acp-state.js'), path.join(temp, 'features', 'codex', 'acp-state.js'));
copyFileSync(path.join(root, 'src', 'features', 'multiagent', 'spawn-aggregation.mjs'), path.join(temp, 'features', 'multiagent', 'spawn-aggregation.mjs'));
copyFileSync(path.join(root, 'src', 'shared', 'internal-message.mjs'), path.join(temp, 'shared', 'internal-message.mjs'));
copyFileSync(path.join(root, 'src', 'shared', 'shell-tools.mjs'), path.join(temp, 'shared', 'shell-tools.mjs'));
copyFileSync(path.join(root, 'src', 'shared', 'path-utils.js'), path.join(temp, 'shared', 'path-utils.js'));

try {
  const { applyNativeChatEvent, createNativeLane } = await import(
    `${pathToFileURL(path.join(temp, 'features', 'codex', 'code-native-lane.js')).href}?t=${Date.now()}`
  );

  // ── chat:approval_required：snake_case 负载弹卡 + approvalId 幂等去重 ──
  {
    const lane = createNativeLane();
    const payload = {
      session_id: 's1',
      approval_id: 'ap-1',
      tool_name: 'shell_exec',
      description: 'run rm -rf build/',
      input: { command: 'rm -rf build/' },
      intent_summary: '清理构建产物',
      approval_key: 'k1',
      approval_grouping_key: 'g1',
      force_prompt: true,
    };
    assert.equal(applyNativeChatEvent(lane, 'chat:approval_required', payload), true, '首张审批卡可视变化');
    const cards = lane.items.filter(item => item.type === 'approval');
    assert.equal(cards.length, 1, '同一审批只出一张卡');
    assert.equal(cards[0].approvalId, 'ap-1');
    assert.equal(cards[0].toolName, 'shell_exec');
    assert.equal(cards[0].description, 'run rm -rf build/');
    assert.equal(cards[0].intentSummary, '清理构建产物');
    assert.deepEqual(cards[0].input, { command: 'rm -rf build/' });
    assert.equal(cards[0].forcePrompt, true, 'force_prompt 的 hold 必须透传');
    assert.equal(cards[0].resolved, false);
    assert.equal(cards[0].cardState, 'active');
    // 同 id 重放（engine 重放 / remount 查询重灌）：不得重复出卡，字段不回退。
    assert.equal(applyNativeChatEvent(lane, 'chat:approval_required', payload), false, '同 id 去重');
    assert.equal(lane.items.filter(item => item.type === 'approval').length, 1);
    // 不同 id 是新审批：正常出第二张卡。
    assert.equal(applyNativeChatEvent(lane, 'chat:approval_required', {
      ...payload, approval_id: 'ap-2',
    }), true);
    assert.equal(lane.items.filter(item => item.type === 'approval').length, 2);
  }

  // ── get_pending_approvals 重放通道：camelCase 查询字段 → 合成事件，幂等 ──
  // CodexAcpView.loadSession 用查询返回（serde camelCase）按事件 snake_case
  // 形状重灌；与 live 事件共用同一 approvalId 去重键。
  {
    const lane = createNativeLane();
    const queryEntry = {
      approvalId: 'ap-9',
      toolName: 'write_file',
      description: 'write src/main.rs',
      input: { path: 'src/main.rs' },
      intentSummary: null,
      approvalKey: 'k9',
      approvalGroupingKey: 'g9',
      forcePrompt: false,
      createdAtMs: 1,
    };
    const synthetic = {
      session_id: 's1',
      approval_id: queryEntry.approvalId,
      tool_name: queryEntry.toolName,
      description: queryEntry.description,
      input: queryEntry.input,
      intent_summary: queryEntry.intentSummary,
      approval_key: queryEntry.approvalKey,
      approval_grouping_key: queryEntry.approvalGroupingKey,
      force_prompt: queryEntry.forcePrompt,
    };
    applyNativeChatEvent(lane, 'chat:approval_required', synthetic);
    applyNativeChatEvent(lane, 'chat:approval_required', synthetic); // 重放去重
    const cards = lane.items.filter(item => item.type === 'approval');
    assert.equal(cards.length, 1, '查询重放与 live 事件共享去重键（approvalId）');
    assert.equal(cards[0].intentSummary, '', 'null intent（只读工具无摘要）归一为空串');
    assert.equal(cards[0].forcePrompt, false);
    // 决策收口后 remount 再查询重放：已收口的卡不得复活为 active。
    cards[0].resolved = true;
    cards[0].cardState = 'denied';
    assert.equal(applyNativeChatEvent(lane, 'chat:approval_required', synthetic), false,
      '决策后的重放不得复活已收口的卡');
    assert.equal(lane.items.filter(item => item.type === 'approval').length, 1);
    assert.equal(lane.items.find(item => item.type === 'approval').cardState, 'denied');
  }

  // ── chat:guardrail_denied：硬拒通知（不出卡）+ 稳定 reason 键透传 ──
  {
    const lane = createNativeLane();
    assert.equal(applyNativeChatEvent(lane, 'chat:guardrail_denied', {
      session_id: 's1', tool_name: 'shell_exec', reason: 'plan_mode',
    }), true);
    const notice = lane.items.find(item => item.type === 'system' && item.guardrailDenied);
    assert.ok(notice, '硬拒必须落一条系统提示');
    assert.equal(notice.toolName, 'shell_exec');
    assert.equal(notice.reason, 'plan_mode', 'reason 是稳定键，文案映射交给渲染层');
  }

  // ── chat:mode_changed：lane 无可视变化（native controls 由刷新路径驱动）──
  {
    const lane = createNativeLane();
    assert.equal(applyNativeChatEvent(lane, 'chat:mode_changed', {
      session_id: 's1', mode: 'auto', ask_on_guardrail: true,
    }), false, 'mode_changed 不得 bump 渲染（返回 false）');
    assert.equal(lane.items.length, 0, 'mode_changed 不产生 lane 项');
  }

  // ── 2. 源码契约 ─────────────────────────────────────────────────
  const chatEvents = read('src', 'platform', 'tauri', 'bridge', 'chat-events.js');
  const interaction = read('src', 'platform', 'tauri', 'bridge', 'interaction.js');
  const sessionsBridge = read('src', 'platform', 'tauri', 'bridge', 'sessions.js');
  const bridgeMain = read('src', 'platform', 'tauri', 'bridge.js');
  const codexView = read('src', 'features', 'codex', 'CodexAcpView.jsx');
  const chatView = read('src', 'features', 'chat', 'ChatView.jsx');
  const toolRenderers = read('src', 'features', 'tools', 'tool-renderers.jsx');
  const composerControls = read('src', 'features', 'chat', 'composer-controls.jsx');

  // chat-events：approval_required 监听器按 approvalId 去重后 addChatItem type "approval"。
  assert.match(chatEvents, /listen\("chat:approval_required"/, '聊天车道必须有审批卡监听器');
  assert.match(chatEvents, /function hasChatItemForApproval\(approvalId\) \{[\s\S]{0,300}?item\.type === "approval" && item\.approvalId === approvalId/,
    '去重按 type "approval" + approvalId（hasChatItemForTool 同款形态扫描）');
  assert.match(chatEvents, /if \(hasChatItemForApproval\(approvalId\)\) return;/, '重放/重复事件必须先去重');
  assert.match(chatEvents, /type: "approval",\s*\n\s*approvalId,/, '卡片项 type "approval" 携带 approvalId');
  // chat-events：guardrail_denied 按稳定 reason 键映射三语（BT 表）。
  assert.match(chatEvents, /listen\("chat:guardrail_denied"/, '守卫硬拒通知监听器');
  for (const key of ['session_denied', 'plan_mode', 'guardrail_ask_off']) {
    assert.ok(chatEvents.includes(`reason === "${key}"`), `reason 键 ${key} 必须按键映射文案`);
  }
  // chat-events：mode_changed 走统一收敛点（epoch 保护）。
  assert.match(chatEvents, /listen\("chat:mode_changed"/, '权威 mode 广播监听器');
  assert.match(chatEvents, /applyAuthoritativeModeState\(String\(p\.session_id\), p\)/,
    'mode_changed 必须经 applyAuthoritativeModeState 写回（在途 syncModeState 旧读作废）');

  // bridge.js：权威 modeState 形状扩为 { mode, multiAgent, askOnGuardrail }，缺省按开。
  assert.match(bridgeMain, /askOnGuardrail: st\.ask_on_guardrail == null \? true : !!st\.ask_on_guardrail/,
    'applyAuthoritativeModeState 必须携带 askOnGuardrail（缺省 true，旧负载不翻成关）');

  // interaction.js：tri-mode 桥接 API 与决策的显式 sessionId invoke 形状。
  assert.match(interaction, /async function setSessionMode\(targetSessionId, mode, askOnGuardrail\)/,
    'setSessionMode 支持显式目标会话与可选 askOnGuardrail');
  assert.match(interaction, /invoke\("set_session_mode", args\)/, '统一模式切换走 set_session_mode');
  assert.match(interaction, /async function setAskOnGuardrail\(targetSessionId, enabled\)/, '会话级守卫询问开关');
  assert.match(interaction, /invoke\("set_ask_on_guardrail", \{ sessionId: sid, enabled: !!enabled \}\)/,
    '守卫询问开关 invoke 形状（显式 sessionId）');
  assert.match(interaction, /const previousAsk = \(state\.modeState && state\.modeState\.askOnGuardrail\) !== false;/,
    '乐观翻转前必须记录旧值（失败回滚用，setMultiAgentMode 同款）');
  assert.match(interaction, /async function decideApproval\(approvalId, decision, targetSessionId\)/,
    '决策回传支持显式目标会话');
  assert.match(interaction, /invoke\("decide_approval", \{ sessionId: sid, approvalId, decision \}\)/,
    'decide_approval invoke 必须显式携带 sessionId（await 期间切会话不串台）');
  assert.match(interaction, /async function getPendingApprovals\(targetSessionId\)/, '挂起审批查询');
  assert.match(interaction, /async function planStuckGoAuto\(itemId, targetSessionId\)/,
    'plan-stuck 卡的 Auto 档继续（不经 Yolo 确认门）');

  // sessions.js：切会话重放（快/慢两条路径都要挂）。
  assert.match(sessionsBridge, /async function replayPendingApprovals\(sid\)/, '聊天车道切会话重放助手');
  assert.match(sessionsBridge, /invoke\("get_pending_approvals", \{ sessionId: sid \}\)/, '重放走挂起登记查询');
  assert.match(sessionsBridge, /item\.type === "approval" && item\.approvalId === approvalId/, '重放按 approvalId 去重');
  assert.equal(
    (sessionsBridge.match(/await replayPendingApprovals\(id\);/g) || []).length, 2,
    'switchToSessionInternal 的快路径与慢路径都必须挂重放',
  );

  // CodexAcpView：事件表 + 决策 invoke 形状 + remount 还原。
  for (const event of ['chat:approval_required', 'chat:guardrail_denied', 'chat:mode_changed']) {
    assert.ok(codexView.includes(`'${event}',`), `NATIVE_CHAT_EVENTS 必须包含 ${event}`);
  }
  assert.match(codexView, /invoke\('decide_approval', \{ sessionId: sid, approvalId, decision \}\)/,
    '原生车道决策 invoke 必须显式携带 sessionId');
  assert.match(codexView, /invoke\('get_pending_approvals', \{ sessionId: id \}\)/,
    'loadSession 原生分支必须还原挂起审批卡');
  assert.match(codexView, /applyNativeChatEvent\(lane, 'chat:approval_required', \{[\s\S]{0,400}?approval_id: entry\.approvalId/,
    '还原按「查询 camelCase → 事件 snake_case」重灌（与 live 事件同一去重键）');
  assert.match(codexView, /if \(name === 'chat:mode_changed' && sessionId === activeIdRef\.current\) \{[\s\S]{0,300}?refreshNativeControls\(sessionId\)/,
    'mode_changed 经 refreshNativeControls 权威刷新（请求序号 + 归属保护）');

  // ChatView：auto 分支在门之前直返；YOLO 确认门仍只对 target==='yolo' 生效。
  {
    const handler = chatView.slice(
      chatView.indexOf('async function handleModeChipSwitch'),
      chatView.indexOf('async function handleToggleAskOnGuardrail'),
    );
    assert.ok(handler.includes('async function handleModeChipSwitch'), '切片必须命中 handleModeChipSwitch');
    const autoAt = handler.indexOf("target === 'auto'");
    const gateAt = handler.indexOf("target !== 'yolo' || !isPlan");
    assert.ok(autoAt >= 0, 'handleModeChipSwitch 必须有 auto 分支');
    assert.ok(gateAt > autoAt, 'auto 分支必须先于 Yolo 门直返（auto 永不过门）');
    assert.match(handler, /bridge\.interaction\.setSessionMode\(activeSessionId, 'auto'\)/,
      'auto 分支显式携带发起会话 sid');
    assert.match(chatView, /pendingChatYoloActionRef\.current = \{ kind: 'planStuckGo', itemId \};/,
      'plan-stuck 的 Yolo 继续仍走一次性确认门');
    // Auto 档继续：goAuto 短路在 pendingChatYoloActionRef 之前（不过门）。
    const planStuckHandler = chatView.slice(
      chatView.indexOf('async function handlePlanStuckGo'),
      chatView.indexOf('async function confirmChatYoloSwitch'),
    );
    const goAutoAt = planStuckHandler.indexOf("targetMode === 'auto'");
    const pendingAt = planStuckHandler.indexOf("pendingChatYoloActionRef.current = { kind: 'planStuckGo', itemId };");
    assert.ok(goAutoAt >= 0 && pendingAt > goAutoAt, 'plan-stuck 的 Auto 继续不得记录 Yolo 确认动作');
    // chip 接线：askOnGuardrail + 开关回调。
    assert.match(chatView, /askOnGuardrail=\{bs && bs\.modeState \? bs\.modeState\.askOnGuardrail : undefined\}/,
      '聊天车道 chip 读取 bridge modeState 的 askOnGuardrail');
    assert.match(chatView, /onToggleAskOnGuardrail=\{handleToggleAskOnGuardrail\}/, '开关回调接线');
    assert.match(chatView, /bridge\.interaction\.setAskOnGuardrail\(activeSessionId, enabled\)/,
      '开关走 bridge（乐观翻转 + 失败回滚在 bridge 层）');
  }

  // tool-renderers：ApprovalCard 三按钮 + PlanStuckCard 的 Auto/Yolo 双出路。
  assert.match(toolRenderers, /const ApprovalCard = \(\{ item, t, onDecide \}\) => \{/, '审批卡组件');
  assert.match(toolRenderers, /decide\('allow'\)/, 'Allow 按钮');
  assert.match(toolRenderers, /decide\('allow_for_session'\)/, 'Allow for session 按钮');
  assert.match(toolRenderers, /decide\('deny'\)/, 'Deny 按钮');
  assert.match(toolRenderers, /export \{[^}]*ApprovalCard[^}]*\}/, 'ApprovalCard 必须导出');
  assert.match(toolRenderers, /onGo\(item\.id, 'auto'\)/, 'plan-stuck 卡的 Auto 出路（经 onGo 的 mode 参数）');
  assert.match(toolRenderers, /onGo\(item\.id, 'yolo'\)/, 'plan-stuck 卡的 Yolo 出路');

  // composer chip：三档选项（Yolo→Auto→Plan，从最宽松到最严格）+ 仅非 Plan 档显示守卫开关。
  assert.match(composerControls, /switchTo\('yolo'\)[\s\S]{0,600}?switchTo\('auto'\)[\s\S]{0,600}?switchTo\('plan'\)/,
    '下拉顺序 Yolo → Auto → Plan（从最宽松到最严格）');
  assert.match(composerControls, /const showAskToggle = activeMode !== 'plan';/, '守卫开关只在 Auto/Yolo 档显示');
  assert.match(composerControls, /const askOnGuardrail = \(askOnGuardrailProp === undefined \|\| askOnGuardrailProp === null\)\s*\n\s*\? \(\(ms && ms\.askOnGuardrail\) !== false\)/,
    '展示值缺省按开（旧快照无此键不翻成关）');

  console.log('approval_card_lane: ok');
} finally {
  rmSync(temp, { recursive: true, force: true });
}
