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

document.querySelector('#hide-pet')?.addEventListener('click', async () => {
  try {
    await core?.invoke('set_pet_enabled', { enabled: false });
  } finally {
    hideMenu();
  }
});
