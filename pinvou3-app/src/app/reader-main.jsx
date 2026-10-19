import { createRoot } from 'react-dom/client';
import '../styles/base.css';
import '../styles/tailwind.css';
import { ReaderApp } from '../features/reader/ReaderApp.jsx';
import { ensureLanguageWithZhFallback, initialSystemLanguage } from '../shared/i18n.js';

// 首帧语言引导:与 app/main.jsx 同口径(惰性 chunk 先装载再渲染,失败降级 zh)
const root = createRoot(document.querySelector('#root'));
ensureLanguageWithZhFallback(initialSystemLanguage()).then(() => {
  root.render(<ReaderApp />);
});
