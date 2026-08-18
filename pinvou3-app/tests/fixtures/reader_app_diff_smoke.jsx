import React from 'react';
import { createRoot } from 'react-dom/client';
import '../../src/styles/base.css';
import { ReaderApp } from '../../src/features/reader/ReaderApp.jsx';

// 阅读器 diff tab：pending 队列携带 kind='diff'，后端返回 WorkspaceDiff。
window.__invokeLog = [];
window.__readerOpenHandler = null;

const PENDING = [
  { kind: 'diff', sessionId: 's-1', workspacePath: null, relativePath: 'src/main.py' },
];

window.__TAURI__ = {
  core: {
    invoke: async (command, args = {}) => {
      window.__invokeLog.push({ command, args });
      switch (command) {
        case 'get_settings':
          // 固定中文，断言不随运行环境系统语言漂移（与 ui_smoke.js 惯例一致）。
          return { theme: 'liquid-light', language: 'zh-Hans' };
        case 'take_code_reader_pending':
          return PENDING;
        case 'get_codex_workspace_diff':
          return {
            relativePath: args.relativePath,
            text: `diff --git a/${args.relativePath} b/${args.relativePath}\n@@ -1,1 +1,1 @@\n-print(1)\n+print(2)\n`,
            truncated: false,
          };
        case 'preview_codex_workspace_file':
          return {
            name: args.relativePath.split('/').pop(),
            relativePath: args.relativePath,
            kind: 'text',
            size: 9,
            modified: 0,
            text: `print('${args.relativePath}')\n`,
            dataUrl: null,
            truncated: false,
          };
        case 'open_codex_workspace_file':
        case 'reveal_codex_workspace_file':
          return null;
        default:
          throw new Error(`unexpected command: ${command}`);
      }
    },
  },
  event: {
    listen: async (eventName, handler) => {
      if (eventName === 'code-reader:open') window.__readerOpenHandler = handler;
      return () => {};
    },
  },
};

createRoot(document.getElementById('root')).render(<ReaderApp />);
