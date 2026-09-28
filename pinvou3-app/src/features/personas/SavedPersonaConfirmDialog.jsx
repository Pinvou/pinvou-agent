// Shown after a persona card is saved: iOS-style confirm offering to view the
// user's own cards or dismiss. Lazy-loaded because it only appears after a save.
export function SavedPersonaConfirmDialog({ onClose, onView, savedConfirm, t, theme }) {
  const dark = theme === 'dark';

  return (
    // biome-ignore lint/a11y/useKeyWithClickEvents: keyboard users close the dialog through its real buttons
    // biome-ignore lint/a11y/noStaticElementInteractions: this is a pointer-only backdrop around an accessible dialog card
    <div className="fixed inset-0 z-[80] flex items-center justify-center p-4" style={{ background:'rgba(0,0,0,.4)' }} onClick={onClose}>
      {/* biome-ignore lint/a11y/useKeyWithClickEvents: background click-to-close layer; keyboard path handled by real buttons inside the card */}
      {/* biome-ignore lint/a11y/noStaticElementInteractions: background click-to-close layer; non-interactive container */}
      <div onClick={(event) => event.stopPropagation()} className="w-[270px] rounded-[14px] overflow-hidden text-center"
        style={{ background: dark ? 'rgba(44,44,46,.95)' : 'rgba(250,250,250,.95)', backdropFilter:'blur(20px)', WebkitBackdropFilter:'blur(20px)', fontFamily:'-apple-system, BlinkMacSystemFont, "SF Pro Text", "PingFang SC", "Microsoft YaHei", sans-serif' }}>
        <div className="px-4 pt-5 pb-4">
          <div className="text-[17px] font-semibold" style={{ color: dark ? '#fff' : '#000' }}>{t.cpSavedTitle}</div>
          <div className="text-[13px] mt-1.5" style={{ color: dark ? 'rgba(235,235,245,.6)' : 'rgba(60,60,67,.6)' }}>{t.cpSavedDesc(savedConfirm.name || '')}</div>
        </div>
        <div className="flex" style={{ borderTop: '0.5px solid ' + (dark ? 'rgba(84,84,88,.65)' : 'rgba(60,60,67,.29)') }}>
          <button type="button" onClick={onClose} className="flex-1 h-11 text-[17px]" style={{ color: dark ? '#0A84FF' : '#007AFF' }}>{t.cpSavedLater}</button>
          <div style={{ width:'0.5px', background: dark ? 'rgba(84,84,88,.65)' : 'rgba(60,60,67,.29)' }} />
          <button type="button" onClick={onView} className="flex-1 h-11 text-[17px] font-semibold" style={{ color: dark ? '#0A84FF' : '#007AFF' }}>{t.cpSavedView}</button>
        </div>
      </div>
    </div>
  );
}
