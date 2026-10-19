import { createRoot } from 'react-dom/client';
import PetWindow from '../features/pet/PetWindow.jsx';
import { ensureLanguageWithZhFallback, initialSystemLanguage } from '../shared/i18n.js';

const query = new URLSearchParams(window.location.search);

// Pet-window-level configuration.
const PET_WINDOW_CONFIG = Object.freeze({
  scale: 0.5,
  verticalAlignment: query.get('verticalAlignment') === 'top' ? 'top' : 'bottom',
});

document.documentElement.classList.add('pet-window');
// 首帧语言引导:与 app/main.jsx 同口径(惰性 chunk 先装载再渲染,失败降级 zh)
const root = createRoot(document.querySelector('#root'));
ensureLanguageWithZhFallback(initialSystemLanguage()).then(() => {
  root.render(
    <PetWindow
      configuredScale={PET_WINDOW_CONFIG.scale}
      configuredVerticalAlignment={PET_WINDOW_CONFIG.verticalAlignment}
    />,
  );
});
