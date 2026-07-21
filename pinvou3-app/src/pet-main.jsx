import React from 'react';
import { createRoot } from 'react-dom/client';
import PetWindow from './features/pet/PetWindow.jsx';

const query = new URLSearchParams(window.location.search);

// 桌宠窗口级配置：设为 false 时不渲染右下角缩放手柄。
const PET_WINDOW_CONFIG = Object.freeze({
  allowResize: false,
  scale: 0.5,
  verticalAlignment: query.get('verticalAlignment') === 'top' ? 'top' : 'bottom',
});

document.documentElement.classList.add('pet-window');
createRoot(document.getElementById('root')).render(
  <PetWindow
    allowResize={PET_WINDOW_CONFIG.allowResize}
    configuredScale={PET_WINDOW_CONFIG.scale}
    configuredVerticalAlignment={PET_WINDOW_CONFIG.verticalAlignment}
  />,
);
