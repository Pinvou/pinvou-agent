import React from 'react';
import { createRoot } from 'react-dom/client';
import '../../src/styles/base.css';
import { CodexWorkspacePanel } from '../../src/features/codex/CodexWorkspacePanel.jsx';

// 会话模式端到端：更改列表 → diff 弹窗 → 阅读器（kind='diff'）；文件模式 open_code_reader 无 kind。
window.__invokeLog = [];
window.__referenceLog = [];

const CHANGES = {
  git: true,
  branch: 'main',
  baselineAvailable: true,
  changes: [
    { relativePath: 'src/main.py', status: 'modified', staged: false, origin: 'session' },
    { relativePath: 'README.md', status: 'deleted', staged: false, origin: 'session' },
  ],
};

window.__TAURI__ = {
  core: {
    invoke: async (command, args = {}) => {
      window.__invokeLog.push({ command, args });
      switch (command) {
        case 'list_codex_workspace':
          return {
            entries: [
              { name: 'main.py', relativePath: 'src/main.py', kind: 'file' },
            ],
          };
        case 'get_codex_workspace_changes':
          return CHANGES;
        case 'get_codex_workspace_diff':
          return {
            relativePath: args.relativePath,
            text: `diff --git a/${args.relativePath} b/${args.relativePath}\n@@ -1,1 +1,1 @@\n-print(1)\n+print(2)\n`,
            truncated: true,
          };
        case 'preview_codex_workspace_file':
          return {
            name: args.relativePath.split('/').pop(),
            relativePath: args.relativePath,
            kind: 'text',
            size: 9,
            modified: 0,
            text: 'print(1)\n',
            dataUrl: null,
            truncated: false,
          };
        case 'open_codex_workspace_file':
        case 'reveal_codex_workspace_file':
        case 'open_code_reader':
          return null;
        default:
          throw new Error(`unexpected command: ${command}`);
      }
    },
  },
};

const copy = {
  changes: { added: '新增', modified: '修改', deleted: '删除', renamed: '重命名', copied: '复制', conflict: '冲突', untracked: '未跟踪', unknown: '文件' },
  origins: { session: '本会话', preexisting: '会话前已有', preexisting_modified: '会话前已有 · 本会话继续修改', unknown: '来源未记录' },
  addedPath: path => `已添加 ${path}`,
  addPath: path => `添加 ${path} 到对话`,
  added: '已添加到对话',
  add: '添加到对话',
  back: '返回工作区列表',
  copyPath: '复制相对路径',
  reveal: '在文件管理器中显示',
  open: '用系统应用打开',
  reading: '正在读取…',
  noDiff: '没有可显示的文本差异',
  tooLarge: '文件过大，未生成内置预览。',
  unsupported: '该文件不支持内置预览。',
  openHint: '可以用系统应用打开。',
  truncated: '内容过大，当前只显示前一部分。',
  resize: '调整工作区宽度',
  resizeHint: '拖拽调整宽度，双击恢复默认',
  title: '工作区',
  temporary: '临时工作区',
  refresh: '刷新工作区',
  close: '关闭工作区',
  files: '文件',
  changed: '更改',
  search: '搜索文件',
  noFiles: '没有匹配文件',
  noBaseline: '该旧会话没有创建时基线，因此无法判断更改是否由本会话产生。',
  branch: '分支',
  staged: '已暂存',
  noChanges: '工作区没有更改',
  copyContent: '复制内容',
  copied: '已复制',
  closeViewer: '关闭预览',
  loadFailed: '文件读取失败',
  resizeWidth: '调整弹窗宽度',
  resizeHeight: '调整弹窗高度',
  resizeCorner: '调整弹窗大小，双击恢复默认',
  fontDecrease: '减小字号',
  fontIncrease: '增大字号',
  openInNewWindow: '在新窗口打开',
  readerTitle: '代码阅读器',
  readerEmpty: '从工作区文件弹窗选择「在新窗口打开」，文件会在此以标签页累积。',
  closeTab: '关闭标签页',
  noSessionChanges: '创建会话后，这里会列出本会话对项目的更改。',
  showRawErrors: true,
  operationFailed: '工作区操作失败，请重试',
};

createRoot(document.getElementById('root')).render(
  <div className="flex h-screen">
    <div className="flex-1" />
    <CodexWorkspacePanel
      session={{ id: 's-1' }}
      visible
      onClose={() => {}}
      references={[]}
      onAddReference={(path) => window.__referenceLog.push(path)}
      copy={copy}
    />
  </div>,
);
