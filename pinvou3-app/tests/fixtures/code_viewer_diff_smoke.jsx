import React, { useState } from 'react';
import { createRoot } from 'react-dom/client';
import '../../src/styles/base.css';
import { CodeViewerModal } from '../../src/features/codex/CodeViewerModal.jsx';

// diff 模式：只传 WorkspaceDiff，preview 为空；弹窗内部适配为文本预览并强制 diff 高亮。
const diff = {
  relativePath: 'src/main.py',
  text: '# 已暂存\ndiff --git a/src/main.py b/src/main.py\n@@ -1,2 +1,3 @@\n-print(1)\n+print(2)\n',
  truncated: true,
};

const copy = {
  reading: '正在读取…',
  copyContent: '复制内容',
  copied: '已复制',
  copyPath: '复制相对路径',
  reveal: '在文件管理器中显示',
  open: '用系统应用打开',
  closeViewer: '关闭预览',
  truncated: '内容过大，当前只显示前一部分。',
  unsupported: '该文件不支持内置预览。',
  openHint: '可以用系统应用打开。',
  loadFailed: '文件读取失败',
  resizeWidth: '调整弹窗宽度',
  resizeHeight: '调整弹窗高度',
  resizeCorner: '调整弹窗大小，双击恢复默认',
  fontDecrease: '减小字号',
  fontIncrease: '增大字号',
  openInNewWindow: '在新窗口打开',
  noDiff: '没有可显示的文本差异',
};

const Fixture = () => {
  const [open, setOpen] = useState(true);
  return (
    <>
      <button type="button" data-testid="reopen" onClick={() => setOpen(true)}>reopen</button>
      {open && (
        <CodeViewerModal
          name="main.py"
          relativePath="src/main.py"
          diff={diff}
          loading={false}
          error=""
          onClose={() => setOpen(false)}
          onOpen={() => {}}
          onReveal={() => {}}
          onOpenInNewWindow={() => {
            window.__newWindowCalls = (window.__newWindowCalls || 0) + 1;
          }}
          copy={copy}
        />
      )}
    </>
  );
};

createRoot(document.getElementById('root')).render(<Fixture />);
