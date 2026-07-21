import './features/pet/pet-menu.css';

const core = window.__TAURI__?.core;
const hideMenu = () => {
  document.documentElement.classList.add('pet-menu-hidden');
  core?.invoke('hide_pet_context_menu').catch(() => {});
};

window.addEventListener('blur', hideMenu);
window.addEventListener('keydown', (event) => {
  if (event.key === 'Escape') hideMenu();
});
// 窗口可能被 GTK 钳在 200x200(见 pet-menu.css):按钮外的透明区域仍属于
// 本窗口、会拦截点击,点到就当作点空白收起菜单,与 blur 收起的语义一致。
window.addEventListener('pointerdown', (event) => {
  if (event.target === document.body || event.target === document.documentElement) hideMenu();
});

document.querySelector('#hide-pet')?.addEventListener('click', async () => {
  try {
    await core?.invoke('set_pet_enabled', { enabled: false });
  } finally {
    hideMenu();
  }
});
