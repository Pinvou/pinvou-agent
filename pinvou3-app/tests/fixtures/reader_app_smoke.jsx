import React from 'react';
import { createRoot } from 'react-dom/client';
import '../../src/styles/base.css';
import { ReaderApp } from '../../src/features/reader/ReaderApp.jsx';

window.__invokeLog = [];
window.__readerOpenHandler = null;

const PENDING = [
  { sessionId: null, workspacePath: 'D:/proj/demo', relativePath: 'main.py' },
];

window.__TAURI__ = {
  core: {
    invoke: async (command, args = {}) => {
      window.__invokeLog.push({ command, args });
      switch (command) {
        case 'get_settings':
          throw new Error('fixture: 无设置后端，回退默认语言');
        case 'take_code_reader_pending':
          return PENDING;
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
