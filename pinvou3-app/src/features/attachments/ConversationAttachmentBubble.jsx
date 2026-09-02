import { useCallback, useEffect, useState } from 'react';
import { createPortal } from 'react-dom';
import { FileTypeIcon } from '../../components/files/FileTypeIcon.jsx';
import { Copy, Download, ExternalLink, FolderOpen } from '../../components/icons.jsx';
import { bridge } from '../../hooks/useBridge.js';
import { isWeb } from '../../shared/platform.js';

export function ConversationAttachmentBubble({
  name,
  displayText,
  messageIndex,
  attachmentIndex,
  sessionId,
  labels,
  copyText,
}) {
  const [menu, setMenu] = useState(null);
  const reference = {
    basename: name,
    displayText,
    messageIndex,
    attachmentIndex,
    sessionId,
  };
  const closeMenu = useCallback(() => setMenu(null), []);

  useEffect(() => {
    if (!menu) return;
    const close = event => {
      if (event?.target?.closest?.('[data-conversation-attachment-menu]')) return;
      closeMenu();
    };
    const onKey = event => { if (event.key === 'Escape') closeMenu(); };
    document.addEventListener('mousedown', close, true);
    document.addEventListener('keydown', onKey, true);
    window.addEventListener('resize', close);
    window.addEventListener('scroll', close, true);
    return () => {
      document.removeEventListener('mousedown', close, true);
      document.removeEventListener('keydown', onKey, true);
      window.removeEventListener('resize', close);
      window.removeEventListener('scroll', close, true);
    };
  }, [closeMenu, menu]);

  const openAttachment = () => {
    closeMenu();
    if (bridge.available && bridge.attachments.openConversationAttachment) {
      bridge.attachments.openConversationAttachment(reference);
    }
  };
  const copyAddress = async () => {
    closeMenu();
    if (isWeb) {
      await copyText(name);
      return;
    }
    if (!bridge.available || !bridge.attachments.resolveConversationAttachment) return;
    try {
      const path = await bridge.attachments.resolveConversationAttachment(reference);
      await copyText(path);
    } catch { /* silently ignore parse/copy failures; no error toast */ }
  };
  const revealAttachment = () => {
    closeMenu();
    if (bridge.available && bridge.attachments.revealConversationAttachment) {
      bridge.attachments.revealConversationAttachment(reference);
    }
  };
  const openContextMenu = event => {
    event.preventDefault();
    event.stopPropagation();
    const width = 216;
    const height = isWeb ? 82 : 118;
    setMenu({
      x: Math.max(6, Math.min(event.clientX, window.innerWidth - width - 6)),
      y: Math.max(6, Math.min(event.clientY, window.innerHeight - height - 6)),
    });
  };
  const menuItemClass = `flex h-9 w-full items-center gap-2.5 px-3 text-left text-[13px] transition-colors text-[#1F1F1F] hover:bg-black/[0.06] dark:text-[#E3E3E3] dark:hover:bg-white/10`;

  return (
    <>
      <button
        type="button"
        aria-haspopup="menu"
        data-testid="conversation-attachment"
        onClick={openAttachment}
        onContextMenu={openContextMenu}
        className={`inline-flex max-w-[280px] cursor-pointer items-center gap-2 rounded-[14px] border px-2 py-1.5 text-[12px] leading-4 shadow-sm transition-colors focus:outline-none focus-visible:ring-2 focus-visible:ring-[#0A84FF]/60 border-black/[0.08] bg-white text-[#1F1F1F] hover:bg-[#F3F6FB] dark:border-white/10 dark:bg-[#2A2B2E] dark:text-[#E3E3E3] dark:hover:bg-[#34363A]`}
      >
        <span className={`inline-flex h-6 w-6 shrink-0 items-center justify-center rounded-[7px] bg-black/[0.04] dark:bg-white/[0.08]`}>
          <FileTypeIcon name={name} className="h-5 w-5" />
        </span>
        <span className="truncate font-medium">{name}</span>
      </button>
      {menu && createPortal((
        <div
          role="menu"
          data-conversation-attachment-menu
          data-testid="conversation-attachment-menu"
          className={`w-[216px] overflow-hidden rounded-[12px] border py-1 shadow-xl backdrop-blur border-black/10 bg-white dark:border-white/10 dark:bg-[#2B2C2F]`}
          style={{ position: 'fixed', zIndex: 9999, left: menu.x, top: menu.y }}
          onMouseDown={event => event.stopPropagation()}
          onContextMenu={event => event.preventDefault()}
        >
          <button type="button" role="menuitem" className={menuItemClass} onClick={openAttachment}>
            {isWeb ? <Download size={15} /> : <ExternalLink size={15} />}
            <span>{isWeb ? labels.download : labels.open}</span>
          </button>
          <button type="button" role="menuitem" className={menuItemClass} onClick={copyAddress}>
            <Copy size={15} />
            <span>{isWeb ? labels.copyName : labels.copyAddress}</span>
          </button>
          {!isWeb && (
            <button type="button" role="menuitem" className={menuItemClass} onClick={revealAttachment}>
              <FolderOpen size={15} />
              <span>{labels.reveal}</span>
            </button>
          )}
        </div>
      ), document.body)}
    </>
  );
}
