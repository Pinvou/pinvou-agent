// 「选择工作区」单入口的接线契约(源码扫描,与 multiagent_plan_normalize 同
// 形态):四条路径(打开/选中项目/浏览/临时会话)在容器层的接线不得漂移。
import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const read = (...segments) => fs.readFileSync(path.join(root, ...segments), 'utf8');

test('workspace picker wiring contract', () => {
  const main = read('src', 'app', 'main.jsx');
  const chatView = read('src', 'features', 'chat', 'ChatView.jsx');
  const codexView = read('src', 'features', 'codex', 'CodexAcpView.jsx');
  const acpClient = read('src', 'features', 'codex', 'acpClient.js');
  const bridgeSessions = read('src', 'platform', 'tauri', 'bridge', 'sessions.js');
  const dialog = read('src', 'features', 'projects', 'WorkspacePickerDialog.jsx');

  // 选择器由宿主(main.jsx)挂载,行数据经 computePickerRows(热视图)。
  assert.match(main, /<WorkspacePickerDialog[\s\S]*?rows=\{workspacePickerRows\}/, '选择器挂载在宿主');
  assert.match(main, /computePickerRows\(\{[\s\S]*?assignments/, '热视图由 computePickerRows 计算');
  // 四条路径回调齐备。
  for (const prop of ['onSelectProject={handlePickerSelectProject}', 'onTemporary={handlePickerTemporary}', 'onBrowse={handlePickerBrowse}', 'onBrowseExcluded']) {
    assert.ok(main.includes(prop), `选择器回调 ${prop}`);
  }
  // 浏览通道:ensure 物化/锚定复用先行,排除列表(无 outcome)走如实告知分支。
  assert.match(main, /ensureFolderProjects\(\[folder\]\)/, '浏览通道先 ensure');
  assert.match(main, /setPickerExcluded\(folder\)/, '排除列表分支');
  // 浏览通道在 await 系统目录选择器前同步捕获车道,防止选择器中途关闭错投
  // chat 车道(review #484 m1);setDraftWorkspace 缺失时不得谎报 applied
  // (review #484 n5)。
  assert.match(main, /const lane = pickerLane\(\);[\s\S]*?applyWorkspaceTarget\(\{ lane, path: folder/, '浏览车道同步捕获');
  assert.match(main, /applyWorkspaceTarget[\s\S]{0,300}?let applied = false;/, 'applied 缺省 false');

  // chat 车道:草稿选择经 bridge setDraftWorkspace 带项目归属与钥匙串;
  // 物化 create_session 透传 workspaceRoots/projectId。
  assert.match(chatView, /onOpenWorkspacePicker\(\{ lane: 'chat', mode:/, 'chat 入口带车道与模式');
  assert.match(bridgeSessions, /function setDraftWorkspace\(path, extras\)/, '草稿携带扩展归属');
  assert.match(bridgeSessions, /invoke\("create_session", \{[\s\S]*?workspaceRoots: payloadRoots,[\s\S]*?projectId: payloadProjectId,/, 'create_session 透传钥匙串与项目');

  // codex 车道:入口开选择器(Web 维持旧通道),请求经 workspacePickerRequest
  // 落地 beginDraft,物化 createAcpSession 透传(仅桌面)。
  assert.match(codexView, /isWeb \|\| !onOpenWorkspacePicker[\s\S]{0,80}onOpenWorkspacePicker\(\{ lane: 'codex', mode:/, 'codex 入口');
  assert.match(codexView, /consumePickerRequest\(workspacePickerRequest,/, '请求按 epoch 消费');
  assert.match(acpClient, /invokeTauri\('create_codex_acp_session', \{[\s\S]*?workspaceRoots:[\s\S]*?projectId:/, 'ACP 创建透传');
  assert.match(acpClient, /web_access_create_codex_acp_session', \{\s*workspaceHandle[\s\S]*?\}\)/, 'Web 通道不带钥匙串(单根授权目录)');

  // 对话框纯展示:不含 invoke/直读 Tauri 全局。
  assert.doesNotMatch(dialog, /__TAURI__|invoke\(/, '选择器组件不碰 Tauri 全局');
  // 分模式告知经共享纯函数(§9.4)。
  assert.match(dialog, /workspaceNoticeTone\(mode\)/, '告知走共享判定');
  // 每个授权入口同等告知:单根行与浏览入口也带 noticeRestricted(1)。
  assert.ok((dialog.match(/noticeRestricted\(1\)/g) || []).length >= 2, '单根行与浏览入口都要带单数授权告知');
  // 残留状态不跨打开泄漏:宿主条件挂载(开关状态随卸载复位)。
  assert.match(main, /\{workspacePicker && \(/, '选择器条件挂载');
  // 区分"无项目"与"无匹配";排除列表面板 Escape 先退回列表。
  assert.ok(dialog.includes('copy.noMatch'), '搜索无匹配提示');
  assert.match(dialog, /excludedFolderRef[\s\S]*?onDismissExcludedRef/, '排除面板 Escape 只关面板');
  // 排除面板打开时搜索框不渲染(查询对面板无效,review #484 m4)。
  assert.match(dialog, /\{!excludedFolder && !webOnly &&/, '排除面板打开时隐藏搜索框');
});

test('picker/manage dialogs: every close path is busy-gated (round-5 M3)', () => {
  const picker = read('src', 'features', 'projects', 'WorkspacePickerDialog.jsx');
  const manage = read('src', 'features', 'projects', 'ManageProjectFoldersDialog.jsx');

  // Picker:Escape / 背板 / X 三条关闭路径统一挂 busy 门控(busyRef 模式,
  // 与管理文件夹面板一致)——中途关闭会丢掉 in-flight ensure/物化的重试上下文。
  assert.match(picker, /busyRef\.current = busy;/, 'picker 同步 busyRef');
  assert.match(picker, /if \(busyRef\.current\) return;[\s\S]{0,80}?onCloseRef\.current\(\)/, 'picker Escape 受 busy 门控');
  assert.match(picker, /backdropPressRef\.current = e\.target === e\.currentTarget && !busyRef\.current/, 'picker 背板 busy 时不武装关闭');
  assert.match(picker, /title=\{t\.cpCancel\}[\s\S]{0,120}?disabled=\{busy\}[\s\S]{0,120}?onClick=\{onClose\}/, 'picker X 按钮 busy 禁用');
  // Manage panel:背板与 Escape 已门控(m3),补齐 X 按钮同一口径。
  assert.match(manage, /title=\{t\.cpCancel\}[\s\S]{0,120}?disabled=\{busy\}[\s\S]{0,120}?onClick=\{onClose\}/, 'manage X 按钮 busy 禁用');
});

test('picker liveness reads the full session list, not the sidebar-filtered one (round-5 M5)', () => {
  const main = read('src', 'app', 'main.jsx');
  // sidebarCodeTasks 在侧栏非项目形态(sidebarCodeListActive=false)时恒为 [],
  // 以其计算 picker 活跃度会把纯 chat 车道使用的 folder 项目 30 天冷隐藏。
  // boundWorkspaceItems 必须改用未按侧栏样式过滤的全量会话列表。
  assert.match(main, /const boundWorkspaceItems = useMemo\(\(\) => \[[\s\S]{0,400}?allSidebarTasks/, 'picker 活跃度用全量会话列表');
  assert.doesNotMatch(main, /const boundWorkspaceItems = useMemo[\s\S]{0,300}?sidebarCodeTasks/, '不得再用侧栏样式过滤的列表');
});

test('workspace picker i18n keys exist in all three languages', () => {
  for (const lang of ['zh', 'en', 'ja']) {
    const source = read('src', 'shared', 'i18n', `${lang}.js`);
    assert.match(source, /uiWorkspacePicker: \{/, `${lang} 缺 uiWorkspacePicker 段`);
    for (const key of ['title', 'temporary', 'browse', 'noticeRestricted', 'noticeVisibility', 'excludedTitle', 'excludedProceed', 'noMatch']) {
      assert.ok(source.includes(`${key}:`), `${lang} 缺键 ${key}`);
    }
  }
});
