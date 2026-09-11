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
  assert.match(main, /onNewSession: bridge\.projects \? \(\) => handleProjectNewSession\(group\.projectId\)/, '侧栏接线(桌面守门)');
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
  // 两车道的 busy/无变化文案走类型化标记 + 三语键。
  for (const src of [chatView, codexView]) {
    assert.match(src, /ALIGN_BUSY/, 'ALIGN_BUSY 标记处理');
    assert.match(src, /alignNoChange/, 'no_change 文案');
  }
  // codex 车道成功后刷新会话列表;chip 数据来自会话项 workspace_roots。
  assert.match(codexView, /refreshSessions\(\)/, 'codex 对齐后刷新');
  assert.match(chatView, /describeKeychain\(activeItem && activeItem\.workspace_roots\)/, 'chat chip 数据源');
  assert.match(codexView, /describeKeychain\(activeSession\.workspace_roots\)/, 'codex chip 数据源');
});

test('manage-folders panel wiring (F6)', () => {
  const main = read('src', 'app', 'main.jsx');
  const dialog = read('src', 'features', 'projects', 'ManageProjectFoldersDialog.jsx');
  const header = read('src', 'features', 'projects', 'ProjectGroupHeader.jsx');
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
  // 排除列表随 projectsList 快照下发。
  assert.match(bridgeProjects, /neverMaterializeRoots: snapshot\.never_materialize_roots \|\| \[\]/, '快照带排除列表');
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
