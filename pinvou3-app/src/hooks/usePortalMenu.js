// Shared placement/close plumbing for the sidebar rows' portal "more" menus
// (RecentItem, ProjectGroupHeader). Owns the open flag, the fixed-position
// style, the toggle/context-menu openers and the close-on
// (pointerdown | Escape | resize | scroll) effect, so each row component only
// renders its own menu items. The "click the button to open, the same click's
// document pointerdown already closed it" toggle quirk is inherited from the
// pre-extraction implementation on purpose — fixing it belongs to a dedicated
// interaction PR, not to a refactor.
import { useEffect, useState } from 'react';

export function usePortalMenu({ width = 176, height = 184 } = {}) {
  const [menuOpen, setMenuOpen] = useState(false);
  const [menuStyle, setMenuStyle] = useState(null);

  const closeMenu = () => setMenuOpen(false);
  const placeMenu = (target) => {
    const rect = target.getBoundingClientRect();
    const left = Math.max(8, Math.min(rect.right - width, window.innerWidth - width - 8));
    const top = rect.bottom + 6 + height > window.innerHeight
      ? Math.max(8, rect.top - height - 6)
      : Math.max(8, rect.bottom + 6);
    setMenuStyle({ left, top, width });
  };
  const toggleMenu = (e) => {
    e.stopPropagation();
    placeMenu(e.currentTarget);
    setMenuOpen(v => !v);
  };
  const openMenuAt = (e) => {
    e.preventDefault();
    e.stopPropagation();
    placeMenu(e.currentTarget);
    setMenuOpen(true);
  };

  useEffect(() => {
    if (!menuOpen) return;
    const close = () => setMenuOpen(false);
    const closeOnEscape = (event) => {
      if (event.key === 'Escape') {
        event.preventDefault();
        close();
      }
    };
    document.addEventListener('pointerdown', close);
    window.addEventListener('keydown', closeOnEscape);
    window.addEventListener('resize', close);
    window.addEventListener('scroll', close, true);
    return () => {
      document.removeEventListener('pointerdown', close);
      window.removeEventListener('keydown', closeOnEscape);
      window.removeEventListener('resize', close);
      window.removeEventListener('scroll', close, true);
    };
  }, [menuOpen]);

  return { menuOpen, menuStyle, closeMenu, toggleMenu, openMenuAt };
}
