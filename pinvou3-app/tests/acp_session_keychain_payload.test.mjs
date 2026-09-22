// createAcpSession 的桌面载荷契约(review #484 round-8 M2):钥匙串快照与
// tier-① 项目归属必须随 create_codex_acp_session 下发;空根集合显式归 null,
// 仅路径/临时草稿不得伪造附加根。此前该表面只有协议哈希与源码扫描钉住。
import assert from 'node:assert/strict';
import test from 'node:test';

test('createAcpSession:桌面通道把 workspaceRoots/projectId 送进 invoke 载荷', async () => {
  const invocations = [];
  globalThis.__TAURI__ = {
    core: {
      async invoke(command, args) {
        invocations.push([command, args]);
        return { id: 'acp-new' };
      },
    },
  };
  // 不设 PinvouPlatform 全局 → platform.js 走桌面 fallback(isWeb=false)。
  const mod = await import(`../src/features/codex/acpClient.js?test=${Date.now()}`);

  await mod.createAcpSession({
    workspacePath: '/work/project',
    agentId: 'claude',
    workspaceRoots: ['/work/project', '/work/extra'],
    projectId: 'prj-1',
  });
  assert.deepEqual(invocations, [[
    'create_codex_acp_session',
    {
      workspacePath: '/work/project',
      agentId: 'claude',
      workspaceRoots: ['/work/project', '/work/extra'],
      projectId: 'prj-1',
    },
  ]], '§9.9 tier-① 归属与钥匙串快照必须随会话创建下发');

  // 空根集合显式归 null:与后端「缺省 = 单根语义」对齐,不得传空数组伪装多根。
  await mod.createAcpSession({
    workspacePath: '/work/project',
    agentId: 'claude',
    workspaceRoots: [],
    projectId: null,
  });
  assert.deepEqual(invocations[1][1], {
    workspacePath: '/work/project',
    agentId: 'claude',
    workspaceRoots: null,
    projectId: null,
  });

  delete globalThis.__TAURI__;
});
