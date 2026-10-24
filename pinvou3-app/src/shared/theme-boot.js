// Pre-paint theme boot for auxiliary window entries (reader.html). Loaded
// synchronously as a classic script in <head>, before any stylesheet or
// application code, so the first frame already carries the right theme class
// and canvas colour. index.html keeps its own inline copy of this recipe on the
// startup critical path; keep the two in sync.
//
// Rules mirror src/shared/color-scheme.js: a stored 'light'/'dark' wins
// ('pinvou.web.theme' is the authoritative store on web and a boot-time cache
// of the resolved scheme on desktop), anything else follows the OS preference.
// The window's React root reconciles the class once real settings arrive; it
// must seed its state from the same stored value or it will strip the class
// again on mount (see ReaderApp.jsx).
//
// The canvas colour comes from the script tag's data-dark-bg attribute so each
// window can match its own dark root background and avoid a shade pop at mount.
(function () {
  'use strict';

  const script = document.currentScript;
  const darkBackground = (script && script.dataset.darkBg) || '#131314';
  const style = document.createElement('style');
  style.textContent = `html{background:#ffffff}html.dark{background:${darkBackground}}`;
  document.head.append(style);

  const prefersDark = () => !!(window.matchMedia && window.matchMedia('(prefers-color-scheme: dark)').matches);
  let dark;
  try {
    const stored = window.localStorage.getItem('pinvou.web.theme');
    dark = stored === 'light' || stored === 'dark' ? stored === 'dark' : prefersDark();
  } catch {
    dark = prefersDark();
  }
  if (dark) document.documentElement.classList.add('dark');
})();
