import { createRoot } from 'react-dom/client';
import '../styles/base.css';
import { ReaderApp } from '../features/reader/ReaderApp.jsx';
import { ensureLanguage, initialSystemLanguage } from '../shared/i18n.js';

// 首帧语言引导:与 app/main.jsx 同口径(zh 内嵌零成本,en/ja 先取词典 chunk 再渲染)
const root = createRoot(document.getElementById('root'));
ensureLanguage(initialSystemLanguage()).catch(() => {}).then(() => {
  root.render(<ReaderApp />);
});
