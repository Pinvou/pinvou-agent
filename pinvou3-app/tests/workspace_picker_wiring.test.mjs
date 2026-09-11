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

  // chat 车道:草稿选择经 bridge setDraftWorkspace 带项目归属与钥匙串;
  // 物化 create_session 透传 workspaceRoots/projectId。
  assert.match(chatView, /onOpenWorkspacePicker\(\{ lane: 'chat', mode:/, 'chat 入口带车道与模式');
  assert.match(bridgeSessions, /function setDraftWorkspace\(path, extras\)/, '草稿携带扩展归属');
  assert.match(bridgeSessions, /invoke\("create_session", \{[\s\S]*?workspaceRoots: payloadRoots,[\s\S]*?projectId: payloadProjectId,/, 'create_session 透传钥匙串与项目');

  // codex 车道:入口开选择器(Web 维持旧通道),请求经 workspacePickerRequest
  // 落地 beginDraft,物化 createAcpSession 透传(仅桌面)。
  assert.match(codexView, /isWeb \|\| !onOpenWorkspacePicker[\s\S]{0,80}onOpenWorkspacePicker\(\{ lane: 'codex', mode:/, 'codex 入口');
  assert.match(codexView, /workspacePickerRequest\.epoch/, '请求按 epoch 消费');
  assert.match(acpClient, /invokeTauri\('create_codex_acp_session', \{[\s\S]*?workspaceRoots:[\s\S]*?projectId:/, 'ACP 创建透传');
  assert.match(acpClient, /web_access_create_codex_acp_session', \{\s*workspaceHandle[\s\S]*?\}\)/, 'Web 通道不带钥匙串(单根授权目录)');

  // 对话框纯展示:不含 invoke/直读 Tauri 全局。
  assert.doesNotMatch(dialog, /__TAURI__|invoke\(/, '选择器组件不碰 Tauri 全局');
  // 分模式告知经共享纯函数(§9.4)。
  assert.match(dialog, /workspaceNoticeTone\(mode\)/, '告知走共享判定');
});

test('workspace picker i18n keys exist in all three languages', () => {
  for (const lang of ['zh', 'en', 'ja']) {
    const source = read('src', 'shared', 'i18n', `${lang}.js`);
    assert.match(source, /uiWorkspacePicker: \{/, `${lang} 缺 uiWorkspacePicker 段`);
    for (const key of ['title', 'temporary', 'browse', 'noticeRestricted', 'noticeVisibility', 'excludedTitle', 'excludedProceed']) {
      assert.ok(source.includes(`${key}:`), `${lang} 缺键 ${key}`);
    }
  }
});
