#!/usr/bin/env node
// tri-mode 审批面（approval card）双车道回归：
// 1. 原生车道纯逻辑（code-native-lane.js，临时 type:module 目录导入，风格对齐
//    code_native_lane.test.mjs）：chat:approval_required 幂等去重、snake_case /
//    camelCase 双通道负载、guardrail 硬拒通知、mode_changed 无可视变化、
//    get_pending_approvals 重放幂等（含决策后重放不得复活已收口的卡）。
// 2. 源码契约（multiagent_plan_normalize 风格的 regex pin）：bridge chat-events
//    监听器去重 + mode_changed 走统一收敛点、interaction 决策的显式 sessionId
//    invoke 形状、sessions 切会话重放、CodexAcpView 事件表与还原路径、
//    ChatView 的 auto 分支不经 Yolo 确认门（门对一切进入 Yolo 的切换生效，
//    v3.5：Auto→Yolo 直达不再绕道 Plan）。
import assert from 'node:assert/strict';
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import vm from 'node:vm';
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

  // ── round-2 M3：turn 终态清扫把未决审批卡就地落 cancelled（不再可点）──
  {
    const lane = createNativeLane();
    applyNativeChatEvent(lane, 'chat:approval_required', {
      session_id: 's1', approval_id: 'ap-t', tool_name: 'shell_exec',
      description: 'd', input: null, intent_summary: null,
      approval_key: 'k', approval_grouping_key: 'g', force_prompt: false,
    });
    assert.equal(applyNativeChatEvent(lane, 'chat:done', { session_id: 's1', status: 'cancelled' }), true,
      '终态清扫可视变化');
    const card = lane.items.find(item => item.type === 'approval');
    assert.equal(card.resolved, true, '终态卡必须落 resolved');
    assert.equal(card.cardState, 'cancelled', '终态卡落 cancelled（渲染层映射三语取消文案）');
    // 已决卡不被清扫改写：决策终态保留。
    const lane2 = createNativeLane();
    applyNativeChatEvent(lane2, 'chat:approval_required', {
      session_id: 's1', approval_id: 'ap-d', tool_name: 'shell_exec',
      description: 'd', input: null, intent_summary: null,
      approval_key: 'k', approval_grouping_key: 'g', force_prompt: false,
    });
    applyNativeChatEvent(lane2, 'chat:approval_resolved', {
      session_id: 's1', approval_id: 'ap-d', decision: 'deny',
    });
    applyNativeChatEvent(lane2, 'chat:done', { session_id: 's1', status: 'interrupted' });
    const decided = lane2.items.find(item => item.type === 'approval');
    assert.equal(decided.cardState, 'denied', '已决卡的决策终态不被终态清扫覆盖');
  }

  // ── round-2 M5：chat:approval_resolved 收敛广播——未决卡落终态，已决卡不被回声覆盖 ──
  {
    const lane = createNativeLane();
    applyNativeChatEvent(lane, 'chat:approval_required', {
      session_id: 's1', approval_id: 'ap-r', tool_name: 'write_file',
      description: 'd', input: null, intent_summary: null,
      approval_key: 'k', approval_grouping_key: 'g', force_prompt: false,
    });
    assert.equal(applyNativeChatEvent(lane, 'chat:approval_resolved', {
      session_id: 's1', approval_id: 'ap-r', decision: 'allow_for_session',
    }), true, '收敛广播落终态可视变化');
    const card = lane.items.find(item => item.type === 'approval');
    assert.equal(card.resolved, true);
    assert.equal(card.cardState, 'allowed_session', '决策值映射 allowed / allowed_session / denied');
    // 幂等：重复回声、对已决卡不得改写；未知 id 无操作。
    assert.equal(applyNativeChatEvent(lane, 'chat:approval_resolved', {
      session_id: 's1', approval_id: 'ap-r', decision: 'deny',
    }), false, '已决卡不被回声覆盖');
    assert.equal(card.cardState, 'allowed_session');
    assert.equal(applyNativeChatEvent(lane, 'chat:approval_resolved', {
      session_id: 's1', approval_id: 'ap-unknown', decision: 'deny',
    }), false, '未知 id 无可视变化');
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

  // ── 评审 P2：快照 await 窗口的决议竞态——落空 resolved 留墓碑，迟到快照不复活 ──
  // 时序：get_pending_approvals 快照（在途）→ 同一审批从另一表面落定，
  // chat:approval_resolved 先到而此刻 lane 无卡（落空）→ 旧快照回程按
  // approval_required 重放。无墓碑时已决审批会被当活卡插回（可点、可提交，
  // 提交只会得到 approval-not-found）；有墓碑时必须拦截。已知决策值才落
  // 墓碑（fail-neutral 的未知值不落，留待权威回声）。
  {
    const lane = createNativeLane();
    const synthetic = {
      session_id: 's1',
      approval_id: 'ap-race',
      tool_name: 'shell_exec',
      description: 'd', input: null, intent_summary: null,
      approval_key: 'k', approval_grouping_key: 'g', force_prompt: false,
    };
    // 快照在途：lane 还没有这张卡，决议先到 → 落空分支。
    assert.equal(applyNativeChatEvent(lane, 'chat:approval_resolved', {
      session_id: 's1', approval_id: 'ap-race', decision: 'deny',
    }), false, '无卡可落的 resolved 无可视变化');
    // 旧快照回程：同 id 的 approval_required 必须被墓碑拦下。
    assert.equal(applyNativeChatEvent(lane, 'chat:approval_required', synthetic), false,
      '墓碑拦截迟到快照的已决审批');
    assert.equal(lane.items.filter(item => item.type === 'approval').length, 0,
      '已决审批不得以活卡形态出现');
    // 墓碑只拦该 id：后续新审批不受影响。
    assert.equal(applyNativeChatEvent(lane, 'chat:approval_required', {
      ...synthetic, approval_id: 'ap-fresh',
    }), true);
    // 卡在场时的 resolved 正常落终态，不依赖墓碑（既有语义回归护栏）。
    assert.equal(applyNativeChatEvent(lane, 'chat:approval_resolved', {
      session_id: 's1', approval_id: 'ap-fresh', decision: 'allow',
    }), true);
    const card = lane.items.find(item => item.type === 'approval');
    assert.equal(card.cardState, 'allowed');
  }

  // ── round-4 fail-neutral 的 native 车道同口径：未知决策值不得落终态 ──
  // 主车道（chat-events）已有 fail-neutral 钉；native lane 的 resolved
  // 分支曾是同样的三元直落 allowed 形状，这里锁行为：畸形回声 no-op，
  // 未决卡保持 active 等权威回声/终态清扫。
  {
    const lane = createNativeLane();
    applyNativeChatEvent(lane, 'chat:approval_required', {
      session_id: 's1', approval_id: 'ap-mal', tool_name: 'shell_exec',
      description: 'd', input: null, intent_summary: null,
      approval_key: 'k', approval_grouping_key: 'g', force_prompt: false,
    });
    assert.equal(applyNativeChatEvent(lane, 'chat:approval_resolved', {
      session_id: 's1', approval_id: 'ap-mal', decision: 'allow_all',
    }), false, '未知决策值 no-op（无可视变化）');
    const malformed = lane.items.find(item => item.type === 'approval');
    assert.equal(malformed.resolved, false, '畸形回声不得落 resolved');
    assert.equal(malformed.cardState, 'active', '未决卡保持 active');
    // 缺失决策值同口径。
    assert.equal(applyNativeChatEvent(lane, 'chat:approval_resolved', {
      session_id: 's1', approval_id: 'ap-mal',
    }), false, '缺失决策值 no-op');
    assert.equal(lane.items.find(item => item.type === 'approval').cardState, 'active');
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
  // v3.5：物化时车道全局默认三档都应用（旧代码只认 plan，显式 yolo/auto 的
  // work 默认被静默丢弃）；草稿兜底与后端 resolved 默认对齐为 auto。
  assert.match(sessionsBridge, /\["plan", "yolo", "auto"\]\.includes\(laneDefault\)/,
    '未绑定物化应用三档车道默认');
  assert.match(sessionsBridge, /\["plan", "yolo", "auto"\]\.includes\(stagedDraftMode\)/,
    '绑定物化应用三档暂存 mode');

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
  // v3.5：Auto→Yolo 直达（旧代码无此分支，Yolo 点击静默落空）；确认门对
  // 一切进入 Yolo 的切换生效。
  assert.match(codexView, /if \(target === 'yolo'\) \{\s*\n\s*const prefs = await refreshCodePermPrefs\(\);/,
    '原生车道 Yolo 确认门不再绑定 isPlan');
  assert.match(codexView, /target === 'yolo' && isPlan\) \{[\s\S]{0,200}?exit_plan_to_yolo[\s\S]{0,200}?\} else if \(target === 'yolo'\) \{[\s\S]{0,300}?set_session_mode', \{ sessionId: sid, mode: 'yolo' \}/,
    'Plan 退回 exit_plan_to_yolo；Auto→Yolo 走 set_session_mode（sid 显式钉目标，确认门回程切会话不打 live-active）');
  // v3.5：草稿暂存 auto 也随建会话应用（旧代码只认 plan/yolo）。
  assert.match(codexView, /staged\.mode === 'yolo'\) \{[\s\S]{0,160}?\} else if \(staged\.mode === 'auto'\) \{[\s\S]{0,200}?set_session_mode', \{ sessionId, mode: 'auto' \}/,
    '原生车道草稿暂存 auto 落地');

  // ChatView：auto 分支在门之前直返；YOLO 确认门对一切进入 Yolo 的切换生效
  // （v3.5：Auto→Yolo 直达同一门 + set_session_mode，不再绕道 Plan）。
  {
    const handler = chatView.slice(
      chatView.indexOf('async function handleModeChipSwitch'),
      chatView.indexOf('async function handleToggleAskOnGuardrail'),
    );
    assert.ok(handler.includes('async function handleModeChipSwitch'), '切片必须命中 handleModeChipSwitch');
    const autoAt = handler.indexOf("target === 'auto'");
    const gateAt = handler.indexOf("if (target !== 'yolo') return;");
    assert.ok(autoAt >= 0, 'handleModeChipSwitch 必须有 auto 分支');
    assert.ok(gateAt > autoAt, 'auto 分支必须先于 Yolo 门直返（auto 永不过门）');
    assert.match(handler, /bridge\.interaction\.setSessionMode\(activeSessionId, 'auto'\)/,
      'auto 分支显式携带发起会话 sid');
    // v3.5：门后按来路分派——Plan 退回走 exit_plan_to_yolo；Auto→Yolo 走
    // set_session_mode（同显式 sid），消灭旧 `!isPlan` 早退的静默吞没。
    assert.match(handler, /if \(gateSid !== activeSessionIdRef\.current\) return;/, '门后归属复检仍在');
    assert.match(handler, /if \(isPlan\) \{[\s\S]{0,200}?exitPlanToYolo\(gateSid\);[\s\S]{0,200}?\} else if \(typeof bridge\.interaction\.setSessionMode === 'function'\) \{[\s\S]{0,200}?setSessionMode\(gateSid, 'yolo'\);/,
      'Plan 退回 exit_plan_to_yolo；Auto→Yolo 走 set_session_mode，均过同一确认门');
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
    // chip 接线：askOnGuardrail + 开关回调 + 草稿态禁用（无会话可写）。
    assert.match(chatView, /askOnGuardrail=\{bs && bs\.modeState \? bs\.modeState\.askOnGuardrail : undefined\}/,
      '聊天车道 chip 读取 bridge modeState 的 askOnGuardrail');
    assert.match(chatView, /askToggleDisabled=\{!activeSessionId\s*\|\|\s*!\(bridge\.interaction && typeof bridge\.interaction\.setAskOnGuardrail === 'function'\)\}/,
      '草稿态（无会话）与无 setAskOnGuardrail 能力的表面（web）都禁用守卫开关');
    assert.match(chatView, /onToggleAskOnGuardrail=\{handleToggleAskOnGuardrail\}/, '开关回调接线');
    assert.match(chatView, /bridge\.interaction\.setAskOnGuardrail\(activeSessionId, enabled\)/,
      '开关走 bridge（乐观翻转 + 失败回滚在 bridge 层）');
  }

  // chat:done 终态清扫（v3.5）：后端 clear_pending 清登记后，残留 active 卡
  // 就地落 cancelled，不再可点。
  assert.match(chatEvents, /item\.type === "approval" && !item\.resolved[\s\S]{0,120}?item\.cardState = "cancelled";/,
    'turn 终态清扫未决策审批卡');
  assert.match(chatEvents, /item\.type === "approval" && item\.approvalId === approvalId/, 'live 事件按 approvalId 去重');

  // round-2 M5：决策收敛广播 chat:approval_resolved——只对未决卡生效（幂等），
  // 决策值映射三终态；不覆盖发起方的乐观终态与终态清扫的 cancelled。
  // round-4：未知/缺失决策值先按 no-op 早退（fail-neutral）——审批面上的
  // 畸形载荷不得把未决卡映射成 "allowed"。
  assert.match(chatEvents, /listen\("chat:approval_resolved", function \(e\) \{ onSessionEvent\(e, function \(\) \{[\s\S]{0,520}?!\["deny", "allow", "allow_for_session"\]\.includes\(decision\)[\s\S]{0,200}?item\.type === "approval" && item\.approvalId === approvalId && !item\.resolved[\s\S]{0,300}?decision === "deny" \? "denied"[\s\S]{0,120}?"allowed_session"/,
    '收敛广播处理器拒识别未知决策、只落未决卡并映射三终态');
  // round-2 M5：decideApproval 的失败恢复不得覆盖终态清扫已落的 cancelled
  // （await 期间 chat:done 先到 → catch 恢复让位 resolved）。
  assert.match(interaction, /card\.submitting = false;[\s\S]{0,300}?if \(!card\.resolved\) \{[\s\S]{0,200}?card\.cardState = previous\.cardState;/,
    'decide 失败恢复让位终态（不复活已取消卡）');

  // round-2 M1：聊天车道草稿态芯片切换必须落到 setDraftMode（绝不静默吞没），
  // 且草稿 Yolo 过同一一次性确认门（codex 车道 needsYoloConfirmation 对草稿
  // 生效，两车道同约）。
  {
    const handler = chatView.slice(
      chatView.indexOf('async function handleModeChipSwitch'),
      chatView.indexOf('async function handleToggleAskOnGuardrail'),
    );
    const draftAt = handler.indexOf('if (!activeSessionId) {');
    const setSessionAt = handler.indexOf("bridge.interaction.setSessionMode(activeSessionId, 'auto')");
    assert.ok(draftAt >= 0, 'handleModeChipSwitch 必须有草稿分支');
    assert.ok(setSessionAt > draftAt, '草稿分支先于已物化会话的 setSessionMode 路径');
    assert.match(handler, /await bridge\.interaction\.setDraftMode\(target\);/,
      '草稿态 auto/yolo 切换写 lane 全局默认（setDraftMode），不吞没');
    assert.match(handler, /chatYoloGateAllows\(null, 'yolo'\)/,
      '未绑定草稿点 Yolo 过一次性确认门');
    assert.match(chatView, /let boundTarget = chatYoloGateApplies\(\{[\s\S]{0,300}?if \(!boundTarget && !gateSid && gateTarget === 'yolo'\) boundTarget = true;/,
      '草稿 Yolo 的门判定扩展（lane 默认作用于未来所有会话）');
  }

  // round-2 M4：codex Yolo 确认门在 entry 捕获发起会话，确认回程切会话也
  // 落在弹门会话上（live-active 从未见过门卡）。
  assert.match(codexView, /setPendingYoloSwitch\(\{ draft: !activeId, sid: activeId \|\| null \}\)/,
    '确认门捕获发起会话 sid');
  assert.match(codexView, /performNativeModeSwitch\('yolo', \{ isPlan: true \}, pending\.sid\)/,
    '确认落点钉在弹门会话');
  assert.match(codexView, /const sid = \(typeof targetSessionId === 'string' && targetSessionId\)\s*\n\s*\? targetSessionId\s*\n\s*: activeId;/,
    'performNativeModeSwitch 支持显式目标会话');
  assert.match(codexView, /const isActiveTarget = sid === activeId;/, '非当前会话目标不刷本会话控件');

  // round-2 M3：codex 车道 turn 终态也清扫审批卡（与主车道同约，此前只有
  // 主车道有——僵尸可点卡）。
  assert.match(read('src', 'features', 'codex', 'code-native-lane.js'), /item\.type === 'approval' && !item\.resolved[\s\S]{0,80}?item\.cardState = 'cancelled';/,
    'codex 车道终态清扫未决审批卡');


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

  // ── 3. 主车道运行时（chat-events.js，session_nav_race 风格的 VM harness）──
  // round-4：源码契约之外补可执行回归——畸形收敛回声 fail-neutral、终态清扫
  // 保留显式决策、回声对已决卡幂等（正则 pin 锁形状，这里锁行为）。
  {
    const vmRoot = {};
    vm.runInNewContext(read('src', 'shared', 'bridge-shared-helpers.js') + '\n' + read('src', 'platform', 'tauri', 'bridge', 'chat-events.js'), {
      window: vmRoot, globalThis: vmRoot, setTimeout, clearTimeout,
    }, { filename: 'chat-events-harness.js' });
    const factory = vmRoot.__PINVOU_TAURI_BRIDGE_FEATURES__['chat-events'];
    assert.equal(typeof factory, 'function', 'chat-events feature 工厂已注册');
    const listeners = {};
    const state = {
      settings: { language: 'zh' },
      chatItems: [],
      messages: [],
      modeState: { mode: 'auto', multiAgent: false, askOnGuardrail: true },
      sessions: [{ id: 's1' }],
      activeSessionId: 's1',
      artifacts: [],
      turnDirtyArtifacts: [],
      turnPresentedArtifacts: [],
      tokens: { max: 0 },
    };
    factory({
      state,
      listen: (event, handler) => { listeners[event] = handler; },
      notify: () => {},
      onSessionEvent: (e, fn) => fn(),
      runSyncOnSession: (sid, fn) => fn(),
      addChatItem: item => { item.id = 'item-' + (state.chatItems.length + 1); state.chatItems.push(item); },
      addSystemItem: text => { state.chatItems.push({ type: 'system', text, id: 'sys-' + (state.chatItems.length + 1) }); },
      addAuthoritySyncNotice: () => {},
      bt: key => key,
      timeStr: () => '',
      applyAuthoritativeModeState: () => {},
      recordAuthoritySyncDiagnostic: () => {},
      authoritySyncBufferSnapshot: () => ({}),
      isNotifySuppressed: () => false,
      invoke: () => Promise.resolve({}),
      turnUsageDirty: {},
      sessionStates: {},
      renderMarkdown: s => s,
      autoCreateScheduledTaskDraft: () => {},
      toolCallAlreadyStarted: () => false,
      toolCallAlreadyFinished: () => false,
      hasChatItemForTool: () => false,
      flushPendingTextBlock: () => {},
      flushAssistantMessageToHistory: () => {},
      startReasoningBlock: () => {},
      appendReasoningBlock: () => {},
      finalizeStreamingReasoning: () => {},
      reasoningEventIndex: () => 0,
      startThinking: () => {},
      thinkingTool: () => {},
      thinkingIdle: () => {},
      stopThinking: () => {},
      mergeFloatChunks: () => {},
      downsampleFromBase64: () => {},
      patchItemById: () => {},
      itemIdSeq: { value: 0 },
      pendingStreamNotifications: { get value() { return null; } },
      pendingAssistantText: { get value() { return ''; }, set value(v) {} },
      pendingAssistantBlocks: { get value() { return []; }, set value(v) {} },
      currentStreamText: { get value() { return ''; }, set value(v) {} },
      currentStreamId: { get value() { return 0; }, set value(v) {} },
      toolMeta: {},
      finalizeAssistantStreamBeforeReasoning: () => {},
      flushQueued: () => {},
      preserveInterruptedAssistantPresentation: () => {},
      cancelPendingStreamNotify: () => {},
      recordTurnStarted: () => {},
      markRemoteTurn: () => {},
      isScheduledRunSession: () => false,
      memoryWriteLabel: () => '',
      memoryWriteStatusLabel: () => '',
      normalizeMemoryCandidateText: s => s,
      handleMemoryWrite: () => {},
      scheduledTaskAutoCreateSeq: { get value() { return 0; } },
      shellToolResultDisplay: null,
      toolResultText: s => s,
      stripInternalToolRuntimeSuffix: s => s,
      toolResultDisplayContent: s => s,
      isShellExecutionTool: () => false,
      formatShellSnapshot: () => '',
      shellCommandForItem: () => '',
      shellSnapshotKey: () => '',
      basename: p => String(p),
      scheduleShellPoll: () => {},
      presentArtifactAbsPath: () => null,
      parseToolResultPayload: () => ({}),
      numOr0: v => v || 0,
      isPresentArtifactTool: () => false,
      artifactPathFromToolOutput: () => null,
      shouldUseToolOutputAsArtifact: () => false,
      noteArtifactChange: () => {},
      reconcileArtifacts: () => {},
      pushArtifactPath: () => {},
      extractArtifactPath: () => null,
      extractArtifactPaths: () => [],
      isDeliverable: () => false,
      isTmpPath: () => false,
      normalizedPath: p => p,
      isAbsPath: () => false,
      escapeHtml: s => s,
      mentionsShellTool: () => false,
      persistMessagesFor: () => {},
      hasUnresolvedItem: () => false,
      updateToolItem: () => {},
      saveWorkingSetTo: () => {},
      loadWorkingSetFrom: () => {},
      userMessageDisplayText: s => s,
      isInternalUserMessageProvenance: () => false,
      userMessageInputProvenance: () => '',
      makeQueuedMessage: () => ({}),
      stripMentionBlockForComposerRestore: s => s,
      rebuiltQueuedPayload: () => ({}),
      rebuiltQueuedMetaPayload: () => ({}),
      getComposerDraft: () => '',
      setComposerDraft: () => {},
      prefillComposer: () => {},
      queuedPayloadEnvelope: () => ({}),
      submitUserInput: () => {},
      planCardHydrationKey: () => '',
      parsePlanSnapshot: () => ({ plan: null, todos: null }),
      parseUserAnswers: () => null,
      parseCarefulBlocked: () => null,
      compactPruneRollupText: () => '',
      removeCompactionStartItem: () => {},
      addOrMergePruneCompaction: () => {},
      hasChatItemForApproval: undefined,
      touchSessionBuffer: () => {},
      getBuffer: () => ({ chatItems: state.chatItems, messages: [] }),
      flushPendingStreamRender: () => {},
      safeConsoleInfo: () => {},
      sweepUnpairedToolMeta: () => {},
      findPresentedArtifact: () => null,
      startMonitorPolling: () => {},
      reportSessionSwitchFailure: () => {},
      reconcileRemoteTurn: () => {},
      refreshHistoryList: () => Promise.resolve(),
    });

    const seedCard = (id, decision) => {
      state.chatItems.push({
        type: 'approval', approvalId: id, toolName: 'shell_exec', description: 'd',
        input: null, intentSummary: '', forcePrompt: false,
        resolved: !!decision, cardState: decision || 'active', submitting: false, time: '',
      });
    };

    // round-4 fail-neutral：未知/缺失 decision 的收敛回声不得把未决卡映射成
    // "allowed"——卡保持未决等权威回声/终态清扫。
    seedCard('ap-unk', null);
    const before = JSON.stringify(state.chatItems);
    listeners['chat:approval_resolved']({ payload: { session_id: 's1', approval_id: 'ap-unk', decision: 'SOME_NEW_DECISION' } });
    listeners['chat:approval_resolved']({ payload: { session_id: 's1', approval_id: 'ap-unk' } });
    assert.equal(JSON.stringify(state.chatItems), before, '未知/缺失决策 = no-op，卡不得被落成 allowed');

    // 幂等：deny 落未决卡；对已决卡（发起方乐观终态/清扫 cancelled）是 no-op。
    seedCard('ap-live', null);
    listeners['chat:approval_resolved']({ payload: { session_id: 's1', approval_id: 'ap-live', decision: 'deny' } });
    assert.equal(state.chatItems.at(-1).cardState, 'denied', 'deny 回声落终态');
    assert.equal(state.chatItems.at(-1).resolved, true);
    listeners['chat:approval_resolved']({ payload: { session_id: 's1', approval_id: 'ap-live', decision: 'allow' } });
    assert.equal(state.chatItems.at(-1).cardState, 'denied', '回声不覆盖已决卡');

    // 终态清扫：未决卡 → cancelled；显式决策保留。
    seedCard('ap-sweep', null);
    seedCard('ap-denied', 'denied');
    listeners['chat:done']({ payload: { session_id: 's1', generation: 1, status: 'Completed' } });
    const byId = Object.fromEntries(state.chatItems.map(item => [item.approvalId, item]));
    assert.equal(byId['ap-sweep'].cardState, 'cancelled', '未决卡就地落 cancelled');
    assert.equal(byId['ap-denied'].cardState, 'denied', '显式决策不被清扫覆盖');

    console.log('approval_card_lane: plain-lane runtime ok');
  }

  // ── 3. Web 车道 + 重放栅栏（评审 P2/P3 修复钉）────────────────────
  {
    const web = read('src', 'platform', 'web', 'bridge.js');
    // 去重键 = 会话 id + approval id：不同会话合法复用同一 approval id，
    // 全局 id-only Set 会吞掉第二个会话的等待说明；有界 Map（最旧逐出），
    // 因为 web 不监听 chat:approval_resolved，没有可清除的收敛事件。
    assert.match(web, /const dedupKey = String\(p\.session_id \|\| ""\) \+ "\\u0000" \+ approvalId;/,
      'web 审批通告去重必须按 会话+approval id 复合键（全局 id-only 键跨会话吞通告）');
    assert.match(web, /webApprovalNoticeIds = new Map\(\)/,
      'web 审批通告去重必须是有界 Map 而非无界 Set');
    assert.match(web, /webApprovalNoticeIds\.size > WEB_APPROVAL_NOTICE_CAP/,
      'web 审批通告去重必须有界（无 resolve 收敛事件可清除）');
    // 受理前姿态门：Auto 档在受理前显式拒绝该不受支持的执行路径——
    // 事后通告不是完成态；姿态命令失败不拦发送（fail-open，通告兜底）。
    assert.match(web, /invoke\("get_mode_state", \{ sessionId: sid \}\)/,
      'web 发送受理前必须解析会话姿态');
    assert.match(web, /modeState && modeState\.mode === "auto"[\s\S]{0,200}webAutoSendBlocked/,
      'Auto 姿态必须在受理前显式拒绝（webAutoSendBlocked 三语文案）');
    const blockedHits = (web.match(/webAutoSendBlocked:/g) || []).length;
    assert.ok(blockedHits >= 3, `webAutoSendBlocked 文案必须三语齐备（实得 ${blockedHits} 处）`);
    assert.match(web, /function doSendForAdmitted\(/,
      '发送主体必须拆出为 doSendForAdmitted（姿态门先行，不重构调用方）');

    const sessionsBridge = read('src', 'platform', 'tauri', 'bridge', 'sessions.js');
    assert.match(sessionsBridge, /const terminalFence = approvalTerminalSeq\(sid\)/,
      '审批重放必须在快照发出前捕获终态代栅栏');
    assert.match(sessionsBridge, /approvalTerminalSeq\(sid\) !== terminalFence\) return;/,
      '终态代已推进时整批快照必须作废（终态后回程不得插回已消亡的审批）');
    const chatEvents = read('src', 'platform', 'tauri', 'bridge', 'chat-events.js');
    assert.match(chatEvents, /approvalReplayTerminalSeq/,
      'chat:done 终态清扫必须推进审批重放代栅栏（sessions.js 消费）');
  }

  console.log('approval_card_lane: ok');
} finally {
  rmSync(temp, { recursive: true, force: true });
}
