import React, { useState } from 'react';
import { createRoot } from 'react-dom/client';
import '../../src/styles/base.css';
import { CodeViewerModal } from '../../src/features/codex/CodeViewerModal.jsx';

const preview = {
  name: 'example.js',
  relativePath: 'src/example.js',
  kind: 'text',
  size: 42,
  modified: 0,
  text: 'const answer = 42;\nconsole.log(answer);\n',
  dataUrl: null,
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
};

const Fixture = () => {
  const [open, setOpen] = useState(true);
  return (
    <>
      <button type="button" data-testid="reopen" onClick={() => setOpen(true)}>reopen</button>
      {open && (
        <CodeViewerModal
          name={preview.name}
          relativePath={preview.relativePath}
          preview={preview}
          loading={false}
          error=""
          onClose={() => setOpen(false)}
          onOpen={() => {}}
          onReveal={() => {}}
          copy={copy}
        />
      )}
    </>
  );
};

createRoot(document.getElementById('root')).render(<Fixture />);
