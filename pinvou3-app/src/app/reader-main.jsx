import { createRoot } from 'react-dom/client';
import '../styles/base.css';
import '../styles/tailwind.css';
import { ReaderApp } from '../features/reader/ReaderApp.jsx';
import { ensureLanguageWithZhFallback, initialSystemLanguage } from '../shared/i18n.js';
import { mountWhenLanguageReady } from '../shared/entry-boot.js';

// 首帧语言引导:与 app/main.jsx 同口径(惰性 chunk 先装载再渲染,失败降级 zh;
// 双失败时由引导门挂词典无关的静态失败/重试态,见 entry-boot.js)。
const root = createRoot(document.querySelector('#root'));
mountWhenLanguageReady({
  load: () => ensureLanguageWithZhFallback(initialSystemLanguage()),
  mount: () => {
    root.render(<ReaderApp />);
  },
});
