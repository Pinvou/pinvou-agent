// 第二批(项目行新建会话/钥匙串 chip 对齐/管理文件夹面板)的接线契约
// (源码扫描,与 workspace_picker_wiring 同形态)。
import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const read = (...segments) => fs.readFileSync(path.join(root, ...segments), 'utf8');

test('project-row new-session channel wiring (F4)', () => {
  const main = read('src', 'app', 'main.jsx');
  const header = read('src', 'features', 'projects', 'ProjectGroupHeader.jsx');

  // 组头按钮:仅项目分组,回调注入,组件不碰全局。
  assert.match(header, /kind === 'project' && onNewSession/, '新建会话按钮仅项目组');
  assert.match(header, /data-testid="project-new-session"/, '按钮可定位');
  assert.doesNotMatch(header, /__TAURI__|invoke\(/, '组头组件不碰 Tauri 全局');
  // 宿主:cwd = 项目记忆主根,钥匙串 = 项目全量根,车道跟随当前页。
  assert.match(main, /const handleProjectNewSession[\s\S]*?pickerPrimaryRoot\(project\)[\s\S]*?pickerProjectRoots\(project\)/, '项目通道 cwd/roots 解析');
  // 侧栏接线:桌面守门 + 纯标签项目(无根)不渲染死入口(review #484 M3)。
  assert.match(main, /onNewSession: bridge\.projects && pickerProjectRoots\(group\)\.length > 0 \? \(\) => handleProjectNewSession\(group\.projectId\) : undefined/, '侧栏接线(桌面守门+无根门控)');
  // 授权告知同重量(§9.4):不经选择器的直达通道也要在授权一刻给出分模式告知。
  assert.match(main, /handleProjectNewSession[\s\S]*?setSettingsToast\(workspaceGrantNotice\(activeLaneMode\(\), roots\.length\)\)/, '项目行新对话的授权告知');
  // Round-5 M2:chat 车道有活动会话时,桥层 setDraftWorkspace 是静默 no-op
  // (activeSessionId 非空直接 return false),项目行「+」曾经点击无任何反馈。
  // 修法:applyWorkspaceTarget 的 chat 分支先进草稿(与 handleNewChat 同路
  // 径),再 stage;视图导航到 chat 让新草稿可见,applied=false 维持不 toast。
  assert.match(main, /const applyWorkspaceTarget = async/, 'applyWorkspaceTarget 异步化(进草稿需 await)');
  assert.match(
    main,
    /bridge\.activeSessionId && bridge\.sessions\.createNewSession[\s\S]{0,200}?await bridge\.sessions\.createNewSession\(\);[\s\S]{0,200}?setCurrentView\('chat'\);[\s\S]{0,300}?setDraftWorkspace\(/,
    'chat 车道:有活动会话时先进草稿再 stage,并导航到 chat',
  );
  assert.match(main, /const handleProjectNewSession = async[\s\S]*?await applyWorkspaceTarget/, '项目行回调等待真实 stage 结果再 toast');
  // codex 车道(beginDraft 请求)行为不变:同步落地,不经 createNewSession。
  assert.match(main, /lane === 'codex'[\s\S]{0,200}?setPickerCodexRequest\(\{ epoch/, 'codex 车道仍走 picker 请求');
});

test('keychain chip + align wiring (F5)', () => {
  const chatView = read('src', 'features', 'chat', 'ChatView.jsx');
  const codexView = read('src', 'features', 'codex', 'CodexAcpView.jsx');
  const acpClient = read('src', 'features', 'codex', 'acpClient.js');
  const bridgeProjects = read('src', 'platform', 'tauri', 'bridge', 'projects.js');
  const chip = read('src', 'features', 'projects', 'WorkspaceKeychainChip.jsx');

  // chip 纯展示;对齐走 projects 桥域(chat)/acpClient(codex),同一条后端命令。
  assert.doesNotMatch(chip, /__TAURI__|invoke\(/, 'chip 不碰 Tauri 全局');
  assert.match(bridgeProjects, /invoke\("align_session_to_project", \{ sessionId \}\)/, 'chat 车道桥包装');
  assert.match(acpClient, /invokeTauri\('align_session_to_project', \{ sessionId \}\)/, 'codex 车道包装');
  // 两车道的 busy/无变化文案走类型化标记 + 三语键;applied=false 的意外
  // outcome 与类型化错误都有兜底告知(不静默,review #484 m2)。
  for (const src of [chatView, codexView]) {
    assert.match(src, /ALIGN_BUSY/, 'ALIGN_BUSY 标记处理');
    assert.match(src, /alignNoChange/, 'no_change 文案');
    assert.match(src, /alignFailed/, '兜底失败文案');
  }
  // codex 车道的意外 outcome 不得静默(对齐 chat 车道)。
  assert.match(codexView, /outcome\.reason === 'no_change' && onNotify[\s\S]*?else if \(onNotify\)[\s\S]*?onNotify\(t\.uiKeychain\.alignFailed\)/, 'codex 意外 outcome 兜底告知');
  // chip 根为 inline-flex(块级根会拆断单行头),且 busy 只门控对齐按钮,
  // 查看根列表始终可用(review #484 M2/n2)。
  assert.match(chip, /relative inline-flex min-w-0/, 'chip 根 inline-flex');
  const trigger = chip.match(/data-testid="workspace-keychain-chip"[\s\S]{0,200}?onClick/);
  assert.ok(trigger && !/disabled=\{busy\}/.test(trigger[0]), 'chip 触发按钮不被 busy 锁');
  // codex 车道成功后刷新会话列表;chip 数据来自会话项 workspace_roots。
  assert.match(codexView, /refreshSessions\(\)/, 'codex 对齐后刷新');
  assert.match(chatView, /describeKeychain\(activeItem && activeItem\.workspace_roots\)/, 'chat chip 数据源');
  assert.match(codexView, /describeKeychain\(activeSession\.workspace_roots\)/, 'codex chip 数据源');
  // The chat-lane chip's busy flag is the real in-turn flag (parity with the
  // codex lane), and align outcomes other than applied/no_change are not
  // silent. M4: the host must actually pass onNotify, or every guard in
  // either lane stays a no-op — pin the wiring at both mount points.
  assert.match(chatView, /<WorkspaceKeychainChip[\s\S]*?busy=\{busy\}/, 'chat chip busy uses the real flag');
  assert.match(chatView, /else if \(onNotify\) onNotify\(t\.uiKeychain\.alignFailed\)/, 'chat align fallback exit is not silent');
  const main = read('src', 'app', 'main.jsx');
  assert.match(main, /onNotify: setSettingsToast/, 'host passes onNotify via chatViewBaseProps');
  assert.match(main, /onNotify=\{setSettingsToast\}/, 'host passes onNotify to CodexAcpView');
  // codex 车道每 render 只算一次 describeKeychain(memo)。
  assert.match(codexView, /useMemo\(\s*\(\) => \(activeSession \? describeKeychain/, 'codex chip 派生 memo 化');
  // codex 头部行:chip 分支不带 truncate(overflow:hidden 会裁掉 chip 的
  // 上弹浮层),纯文本分支保留(review #484 M2)。
  assert.match(codexView, /workspace_available !== false \? '' : 'truncate'/, 'codex 头部 chip 分支不带 truncate');
});

test('manage-folders panel wiring (F6)', () => {
  const main = read('src', 'app', 'main.jsx');
  const dialog = read('src', 'features', 'projects', 'ManageProjectFoldersDialog.jsx');
  const header = read('src', 'features', 'projects', 'ProjectGroupHeader.jsx');
  const codexView = read('src', 'features', 'codex', 'CodexAcpView.jsx');
  const bridgeProjects = read('src', 'platform', 'tauri', 'bridge', 'projects.js');

  // 面板纯展示,状态解析在纯函数。
  assert.doesNotMatch(dialog, /__TAURI__|invoke\(/, '面板不碰 Tauri 全局');
  assert.match(dialog, /manageFolderRows\(project\)/, '行解析走纯函数');
  // 组头菜单入口(仅项目组)。
  assert.match(header, /kind === 'project' && onManage/, '管理文件夹菜单项');
  assert.match(main, /onManage: bridge\.projects \? \(\) => setManageFoldersId/, '宿主接线');
  // 移除/主根/排除/重命名四条动作都经桥层命令(移除走 update_project 整组替换,
  // 成员移出在后端;重命名复用侧栏同一 renameProject 命令路径)。
  assert.match(main, /updateProjectRoots\(manageFoldersProject\.id, plan\.roots\)/, '移除经整组替换');
  assert.match(main, /setPrimaryRoot\(manageFoldersProject\.id, root\)/, '设主根');
  assert.match(main, /setNeverMaterialize\(root, never\)/, '排除列表写');
  assert.match(main, /onRename=\{\(name\) => handleRenameProject\(manageFoldersProject\.id, name\)\}/, '重命名同路径');
  // 桥域新包装齐备。
  for (const fn of ['updateProjectRoots', 'setPrimaryRoot', 'setNeverMaterialize']) {
    assert.match(bridgeProjects, new RegExp(`async function ${fn}`), `桥域缺 ${fn}`);
  }
  // 排除列表随 projectsList 快照下发;Web 桩同形(不挂载 projects 域也要
  // 带上 neverMaterializeRoots 键,否则分组读到 undefined 只能靠防御性兜底)。
  assert.match(bridgeProjects, /neverMaterializeRoots: snapshot\.never_materialize_roots \|\| \[\]/, '快照带排除列表');
  const webBridge = read('src', 'platform', 'web', 'bridge.js');
  assert.match(webBridge, /projectsList: \{ projects: \[\], assignments: \{\}, neverMaterializeRoots: \[\], loadedAt: null \}/, 'Web 桩与桌面快照同形');
  // 面板的分模式告知跟随所在车道:code 页打开时用 codex 车道的 mode(由
  // CodexAcpView 上报),而不是恒用 chat 车道的 modeState。
  assert.match(main, /mode=\{activeLaneMode\(\)\}/, '面板告知按车道取 mode');
  assert.match(codexView, /onLaneModeChange/, 'codex 车道上报 mode');
  assert.match(main, /onLaneModeChange=\{setCodexLaneMode\}/, '宿主接 codex mode 上报');
  // Escape 收尾与背板关闭同受 busy 门控(review #484 m3);分模式告知走共享
  // 判定而非就地三元(review #484 n1)。
  assert.match(dialog, /if \(busyRef\.current\) return;[\s\S]{0,80}?onCloseRef\.current\(\)/, 'Escape 受 busy 门控');
  assert.match(dialog, /workspaceNoticeTone\(mode\)/, '面板告知走共享判定');
});

test('composer recents grant notice parity (§9.4)', () => {
  const chatView = read('src', 'features', 'chat', 'ChatView.jsx');
  const codexView = read('src', 'features', 'codex', 'CodexAcpView.jsx');
  const selector = read('src', 'features', 'chat', 'ComposerWorkspaceSelector.jsx');

  // Chat lane: the host computes the mode-aware single-root notice and the
  // selector renders it above the recents list.
  assert.match(chatView, /grantNotice=\{workspaceNoticeTone\(/, 'chat recents notice is mode-aware');
  assert.match(selector, /\{grantNotice && \(/, 'selector renders the notice');
  // Codex lane: the recents section of the draft workspace menu carries the
  // same notice, keyed off the lane mode (native draft staging first, then
  // the reported effective mode).
  assert.match(codexView, /workspaceNoticeTone\(nativeDraftControls\.mode \|\| composerModeValue \|\| null\)/, 'codex recents notice follows the lane mode');
  assert.match(codexView, /noticeRestricted\(1\)[\s\S]{0,120}?noticeVisibility\(1\)/, 'codex recents single-root notice');
});

test('keychain/manage i18n keys exist in all three languages', () => {
  for (const lang of ['zh', 'en', 'ja']) {
    const source = read('src', 'shared', 'i18n', `${lang}.js`);
    assert.match(source, /uiKeychain: \{/, `${lang} 缺 uiKeychain 段`);
    assert.match(source, /uiManageFolders: \{/, `${lang} 缺 uiManageFolders 段`);
    for (const key of ['manageFolders:', 'newSessionHere:', 'alignAction:', 'alignBusy:', 'removeConfirmBody:', 'addDuplicate:', 'revokeExclusion:']) {
      assert.ok(source.includes(key), `${lang} 缺键 ${key}`);
    }
  }
});

test('round-6 pins: picker promise contract, keyboard reveal, availability default', () => {
  const read = (...segments) => fs.readFileSync(path.join(root, ...segments), 'utf8');
  // Chat lane "choose directory": the host's picker opener resolves void, the
  // legacy dialog resolves a path — the selector must tolerate both without
  // throwing on `.then` (review #484 round-6 major).
  const selector = read('src', 'features', 'chat', 'ComposerWorkspaceSelector.jsx');
  assert.match(selector, /Promise\.resolve\(onPickWorkspace\(\)\)/, 'chooseDirectory normalizes the callback to a promise');
  // The project-row "+ new conversation" wrapper reveals on keyboard focus
  // like the menu button (display:none would remove it from the tab order).
  const header = read('src', 'features', 'projects', 'ProjectGroupHeader.jsx');
  const focusReveals = header.split('group-focus-within/header:flex').length - 1;
  assert.ok(focusReveals >= 2, 'project new-session entry reveals on focus-within like the menu button');
  // A roots row without the `available` key reads as available (the inverted
  // default painted every healthy folder unavailable).
  const manage = read('src', 'features', 'projects', 'manageFoldersState.js');
  assert.match(manage, /root\.available !== false/, 'missing availability data defaults to available');
});