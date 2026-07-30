import React from 'react';
import { createPortal } from 'react-dom';
import { Code, FileText, ImageIcon } from '../../components/icons.jsx';

export function AttachmentDropOverlay({
  active,
  dark = false,
  variant = 'desktop',
  releaseLabel = '松开即可添加',
  webTitle = '添加任意内容',
  webHint = '将任意文件拖放到此处，以将其添加到对话中',
}) {
  if (variant === 'web') {
    const overlay = (
      <div
        data-testid="attachment-drop-overlay"
        data-variant="web"
        aria-hidden={!active}
        className={`pointer-events-none fixed inset-0 z-[2147483646] flex items-center justify-center transition-opacity duration-150 ${
          active ? 'visible opacity-100' : 'invisible opacity-0'
        }`}
      >
        <div
          className="absolute inset-0"
          style={{ backgroundColor: dark ? 'rgba(23, 23, 25, 0.94)' : 'rgba(255, 255, 255, 0.94)' }}
        />
        <div className={`relative -translate-y-4 text-center ${dark ? 'text-white' : 'text-[#171717]'}`}>
          <div className="relative mx-auto mb-5 h-[82px] w-[116px]">
            <span className="absolute left-1 top-5 flex h-12 w-12 -rotate-[12deg] items-center justify-center rounded-[13px] bg-[#A7B9FF] text-white shadow-sm">
              <Code size={24} />
            </span>
            <span className="absolute right-1 top-0 flex h-14 w-12 rotate-[16deg] items-center justify-center rounded-[10px] bg-[#7386F8] text-white shadow-sm">
              <FileText size={25} />
            </span>
            <span className="absolute bottom-0 left-[36px] flex h-12 w-14 items-center justify-center rounded-[13px] bg-[#4545F5] text-white shadow-md ring-4 ring-white dark:ring-[#171719]">
              <ImageIcon size={27} />
            </span>
          </div>
          <div className="text-[27px] font-bold leading-9 tracking-[-0.02em]">{webTitle}</div>
          <div className={`mt-1.5 text-[15px] leading-6 ${dark ? 'text-white/70' : 'text-black/65'}`}>
            {webHint}
          </div>
        </div>
      </div>
    );
    return typeof document === 'undefined' ? overlay : createPortal(overlay, document.body);
  }

  return (
    <div
      data-testid="attachment-drop-overlay"
      data-variant="desktop"
      aria-hidden={!active}
      className={`pointer-events-none absolute inset-0 z-[60] flex items-center justify-center transition-opacity duration-150 ${
        active ? 'visible opacity-100' : 'invisible opacity-0'
      }`}
    >
      <div
        className="absolute inset-0"
        style={{ backgroundColor: dark ? 'rgba(16, 36, 58, 0.80)' : 'rgba(232, 243, 253, 0.80)' }}
      />
      <div className={`relative -translate-y-1 rounded-[9px] border px-3 py-1 text-[13px] font-medium leading-[18px] shadow-sm ${
        dark
          ? 'border-[#49627A]/60 bg-[#233B51] text-[#E5EDF5]'
          : 'border-[#C9DDEA] bg-[#DDECF7] text-[#30363B]'
      }`}>
        {releaseLabel}
      </div>
    </div>
  );
}
